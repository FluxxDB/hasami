mod common;

use std::fs;
use std::path::{Path, PathBuf};

use hasami_lib::compress::SIZE_LIMIT_BYTES;
use hasami_lib::jobs::compress::compress;
use hasami_lib::jobs::{JobError, JobOutcome, JobProgress};
use hasami_lib::media::run::CancelToken;
use hasami_lib::media::{probe, FfmpegPaths, MediaInfo};

fn paths() -> FfmpegPaths {
    FfmpegPaths::resolve().unwrap()
}

/// Run `compress` without cancellation, collecting every progress report.
fn run(input: &Path, confirmed_low_res: bool) -> (Result<JobOutcome, JobError>, Vec<JobProgress>) {
    let mut seen = Vec::new();
    let result = compress(
        &paths(),
        input,
        confirmed_low_res,
        &CancelToken::new(),
        &mut |p| seen.push(p),
    );
    (result, seen)
}

/// Unwrap a `Done` outcome, returning the output path and its probe.
fn done(result: Result<JobOutcome, JobError>) -> (PathBuf, MediaInfo) {
    match result {
        Ok(JobOutcome::Done { output, bytes }) => {
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

#[test]
fn compress_big_file_under_limit() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_big_h264(dir.path(), 8);
    assert!(
        fs::metadata(&input).unwrap().len() > SIZE_LIMIT_BYTES,
        "fixture must start over the limit"
    );

    let (result, progress) = run(&input, false);
    let (output, info) = done(result);

    assert_eq!(output, input, "an .mp4 keeps its name");
    assert_compress_output(&info);
    assert_eq!(info.video.as_ref().unwrap().codec, "h264");
    assert_eq!(info.audio.len(), 1);
    assert_eq!(info.audio[0].codec, "aac");
    assert_eq!(names(dir.path()), vec!["big_h264.mp4"]);
    assert_no_leftovers(dir.path());

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
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_multitrack_hevc(dir.path());
    assert_eq!(probe(&paths(), &input).unwrap().audio.len(), 3);

    let (output, info) = done(run(&input, false).0);

    assert_eq!(output, input);
    assert_compress_output(&info);
    assert_eq!(info.video.as_ref().unwrap().codec, "h264");
    assert_eq!(info.audio.len(), 1);
    assert_eq!(info.audio[0].codec, "aac");
    assert_no_leftovers(dir.path());
}

#[test]
fn compress_mkv_becomes_mp4() {
    let dir = tempfile::tempdir().unwrap();
    let big = common::fixture_big_h264(dir.path(), 4);
    let input = common::remux(&big, &dir.path().join("a.mkv"));
    fs::remove_file(&big).unwrap();

    let (output, info) = done(run(&input, false).0);

    assert_eq!(output, dir.path().join("a.mp4"));
    assert!(!input.exists(), "a.mkv should be gone");
    assert_compress_output(&info);
    assert_eq!(names(dir.path()), vec!["a.mp4"]);
}

#[test]
fn compress_mkv_never_overwrites_a_neighbour() {
    let dir = tempfile::tempdir().unwrap();
    let small = common::fixture_small_h264_mp4(dir.path());
    let input = common::remux(&small, &dir.path().join("a.mkv"));
    fs::rename(&small, dir.path().join("a.mp4")).unwrap();
    let neighbour = fs::read(dir.path().join("a.mp4")).unwrap();

    let (output, _) = done(run(&input, false).0);

    assert_eq!(output, dir.path().join("a (2).mp4"));
    assert_eq!(
        fs::read(dir.path().join("a.mp4")).unwrap(),
        neighbour,
        "existing a.mp4 untouched"
    );
    assert_eq!(names(dir.path()), vec!["a (2).mp4", "a.mp4"]);
}

#[test]
fn compress_awkward_name() {
    let dir = tempfile::tempdir().unwrap();
    let big = common::fixture_big_h264(dir.path(), 4);
    let input = dir.path().join("[CLIPPED] Ráplay 'x' (2).mp4");
    fs::rename(&big, &input).unwrap();

    let (output, info) = done(run(&input, false).0);

    assert_eq!(output, input);
    assert_compress_output(&info);
    assert_eq!(names(dir.path()), vec!["[CLIPPED] Ráplay 'x' (2).mp4"]);
}

#[test]
fn compress_video_only() {
    let dir = tempfile::tempdir().unwrap();
    // The .mp4 fixture is already small; as .mkv it has to be encoded.
    let video_only = common::fixture_video_only(dir.path());
    let input = common::remux(&video_only, &dir.path().join("video_only.mkv"));
    fs::remove_file(&video_only).unwrap();

    let (output, info) = done(run(&input, false).0);

    assert_eq!(output, dir.path().join("video_only.mp4"));
    assert_compress_output(&info);
    assert_eq!(info.video.as_ref().unwrap().codec, "h264");
    assert_eq!(info.audio.len(), 0);
    assert_no_leftovers(dir.path());
}

#[test]
fn compress_audio_only() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_audio_only_mkv(dir.path());

    let (result, progress) = run(&input, false);
    let (output, info) = done(result);

    assert_eq!(output, dir.path().join("audio_only.mp4"));
    assert_compress_output(&info);
    assert!(info.video.is_none());
    assert_eq!(info.audio.len(), 1);
    assert_eq!(info.audio[0].codec, "aac");
    assert!(
        progress.iter().all(|p| p.pass == 1 && p.passes == 1),
        "{progress:?}"
    );
    assert_no_leftovers(dir.path());
}

#[test]
fn compress_already_small() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_small_h264_mp4(dir.path());
    let bytes = fs::read(&input).unwrap();
    let mtime = fs::metadata(&input).unwrap().modified().unwrap();

    let (result, progress) = run(&input, false);

    match result {
        Ok(JobOutcome::Unchanged { reason }) => assert_eq!(reason, "Already under 20 MB"),
        other => panic!("expected Unchanged, got {other:?}"),
    }
    assert!(progress.is_empty());
    assert_eq!(fs::read(&input).unwrap(), bytes);
    assert_eq!(fs::metadata(&input).unwrap().modified().unwrap(), mtime);
    assert_eq!(names(dir.path()), vec!["small_h264.mp4"]);
}

#[test]
fn compress_low_res_needs_confirm() {
    let dir = tempfile::tempdir().unwrap();
    // 300 s of 1080p30: the planner picks 640x360, below 480p.
    let input = common::fixture_long_1080p_mkv(dir.path(), 300);
    let bytes = fs::read(&input).unwrap();

    let (result, progress) = run(&input, false);
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
    assert_eq!(
        names(dir.path()),
        vec!["long_1080p.mkv"],
        "nothing is written"
    );
    assert_eq!(fs::read(&input).unwrap(), bytes);

    let (output, info) = done(run(&input, true).0);
    assert_eq!(output, dir.path().join("long_1080p.mp4"));
    assert_compress_output(&info);
    assert!(info.video.as_ref().unwrap().height < 480);
    assert_eq!(names(dir.path()), vec!["long_1080p.mp4"]);
}

#[test]
fn compress_native_360p_does_not_ask() {
    let dir = tempfile::tempdir().unwrap();
    let small = common::fixture_small_h264_mp4(dir.path());
    let input = common::remux(&small, &dir.path().join("native360.mkv"));
    fs::remove_file(&small).unwrap();

    let (output, info) = done(run(&input, false).0);

    assert_eq!(output, dir.path().join("native360.mp4"));
    let video = info.video.as_ref().unwrap();
    assert_eq!(
        (video.width, video.height),
        (640, 360),
        "already below 480p, so kept as is"
    );
}

#[test]
fn compress_cancel_leaves_original() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_big_h264(dir.path(), 4);
    let bytes = fs::read(&input).unwrap();

    let cancel = CancelToken::new();
    let mut max_pass = 0;
    let result = compress(&paths(), &input, false, &cancel, &mut |p| {
        max_pass = max_pass.max(p.pass);
        if p.pass == 1 {
            cancel.cancel();
        }
    });

    assert!(matches!(result, Err(JobError::Cancelled)), "{result:?}");
    assert_eq!(max_pass, 1, "cancelled during pass 1");
    assert_eq!(fs::read(&input).unwrap(), bytes, "original bytes unchanged");
    assert_eq!(
        names(dir.path()),
        vec!["big_h264.mp4"],
        "no temp file remains"
    );
}

#[test]
fn compress_cancel_during_pass_2_removes_temp() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_big_h264(dir.path(), 4);
    let bytes = fs::read(&input).unwrap();
    let temp = dir.path().join(".big_h264.mp4.hasami-tmp");

    let cancel = CancelToken::new();
    let mut temp_existed = false;
    let result = compress(&paths(), &input, false, &cancel, &mut |p| {
        if p.pass == 2 && !cancel.is_cancelled() {
            // Pass 2 is writing the temp file now, so a leftover would be detectable.
            temp_existed = temp.exists();
            cancel.cancel();
        }
    });

    assert!(matches!(result, Err(JobError::Cancelled)), "{result:?}");
    assert!(temp_existed, "the temp file should exist while pass 2 runs");
    assert_eq!(fs::read(&input).unwrap(), bytes, "original bytes unchanged");
    assert_eq!(
        names(dir.path()),
        vec!["big_h264.mp4"],
        "no temp file remains"
    );
}

#[test]
fn compress_unreadable_input_is_plain_language() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("not a video.mp4");
    fs::write(&input, b"this is not a video").unwrap();

    let (result, _) = run(&input, false);

    match result {
        Err(e) => assert_eq!(
            e.to_string(),
            "Hasami couldn't read this video."
        ),
        other => panic!("expected an error, got {other:?}"),
    }
    assert_eq!(fs::read(&input).unwrap(), b"this is not a video");
    assert_eq!(names(dir.path()), vec!["not a video.mp4"]);
}

#[test]
fn six_track_mix_keeps_level() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_six_tracks(dir.path());
    let source_loud = common::mean_volume_db(&input, 0);

    let (output, info) = done(run(&input, false).0);

    assert_eq!(info.audio.len(), 1);
    let mixed = common::mean_volume_db(&output, 0);
    assert!(
        (mixed - source_loud).abs() <= 1.0,
        "mixed mean {mixed} dB vs source loud track {source_loud} dB"
    );
}
