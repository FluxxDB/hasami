//! Small synthetic clips for the job unit tests, made with the bundled ffmpeg. (Integration tests
//! have their own, in `tests/common`.)

use std::path::{Path, PathBuf};

use crate::media::{hidden_command, FfmpegPaths};

/// Run ffmpeg with `args` (after `-y -hide_banner -loglevel error`) writing `dir\name`.
fn make(paths: &FfmpegPaths, dir: &Path, name: &str, args: &[&[&str]]) -> PathBuf {
    let out = dir.join(name);
    let mut command = hidden_command(&paths.ffmpeg);
    command.args(["-y", "-hide_banner", "-loglevel", "error"]);
    for group in args {
        command.args(*group);
    }
    let output = command.arg(&out).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    out
}

/// 2 s of a 320x180 test pattern in H.264, with a 440 Hz AAC track when `audio` is true.
pub(super) fn small(paths: &FfmpegPaths, dir: &Path, name: &str, audio: bool) -> PathBuf {
    let video: &[&str] = &["-f", "lavfi", "-i", "testsrc2=size=320x180:rate=30:duration=2"];
    let sound: &[&str] =
        if audio { &["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=2"] } else { &[] };
    let codecs: &[&str] = &["-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-c:a", "aac"];
    make(paths, dir, name, &[video, sound, codecs])
}

/// 4 s of noisy 1080p30 H.264 at 60 Mbit/s with a 440 Hz AAC track: well over 20 MB.
pub(super) fn big(paths: &FfmpegPaths, dir: &Path, name: &str) -> PathBuf {
    make(
        paths,
        dir,
        name,
        &[
            &["-f", "lavfi", "-i", "testsrc2=size=1920x1080:rate=30:duration=4"],
            &["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=4"],
            &["-vf", "noise=alls=60:allf=t"],
            &["-c:v", "libx264", "-preset", "ultrafast", "-b:v", "60M", "-pix_fmt", "yuv420p"],
            &["-c:a", "aac", "-ac", "2"],
        ],
    )
}

/// 2 s of a 440 Hz AAC track and no video.
pub(super) fn audio_only(paths: &FfmpegPaths, dir: &Path, name: &str) -> PathBuf {
    make(paths, dir, name, &[&["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=2", "-c:a", "aac"]])
}
