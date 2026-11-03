mod common;

use std::path::Path;

use hasami_lib::editor::media::{detect_border, make_proxy, peaks, prepare, thumbnails, Crop};
use hasami_lib::jobs::JobError;
use hasami_lib::media::run::CancelToken;
use hasami_lib::media::{hidden_command, probe, FfmpegPaths};

fn paths() -> FfmpegPaths {
    FfmpegPaths::resolve().expect("bundled ffmpeg not found")
}

/// Height in pixels from a baseline or progressive JPEG's start-of-frame marker.
fn jpeg_height(file: &Path) -> u16 {
    let bytes = std::fs::read(file).unwrap();
    assert_eq!(&bytes[..2], &[0xFF, 0xD8], "{} is not a JPEG", file.display());
    let mut i = 2;
    while i + 9 < bytes.len() {
        assert_eq!(bytes[i], 0xFF, "bad marker at {i}");
        let marker = bytes[i + 1];
        let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        if matches!(marker, 0xC0..=0xC2) {
            return u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]);
        }
        i += 2 + len;
    }
    panic!("no SOF marker in {}", file.display());
}

#[test]
fn prepare_marks_silent_tracks_empty() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_multitrack_hevc(dir.path());
    let work = dir.path().join("work");

    let prepared = prepare(&paths(), &input, &work, &CancelToken::new()).unwrap();

    assert_eq!(prepared.tracks.len(), 3);
    let empty: Vec<bool> = prepared.tracks.iter().map(|t| t.empty).collect();
    assert_eq!(empty, [false, true, false]);
    for (i, track) in prepared.tracks.iter().enumerate() {
        assert_eq!(track.index, i);
        match &track.file {
            Some(f) => {
                assert!(!track.empty);
                assert!(f.is_file(), "{} should exist", f.display());
                assert_eq!(f.extension().unwrap(), "m4a");
            }
            None => assert!(track.empty),
        }
    }
    assert_eq!(prepared.info.audio.len(), 3);
    assert_eq!(prepared.border, None, "the test pattern fills the frame");
}

#[test]
fn detect_border_finds_the_picture_inside_black() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_bordered(dir.path());

    let border = detect_border(&paths(), &input, 5.0, &CancelToken::new()).unwrap();
    assert_eq!(border, Some(Crop { x: 0, y: 0, w: 1440, h: 810 }));

    let prepared = prepare(&paths(), &input, &dir.path().join("work"), &CancelToken::new()).unwrap();
    assert_eq!(prepared.border, border);
}

#[test]
fn detect_border_without_a_border_is_none() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_multitrack_hevc(dir.path());

    assert_eq!(detect_border(&paths(), &input, 5.0, &CancelToken::new()).unwrap(), None);
}

#[test]
fn detect_border_finds_a_border_in_10_bit_video() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_bordered_10bit(dir.path());

    let border = detect_border(&paths(), &input, 5.0, &CancelToken::new()).unwrap();
    assert_eq!(border, Some(Crop { x: 0, y: 0, w: 1440, h: 810 }));
}

#[test]
fn detect_border_finds_a_pillarbox() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_pillarboxed(dir.path());

    let border = detect_border(&paths(), &input, 5.0, &CancelToken::new()).unwrap();
    assert_eq!(border, Some(Crop { x: 240, y: 0, w: 1440, h: 1080 }));
}

#[test]
fn cancelling_mid_detection_kills_ffmpeg_promptly() {
    let dir = tempfile::tempdir().unwrap();
    // Ten 1080p60 HEVC decoders take far longer than 100 ms to start and sample.
    let input = common::fixture_multitrack_hevc(dir.path());
    let cancel = CancelToken::new();
    let later = cancel.clone();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        later.cancel();
        std::time::Instant::now()
    });
    let result = detect_border(&paths(), &input, 5.0, &cancel);
    let returned = std::time::Instant::now();
    let cancelled_at = canceller.join().unwrap();
    assert!(matches!(result, Err(JobError::Cancelled)), "{result:?}");
    assert!(returned.duration_since(cancelled_at) < std::time::Duration::from_millis(500));
}

#[test]
fn detect_border_stops_when_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_bordered(dir.path());
    let cancel = CancelToken::new();
    cancel.cancel();

    assert!(matches!(detect_border(&paths(), &input, 5.0, &cancel), Err(JobError::Cancelled)));
}

#[test]
fn prepare_video_only_has_no_tracks() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_video_only(dir.path());

    let prepared = prepare(&paths(), &input, &dir.path().join("work"), &CancelToken::new()).unwrap();

    assert!(prepared.tracks.is_empty());
}

#[test]
fn peaks_follow_the_signal() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_multitrack_hevc(dir.path());
    let prepared = prepare(&paths(), &input, &dir.path().join("work"), &CancelToken::new()).unwrap();
    let track0 = prepared.tracks[0].file.as_ref().unwrap();

    let p = peaks(&paths(), track0, prepared.info.duration_s, &CancelToken::new()).unwrap();

    assert_eq!(p.rate, 100.0);
    assert!((p.values.len() as i64 - 500).abs() <= 2, "got {} values", p.values.len());
    // The fixture's sine sits at about -21 dBFS per channel (0.09), so "follows the signal"
    // means every window is well above silence (flat peaks are < 0.01 below), not above 0.2.
    let min = p.values.iter().cloned().fold(f32::MAX, f32::min);
    assert!(min > 0.05, "quietest window was {min}");
}

#[test]
fn peaks_of_silence_are_flat() {
    let dir = tempfile::tempdir().unwrap();
    let silent = dir.path().join("silent.m4a");
    let out = hidden_command(&paths().ffmpeg)
        .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo", "-t", "3", "-c:a", "aac"])
        .arg(&silent)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    let p = peaks(&paths(), &silent, 3.0, &CancelToken::new()).unwrap();

    assert!(!p.values.is_empty());
    assert!(p.values.iter().all(|v| *v < 0.01), "max was {:?}", p.values.iter().cloned().fold(0.0, f32::max));
}

#[test]
fn thumbnails_cover_the_clip() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_multitrack_hevc(dir.path());
    let out_dir = dir.path().join("thumbs");

    let thumbs = thumbnails(&paths(), &input, 5.0, &out_dir, &CancelToken::new()).unwrap();

    let times: Vec<f64> = thumbs.iter().map(|t| t.t).collect();
    assert_eq!(times, [0.0, 1.0, 2.0, 3.0, 4.0]);
    for thumb in &thumbs {
        assert_eq!(jpeg_height(&thumb.file), 72, "{}", thumb.file.display());
    }
}

#[test]
fn proxy_is_h264_720p() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_multitrack_hevc(dir.path());
    let out_dir = dir.path().join("proxy");
    let mut last = 0.0;

    let proxy = make_proxy(&paths(), &input, &out_dir, &CancelToken::new(), &mut |f| last = f).unwrap();

    assert_eq!(proxy, out_dir.join("proxy.mp4"));
    let info = probe(&paths(), &proxy).unwrap();
    let video = info.video.expect("video stream");
    assert_eq!(video.codec, "h264");
    assert_eq!(video.height, 720);
    assert!(info.audio.is_empty());
    assert!(last > 0.9, "progress reached only {last}");
}

#[test]
fn prepare_unreadable_is_plain() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("not_a_video.mp4");
    std::fs::write(&input, "this is just text").unwrap();

    let err = prepare(&paths(), &input, &dir.path().join("work"), &CancelToken::new()).err().expect("should fail");

    assert!(matches!(err, JobError::Message(_)));
    assert_eq!(err.to_string(), "Hasami couldn't read this video.");
}

#[test]
fn cancelled_preparation_stops_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_multitrack_hevc(dir.path());
    let cancelled = CancelToken::new();
    cancelled.cancel();

    let work = dir.path().join("work");
    assert!(matches!(prepare(&paths(), &input, &work, &cancelled), Err(JobError::Cancelled)));
    assert!(!work.exists());
    let thumbs = dir.path().join("thumbs");
    assert!(matches!(thumbnails(&paths(), &input, 5.0, &thumbs, &cancelled), Err(JobError::Cancelled)));
    assert!(!thumbs.exists());
}

#[test]
fn cancelling_mid_thumbnails_kills_ffmpeg_promptly() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_big_h264(dir.path(), 20);
    let cancel = CancelToken::new();
    let later = cancel.clone();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        later.cancel();
        std::time::Instant::now()
    });
    let out_dir = dir.path().join("thumbs");
    // A frame every 0.05 s of a 20 s 1080p clip takes far longer than 300 ms to make.
    let result = thumbnails(&paths(), &input, 1.0, &out_dir, &cancel);
    let returned = std::time::Instant::now();
    let cancelled_at = canceller.join().unwrap();
    assert!(matches!(result, Err(JobError::Cancelled)), "{result:?}");
    assert!(returned.duration_since(cancelled_at) < std::time::Duration::from_millis(500));
    // ffmpeg is gone, so nothing holds the folder.
    std::fs::remove_dir_all(&out_dir).unwrap();
}
