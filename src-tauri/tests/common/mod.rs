//! Shared integration-test helpers: fixture generator using the bundled ffmpeg (Task 3).
//!
//! Each generator writes one file into `dir` and returns its path. Fixtures are synthesized
//! with lavfi so no media files live in the repo. Ffmpeg's stderr is surfaced on failure.
#![allow(dead_code)] // each integration test binary uses only some of the fixtures

use std::path::{Path, PathBuf};

use hasami_lib::media::{hidden_command, FfmpegPaths};

/// Run the bundled ffmpeg with `args` (after `-y -hide_banner -loglevel error`), panicking with
/// its stderr if it fails.
fn ffmpeg(args: &[&str]) {
    let paths = FfmpegPaths::resolve().expect("bundled ffmpeg not found");
    let output = hidden_command(&paths.ffmpeg)
        .args(["-y", "-hide_banner", "-loglevel", "error"])
        .args(args)
        .output()
        .expect("failed to spawn ffmpeg");
    assert!(
        output.status.success(),
        "ffmpeg {:?} failed ({}):\n{}",
        args,
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn out_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(name)
}

fn s(path: &Path) -> &str {
    path.to_str().expect("fixture path is not UTF-8")
}

/// 5 s, 1920x1080 @ 60 fps HEVC with three AAC stereo tracks (440 Hz sine, silence, 880 Hz sine).
pub fn fixture_multitrack_hevc(dir: &Path) -> PathBuf {
    let out = out_path(dir, "multitrack_hevc.mp4");
    ffmpeg(&[
        "-f", "lavfi", "-i", "testsrc2=size=1920x1080:rate=60:duration=5",
        "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=5",
        "-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo",
        "-f", "lavfi", "-i", "sine=frequency=880:sample_rate=48000:duration=5",
        "-map", "0:v", "-map", "1:a", "-map", "2:a", "-map", "3:a",
        "-c:v", "libx265", "-preset", "ultrafast", "-crf", "30", "-pix_fmt", "yuv420p",
        "-tag:v", "hvc1", "-x265-params", "log-level=error",
        "-c:a", "aac", "-ac", "2",
        "-t", "5",
        s(&out),
    ]);
    out
}

/// 5 s of a `picture`-sized test pattern placed in a black 1920x1080 frame by `pad`, with one
/// AAC track, as `name` in `dir`. `video` is the video encoder's arguments.
fn boxed(dir: &Path, name: &str, picture: &str, pad: &str, video: &[&str]) -> PathBuf {
    let out = out_path(dir, name);
    let source = format!("testsrc2=size={picture}:rate=30:duration=5");
    let mut args = vec![
        "-f", "lavfi", "-i", &source,
        "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=5",
        "-vf", pad,
    ];
    args.extend(video);
    args.extend(["-c:a", "aac", "-ac", "2", "-t", "5", s(&out)]);
    ffmpeg(&args);
    out
}

const H264_8BIT: [&str; 8] = ["-c:v", "libx264", "-preset", "ultrafast", "-crf", "28", "-pix_fmt", "yuv420p"];

/// 5 s of a 1440x810 picture in the top-left corner of a black 1920x1080 frame, as some OBS
/// setups record a game, in H.264 with one AAC track.
pub fn fixture_bordered(dir: &Path) -> PathBuf {
    boxed(dir, "bordered.mp4", "1440x810", "pad=1920:1080:0:0", &H264_8BIT)
}

/// [`fixture_bordered`] as 10-bit HEVC (Main 10), whose black is 64 rather than 16.
pub fn fixture_bordered_10bit(dir: &Path) -> PathBuf {
    boxed(dir, "bordered_10bit.mp4", "1440x810", "pad=1920:1080:0:0", &[
        "-c:v", "libx265", "-preset", "ultrafast", "-crf", "30", "-pix_fmt", "yuv420p10le",
        "-tag:v", "hvc1", "-x265-params", "log-level=error",
    ])
}

/// 5 s of a 4:3 1440x1080 picture pillarboxed in the middle of a black 1920x1080 frame, in H.264
/// with one AAC track.
pub fn fixture_pillarboxed(dir: &Path) -> PathBuf {
    boxed(dir, "pillarboxed.mp4", "1440x1080", "pad=1920:1080:240:0", &H264_8BIT)
}

/// The RGB colour of the pixel at (`x`, `y`) in the first frame of `file`.
pub fn pixel(file: &Path, x: u32, y: u32) -> [u8; 3] {
    let paths = FfmpegPaths::resolve().expect("bundled ffmpeg not found");
    // 2x2, since 4:2:0 video can't be cropped to a single pixel.
    let crop = format!("crop=2:2:{x}:{y}");
    let output = hidden_command(&paths.ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(file)
        .args(["-vf", &crop, "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .output()
        .expect("failed to spawn ffmpeg");
    assert!(output.status.success(), "pixel read failed:\n{}", String::from_utf8_lossy(&output.stderr));
    [output.stdout[0], output.stdout[1], output.stdout[2]]
}

/// `secs` seconds of noisy 1080p30 H.264 at 60 Mbit/s plus one AAC track. A 4 s clip is over 20 MB.
pub fn fixture_big_h264(dir: &Path, secs: u32) -> PathBuf {
    let out = out_path(dir, "big_h264.mp4");
    let secs = secs.to_string();
    ffmpeg(&[
        "-f", "lavfi", "-i", "testsrc2=size=1920x1080:rate=30",
        "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000",
        "-vf", "noise=alls=60:allf=t",
        "-c:v", "libx264", "-preset", "ultrafast", "-b:v", "60M", "-pix_fmt", "yuv420p",
        "-c:a", "aac", "-ac", "2",
        "-t", &secs,
        s(&out),
    ]);
    out
}

/// 2 s of a 5120x1440 (32:9 super-ultrawide) test pattern in H.264 with one AAC track: too wide
/// for AMD's hardware H.264 encoder.
pub fn fixture_superwide(dir: &Path) -> PathBuf {
    let out = out_path(dir, "superwide.mp4");
    ffmpeg(&[
        "-f", "lavfi", "-i", "testsrc2=size=5120x1440:rate=30:duration=2",
        "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=2",
        "-c:v", "libx264", "-preset", "ultrafast", "-crf", "30", "-pix_fmt", "yuv420p",
        "-c:a", "aac", "-ac", "2",
        s(&out),
    ]);
    out
}

/// 3 s of 640x360 H.264 with no audio stream.
pub fn fixture_video_only(dir: &Path) -> PathBuf {
    let out = out_path(dir, "video_only.mp4");
    ffmpeg(&[
        "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=30:duration=3",
        "-c:v", "libx264", "-preset", "ultrafast", "-crf", "28", "-pix_fmt", "yuv420p",
        "-an",
        s(&out),
    ]);
    out
}

/// 2 s of 640x360 H.264 with one AAC track, under 2 MB.
pub fn fixture_small_h264_mp4(dir: &Path) -> PathBuf {
    let out = out_path(dir, "small_h264.mp4");
    ffmpeg(&[
        "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=30:duration=2",
        "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=2",
        "-c:v", "libx264", "-preset", "ultrafast", "-crf", "30", "-pix_fmt", "yuv420p",
        "-c:a", "aac", "-b:a", "96k", "-ac", "2",
        "-t", "2",
        s(&out),
    ]);
    out
}

/// 2 s of 640x360 H.264 + AAC in a `.mov` whose display matrix turns it 90°, as phones record
/// portrait video: shown as 360x640.
pub fn fixture_rotated_mov(dir: &Path) -> PathBuf {
    let plain = fixture_small_h264_mp4(dir);
    let out = out_path(dir, "rotated.mov");
    ffmpeg(&["-display_rotation", "90", "-i", s(&plain), "-c", "copy", s(&out)]);
    std::fs::remove_file(&plain).unwrap();
    out
}

/// 3 s of 320x180 H.264 with six AAC tracks: a 440 Hz sine peaking at -12 dBFS, then five silent ones.
pub fn fixture_six_tracks(dir: &Path) -> PathBuf {
    let out = out_path(dir, "six_tracks.mp4");
    let mut args: Vec<&str> = vec![
        "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=30:duration=3",
        // 10^(-12/20) = 0.2512. Two expressions make the source stereo, so `-ac 2` below
        // doesn't downmix/upmix and rescale it.
        "-f", "lavfi", "-i", "aevalsrc=0.2512*sin(2*PI*440*t)|0.2512*sin(2*PI*440*t):s=48000:d=3",
    ];
    for _ in 0..5 {
        args.extend(["-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo"]);
    }
    args.extend(["-map", "0:v"]);
    for i in ["1:a", "2:a", "3:a", "4:a", "5:a", "6:a"] {
        args.extend(["-map", i]);
    }
    args.extend([
        "-c:v", "libx264", "-preset", "ultrafast", "-crf", "30", "-pix_fmt", "yuv420p",
        "-c:a", "aac", "-ac", "2",
        "-t", "3",
        s(&out),
    ]);
    ffmpeg(&args);
    out
}

/// `secs` seconds of 1920x1080 @ 30 fps H.264 at a very low bitrate plus one AAC track, in
/// `.mkv` so it is never "already small". Cheap to generate even for long durations.
pub fn fixture_long_1080p_mkv(dir: &Path, secs: u32) -> PathBuf {
    let out = out_path(dir, "long_1080p.mkv");
    let video = format!("testsrc2=size=1920x1080:rate=30:duration={secs}");
    let audio = format!("sine=frequency=440:sample_rate=48000:duration={secs}");
    ffmpeg(&[
        "-f", "lavfi", "-i", &video,
        "-f", "lavfi", "-i", &audio,
        "-c:v", "libx264", "-preset", "ultrafast", "-b:v", "100k", "-pix_fmt", "yuv420p",
        "-c:a", "aac", "-b:a", "32k", "-ac", "1",
        s(&out),
    ]);
    out
}

/// 3 s of a 440 Hz AAC sine in `.mkv`, with no video stream.
pub fn fixture_audio_only_mkv(dir: &Path) -> PathBuf {
    let out = out_path(dir, "audio_only.mkv");
    ffmpeg(&[
        "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=3",
        "-c:a", "aac", "-ac", "2",
        s(&out),
    ]);
    out
}

/// Stream-copy `src` into `dst` (the container follows `dst`'s extension) and return `dst`.
pub fn remux(src: &Path, dst: &Path) -> PathBuf {
    ffmpeg(&["-i", s(src), "-map", "0", "-c", "copy", s(dst)]);
    dst.to_path_buf()
}

/// Run `volumedetect` on the `n`th audio stream of `file` and return the value after `key`
/// (`max_volume:` or `mean_volume:`), in dB.
fn volumedetect(file: &Path, n: usize, key: &str) -> f64 {
    let paths = FfmpegPaths::resolve().expect("bundled ffmpeg not found");
    let map = format!("0:a:{n}");
    let output = hidden_command(&paths.ffmpeg)
        .args(["-hide_banner", "-nostats", "-i"])
        .arg(file)
        .args(["-map", &map, "-af", "volumedetect", "-f", "null", "-"])
        .output()
        .expect("failed to spawn ffmpeg");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "volumedetect failed:\n{stderr}");
    stderr
        .lines()
        .find_map(|l| l.split(key).nth(1))
        .and_then(|v| v.trim().trim_end_matches("dB").trim().parse().ok())
        .unwrap_or_else(|| panic!("no {key} in volumedetect output:\n{stderr}"))
}

/// Peak level in dBFS of the `n`th audio stream of `file`, measured with ffmpeg's `volumedetect`.
pub fn max_volume_db(file: &Path, n: usize) -> f64 {
    volumedetect(file, n, "max_volume:")
}

/// Mean (RMS) level in dBFS of the `n`th audio stream of `file`, measured with `volumedetect`.
pub fn mean_volume_db(file: &Path, n: usize) -> f64 {
    volumedetect(file, n, "mean_volume:")
}

/// 2 s of 320x180 VP9 video with one Opus track, in a WebM container.
pub fn fixture_webm_vp9_opus(dir: &Path) -> PathBuf {
    let out = out_path(dir, "clip_vp9.webm");
    ffmpeg(&[
        "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=30:duration=2",
        "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=2",
        "-c:v", "libvpx-vp9", "-deadline", "realtime", "-cpu-used", "8", "-b:v", "500k",
        "-pix_fmt", "yuv420p",
        "-c:a", "libopus", "-ac", "2",
        "-t", "2",
        s(&out),
    ]);
    out
}

/// 2 s of 320x180 VP8 video with one Opus track, in a WebM container (a browser recorder's kind).
pub fn fixture_webm_vp8_opus(dir: &Path) -> PathBuf {
    let out = out_path(dir, "clip_vp8.webm");
    ffmpeg(&[
        "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=30:duration=2",
        "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=2",
        "-c:v", "libvpx", "-deadline", "realtime", "-cpu-used", "8", "-b:v", "500k",
        "-pix_fmt", "yuv420p",
        "-c:a", "libopus", "-ac", "2",
        "-t", "2",
        s(&out),
    ]);
    out
}
