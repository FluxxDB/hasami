mod common;

use std::fs;
use std::path::{Path, PathBuf};

use hasami_lib::editor::request::{
    build_graph, encoder_available, AudioPiece, AudioTrackRequest, ExportMode, ExportRequest, VideoPiece,
};
use hasami_lib::jobs::export::export;
use hasami_lib::jobs::{JobError, JobOutcome};
use hasami_lib::media::run::CancelToken;
use hasami_lib::media::{hidden_command, probe, FfmpegPaths};

/// Render `req`'s graph over `input` into `out` with plain libx264/aac, panicking with ffmpeg's
/// stderr on failure.
fn render_graph(paths: &FfmpegPaths, input: &Path, req: &ExportRequest, out: &Path) {
    let graph = build_graph(req);
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
    }
}

fn run_export(req: &ExportRequest) -> Result<JobOutcome, JobError> {
    let paths = FfmpegPaths::resolve().unwrap();
    export(&paths, req, &CancelToken::new(), &mut |_| {})
}

/// The output path of a successful export, panicking otherwise.
fn done(result: Result<JobOutcome, JobError>) -> PathBuf {
    match result {
        Ok(JobOutcome::Done { output, bytes }) => {
            assert_eq!(bytes, fs::metadata(&output).unwrap().len(), "reported size");
            output
        }
        other => panic!("expected Done, got {other:?}"),
    }
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
    assert_eq!(info.video.as_ref().unwrap().codec, "hevc");
    assert_eq!(info.audio.len(), 1);
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

#[test]
fn fit_export_under_limit() {
    let dir = tempfile::tempdir().unwrap();
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
    };

    let mut passes = Vec::new();
    let result = export(&paths, &req, &CancelToken::new(), &mut |p| passes.push((p.pass, p.passes)));
    let output = done(result);

    // The intermediate is far over 20 MB, so Compress's two passes run as passes 2 and 3 of 3.
    for pass in 1..=3 {
        assert!(passes.contains(&(pass, 3)), "no pass {pass} of 3 in {passes:?}");
    }
    assert!(passes.iter().all(|&(_, of)| of == 3), "{passes:?}");
    assert_eq!(output, dir.path().join("[CLIPPED] big_h264.mp4"));
    let size = fs::metadata(&output).unwrap().len();
    assert!(size <= 20_000_000, "{size} bytes");
    let info = probe(&paths, &output).unwrap();
    assert_eq!(info.video.as_ref().unwrap().codec, "h264");
    assert_eq!(info.audio.len(), 1);
    assert!(!input.exists(), "the original should be recycled");
    assert!(leftover_temp_files(dir.path()).is_empty());
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
    };

    let output = done(run_export(&req));

    assert_eq!(output, dir.path().join("[CLIPPED] big_h264.mp4"));
    assert!(fs::metadata(&output).unwrap().len() <= 20_000_000);
    let info = probe(&paths, &output).unwrap();
    assert_eq!(info.video.as_ref().unwrap().codec, "h264");
    assert_eq!(info.audio.len(), 0);
    assert!(leftover_temp_files(dir.path()).is_empty());
}
