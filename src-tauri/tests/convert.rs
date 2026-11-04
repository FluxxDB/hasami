//! Integration tests for the Convert job: remux or re-encode, the original recycled only after
//! the new file is verified, and nothing left behind on failure or cancel.

mod common;

use std::path::Path;

use hasami_lib::cli::Target;
use hasami_lib::jobs::convert::convert;
use hasami_lib::jobs::{JobError, JobOutcome, JobProgress};
use hasami_lib::media::run::CancelToken;
use hasami_lib::media::{probe, FfmpegPaths};

fn run(input: &Path, target: Target) -> Result<JobOutcome, JobError> {
    let paths = FfmpegPaths::resolve().unwrap();
    let mut progress: Vec<JobProgress> = Vec::new();
    let result = convert(&paths, input, target, &CancelToken::new(), &mut |p| progress.push(p));
    if result.is_ok() {
        assert!(progress.iter().all(|p| p.pass == 1 && p.passes == 1), "{progress:?}");
    }
    result
}

fn leftover_temp_files(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".hasami-tmp"))
        .collect()
}

#[test]
fn convert_mp4_to_mov_keeps_all_tracks() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_multitrack_hevc(dir.path());
    let paths = FfmpegPaths::resolve().unwrap();

    let outcome = run(&input, Target::Mov).unwrap();

    let JobOutcome::Done { output, bytes, note: None } = outcome else { panic!("expected Done, got {outcome:?}") };
    assert_eq!(output, dir.path().join("multitrack_hevc.mov"));
    assert!(bytes > 0);
    assert!(!input.exists(), "original should be recycled");
    let info = probe(&paths, &output).unwrap();
    assert_eq!(info.audio.len(), 3);
    assert_eq!(info.video.unwrap().codec, "hevc", "HEVC should be remuxed, not re-encoded");
    assert!(leftover_temp_files(dir.path()).is_empty());
}

#[test]
fn convert_webm_to_mp4() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_webm_vp9_opus(dir.path());
    let paths = FfmpegPaths::resolve().unwrap();

    let outcome = run(&input, Target::Mp4).unwrap();

    let JobOutcome::Done { output, .. } = outcome else { panic!("expected Done, got {outcome:?}") };
    assert_eq!(output, dir.path().join("clip_vp9.mp4"));
    assert!(!input.exists());
    let info = probe(&paths, &output).unwrap();
    assert_eq!(info.video.unwrap().codec, "h264");
    assert_eq!(info.audio.len(), 1);
    assert_eq!(info.audio[0].codec, "aac");
    assert!(leftover_temp_files(dir.path()).is_empty());
}

#[test]
fn convert_to_mp3_mixes() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_multitrack_hevc(dir.path());
    let paths = FfmpegPaths::resolve().unwrap();

    let outcome = run(&input, Target::Mp3).unwrap();

    let JobOutcome::Done { output, .. } = outcome else { panic!("expected Done, got {outcome:?}") };
    assert_eq!(output, dir.path().join("multitrack_hevc.mp3"));
    assert!(!input.exists());
    let info = probe(&paths, &output).unwrap();
    assert_eq!(info.audio.len(), 1);
    assert_eq!(info.audio[0].codec, "mp3");
    assert!(info.video.is_none());
}

#[test]
fn convert_video_only_to_mp3_errors() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_video_only(dir.path());

    let err = run(&input, Target::Mp3).unwrap_err();

    assert_eq!(err.to_string(), "This video has no audio");
    assert!(input.exists(), "original must be untouched");
    assert!(!dir.path().join("video_only.mp3").exists());
    assert!(leftover_temp_files(dir.path()).is_empty());
}

#[test]
fn convert_same_extension_errors() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_small_h264_mp4(dir.path());

    let err = run(&input, Target::Mp4).unwrap_err();

    assert_eq!(err.to_string(), "Already a .mp4 file");
    assert!(input.exists());
}

#[test]
fn convert_picks_unique_name_when_target_exists() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_multitrack_hevc(dir.path());
    std::fs::write(dir.path().join("multitrack_hevc.mov"), b"someone else's file").unwrap();

    let outcome = run(&input, Target::Mov).unwrap();

    let JobOutcome::Done { output, .. } = outcome else { panic!("expected Done, got {outcome:?}") };
    assert_eq!(output, dir.path().join("multitrack_hevc (2).mov"));
    assert_eq!(std::fs::read(dir.path().join("multitrack_hevc.mov")).unwrap(), b"someone else's file");
}

#[test]
fn convert_cancelled_leaves_original_and_no_temp() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_multitrack_hevc(dir.path());
    let paths = FfmpegPaths::resolve().unwrap();
    let cancel = CancelToken::new();
    cancel.cancel();

    let err = convert(&paths, &input, Target::Mov, &cancel, &mut |_| {}).unwrap_err();
    assert!(matches!(err, JobError::Cancelled), "{err:?}");
    assert!(input.exists());
    assert!(!dir.path().join("multitrack_hevc.mov").exists());
    assert!(leftover_temp_files(dir.path()).is_empty());
}

#[test]
fn convert_unreadable_input_is_plain_language() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("not a video.mkv");
    std::fs::write(&input, b"this is not a video").unwrap();

    let err = run(&input, Target::Mp4).unwrap_err();

    assert_eq!(err.to_string(), "Hasami couldn't read this video.");
    assert_eq!(std::fs::read(&input).unwrap(), b"this is not a video");
    assert!(!dir.path().join("not a video.mp4").exists());
    assert!(leftover_temp_files(dir.path()).is_empty());
}
