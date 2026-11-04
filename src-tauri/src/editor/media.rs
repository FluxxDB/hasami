//! Prepares a video for the Quick Trim editor: which audio tracks are empty, one extracted audio
//! file per track, any fixed black border around the picture (see [`super::border`]), waveform
//! peaks (see [`super::peaks`]), timeline thumbnails and an H.264 preview proxy.
//!
//! The editor plays the extracted tracks, and the original video when the webview can decode it
//! (otherwise the proxy), so any codec the user recorded in (HEVC, AV1, odd audio) still previews
//! and mixes in the webview.

use std::ffi::OsString;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus, Stdio};
use std::thread;
use std::time::Duration;

use serde::Serialize;

use super::border::find_border;
pub use super::border::{detect_border, Crop};
pub use super::peaks::{peaks, peaks_rate, Peaks, MAX_PEAKS, PEAKS_PER_SECOND};
use crate::jobs::{JobError, UNREADABLE};
use crate::media::run::{run_ffmpeg, CancelToken};
use crate::media::{hidden_command, probe, FfmpegPaths, MediaError, MediaInfo};

/// A track whose loudest sample is quieter than this (dBFS) counts as empty.
pub const EMPTY_TRACK_PEAK_DB: f64 = -60.0;

/// Upper bound on timeline thumbnails.
pub const MAX_THUMBS: usize = 300;

/// Thumbnail height in pixels; the width follows the aspect ratio.
pub const THUMB_HEIGHT: u32 = 72;

/// The proxy is never taller than this.
const PROXY_MAX_HEIGHT: u32 = 720;

/// How often a running ffmpeg is checked for exit or cancellation.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// One audio track of the input.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TrackMedia {
    /// N in ffmpeg's `0:a:N`.
    pub index: usize,
    pub empty: bool,
    /// The extracted audio; `None` when the track is empty.
    pub file: Option<PathBuf>,
}

/// A clip ready for the editor: its probe, its audio tracks and any border.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PreparedMedia {
    pub info: MediaInfo,
    pub tracks: Vec<TrackMedia>,
    /// The picture area inside a fixed black border (see [`detect_border`]); `None` when there
    /// is no such border.
    pub border: Option<Crop>,
}

/// One timeline thumbnail: a JPEG in the session's work folder.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Thumb {
    /// Seconds into the video.
    pub t: f64,
    pub file: PathBuf,
}

pub(super) fn os(s: &str) -> OsString {
    OsString::from(s)
}

/// An I/O-flavoured [`MediaError`] for a tool run that exited badly.
pub(super) fn tool_failed(what: &str, stderr: &[u8]) -> MediaError {
    let text = String::from_utf8_lossy(stderr);
    let tail: Vec<&str> = text.lines().rev().take(10).collect();
    let tail = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
    MediaError::Io(io::Error::other(format!("{what} failed:\n{tail}")))
}

/// Wait for `child` to exit. A cancel kills and reaps it, so it lets go of its files.
fn wait_or_cancel(child: &mut Child, cancel: &CancelToken) -> Result<ExitStatus, JobError> {
    loop {
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(JobError::Cancelled);
        }
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => thread::sleep(POLL_INTERVAL),
            // Can't tell how it's doing: stop it rather than leave it running unwatched.
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(MediaError::Io(e).into());
            }
        }
    }
}

/// Run ffmpeg with `args` to completion and return its stderr; a failure status becomes an error.
/// Stops (killing ffmpeg) when `cancel` is set.
pub(super) fn run_checked(
    paths: &FfmpegPaths,
    what: &str,
    args: &[OsString],
    cancel: &CancelToken,
) -> Result<Vec<u8>, JobError> {
    // Held until ffmpeg is reaped, so closing the session waits for it to let go of its files.
    let _run = cancel.start_run().ok_or(JobError::Cancelled)?;
    let mut child = hidden_command(&paths.ffmpeg)
        .args(["-hide_banner", "-nostdin", "-nostats", "-y"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(MediaError::Io)?;
    let mut stderr = child.stderr.take().expect("stderr was piped");
    // Drained on its own thread so a chatty ffmpeg can't block on a full pipe.
    let reader = thread::spawn(move || {
        let mut text = Vec::new();
        let _ = stderr.read_to_end(&mut text);
        text
    });
    let status = wait_or_cancel(&mut child, cancel);
    let stderr = reader.join().unwrap_or_default();
    if status?.success() {
        Ok(stderr)
    } else {
        Err(tool_failed(what, &stderr).into())
    }
}

/// Probe `input`, find its empty audio tracks and extract the others into `work_dir` as
/// `a<N>.m4a`, and look for a black border. Stops with [`JobError::Cancelled`] when `cancel` is
/// set.
pub fn prepare(
    paths: &FfmpegPaths,
    input: &Path,
    work_dir: &Path,
    cancel: &CancelToken,
) -> Result<PreparedMedia, JobError> {
    let info = probe(paths, input).map_err(|_| JobError::Message(UNREADABLE.to_owned()))?;
    if cancel.is_cancelled() {
        return Err(JobError::Cancelled);
    }
    std::fs::create_dir_all(work_dir).map_err(MediaError::Io)?;

    let empty = detect_empty_tracks(paths, input, info.audio.len(), cancel)?;
    let mut tracks = Vec::with_capacity(empty.len());
    for (index, &empty) in empty.iter().enumerate() {
        let file = if empty {
            None
        } else {
            let out = work_dir.join(format!("a{index}.m4a"));
            extract_track(paths, input, index, &info.audio[index].codec, &out, cancel)?;
            Some(out)
        };
        tracks.push(TrackMedia { index, empty, file });
    }

    let border = match &info.video {
        Some(v) => match find_border(paths, input, (v.width, v.height), info.duration_s, cancel) {
            Ok(border) => border,
            Err(JobError::Cancelled) => return Err(JobError::Cancelled),
            // The border is a nicety: a clip that can't be sampled still opens, uncropped.
            Err(e) => {
                eprintln!("hasami: border detection failed for {}: {e}", input.display());
                None
            }
        },
        None => None,
    };
    Ok(PreparedMedia { info, tracks, border })
}

/// Whether each of the first `count` audio tracks of `input` is empty (see
/// [`EMPTY_TRACK_PEAK_DB`]).
pub fn detect_empty_tracks(
    paths: &FfmpegPaths,
    input: &Path,
    count: usize,
    cancel: &CancelToken,
) -> Result<Vec<bool>, JobError> {
    (0..count)
        .map(|n| {
            let args = [
                os("-i"),
                input.as_os_str().to_os_string(),
                os("-map"),
                os(&format!("0:a:{n}")),
                os("-vn"),
                os("-af"),
                os("volumedetect"),
                os("-f"),
                os("null"),
                os("-"),
            ];
            let stderr = run_checked(paths, "Measuring an audio track", &args, cancel)?;
            // No level reported means no samples were decoded at all: nothing to hear.
            Ok(parse_max_volume(&String::from_utf8_lossy(&stderr)).is_none_or(|db| db < EMPTY_TRACK_PEAK_DB))
        })
        .collect()
}

/// The `max_volume` (dBFS) in `volumedetect`'s log, `-inf` for digital silence.
fn parse_max_volume(log: &str) -> Option<f64> {
    log.lines()
        .find_map(|l| l.split("max_volume:").nth(1))
        .and_then(|v| v.trim().trim_end_matches("dB").trim().parse().ok())
}

/// Write audio track `n` of `input` to `out`: stream-copied when it is already AAC, otherwise
/// re-encoded to AAC.
fn extract_track(
    paths: &FfmpegPaths,
    input: &Path,
    n: usize,
    codec: &str,
    out: &Path,
    cancel: &CancelToken,
) -> Result<(), JobError> {
    let mut args = vec![os("-i"), input.as_os_str().to_os_string(), os("-map"), os(&format!("0:a:{n}")), os("-vn")];
    if codec == "aac" {
        args.extend([os("-c"), os("copy")]);
    } else {
        args.extend([os("-c:a"), os("aac"), os("-b:a"), os("192k")]);
    }
    args.push(out.as_os_str().to_os_string());
    run_checked(paths, "Extracting an audio track", &args, cancel).map(drop)
}

/// Seconds between thumbnails: one a second, spread out so there are at most [`MAX_THUMBS`].
pub fn thumb_interval(duration_s: f64) -> f64 {
    (duration_s / MAX_THUMBS as f64).max(1.0)
}

/// How many of `found` consecutive thumbnails (frame k at `(k - 1) * interval` seconds) to keep:
/// those inside the clip, and never more than [`MAX_THUMBS`]. The fps filter can emit a frame at
/// or past the end, and float rounding in [`thumb_interval`] can let the count creep over the cap.
fn thumbs_to_keep(found: usize, interval: f64, duration_s: f64) -> usize {
    (0..found).take_while(|&i| (i as f64) * interval < duration_s).count().min(MAX_THUMBS)
}

/// Timeline thumbnails of `input`, one per [`thumb_interval`] seconds, as `thumb_%05d.jpg` in
/// `out_dir` (any earlier `thumb_*.jpg` there is removed first). Frames at or past `duration_s`
/// are dropped, and there are never more than [`MAX_THUMBS`]. Stops (killing ffmpeg) when `cancel`
/// is set.
pub fn thumbnails(
    paths: &FfmpegPaths,
    input: &Path,
    duration_s: f64,
    out_dir: &Path,
    cancel: &CancelToken,
) -> Result<Vec<Thumb>, JobError> {
    if cancel.is_cancelled() {
        return Err(JobError::Cancelled);
    }
    std::fs::create_dir_all(out_dir).map_err(MediaError::Io)?;
    // A reused folder must not leak a previous run's higher-numbered frames into this one.
    for entry in std::fs::read_dir(out_dir).map_err(MediaError::Io)? {
        let path = entry.map_err(MediaError::Io)?.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with("thumb_") && name.ends_with(".jpg") {
            let _ = std::fs::remove_file(&path);
        }
    }
    let interval = thumb_interval(duration_s);
    let args = [
        os("-loglevel"),
        os("error"),
        os("-i"),
        input.as_os_str().to_os_string(),
        os("-map"),
        os("0:v:0"),
        os("-an"),
        os("-vf"),
        os(&format!("fps=1/{interval},scale=-2:{THUMB_HEIGHT}")),
        os("-q:v"),
        os("5"),
        out_dir.join("thumb_%05d.jpg").into_os_string(),
    ];
    run_checked(paths, "Making thumbnails", &args, cancel)?;

    let mut files = Vec::new();
    for k in 1.. {
        let file = out_dir.join(format!("thumb_{k:05}.jpg"));
        if !file.is_file() {
            break;
        }
        files.push(file);
    }
    let keep = thumbs_to_keep(files.len(), interval, duration_s);
    // Frames past the end (or past the cap) would hang off the timeline.
    for file in files.split_off(keep) {
        let _ = std::fs::remove_file(file);
    }
    let thumbs = files.into_iter().enumerate().map(|(i, file)| Thumb { t: i as f64 * interval, file }).collect();
    Ok(thumbs)
}

/// Encode a small H.264 preview of `input`'s video to `out_dir\proxy.mp4`: at most 720 p (never
/// upscaled), no audio.
pub fn make_proxy(
    paths: &FfmpegPaths,
    input: &Path,
    out_dir: &Path,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(f64),
) -> Result<PathBuf, JobError> {
    let info = probe(paths, input).map_err(|_| JobError::Message(UNREADABLE.to_owned()))?;
    std::fs::create_dir_all(out_dir).map_err(MediaError::Io)?;
    let out = out_dir.join("proxy.mp4");

    // libx264 with yuv420p needs even dimensions: -2 keeps the width even, and the height is
    // rounded down to even in case a short source is odd.
    let scale = format!("scale=-2:'min({PROXY_MAX_HEIGHT},trunc(ih/2)*2)'");
    let args = [
        os("-i"),
        input.as_os_str().to_os_string(),
        os("-map"),
        os("0:v:0"),
        os("-an"),
        os("-vf"),
        os(&scale),
        os("-c:v"),
        os("libx264"),
        os("-preset"),
        os("ultrafast"),
        os("-crf"),
        os("23"),
        os("-pix_fmt"),
        os("yuv420p"),
        out.as_os_str().to_os_string(),
    ];
    let result = run_ffmpeg(paths, &args, info.duration_s, cancel, on_progress);
    if let Err(e) = result {
        let _ = std::fs::remove_file(&out);
        return Err(e.into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumb_interval_bounds_count() {
        assert_eq!(thumb_interval(90.0), 1.0);
        assert_eq!(thumb_interval(7200.0), 24.0);
    }

    #[test]
    fn thumbnails_are_dropped_past_the_end_and_capped() {
        // A 5 s clip at 1 s spacing: the frame at t = 5 is dropped.
        assert_eq!(thumbs_to_keep(6, 1.0, 5.0), 5);
        // A long clip where the extra end-of-clip frame (or float creep) offers 301.
        let duration = 7300.5;
        let interval = thumb_interval(duration);
        assert_eq!(thumbs_to_keep(301, interval, duration), MAX_THUMBS);
        assert_eq!(thumbs_to_keep(0, 1.0, 5.0), 0);
    }

    #[test]
    fn volumedetect_levels_parse() {
        let log = "[Parsed_volumedetect_0 @ 000001] mean_volume: -91.0 dB\n\
                   [Parsed_volumedetect_0 @ 000001] max_volume: -12.3 dB\n";
        assert_eq!(parse_max_volume(log), Some(-12.3));
        assert_eq!(parse_max_volume("[x] max_volume: -inf dB"), Some(f64::NEG_INFINITY));
        assert_eq!(parse_max_volume("no levels here"), None);
    }
}
