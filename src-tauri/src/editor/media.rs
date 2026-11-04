//! Prepares a video for the Quick Trim editor: which audio tracks are empty, one extracted audio
//! file per track, any fixed black border around the picture, waveform peaks, timeline thumbnails
//! and an H.264 preview proxy.
//!
//! The editor plays the proxy and the extracted tracks, never the original, so any codec the user
//! recorded in (HEVC, AV1, odd audio) still previews and mixes in the webview.

use std::ffi::OsString;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus, Stdio};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

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
/// How often a running ffmpeg is checked for exit or cancellation.
const POLL_INTERVAL: Duration = Duration::from_millis(50);
/// Short stretches of the clip sampled for a black border, spread evenly across it.
const BORDER_WINDOWS: usize = 10;
/// Seconds decoded per border window, from the keyframe at or before its start.
const BORDER_WINDOW_S: f64 = 0.25;
/// `cropdetect`: anything at most 24/255 bright counts as black, the result is rounded to even
/// sizes, and it reports the union of every frame it has seen. The limit is given as a fraction
/// so ffmpeg scales it to the source's bit depth: an absolute 24 is below 10-bit black (64).
const CROPDETECT: &str = "cropdetect=limit=0.0941:round=2:reset=0";
/// Decoder threads per border window, so ten windows at once don't each start a full set.
const BORDER_WINDOW_THREADS: &str = "2";

/// One audio track of the input.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TrackMedia {
    /// N in ffmpeg's `0:a:N`.
    pub index: usize,
    pub empty: bool,
    /// The extracted audio; `None` when the track is empty.
    pub file: Option<PathBuf>,
}

/// A rectangle of the picture in source pixels: `w`×`h` with its top-left corner at `x`,`y`.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct Crop {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PreparedMedia {
    pub info: MediaInfo,
    pub tracks: Vec<TrackMedia>,
    /// The picture area inside a fixed black border (see [`detect_border`]); `None` when there
    /// is no such border.
    pub border: Option<Crop>,
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

/// Wait for `child` to exit. A cancel kills and reaps it, so it lets go of its files.
fn wait_or_cancel(child: &mut Child, cancel: &CancelToken) -> Result<ExitStatus, JobError> {
    loop {
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(JobError::Cancelled);
        }
        if let Some(status) = child.try_wait().map_err(MediaError::Io)? {
            return Ok(status);
        }
        thread::sleep(POLL_INTERVAL);
    }
}

/// Run ffmpeg with `args` to completion and return its stderr; a failure status becomes an error.
/// Stops (killing ffmpeg) when `cancel` is set.
fn run_checked(paths: &FfmpegPaths, what: &str, args: &[OsString], cancel: &CancelToken) -> Result<Vec<u8>, JobError> {
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

/// The picture area of `input` when it sits inside a fixed black border: `cropdetect` runs on
/// [`BORDER_WINDOWS`] short stretches spread across the clip, and the area counts only when every
/// stretch agrees on it, it is smaller than the frame, and it covers at least a quarter of it.
/// `None` for no border (or no video). Stops (killing ffmpeg) when `cancel` is set.
pub fn detect_border(
    paths: &FfmpegPaths,
    input: &Path,
    duration_s: f64,
    cancel: &CancelToken,
) -> Result<Option<Crop>, JobError> {
    if cancel.is_cancelled() {
        return Err(JobError::Cancelled);
    }
    let info = probe(paths, input)?;
    match &info.video {
        Some(v) => find_border(paths, input, (v.width, v.height), duration_s, cancel),
        None => Ok(None),
    }
}

/// [`detect_border`] for a video whose displayed frame is `frame` (width, height).
fn find_border(
    paths: &FfmpegPaths,
    input: &Path,
    frame: (u32, u32),
    duration_s: f64,
    cancel: &CancelToken,
) -> Result<Option<Crop>, JobError> {
    let args = border_args(input, duration_s);
    let stderr = run_checked(paths, "Looking for a black border", &args, cancel)?;
    let found = window_crops(&String::from_utf8_lossy(&stderr), BORDER_WINDOWS);
    Ok(agreed_border(&found, frame))
}

/// One ffmpeg run over every border window: each is its own input, fast-seeked to the keyframe
/// at or before its start (so nothing before it is decoded) and read for [`BORDER_WINDOW_S`],
/// with its own `cropdetect` (named `Parsed_cropdetect_<window>` in the log).
fn border_args(input: &Path, duration_s: f64) -> Vec<OsString> {
    let duration_s = if duration_s.is_finite() && duration_s > 0.0 { duration_s } else { 0.0 };
    let mut args = Vec::new();
    let mut chains = Vec::new();
    for i in 0..BORDER_WINDOWS {
        let start = duration_s * (i as f64 + 0.5) / BORDER_WINDOWS as f64;
        args.extend([os("-threads"), os(BORDER_WINDOW_THREADS)]);
        args.extend([os("-ss"), os(&format!("{start:.3}")), os("-noaccurate_seek")]);
        args.extend([os("-t"), os(&BORDER_WINDOW_S.to_string()), os("-i"), input.as_os_str().to_os_string()]);
        chains.push(format!("[{i}:v:0]{CROPDETECT}[w{i}]"));
    }
    args.extend([os("-filter_complex"), os(&chains.join(";"))]);
    for i in 0..BORDER_WINDOWS {
        args.extend([os("-map"), os(&format!("[w{i}]"))]);
    }
    args.extend([os("-f"), os("null"), os("-")]);
    args
}

/// What each window's `cropdetect` settled on: its last `crop=w:h:x:y`, as `(x, y, w, h)` (the
/// size can come out negative for an all-black stretch). `None` for a window that reported nothing.
fn window_crops(log: &str, windows: usize) -> Vec<Option<(i64, i64, i64, i64)>> {
    let mut found = vec![None; windows];
    for line in log.lines() {
        let Some(rest) = line.strip_prefix("[Parsed_cropdetect_") else { continue };
        let Some(window) = rest.split(' ').next().and_then(|n| n.parse::<usize>().ok()) else { continue };
        let Some((_, crop)) = line.rsplit_once("crop=") else { continue };
        let n: Vec<i64> = crop.trim().split(':').filter_map(|v| v.parse().ok()).collect();
        if let (&[w, h, x, y], Some(slot)) = (n.as_slice(), found.get_mut(window)) {
            *slot = Some((x, y, w, h));
        }
    }
    found
}

/// The border every window agrees on, if it is a real one: inside `frame`, smaller than it and at
/// least a quarter of its area. An all-black window (a fade or a loading screen, reported with a
/// non-positive size) agrees with any border, but at least one window must show the picture; a
/// window that reported nothing at all disagrees.
fn agreed_border(found: &[Option<(i64, i64, i64, i64)>], frame: (u32, u32)) -> Option<Crop> {
    let all_black = |&(_, _, w, h): &(i64, i64, i64, i64)| w <= 0 || h <= 0;
    let mut seen = found.iter().copied().collect::<Option<Vec<_>>>()?;
    seen.retain(|f| !all_black(f));
    let first = *seen.first()?;
    if seen.iter().any(|f| *f != first) {
        return None;
    }
    let (x, y, w, h) = first;
    let (fw, fh) = (i64::from(frame.0), i64::from(frame.1));
    let inside = x >= 0 && y >= 0 && w > 0 && h > 0 && x + w <= fw && y + h <= fh;
    if !inside || w * h == fw * fh || w * h * 4 < fw * fh {
        return None;
    }
    Some(Crop { x: x as u32, y: y as u32, w: w as u32, h: h as u32 })
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

    /// The values, including a final partial window, at most [`MAX_PEAKS`]. Decoders often return
    /// a little more audio than the container's duration, which can tip a long input over.
    fn finish(mut self) -> Vec<f32> {
        if self.window_has_samples {
            self.close_window();
        }
        self.values.truncate(MAX_PEAKS);
        self.values
    }
}

/// Fails every read once `cancel` is set, so a streaming decode stops.
struct CancellableRead<'a, R> {
    inner: R,
    cancel: &'a CancelToken,
}

impl<R: Read> Read for CancellableRead<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.cancel.is_cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        self.inner.read(buf)
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
/// windowed as it streams in. Stops (killing ffmpeg) when `cancel` is set.
pub fn peaks(paths: &FfmpegPaths, track_file: &Path, duration_s: f64, cancel: &CancelToken) -> Result<Peaks, JobError> {
    // Held until ffmpeg is reaped (see `run_checked`).
    let _run = cancel.start_run().ok_or(JobError::Cancelled)?;
    let rate = peaks_rate(duration_s);
    let mut child = hidden_command(&paths.ffmpeg)
        .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-i"])
        .arg(track_file)
        .args(["-vn", "-ac", "1", "-ar", &PEAKS_SAMPLE_RATE.to_string(), "-f", "s16le", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(MediaError::Io)?;
    let stdout = child.stdout.take().expect("stdout was piped");
    let mut stderr = child.stderr.take().expect("stderr was piped");
    // Drained on its own thread so a chatty ffmpeg can't stall the stdout read.
    let stderr_thread = thread::spawn(move || {
        let mut text = Vec::new();
        let _ = stderr.read_to_end(&mut text);
        text
    });

    let values = peaks_from_reader(CancellableRead { inner: stdout, cancel }, rate);
    if values.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(MediaError::Io)?;
    let stderr = stderr_thread.join().unwrap_or_default();
    if cancel.is_cancelled() {
        return Err(JobError::Cancelled);
    }
    let values = values.map_err(MediaError::Io)?;
    if !status.success() {
        return Err(tool_failed("Reading the audio waveform", &stderr).into());
    }
    Ok(Peaks { rate, values })
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
    fn peaks_rate_caps_long_inputs() {
        assert_eq!(peaks_rate(90.0), 100.0);
        assert_eq!(peaks_rate(7200.0), 200_000.0 / 7200.0);
    }

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
    fn peaks_never_exceed_the_cap() {
        // rate 8000 -> one sample per value, so 200,001 samples would be 200,001 values.
        let mut acc = PeakAccumulator::new(8000.0);
        for _ in 0..=MAX_PEAKS {
            acc.push(1);
        }
        assert_eq!(acc.finish().len(), MAX_PEAKS);
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

    fn window_line(window: usize, crop: &str) -> String {
        format!(
            "[Parsed_cropdetect_{window} @ 000001d2] x1:0 x2:1439 y1:0 y2:809 w:1440 h:810 x:0 y:0 \
             pts:0 t:0.000000 limit:0.094118 crop={crop}\n"
        )
    }

    #[test]
    fn each_window_keeps_its_last_crop() {
        let mut log = String::from("Input #0, mov,mp4 ...\n");
        log += &window_line(0, "1920:1080:0:0");
        log += &window_line(1, "1440:810:0:0");
        log += &window_line(0, "1440:810:0:0");
        log += &window_line(2, "-1918:-1078:1920:1080");
        log += "[Parsed_cropdetect_9 @ 000001d2] something else\n";
        let found = window_crops(&log, 4);
        assert_eq!(found, [Some((0, 0, 1440, 810)), Some((0, 0, 1440, 810)), Some((1920, 1080, -1918, -1078)), None]);
        // A window number past the count is ignored rather than trusted.
        assert_eq!(window_crops(&window_line(12, "2:2:0:0"), 4), [None; 4]);
    }

    #[test]
    fn a_border_counts_only_when_every_window_agrees() {
        const FRAME: (u32, u32) = (1920, 1080);
        let picture = Some((0, 0, 1440, 810));
        assert_eq!(agreed_border(&[picture; 10], FRAME), Some(Crop { x: 0, y: 0, w: 1440, h: 810 }));

        let mut one_off = [picture; 10];
        one_off[7] = Some((0, 0, 1440, 808));
        assert_eq!(agreed_border(&one_off, FRAME), None, "one stretch disagrees");
        let mut silent = [picture; 10];
        silent[3] = None;
        assert_eq!(agreed_border(&silent, FRAME), None, "one stretch reported nothing");
        assert_eq!(agreed_border(&[], FRAME), None);

        // A fade to black in one stretch doesn't hide the border; an all-black clip has none.
        let black = Some((1920, 1080, -1918, -1078));
        let mut faded = [picture; 10];
        faded[0] = black;
        assert_eq!(agreed_border(&faded, FRAME), Some(Crop { x: 0, y: 0, w: 1440, h: 810 }));
        assert_eq!(agreed_border(&[black; 10], FRAME), None);
    }

    #[test]
    fn a_border_must_be_smaller_than_the_frame_and_a_quarter_of_it() {
        const FRAME: (u32, u32) = (1920, 1080);
        assert_eq!(agreed_border(&[Some((0, 0, 1920, 1080)); 3], FRAME), None, "no border at all");
        // Exactly a quarter still counts; any less doesn't.
        let quarter = Some(Crop { x: 480, y: 270, w: 960, h: 540 });
        assert_eq!(agreed_border(&[Some((480, 270, 960, 540)); 3], FRAME), quarter);
        assert_eq!(agreed_border(&[Some((480, 270, 958, 540)); 3], FRAME), None);
        // Numbers that don't fit the frame.
        assert_eq!(agreed_border(&[Some((600, 0, 1440, 810)); 3], FRAME), None);
    }

    #[test]
    fn border_windows_are_spread_across_the_clip() {
        let text: Vec<String> =
            border_args(Path::new("in.mp4"), 100.0).iter().map(|a| a.to_string_lossy().into_owned()).collect();
        let starts: Vec<&str> = text.windows(2).filter(|w| w[0] == "-ss").map(|w| w[1].as_str()).collect();
        let want: Vec<String> = (0..10).map(|i| format!("{}.000", 5 + 10 * i)).collect();
        assert_eq!(starts, want);
        let graph = &text[text.iter().position(|a| a == "-filter_complex").unwrap() + 1];
        assert!(graph.starts_with("[0:v:0]cropdetect=limit=0.0941:round=2:reset=0[w0];[1:v:0]"), "{graph}");
        assert!(graph.ends_with("[9:v:0]cropdetect=limit=0.0941:round=2:reset=0[w9]"), "{graph}");
        assert_eq!(text.windows(2).filter(|w| w[0] == "-threads" && w[1] == "2").count(), 10);
        assert_eq!(text.iter().filter(|a| *a == "-map").count(), 10);
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
