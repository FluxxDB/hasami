//! Shared integration-test helpers: synthetic media fixtures made with the bundled ffmpeg, and
//! ways to measure the results.
//!
//! Each fixture writes one file into `dir` and returns its path. Fixtures are synthesized with lavfi
//! so no media files live in the repo. ffmpeg's stderr is surfaced on failure.
#![allow(dead_code)] // each integration test binary uses only some of the fixtures

use std::path::{Path, PathBuf};

use hasami_lib::media::{hidden_command, FfmpegPaths};

// The fixtures' ffmpeg arguments are grouped by option, one input or output setting per line.
#[rustfmt::skip]
mod fixtures;

pub use fixtures::*;

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
