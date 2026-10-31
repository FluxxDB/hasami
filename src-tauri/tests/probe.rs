mod common;

use hasami_lib::media::{probe, FfmpegPaths};

#[test]
fn probe_multitrack() {
    let dir = tempfile::tempdir().unwrap();
    let file = common::fixture_multitrack_hevc(dir.path());
    let paths = FfmpegPaths::resolve().unwrap();

    let info = probe(&paths, &file).unwrap();

    let video = info.video.expect("video stream");
    assert_eq!(video.codec, "hevc");
    assert_eq!((video.width, video.height), (1920, 1080));
    assert!((video.fps - 60.0).abs() < 0.01, "fps was {}", video.fps);
    assert_eq!(info.audio.len(), 3);
    assert!(info.audio.iter().all(|a| a.codec == "aac" && a.channels == 2));
    assert!((info.duration_s - 5.0).abs() < 0.1, "duration was {}", info.duration_s);
    assert_eq!(info.container, "mp4");
    assert_eq!(info.path, file);
    assert_eq!(info.size_bytes, std::fs::metadata(&file).unwrap().len());
}

#[test]
fn probe_video_only() {
    let dir = tempfile::tempdir().unwrap();
    let file = common::fixture_video_only(dir.path());
    let paths = FfmpegPaths::resolve().unwrap();

    let info = probe(&paths, &file).unwrap();

    assert!(info.video.is_some());
    assert!(info.audio.is_empty());
}

#[test]
fn probe_missing_file_errors() {
    let dir = tempfile::tempdir().unwrap();
    let paths = FfmpegPaths::resolve().unwrap();

    let result = probe(&paths, &dir.path().join("does-not-exist.mp4"));

    assert!(result.is_err());
}

#[test]
fn fixtures_have_expected_shape() {
    let dir = tempfile::tempdir().unwrap();
    let paths = FfmpegPaths::resolve().unwrap();

    let small = common::fixture_small_h264_mp4(dir.path());
    let info = probe(&paths, &small).unwrap();
    assert!(info.size_bytes < 2 * 1024 * 1024, "small fixture is {} bytes", info.size_bytes);
    assert_eq!(info.video.unwrap().codec, "h264");
    assert_eq!(info.audio.len(), 1);

    let six = common::fixture_six_tracks(dir.path());
    assert_eq!(probe(&paths, &six).unwrap().audio.len(), 6);

    let big = common::fixture_big_h264(dir.path(), 4);
    let info = probe(&paths, &big).unwrap();
    assert!(info.size_bytes > 20 * 1024 * 1024, "big fixture is {} bytes", info.size_bytes);
    assert_eq!(info.video.unwrap().codec, "h264");
}
