mod common;

use std::fs;
use std::path::{Path, PathBuf};

use hasami_lib::compress::SIZE_LIMIT_BYTES;
use hasami_lib::jobs::compress::compress_in;
use hasami_lib::jobs::{JobError, JobOutcome, JobProgress};
use hasami_lib::media::run::CancelToken;
use hasami_lib::media::{probe, FfmpegPaths, MediaInfo};

fn paths() -> FfmpegPaths {
    FfmpegPaths::resolve().unwrap()
}

/// A folder for the source and a stand-in for the Windows Videos folder, so no test ever writes
/// to the real one.
struct Dirs {
    src: tempfile::TempDir,
    videos: tempfile::TempDir,
}

fn dirs() -> Dirs {
    Dirs {
        src: tempfile::tempdir().unwrap(),
        videos: tempfile::tempdir().unwrap(),
    }
}

impl Dirs {
    fn src(&self) -> &Path {
        self.src.path()
    }

    fn videos(&self) -> &Path {
        self.videos.path()
    }

    /// Run Compress on `input` into the stand-in Videos folder, without cancellation,
    /// collecting every progress report.
    fn run(
        &self,
        input: &Path,
        confirmed_low_res: bool,
    ) -> (Result<JobOutcome, JobError>, Vec<JobProgress>) {
        let mut seen = Vec::new();
        let result = compress_in(
            &paths(),
            input,
            self.videos(),
            confirmed_low_res,
            &CancelToken::new(),
            &mut |p| seen.push(p),
        );
        (result, seen)
    }

    /// Compress with `cancel`, calling `watch` on every progress report.
    fn run_cancellable(
        &self,
        input: &Path,
        cancel: &CancelToken,
        watch: &mut dyn FnMut(JobProgress),
    ) -> Result<JobOutcome, JobError> {
        compress_in(&paths(), input, self.videos(), false, cancel, watch)
    }
}

/// Unwrap a `Done` outcome, returning the output path and its probe.
fn done(result: Result<JobOutcome, JobError>) -> (PathBuf, MediaInfo) {
    match result {
        Ok(JobOutcome::Done { output, bytes, note }) => {
            assert_eq!(note, None);
            let info = probe(&paths(), &output).unwrap();
            assert_eq!(
                bytes,
                fs::metadata(&output).unwrap().len(),
                "reported size is the file's size"
            );
            (output, info)
        }
        other => panic!("expected Done, got {other:?}"),
    }
}

/// File names in `dir`, sorted.
fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn assert_no_leftovers(dir: &Path) {
    let leftovers: Vec<String> = names(dir)
        .into_iter()
        .filter(|n| n.contains(".hasami-"))
        .collect();
    assert!(leftovers.is_empty(), "leftover work files: {leftovers:?}");
}

fn assert_compress_output(info: &MediaInfo) {
    assert!(
        info.size_bytes <= SIZE_LIMIT_BYTES,
        "output is {} bytes",
        info.size_bytes
    );
    assert_eq!(info.container, "mp4");
}

/// The source folder holds exactly the untouched `input` and nothing else.
fn assert_original_kept(d: &Dirs, input: &Path, bytes: &[u8]) {
    assert_eq!(fs::read(input).unwrap(), bytes, "the original must be untouched");
    assert_eq!(
        names(d.src()),
        vec![input.file_name().unwrap().to_string_lossy().into_owned()],
        "nothing is written beside the original"
    );
}

#[test]
fn compress_big_file_under_limit() {
    let d = dirs();
    let input = common::fixture_big_h264(d.src(), 8);
    let original = fs::read(&input).unwrap();
    assert!(
        original.len() as u64 > SIZE_LIMIT_BYTES,
        "fixture must start over the limit"
    );

    let (result, progress) = d.run(&input, false);
    let (output, info) = done(result);

    assert_eq!(output, d.videos().join("[20MB] big_h264.mp4"));
    assert_compress_output(&info);
    assert_eq!(info.video.as_ref().unwrap().codec, "h264");
    assert_eq!(info.audio.len(), 1);
    assert_eq!(info.audio[0].codec, "aac");
    assert_original_kept(&d, &input, &original);
    assert_eq!(names(d.videos()), vec!["[20MB] big_h264.mp4"]);
    assert_no_leftovers(d.videos());

    assert!(
        progress
            .iter()
            .all(|p| p.passes == 2 && (0.0..=1.0).contains(&p.fraction)),
        "{progress:?}"
    );
    assert!(
        progress.iter().any(|p| p.pass == 1),
        "no pass 1 progress: {progress:?}"
    );
    assert!(
        progress.iter().any(|p| p.pass == 2),
        "no pass 2 progress: {progress:?}"
    );
}

#[test]
fn compress_multitrack_hevc_mixes_to_one() {
    let d = dirs();
    let input = common::fixture_multitrack_hevc(d.src());
    let original = fs::read(&input).unwrap();
    assert_eq!(probe(&paths(), &input).unwrap().audio.len(), 3);

    let (output, info) = done(d.run(&input, false).0);

    assert_eq!(output, d.videos().join("[20MB] multitrack_hevc.mp4"));
    assert_compress_output(&info);
    assert_eq!(info.video.as_ref().unwrap().codec, "h264");
    assert_eq!(info.audio.len(), 1);
    assert_eq!(info.audio[0].codec, "aac");
    assert_original_kept(&d, &input, &original);
    assert_no_leftovers(d.videos());
}

#[test]
fn compress_mkv_becomes_mp4() {
    let d = dirs();
    let big = common::fixture_big_h264(d.src(), 4);
    let input = common::remux(&big, &d.src().join("a.mkv"));
    fs::remove_file(&big).unwrap();
    let original = fs::read(&input).unwrap();

    let (output, info) = done(d.run(&input, false).0);

    assert_eq!(output, d.videos().join("[20MB] a.mp4"));
    assert_compress_output(&info);
    assert_original_kept(&d, &input, &original);
    assert_eq!(names(d.videos()), vec!["[20MB] a.mp4"]);
}

#[test]
fn compress_never_overwrites_an_earlier_copy() {
    let d = dirs();
    let small = common::fixture_small_h264_mp4(d.src());
    let input = common::remux(&small, &d.src().join("a.mkv"));
    fs::remove_file(&small).unwrap();
    let earlier = d.videos().join("[20MB] a.mp4");
    fs::write(&earlier, b"an earlier copy").unwrap();

    let (output, _) = done(d.run(&input, false).0);

    assert_eq!(output, d.videos().join("[20MB] a (2).mp4"));
    assert_eq!(
        fs::read(&earlier).unwrap(),
        b"an earlier copy",
        "existing [20MB] a.mp4 untouched"
    );
    assert_eq!(
        names(d.videos()),
        vec!["[20MB] a (2).mp4", "[20MB] a.mp4"]
    );
}

#[test]
fn compress_awkward_name() {
    let d = dirs();
    let big = common::fixture_big_h264(d.src(), 4);
    let input = d.src().join("[CLIPPED] Ráplay 'x' (2).mp4");
    fs::rename(&big, &input).unwrap();
    let original = fs::read(&input).unwrap();

    let (output, info) = done(d.run(&input, false).0);

    assert_eq!(
        output,
        d.videos().join("[20MB] Ráplay 'x' (2).mp4"),
        "[CLIPPED] is dropped from the name"
    );
    assert_compress_output(&info);
    assert_original_kept(&d, &input, &original);
}

#[test]
fn compress_video_only() {
    let d = dirs();
    // The .mp4 fixture is already small; as .mkv it has to be encoded.
    let video_only = common::fixture_video_only(d.src());
    let input = common::remux(&video_only, &d.src().join("video_only.mkv"));
    fs::remove_file(&video_only).unwrap();

    let (output, info) = done(d.run(&input, false).0);

    assert_eq!(output, d.videos().join("[20MB] video_only.mp4"));
    assert_compress_output(&info);
    assert_eq!(info.video.as_ref().unwrap().codec, "h264");
    assert_eq!(info.audio.len(), 0);
    assert!(input.exists());
    assert_no_leftovers(d.videos());
}

#[test]
fn compress_audio_only() {
    let d = dirs();
    let input = common::fixture_audio_only_mkv(d.src());

    let (result, progress) = d.run(&input, false);
    let (output, info) = done(result);

    assert_eq!(output, d.videos().join("[20MB] audio_only.mp4"));
    assert_compress_output(&info);
    assert!(info.video.is_none());
    assert_eq!(info.audio.len(), 1);
    assert_eq!(info.audio[0].codec, "aac");
    assert!(
        progress.iter().all(|p| p.pass == 1 && p.passes == 1),
        "{progress:?}"
    );
    assert!(input.exists());
    assert_no_leftovers(d.videos());
}

#[test]
fn compress_already_small() {
    let d = dirs();
    let input = common::fixture_small_h264_mp4(d.src());
    let bytes = fs::read(&input).unwrap();
    let mtime = fs::metadata(&input).unwrap().modified().unwrap();

    let (result, progress) = d.run(&input, false);

    match result {
        Ok(JobOutcome::Unchanged { reason }) => assert_eq!(reason, "Already under 20 MB"),
        other => panic!("expected Unchanged, got {other:?}"),
    }
    assert!(progress.is_empty());
    assert_eq!(fs::metadata(&input).unwrap().modified().unwrap(), mtime);
    assert_original_kept(&d, &input, &bytes);
    assert!(names(d.videos()).is_empty(), "no file is written");
}

#[test]
fn compress_low_res_needs_confirm() {
    let d = dirs();
    // 300 s of 1080p30: the planner picks 640x360, below 480p.
    let input = common::fixture_long_1080p_mkv(d.src(), 300);
    let bytes = fs::read(&input).unwrap();

    let (result, progress) = d.run(&input, false);
    match result {
        Ok(JobOutcome::NeedsConfirm { plan }) => {
            assert!(plan.warn_low_res);
            assert!(plan.height < 480, "plan height {}", plan.height);
        }
        other => panic!("expected NeedsConfirm, got {other:?}"),
    }
    assert!(
        progress.is_empty(),
        "nothing should be encoded before confirming"
    );
    assert!(names(d.videos()).is_empty(), "nothing is written");
    assert_original_kept(&d, &input, &bytes);

    let (output, info) = done(d.run(&input, true).0);
    assert_eq!(output, d.videos().join("[20MB] long_1080p.mp4"));
    assert_compress_output(&info);
    assert!(info.video.as_ref().unwrap().height < 480);
    assert_original_kept(&d, &input, &bytes);
}

#[test]
fn compress_native_360p_does_not_ask() {
    let d = dirs();
    let small = common::fixture_small_h264_mp4(d.src());
    let input = common::remux(&small, &d.src().join("native360.mkv"));
    fs::remove_file(&small).unwrap();

    let (output, info) = done(d.run(&input, false).0);

    assert_eq!(output, d.videos().join("[20MB] native360.mp4"));
    let video = info.video.as_ref().unwrap();
    assert_eq!(
        (video.width, video.height),
        (640, 360),
        "already below 480p, so kept as is"
    );
}

#[test]
fn compress_cancel_leaves_original() {
    let d = dirs();
    let input = common::fixture_big_h264(d.src(), 4);
    let bytes = fs::read(&input).unwrap();

    let cancel = CancelToken::new();
    let mut max_pass = 0;
    let result = d.run_cancellable(&input, &cancel, &mut |p| {
        max_pass = max_pass.max(p.pass);
        if p.pass == 1 {
            cancel.cancel();
        }
    });

    assert!(matches!(result, Err(JobError::Cancelled)), "{result:?}");
    assert_eq!(max_pass, 1, "cancelled during pass 1");
    assert_original_kept(&d, &input, &bytes);
    assert!(names(d.videos()).is_empty(), "no temp file remains");
}

#[test]
fn compress_cancel_during_pass_2_removes_temp() {
    let d = dirs();
    let input = common::fixture_big_h264(d.src(), 4);
    let bytes = fs::read(&input).unwrap();
    let temp = d.videos().join(".[20MB] big_h264.mp4.hasami-tmp");

    let cancel = CancelToken::new();
    let mut temp_existed = false;
    let result = d.run_cancellable(&input, &cancel, &mut |p| {
        if p.pass == 2 && !cancel.is_cancelled() {
            // Pass 2 is writing the temp file now, so a leftover would be detectable.
            temp_existed = temp.exists();
            cancel.cancel();
        }
    });

    assert!(matches!(result, Err(JobError::Cancelled)), "{result:?}");
    assert!(temp_existed, "the temp file should exist while pass 2 runs");
    assert_original_kept(&d, &input, &bytes);
    assert!(names(d.videos()).is_empty(), "no temp file remains");
}

#[test]
fn compress_keeps_a_rotated_video_upright_and_unsquashed() {
    let d = dirs();
    let input = common::fixture_rotated_mov(d.src());

    let (_, info) = done(d.run(&input, false).0);

    let video = info.video.expect("video stream");
    let aspect = video.width as f64 / video.height as f64;
    assert!(
        (aspect - 360.0 / 640.0).abs() < 0.02,
        "output {}x{} should be portrait 9:16",
        video.width,
        video.height
    );
}

#[test]
fn compress_unreadable_input_is_plain_language() {
    let d = dirs();
    let input = d.src().join("not a video.mp4");
    fs::write(&input, b"this is not a video").unwrap();

    let (result, _) = d.run(&input, false);

    match result {
        Err(e) => assert_eq!(e.to_string(), "Hasami couldn't read this video."),
        other => panic!("expected an error, got {other:?}"),
    }
    assert_original_kept(&d, &input, b"this is not a video");
    assert!(names(d.videos()).is_empty());
}

#[test]
fn six_track_mix_keeps_level() {
    let d = dirs();
    let input = common::fixture_six_tracks(d.src());
    let source_loud = common::mean_volume_db(&input, 0);

    let (output, info) = done(d.run(&input, false).0);

    assert_eq!(info.audio.len(), 1);
    let mixed = common::mean_volume_db(&output, 0);
    assert!(
        (mixed - source_loud).abs() <= 1.0,
        "mixed mean {mixed} dB vs source loud track {source_loud} dB"
    );
}
