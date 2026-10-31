//! Prepares a video for the Quick Trim editor: which audio tracks are empty, one extracted audio
//! file per track, waveform peaks, timeline thumbnails and an H.264 preview proxy.
//!
//! The editor plays the proxy and the extracted tracks, never the original, so any codec the user
//! recorded in (HEVC, AV1, odd audio) still previews and mixes in the webview.

use std::ffi::OsString;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::thread;

use serde::Serialize;

use crate::jobs::{JobError, UNREADABLE};
use crate::media::run::{run_ffmpeg, CancelToken};
use crate::media::{hidden_command, probe, FfmpegPaths, MediaError, MediaInfo};

/// A track whose loudest sample is quieter than this (dBFS) counts as empty.
pub const EMPTY_TRACK_PEAK_DB: f64 = -60.0;
/// Waveform values per second of audio, for inputs short enough to afford it.
pub const PEAKS_PER_SECOND: f64 = 100.0;
/// Upper bound on waveform values, so a multi-hour input stays a reasonable size over IPC.
pub const MAX_PEAKS: usize = 200_000;
/// Upper bound on timeline thumbnails.
pub const MAX_THUMBS: usize = 300;
/// Thumbnail height in pixels; the width follows the aspect ratio.
pub const THUMB_HEIGHT: u32 = 72;

/// Sample rate peaks are computed at. 8 kHz keeps the decode cheap and is plenty for a waveform.
const PEAKS_SAMPLE_RATE: u32 = 8000;
/// The proxy is never taller than this.
const PROXY_MAX_HEIGHT: u32 = 720;

/// One audio track of the input.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TrackMedia {
    /// N in ffmpeg's `0:a:N`.
    pub index: usize,
    pub empty: bool,
    /// The extracted audio; `None` when the track is empty.
    pub file: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PreparedMedia {
    pub info: MediaInfo,
    pub tracks: Vec<TrackMedia>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Peaks {
    /// Values per second.
    pub rate: f64,
    /// Each in `0..=1`.
    pub values: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Thumb {
    /// Seconds into the video.
    pub t: f64,
    pub file: PathBuf,
}

fn os(s: &str) -> OsString {
    OsString::from(s)
}

/// An I/O-flavoured [`MediaError`] for a tool run that exited badly.
fn tool_failed(what: &str, stderr: &[u8]) -> MediaError {
    let text = String::from_utf8_lossy(stderr);
    let tail: Vec<&str> = text.lines().rev().take(10).collect();
    let tail = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
    MediaError::Io(io::Error::other(format!("{what} failed:\n{tail}")))
}

/// Run ffmpeg with `args` to completion and return its output; a failure status becomes an error.
fn run_checked(paths: &FfmpegPaths, what: &str, args: &[OsString]) -> Result<Output, MediaError> {
    let output = hidden_command(&paths.ffmpeg)
        .args(["-hide_banner", "-nostdin", "-nostats", "-y"])
        .args(args)
        .stdin(Stdio::null())
        .output()?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(tool_failed(what, &output.stderr))
    }
}

/// Probe `input`, find its empty audio tracks and extract the others into `work_dir` as
/// `a<N>.m4a`.
pub fn prepare(paths: &FfmpegPaths, input: &Path, work_dir: &Path) -> Result<PreparedMedia, JobError> {
    let info = probe(paths, input).map_err(|_| JobError::Message(UNREADABLE.to_owned()))?;
    std::fs::create_dir_all(work_dir).map_err(MediaError::Io)?;

    let empty = detect_empty_tracks(paths, input, info.audio.len())?;
    let mut tracks = Vec::with_capacity(empty.len());
    for (index, &empty) in empty.iter().enumerate() {
        let file = if empty {
            None
        } else {
            let out = work_dir.join(format!("a{index}.m4a"));
            extract_track(paths, input, index, &info.audio[index].codec, &out)?;
            Some(out)
        };
        tracks.push(TrackMedia { index, empty, file });
    }
    Ok(PreparedMedia { info, tracks })
}

/// Whether each of the first `count` audio tracks of `input` is empty (see
/// [`EMPTY_TRACK_PEAK_DB`]).
pub fn detect_empty_tracks(paths: &FfmpegPaths, input: &Path, count: usize) -> Result<Vec<bool>, MediaError> {
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
            let output = run_checked(paths, "Measuring an audio track", &args)?;
            // No level reported means no samples were decoded at all: nothing to hear.
            Ok(parse_max_volume(&String::from_utf8_lossy(&output.stderr)).is_none_or(|db| db < EMPTY_TRACK_PEAK_DB))
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
fn extract_track(paths: &FfmpegPaths, input: &Path, n: usize, codec: &str, out: &Path) -> Result<(), MediaError> {
    let mut args = vec![os("-i"), input.as_os_str().to_os_string(), os("-map"), os(&format!("0:a:{n}")), os("-vn")];
    if codec == "aac" {
        args.extend([os("-c"), os("copy")]);
    } else {
        args.extend([os("-c:a"), os("aac"), os("-b:a"), os("192k")]);
    }
    args.push(out.as_os_str().to_os_string());
    run_checked(paths, "Extracting an audio track", &args).map(drop)
}

/// Waveform values per second for an input of `duration_s`: [`PEAKS_PER_SECOND`], lowered so the
/// total never exceeds [`MAX_PEAKS`].
pub fn peaks_rate(duration_s: f64) -> f64 {
    if duration_s > 0.0 {
        PEAKS_PER_SECOND.min(MAX_PEAKS as f64 / duration_s)
    } else {
        PEAKS_PER_SECOND
    }
}

/// Seconds between thumbnails: one a second, spread out so there are at most [`MAX_THUMBS`].
pub fn thumb_interval(duration_s: f64) -> f64 {
    (duration_s / MAX_THUMBS as f64).max(1.0)
}

/// Folds a stream of samples into one max-|sample| value per window. Windows are
/// `samples_per_value` long, which need not be a whole number; boundaries are rounded cumulatively
/// so the value count stays exact over a long input.
struct PeakAccumulator {
    samples_per_value: f64,
    values: Vec<f32>,
    seen: u64,
    window_max: u32,
    window_has_samples: bool,
}

impl PeakAccumulator {
    fn new(rate: f64) -> Self {
        Self {
            samples_per_value: f64::from(PEAKS_SAMPLE_RATE) / rate,
            values: Vec::new(),
            seen: 0,
            window_max: 0,
            window_has_samples: false,
        }
    }

    fn push(&mut self, sample: i16) {
        self.window_max = self.window_max.max(i32::from(sample).unsigned_abs());
        self.window_has_samples = true;
        self.seen += 1;
        if self.seen as f64 >= (self.values.len() + 1) as f64 * self.samples_per_value {
            self.close_window();
        }
    }

    fn close_window(&mut self) {
        self.values.push(self.window_max as f32 / 32768.0);
        self.window_max = 0;
        self.window_has_samples = false;
    }

    /// The values, including a final partial window.
    fn finish(mut self) -> Vec<f32> {
        if self.window_has_samples {
            self.close_window();
        }
        self.values
    }
}

/// Window s16le samples read from `reader` into peak values at `rate`, without holding the whole
/// stream in memory.
fn peaks_from_reader(mut reader: impl Read, rate: f64) -> io::Result<Vec<f32>> {
    let mut acc = PeakAccumulator::new(rate);
    let mut buf = vec![0u8; 64 * 1024];
    // A read can end mid-sample; the odd byte waits here for the next one.
    let mut carry: Option<u8> = None;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        let mut bytes = buf[..n].iter().copied();
        if let Some(lo) = carry.take() {
            match bytes.next() {
                Some(hi) => acc.push(i16::from_le_bytes([lo, hi])),
                None => carry = Some(lo),
            }
        }
        while let Some(lo) = bytes.next() {
            match bytes.next() {
                Some(hi) => acc.push(i16::from_le_bytes([lo, hi])),
                None => carry = Some(lo),
            }
        }
    }
    Ok(acc.finish())
}

/// Waveform of `track_file` (an extracted track), decoded to mono 8 kHz through a pipe and
/// windowed as it streams in.
pub fn peaks(paths: &FfmpegPaths, track_file: &Path, duration_s: f64) -> Result<Peaks, MediaError> {
    let rate = peaks_rate(duration_s);
    let mut child = hidden_command(&paths.ffmpeg)
        .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-i"])
        .arg(track_file)
        .args(["-vn", "-ac", "1", "-ar", &PEAKS_SAMPLE_RATE.to_string(), "-f", "s16le", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().expect("stdout was piped");
    let mut stderr = child.stderr.take().expect("stderr was piped");
    // Drained on its own thread so a chatty ffmpeg can't stall the stdout read.
    let stderr_thread = thread::spawn(move || {
        let mut text = Vec::new();
        let _ = stderr.read_to_end(&mut text);
        text
    });

    let values = peaks_from_reader(stdout, rate);
    if values.is_err() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    let stderr = stderr_thread.join().unwrap_or_default();
    let values = values?;
    if !status.success() {
        return Err(tool_failed("Reading the audio waveform", &stderr));
    }
    Ok(Peaks { rate, values })
}

/// Timeline thumbnails of `input`, one per [`thumb_interval`] seconds, as `thumb_%05d.jpg` in
/// `out_dir`. Frames at or past `duration_s` are dropped.
pub fn thumbnails(paths: &FfmpegPaths, input: &Path, duration_s: f64, out_dir: &Path) -> Result<Vec<Thumb>, MediaError> {
    std::fs::create_dir_all(out_dir)?;
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
    run_checked(paths, "Making thumbnails", &args)?;

    let mut thumbs = Vec::new();
    for k in 1.. {
        let file = out_dir.join(format!("thumb_{k:05}.jpg"));
        if !file.is_file() {
            break;
        }
        let t = (k - 1) as f64 * interval;
        if t < duration_s {
            thumbs.push(Thumb { t, file });
        } else {
            // The fps filter can emit one frame past the end; it would hang off the timeline.
            let _ = std::fs::remove_file(&file);
        }
    }
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
    fn peaks_rate_caps_long_inputs() {
        assert_eq!(peaks_rate(90.0), 100.0);
        assert_eq!(peaks_rate(7200.0), 200_000.0 / 7200.0);
    }

    #[test]
    fn thumb_interval_bounds_count() {
        assert_eq!(thumb_interval(90.0), 1.0);
        assert_eq!(thumb_interval(7200.0), 24.0);
    }

    fn s16le(samples: &[i16]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    #[test]
    fn windows_take_the_largest_absolute_sample() {
        // rate 100 -> 80-sample windows.
        let mut samples = vec![0i16; 240];
        samples[5] = -16384; // window 0
        samples[100] = 8192; // window 1
        samples[239] = i16::MIN; // window 2, full scale
        let values = peaks_from_reader(&s16le(&samples)[..], 100.0).unwrap();
        assert_eq!(values, vec![0.5, 0.25, 1.0]);
    }

    #[test]
    fn a_partial_last_window_still_counts() {
        let mut samples = vec![0i16; 100];
        samples[90] = 3276;
        let values = peaks_from_reader(&s16le(&samples)[..], 100.0).unwrap();
        assert_eq!(values.len(), 2);
        assert!((values[1] - 3276.0 / 32768.0).abs() < 1e-6);
    }

    /// Yields one byte per read, so every sample straddles a read boundary.
    struct Dribble<'a>(&'a [u8]);
    impl Read for Dribble<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match self.0.split_first() {
                Some((b, rest)) if !buf.is_empty() => {
                    buf[0] = *b;
                    self.0 = rest;
                    Ok(1)
                }
                _ => Ok(0),
            }
        }
    }

    #[test]
    fn samples_split_across_reads_are_reassembled() {
        let samples: Vec<i16> = (0..160).map(|i| if i == 100 { 1000 } else { 1 }).collect();
        let bytes = s16le(&samples);
        let whole = peaks_from_reader(&bytes[..], 100.0).unwrap();
        let split = peaks_from_reader(Dribble(&bytes), 100.0).unwrap();
        assert_eq!(whole, split);
        assert_eq!(whole.len(), 2);
    }

    #[test]
    fn fractional_windows_keep_the_count_exact() {
        // rate 27.78 -> 288-sample windows (not whole): 10 s of samples gives 278 values.
        let rate = 200_000.0 / 7200.0;
        let values = peaks_from_reader(&s16le(&vec![1i16; 80_000])[..], rate).unwrap();
        assert_eq!(values.len(), 278);
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
