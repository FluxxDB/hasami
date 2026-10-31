mod common;

use hasami_lib::media::FfmpegPaths;
use hasami_lib::verify::{verify_output, VerifyError};

#[test]
fn verify_accepts_good() {
    let dir = tempfile::tempdir().unwrap();
    let file = common::fixture_small_h264_mp4(dir.path());
    let paths = FfmpegPaths::resolve().unwrap();

    let info = verify_output(&paths, &file, 2.0, Some(20_000_000)).unwrap();
    assert_eq!(info.path, file);
}

#[test]
fn verify_rejects_oversize() {
    let dir = tempfile::tempdir().unwrap();
    let file = common::fixture_small_h264_mp4(dir.path());
    let paths = FfmpegPaths::resolve().unwrap();

    let err = verify_output(&paths, &file, 2.0, Some(1000)).unwrap_err();
    assert!(matches!(err, VerifyError::TooLarge { max_bytes: 1000, .. }), "{err:?}");
}

#[test]
fn verify_rejects_wrong_duration() {
    let dir = tempfile::tempdir().unwrap();
    let file = common::fixture_small_h264_mp4(dir.path());
    let paths = FfmpegPaths::resolve().unwrap();

    let err = verify_output(&paths, &file, 99.0, None).unwrap_err();
    assert!(matches!(err, VerifyError::WrongDuration { .. }), "{err:?}");
}

#[test]
fn verify_rejects_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let paths = FfmpegPaths::resolve().unwrap();

    let err = verify_output(&paths, &dir.path().join("nope.mp4"), 2.0, None).unwrap_err();
    assert!(matches!(err, VerifyError::Probe(_)), "{err:?}");
}
