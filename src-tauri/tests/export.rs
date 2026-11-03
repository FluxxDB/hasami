mod common;

use std::fs;
use std::path::{Path, PathBuf};

use hasami_lib::editor::media::Crop;
use hasami_lib::editor::request::{
    build_graph, encoder_available, AudioPiece, AudioTrackRequest, ExportMode, ExportRequest, VideoPiece,
};
use hasami_lib::jobs::export::{export, export_in};
use hasami_lib::jobs::{JobError, JobOutcome};
use hasami_lib::media::run::CancelToken;
use hasami_lib::media::{hidden_command, probe, FfmpegPaths};

/// Render `req`'s graph over `input` into `out` with plain libx264/aac, panicking with ffmpeg's
/// stderr on failure.
fn render_graph(paths: &FfmpegPaths, input: &Path, req: &ExportRequest, out: &Path) {
    let graph = build_graph(req, (1920, 1080));
    let mut cmd = hidden_command(&paths.ffmpeg);
    cmd.args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(input)
        .args(["-filter_complex", &graph.filter]);
    if let Some(v) = &graph.video_out {
        cmd.args(["-map", v, "-c:v", "libx264", "-pix_fmt", "yuv420p"]);
    }
    if let Some(a) = &graph.audio_out {
        cmd.args(["-map", a, "-c:a", "aac"]);
    }
    let output = cmd.arg(out).output().expect("failed to spawn ffmpeg");
    assert!(
        output.status.success(),
        "ffmpeg failed ({}) for filter {}:\n{}",
        output.status,
        graph.filter,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn render_graph_cuts_duration() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_multitrack_hevc(dir.path());
    let paths = FfmpegPaths::resolve().unwrap();
    let req = ExportRequest {
        session: 1,
        input: input.clone(),
        mode: ExportMode::Quick,
        fit_20mb: false,
        duration_s: 3.0,
        video: vec![
            VideoPiece { src_start: 0.0, src_end: 1.0 },
            VideoPiece { src_start: 2.0, src_end: 4.0 },
        ],
        audio: vec![AudioTrackRequest {
            index: 0,
            gain_db: 0.0,
            pieces: vec![
                AudioPiece { at: 0.0, src_start: 0.0, src_end: 1.0 },
                AudioPiece { at: 1.0, src_start: 2.0, src_end: 4.0 },
            ],
        }],
        master_db: 0.0,
        crop: None,
    };

    let out = dir.path().join("cut.mp4");
    render_graph(&paths, &input, &req, &out);

    let info = probe(&paths, &out).unwrap();
    assert!((info.duration_s - 3.0).abs() < 0.1, "duration was {}", info.duration_s);
    assert!(info.video.is_some());
    assert_eq!(info.audio.len(), 1);
}

#[test]
fn encoder_available_tests_with_a_real_encode() {
    let paths = FfmpegPaths::resolve().unwrap();
    assert!(encoder_available(&paths, "libx264"));
    assert!(!encoder_available(&paths, "no_such_encoder"));
    // Second call is served from the cache and agrees.
    assert!(encoder_available(&paths, "libx264"));
}

/// The multitrack HEVC fixture renamed to `clip.mp4` inside `dir`.
fn clip_in(dir: &Path) -> PathBuf {
    let generated = common::fixture_multitrack_hevc(dir);
    let clip = dir.join("clip.mp4");
    fs::rename(generated, &clip).unwrap();
    clip
}

/// The standard edit of the multitrack fixture: video 0–1 s and 2–4 s; track 0 follows the
/// video, track 2 plays its first second at −6 dB; 3 s long.
fn cut_request(input: &Path, mode: ExportMode) -> ExportRequest {
    ExportRequest {
        session: 1,
        input: input.to_path_buf(),
        mode,
        fit_20mb: false,
        duration_s: 3.0,
        video: vec![
            VideoPiece { src_start: 0.0, src_end: 1.0 },
            VideoPiece { src_start: 2.0, src_end: 4.0 },
        ],
        audio: vec![
            AudioTrackRequest {
                index: 0,
                gain_db: 0.0,
                pieces: vec![
                    AudioPiece { at: 0.0, src_start: 0.0, src_end: 1.0 },
                    AudioPiece { at: 1.0, src_start: 2.0, src_end: 4.0 },
                ],
            },
            AudioTrackRequest {
                index: 2,
                gain_db: -6.0,
                pieces: vec![AudioPiece { at: 0.0, src_start: 0.0, src_end: 1.0 }],
            },
        ],
        master_db: 0.0,
        crop: None,
    }
}

fn run_export(req: &ExportRequest) -> Result<JobOutcome, JobError> {
    let paths = FfmpegPaths::resolve().unwrap();
    export(&paths, req, &CancelToken::new(), &mut |_| {})
}

/// Run `req` with `videos` standing in for the Windows Videos folder, recording each progress
/// report's `(pass, passes)`.
fn run_export_into(req: &ExportRequest, videos: &Path) -> (Result<JobOutcome, JobError>, Vec<(u8, u8)>) {
    let paths = FfmpegPaths::resolve().unwrap();
    let mut passes = Vec::new();
    let result = export_in(&paths, req, videos, &CancelToken::new(), &mut |p| passes.push((p.pass, p.passes)));
    (result, passes)
}

/// The output path and note of a successful export, panicking otherwise.
fn done_with_note(result: Result<JobOutcome, JobError>) -> (PathBuf, Option<String>) {
    match result {
        Ok(JobOutcome::Done { output, bytes, note }) => {
            assert_eq!(bytes, fs::metadata(&output).unwrap().len(), "reported size");
            (output, note)
        }
        other => panic!("expected Done, got {other:?}"),
    }
}

/// The output path of a successful export with nothing to add, panicking otherwise.
fn done(result: Result<JobOutcome, JobError>) -> PathBuf {
    let (output, note) = done_with_note(result);
    assert_eq!(note, None);
    output
}

/// A Fit export's progress: passes 1 to 3 of 3, nothing else.
fn assert_three_passes(passes: &[(u8, u8)]) {
    for pass in 1..=3 {
        assert!(passes.contains(&(pass, 3)), "no pass {pass} of 3 in {passes:?}");
    }
    assert!(passes.iter().all(|&(_, of)| of == 3), "{passes:?}");
}

fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn leftover_temp_files(dir: &Path) -> Vec<String> {
    names_in(dir).into_iter().filter(|n| n.ends_with(".hasami-tmp")).collect()
}

#[test]
fn quick_export_replaces_with_clipped() {
    let dir = tempfile::tempdir().unwrap();
    let input = clip_in(dir.path());
    let paths = FfmpegPaths::resolve().unwrap();

    let output = done(run_export(&cut_request(&input, ExportMode::Quick)));

    assert_eq!(output, dir.path().join("[CLIPPED] clip.mp4"));
    let info = probe(&paths, &output).unwrap();
    assert!((info.duration_s - 3.0).abs() < 0.1, "duration was {}", info.duration_s);
    // The HEVC source is exported as Discord-safe H.264 at its own size.
    let video = info.video.as_ref().unwrap();
    assert_eq!((video.codec.as_str(), video.width, video.height), ("h264", 1920, 1080));
    assert_eq!(info.audio.len(), 1);
    assert_eq!(info.audio[0].codec, "aac");
    assert!(!input.exists(), "the original should be recycled");
    assert!(leftover_temp_files(dir.path()).is_empty());
}

#[test]
fn save_raw_moves_original() {
    let dir = tempfile::tempdir().unwrap();
    let input = clip_in(dir.path());
    let original = fs::read(&input).unwrap();

    let output = done(run_export(&cut_request(&input, ExportMode::SaveRaw)));

    assert_eq!(output, dir.path().join("[CLIPPED] clip.mp4"));
    assert!(!input.exists());
    assert_eq!(fs::read(dir.path().join("Raw").join("clip.mp4")).unwrap(), original);
    assert!(leftover_temp_files(dir.path()).is_empty());
}

#[test]
fn audio_export_is_mp3() {
    let dir = tempfile::tempdir().unwrap();
    let input = clip_in(dir.path());
    let original = fs::read(&input).unwrap();
    let paths = FfmpegPaths::resolve().unwrap();

    let output = done(run_export(&cut_request(&input, ExportMode::Audio)));

    assert_eq!(output, dir.path().join("[CLIPPED] clip.mp3"));
    let info = probe(&paths, &output).unwrap();
    assert!(info.video.is_none());
    assert_eq!(info.audio.len(), 1);
    assert_eq!(info.audio[0].codec, "mp3");
    assert!((info.duration_s - 3.0).abs() < 0.1, "duration was {}", info.duration_s);
    assert_eq!(fs::read(&input).unwrap(), original, "the original must be untouched");
    assert!(leftover_temp_files(dir.path()).is_empty());
}

/// Check the 20 MB copy at `copy`: an H.264 `.mp4` of at most 20,000,000 bytes and `duration_s`
/// long, with `audio_tracks` audio streams.
fn assert_20mb_copy(paths: &FfmpegPaths, copy: &Path, duration_s: f64, audio_tracks: usize) {
    let size = fs::metadata(copy).unwrap().len();
    assert!(size <= 20_000_000, "{size} bytes");
    let info = probe(paths, copy).unwrap();
    assert_eq!(info.container, "mp4");
    assert_eq!(info.video.as_ref().unwrap().codec, "h264");
    assert_eq!(info.audio.len(), audio_tracks);
    assert!((info.duration_s - duration_s).abs() < 0.1, "copy lasts {} s", info.duration_s);
}

#[test]
fn fit_export_saves_clipped_and_a_20mb_copy_in_videos() {
    let dir = tempfile::tempdir().unwrap();
    let videos = tempfile::tempdir().unwrap();
    let input = clip_in(dir.path());
    let paths = FfmpegPaths::resolve().unwrap();
    let req = ExportRequest { fit_20mb: true, ..cut_request(&input, ExportMode::Quick) };

    let (result, passes) = run_export_into(&req, videos.path());
    let output = done(result);

    // The export itself is exactly the non-fit one: H.264, beside the original.
    assert_eq!(output, dir.path().join("[CLIPPED] clip.mp4"));
    let info = probe(&paths, &output).unwrap();
    assert_eq!(info.video.as_ref().unwrap().codec, "h264");
    assert!((info.duration_s - 3.0).abs() < 0.1, "duration was {}", info.duration_s);
    assert!(!input.exists(), "the original should be recycled");
    assert_eq!(names_in(dir.path()), ["[CLIPPED] clip.mp4"]);

    // A small H.264 export is already Discord-ready, so the copy is the same file.
    assert_eq!(names_in(videos.path()), ["[20MB] clip.mp4"]);
    let copy = videos.path().join("[20MB] clip.mp4");
    assert_20mb_copy(&paths, &copy, 3.0, 1);
    assert_eq!(fs::read(&copy).unwrap(), fs::read(&output).unwrap());
    assert!(passes.contains(&(1, 3)) && passes.contains(&(3, 3)), "{passes:?}");
}

#[test]
fn fit_export_under_limit() {
    let dir = tempfile::tempdir().unwrap();
    let videos = tempfile::tempdir().unwrap();
    let input = common::fixture_big_h264(dir.path(), 8);
    let paths = FfmpegPaths::resolve().unwrap();
    let req = ExportRequest {
        session: 1,
        input: input.clone(),
        mode: ExportMode::Quick,
        fit_20mb: true,
        duration_s: 8.0,
        video: vec![VideoPiece { src_start: 0.0, src_end: 8.0 }],
        audio: vec![AudioTrackRequest {
            index: 0,
            gain_db: 0.0,
            pieces: vec![AudioPiece { at: 0.0, src_start: 0.0, src_end: 8.0 }],
        }],
        master_db: 0.0,
        crop: None,
    };

    let (result, passes) = run_export_into(&req, videos.path());
    let output = done(result);

    // Full quality: the export of a 60 Mbit/s clip is far over 20 MB, so the copy is encoded.
    assert_eq!(output, dir.path().join("[CLIPPED] big_h264.mp4"));
    assert!(fs::metadata(&output).unwrap().len() > 20_000_000);
    assert!(!input.exists(), "the original should be recycled");
    assert!(leftover_temp_files(dir.path()).is_empty());
    assert_20mb_copy(&paths, &videos.path().join("[20MB] big_h264.mp4"), 8.0, 1);
    assert_eq!(names_in(videos.path()), ["[20MB] big_h264.mp4"]);
    assert_three_passes(&passes);
}

#[test]
fn fit_export_of_a_small_clip_copies_it_as_is() {
    let dir = tempfile::tempdir().unwrap();
    let videos = tempfile::tempdir().unwrap();
    let input = common::fixture_small_h264_mp4(dir.path());
    let req = ExportRequest {
        session: 1,
        input: input.clone(),
        mode: ExportMode::SaveRaw,
        fit_20mb: true,
        duration_s: 1.5,
        video: vec![VideoPiece { src_start: 0.5, src_end: 2.0 }],
        audio: vec![AudioTrackRequest {
            index: 0,
            gain_db: 0.0,
            pieces: vec![AudioPiece { at: 0.0, src_start: 0.5, src_end: 2.0 }],
        }],
        master_db: 0.0,
        crop: None,
    };

    let (result, passes) = run_export_into(&req, videos.path());
    let output = done(result);

    // An H.264/AAC .mp4 under 20 MB is already Discord-ready: the copy is byte for byte.
    assert_eq!(output, dir.path().join("[CLIPPED] small_h264.mp4"));
    let copy = videos.path().join("[20MB] small_h264.mp4");
    assert_eq!(fs::read(&copy).unwrap(), fs::read(&output).unwrap());
    assert_eq!(names_in(videos.path()), ["[20MB] small_h264.mp4"]);
    // No encode for the copy: it is reported as the last pass, so the bar still ends full.
    assert!(passes.contains(&(1, 3)) && passes.contains(&(3, 3)), "{passes:?}");
    assert!(passes.iter().all(|&(pass, of)| of == 3 && pass != 2), "{passes:?}");
    assert!(dir.path().join("Raw").join("small_h264.mp4").exists());
}

#[test]
fn fit_export_whose_copy_fails_is_still_done_with_a_note() {
    let dir = tempfile::tempdir().unwrap();
    let videos = tempfile::tempdir().unwrap();
    let input = clip_in(dir.path());
    let original = fs::read(&input).unwrap();
    // A Videos folder that isn't there: the copy can't be written.
    let missing = videos.path().join("gone");
    let req = ExportRequest { fit_20mb: true, ..cut_request(&input, ExportMode::SaveRaw) };

    let (result, _) = run_export_into(&req, &missing);
    let (output, note) = done_with_note(result);

    assert_eq!(output, dir.path().join("[CLIPPED] clip.mp4"));
    let note = note.expect("a note about the copy");
    assert!(
        note.starts_with("Saved \"[CLIPPED] clip.mp4\", but couldn't make the 20 MB copy: "),
        "{note}"
    );
    assert_eq!(fs::read(dir.path().join("Raw").join("clip.mp4")).unwrap(), original);
    assert!(leftover_temp_files(dir.path()).is_empty());
    assert!(names_in(videos.path()).is_empty(), "{:?}", names_in(videos.path()));
}

#[test]
fn master_gain_changes_the_exported_level() {
    let dir = tempfile::tempdir().unwrap();
    let input = clip_in(dir.path());
    let req = |master_db| ExportRequest {
        video: vec![VideoPiece { src_start: 0.0, src_end: 3.0 }],
        audio: vec![AudioTrackRequest {
            index: 0,
            gain_db: 0.0,
            pieces: vec![AudioPiece { at: 0.0, src_start: 0.0, src_end: 3.0 }],
        }],
        master_db,
        ..cut_request(&input, ExportMode::Audio)
    };

    let unity = done(run_export(&req(0.0)));
    let quieter = done(run_export(&req(-12.0)));

    let drop = common::mean_volume_db(&unity, 0) - common::mean_volume_db(&quieter, 0);
    assert!((drop - 12.0).abs() <= 1.0, "master -12 dB lowered the mix by {drop} dB");
}

#[test]
fn all_muted_has_no_audio() {
    let dir = tempfile::tempdir().unwrap();
    let input = clip_in(dir.path());
    let paths = FfmpegPaths::resolve().unwrap();
    let req = ExportRequest { audio: vec![], ..cut_request(&input, ExportMode::Quick) };

    let output = done(run_export(&req));

    let info = probe(&paths, &output).unwrap();
    assert!(info.video.is_some());
    assert_eq!(info.audio.len(), 0);
}

#[test]
fn video_only_source() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_video_only(dir.path());
    let paths = FfmpegPaths::resolve().unwrap();
    let req = |mode| ExportRequest {
        session: 1,
        input: input.clone(),
        mode,
        fit_20mb: false,
        duration_s: 2.0,
        video: vec![VideoPiece { src_start: 0.5, src_end: 2.5 }],
        audio: vec![],
        master_db: 0.0,
        crop: None,
    };

    // Audio first: the Quick export below recycles the source.
    let err = run_export(&req(ExportMode::Audio)).unwrap_err();
    assert_eq!(err.to_string(), "This clip has no audio to export.");
    assert_eq!(names_in(dir.path()), ["video_only.mp4"]);

    let output = done(run_export(&req(ExportMode::Quick)));
    let info = probe(&paths, &output).unwrap();
    assert!(info.video.is_some());
    assert_eq!(info.audio.len(), 0);
}

#[test]
fn vp8_webm_exports_as_h264_mp4() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_webm_vp8_opus(dir.path());
    let paths = FfmpegPaths::resolve().unwrap();
    let req = ExportRequest {
        session: 1,
        input: input.clone(),
        mode: ExportMode::SaveRaw, // moves the original to Raw\ rather than recycling it
        fit_20mb: false,
        duration_s: 1.0,
        video: vec![VideoPiece { src_start: 0.5, src_end: 1.5 }],
        audio: vec![AudioTrackRequest {
            index: 0,
            gain_db: 0.0,
            pieces: vec![AudioPiece { at: 0.0, src_start: 0.5, src_end: 1.5 }],
        }],
        master_db: 0.0,
        crop: None,
    };

    let output = done(run_export(&req));
    assert_eq!(output, dir.path().join("[CLIPPED] clip_vp8.mp4"));
    let info = probe(&paths, &output).unwrap();
    assert_eq!(info.container, "mp4");
    assert_eq!(info.video.as_ref().unwrap().codec, "h264");
    assert_eq!(info.audio[0].codec, "aac");
    assert!((info.duration_s - 1.0).abs() < 0.1, "duration was {}", info.duration_s);
    assert!(dir.path().join("Raw").join("clip_vp8.webm").exists());
}

#[test]
fn mkv_source_exports_as_mp4() {
    let dir = tempfile::tempdir().unwrap();
    let small = common::fixture_small_h264_mp4(dir.path());
    let input = common::remux(&small, &dir.path().join("clip.mkv"));
    fs::remove_file(small).unwrap();
    let paths = FfmpegPaths::resolve().unwrap();
    let req = ExportRequest {
        session: 1,
        input: input.clone(),
        mode: ExportMode::SaveRaw,
        fit_20mb: false,
        duration_s: 1.0,
        video: vec![VideoPiece { src_start: 0.5, src_end: 1.5 }],
        audio: vec![AudioTrackRequest {
            index: 0,
            gain_db: 0.0,
            pieces: vec![AudioPiece { at: 0.0, src_start: 0.5, src_end: 1.5 }],
        }],
        master_db: 0.0,
        crop: None,
    };

    let output = done(run_export(&req));

    assert_eq!(output, dir.path().join("[CLIPPED] clip.mp4"));
    let info = probe(&paths, &output).unwrap();
    assert_eq!(info.container, "mp4");
    assert_eq!(info.video.as_ref().unwrap().codec, "h264");
    assert_eq!(info.audio[0].codec, "aac");
    assert!(dir.path().join("Raw").join("clip.mkv").exists());
}

/// Whether an RGB colour is (near) black.
fn is_black([r, g, b]: [u8; 3]) -> bool {
    r.max(g).max(b) < 32
}

#[test]
fn crop_removes_the_border_and_fills_the_frame() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_bordered(dir.path());
    let paths = FfmpegPaths::resolve().unwrap();
    assert!(is_black(common::pixel(&input, 1800, 1000)), "the source is black there");
    // Sampled now: Save raw moves the original into Raw\.
    let bars = [common::pixel(&input, 300, 150), common::pixel(&input, 750, 450)];
    let req = ExportRequest {
        session: 1,
        input: input.clone(),
        mode: ExportMode::SaveRaw,
        fit_20mb: false,
        duration_s: 3.0,
        video: vec![VideoPiece { src_start: 0.0, src_end: 1.0 }, VideoPiece { src_start: 2.0, src_end: 4.0 }],
        audio: vec![AudioTrackRequest {
            index: 0,
            gain_db: 0.0,
            pieces: vec![
                AudioPiece { at: 0.0, src_start: 0.0, src_end: 1.0 },
                AudioPiece { at: 1.0, src_start: 2.0, src_end: 4.0 },
            ],
        }],
        master_db: 0.0,
        crop: Some(Crop { x: 0, y: 0, w: 1440, h: 810 }),
    };

    let output = done(run_export(&req));

    let info = probe(&paths, &output).unwrap();
    let video = info.video.as_ref().unwrap();
    assert_eq!((video.width, video.height), (1920, 1080));
    assert!((info.duration_s - 3.0).abs() < 0.1, "duration was {}", info.duration_s);
    let corner = common::pixel(&output, 1800, 1000);
    assert!(!is_black(corner), "the picture should fill the frame, got {corner:?}");
    // Scaled by 4/3 from the top-left corner: the colour bars line up with the source's.
    assert_close(common::pixel(&output, 400, 200), bars[0]);
    assert_close(common::pixel(&output, 1000, 600), bars[1]);
}

/// Two colours from different encodes of the same picture: each channel within 40.
fn assert_close(got: [u8; 3], want: [u8; 3]) {
    let near = got.iter().zip(want).all(|(&g, w)| g.abs_diff(w) <= 40);
    assert!(near, "got {got:?}, want about {want:?}");
}

#[test]
fn crop_of_another_aspect_is_cut_out_unstretched() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_pillarboxed(dir.path());
    let paths = FfmpegPaths::resolve().unwrap();
    let req = ExportRequest {
        session: 1,
        input: input.clone(),
        mode: ExportMode::SaveRaw,
        fit_20mb: false,
        duration_s: 2.0,
        video: vec![VideoPiece { src_start: 0.0, src_end: 2.0 }],
        audio: vec![],
        master_db: 0.0,
        crop: Some(Crop { x: 240, y: 0, w: 1440, h: 1080 }),
    };
    // Sampled now: Save raw moves the original into Raw\.
    let spots = [(4, 4), (720, 540), (1430, 1070)];
    let source: Vec<[u8; 3]> = spots.iter().map(|&(x, y)| common::pixel(&input, x + 240, y)).collect();

    let output = done(run_export(&req));

    // A 4:3 picture in a 16:9 frame keeps its own size rather than being stretched to 1920x1080.
    let info = probe(&paths, &output).unwrap();
    let video = info.video.as_ref().unwrap();
    assert_eq!((video.width, video.height), (1440, 1080));
    for (&(x, y), &want) in spots.iter().zip(&source) {
        assert_close(common::pixel(&output, x, y), want);
    }
}

#[test]
fn crop_gain_and_20mb_copy_together_apply_once() {
    let dir = tempfile::tempdir().unwrap();
    let videos = tempfile::tempdir().unwrap();
    let input = common::fixture_big_h264(dir.path(), 5);
    let paths = FfmpegPaths::resolve().unwrap();
    // Measured now: Save raw moves the original into Raw\.
    let source_db = common::mean_volume_db(&input, 0);
    let req = ExportRequest {
        session: 1,
        input: input.clone(),
        mode: ExportMode::SaveRaw,
        fit_20mb: true,
        duration_s: 5.0,
        video: vec![VideoPiece { src_start: 0.0, src_end: 5.0 }],
        audio: vec![AudioTrackRequest {
            index: 0,
            gain_db: 0.0,
            pieces: vec![AudioPiece { at: 0.0, src_start: 0.0, src_end: 5.0 }],
        }],
        master_db: -6.0,
        // A 4:3 area of the 16:9 frame: kept at its own size.
        crop: Some(Crop { x: 240, y: 0, w: 1440, h: 1080 }),
    };

    let (result, passes) = run_export_into(&req, videos.path());
    let output = done(result);

    let clipped = probe(&paths, &output).unwrap();
    let v = clipped.video.as_ref().unwrap();
    assert_eq!((v.width, v.height), (1440, 1080), "the [CLIPPED] file is the crop's size");
    assert!(fs::metadata(&output).unwrap().len() > 20_000_000, "the copy should have to be encoded");
    let clipped_db = common::mean_volume_db(&output, 0);
    assert!((source_db - 6.0 - clipped_db).abs() <= 1.0, "source {source_db} dB, [CLIPPED] {clipped_db} dB");

    // The copy is made from the [CLIPPED] file: its aspect, and its level, with nothing applied twice.
    let copy = videos.path().join("[20MB] big_h264.mp4");
    assert_20mb_copy(&paths, &copy, 5.0, 1);
    let small = probe(&paths, &copy).unwrap();
    let sv = small.video.as_ref().unwrap();
    let aspect = f64::from(sv.width) / f64::from(sv.height);
    assert!((aspect / (4.0 / 3.0) - 1.0).abs() <= 0.01, "copy is {}x{}", sv.width, sv.height);
    let copy_db = common::mean_volume_db(&copy, 0);
    assert!((copy_db - clipped_db).abs() <= 1.0, "[CLIPPED] {clipped_db} dB, copy {copy_db} dB");
    assert_three_passes(&passes);
}

#[test]
fn superwide_source_exports_at_its_own_size() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_superwide(dir.path());
    let paths = FfmpegPaths::resolve().unwrap();
    let req = ExportRequest {
        session: 1,
        input: input.clone(),
        mode: ExportMode::SaveRaw,
        fit_20mb: false,
        duration_s: 1.5,
        video: vec![VideoPiece { src_start: 0.5, src_end: 2.0 }],
        audio: vec![AudioTrackRequest {
            index: 0,
            gain_db: 0.0,
            pieces: vec![AudioPiece { at: 0.0, src_start: 0.5, src_end: 2.0 }],
        }],
        master_db: 0.0,
        crop: None,
    };

    let output = done(run_export(&req));

    let info = probe(&paths, &output).unwrap();
    let video = info.video.as_ref().unwrap();
    assert_eq!(video.codec, "h264");
    assert_eq!((video.width, video.height), (5120, 1440));
    assert!((info.duration_s - 1.5).abs() < 0.1, "duration was {}", info.duration_s);
}

#[test]
fn missing_source_is_plain() {
    let dir = tempfile::tempdir().unwrap();
    let input = clip_in(dir.path());
    fs::remove_file(&input).unwrap();

    let err = run_export(&cut_request(&input, ExportMode::Quick)).unwrap_err();

    assert_eq!(err.to_string(), "Hasami couldn't read this video.");
    assert!(names_in(dir.path()).is_empty(), "{:?}", names_in(dir.path()));
}

#[test]
fn raw_folder_source_exports_to_parent() {
    let dir = tempfile::tempdir().unwrap();
    let raw = dir.path().join("Raw");
    fs::create_dir(&raw).unwrap();
    let input = clip_in(&raw);
    let original = fs::read(&input).unwrap();

    let output = done(run_export(&cut_request(&input, ExportMode::SaveRaw)));

    assert_eq!(output, dir.path().join("[CLIPPED] clip.mp4"));
    assert_eq!(fs::read(&input).unwrap(), original, "the source stays in Raw");
    assert_eq!(names_in(&raw), ["clip.mp4"]);
}

#[test]
fn cancel_leaves_original() {
    let dir = tempfile::tempdir().unwrap();
    let input = clip_in(dir.path());
    let original = fs::read(&input).unwrap();
    let paths = FfmpegPaths::resolve().unwrap();
    let cancel = CancelToken::new();

    let result = export(&paths, &cut_request(&input, ExportMode::Quick), &cancel, &mut |_| cancel.cancel());

    assert!(matches!(result, Err(JobError::Cancelled)), "{result:?}");
    assert_eq!(fs::read(&input).unwrap(), original);
    assert_eq!(names_in(dir.path()), ["clip.mp4"]);
}

/// Duration in seconds of `file`'s first audio stream, as ffprobe reports it.
fn audio_stream_duration(paths: &FfmpegPaths, file: &Path) -> f64 {
    let output = hidden_command(&paths.ffprobe)
        .args(["-v", "error", "-select_streams", "a:0", "-show_entries", "stream=duration", "-of", "csv=p=0"])
        .arg(file)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    text.trim().parse().unwrap_or_else(|_| panic!("no audio stream duration: {text:?}"))
}

/// Only the first second of track 0 is kept, on a 3 s timeline.
fn early_audio_request(input: &Path, mode: ExportMode) -> ExportRequest {
    ExportRequest {
        video: vec![VideoPiece { src_start: 0.0, src_end: 3.0 }],
        audio: vec![AudioTrackRequest {
            index: 0,
            gain_db: 0.0,
            pieces: vec![AudioPiece { at: 0.0, src_start: 0.0, src_end: 1.0 }],
        }],
        ..cut_request(input, mode)
    }
}

#[test]
fn audio_export_pads_to_the_video_end() {
    let dir = tempfile::tempdir().unwrap();
    let input = clip_in(dir.path());
    let paths = FfmpegPaths::resolve().unwrap();

    let output = done(run_export(&early_audio_request(&input, ExportMode::Audio)));

    let info = probe(&paths, &output).unwrap();
    assert!((info.duration_s - 3.0).abs() < 0.1, "duration was {}", info.duration_s);
}

#[test]
fn video_export_audio_runs_to_the_video_end() {
    let dir = tempfile::tempdir().unwrap();
    let input = clip_in(dir.path());
    let paths = FfmpegPaths::resolve().unwrap();

    let output = done(run_export(&early_audio_request(&input, ExportMode::Quick)));

    let audio_s = audio_stream_duration(&paths, &output);
    assert!((audio_s - 3.0).abs() < 0.1, "audio stream lasts {audio_s} s");
}

#[test]
fn fit_all_muted_has_no_audio() {
    let dir = tempfile::tempdir().unwrap();
    let videos = tempfile::tempdir().unwrap();
    let input = common::fixture_big_h264(dir.path(), 4);
    let paths = FfmpegPaths::resolve().unwrap();
    let req = ExportRequest {
        session: 1,
        input: input.clone(),
        mode: ExportMode::SaveRaw,
        fit_20mb: true,
        duration_s: 4.0,
        video: vec![VideoPiece { src_start: 0.0, src_end: 4.0 }],
        audio: vec![],
        master_db: 0.0,
        crop: None,
    };

    let output = done(run_export_into(&req, videos.path()).0);

    assert_eq!(output, dir.path().join("[CLIPPED] big_h264.mp4"));
    assert_eq!(probe(&paths, &output).unwrap().audio.len(), 0);
    assert!(leftover_temp_files(dir.path()).is_empty());
    assert_20mb_copy(&paths, &videos.path().join("[20MB] big_h264.mp4"), 4.0, 0);
}
