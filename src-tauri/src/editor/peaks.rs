//! Waveform peaks of an extracted audio track, decoded through a pipe and windowed as they stream
//! in, so even a long track never sits in memory whole.

use std::io::{self, Read};
use std::path::Path;
use std::process::Stdio;
use std::thread;

use serde::Serialize;

use super::media::tool_failed;
use crate::jobs::JobError;
use crate::media::run::CancelToken;
use crate::media::{hidden_command, FfmpegPaths, MediaError};

/// Waveform values per second of audio, for inputs short enough to afford it.
pub const PEAKS_PER_SECOND: f64 = 100.0;

/// Upper bound on waveform values, so a multi-hour input stays a reasonable size over IPC.
pub const MAX_PEAKS: usize = 200_000;

/// Sample rate peaks are computed at. 8 kHz keeps the decode cheap and is plenty for a waveform.
const PEAKS_SAMPLE_RATE: u32 = 8000;

/// An audio track's waveform: the loudest sample of each window, `rate` windows per second.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Peaks {
    /// Values per second.
    pub rate: f64,
    /// Each in `0..=1`.
    pub values: Vec<f32>,
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

/// Folds a stream of samples into one max-|sample| value per window. Windows are
/// `samples_per_value` long, which need not be a whole number; boundaries are rounded cumulatively
/// so the value count stays exact over a long input.
struct PeakAccumulator {
    samples_per_value: f64,
    values: Vec<f32>,
    /// Windows closed so far, including any past [`MAX_PEAKS`] that weren't kept.
    windows: u64,
    seen: u64,
    window_max: u32,
    window_has_samples: bool,
}

impl PeakAccumulator {
    fn new(rate: f64) -> Self {
        Self {
            samples_per_value: f64::from(PEAKS_SAMPLE_RATE) / rate,
            values: Vec::new(),
            windows: 0,
            seen: 0,
            window_max: 0,
            window_has_samples: false,
        }
    }

    fn push(&mut self, sample: i16) {
        self.window_max = self.window_max.max(i32::from(sample).unsigned_abs());
        self.window_has_samples = true;
        self.seen += 1;
        if self.seen as f64 >= (self.windows + 1) as f64 * self.samples_per_value {
            self.close_window();
        }
    }

    /// Ends the current window. Values past [`MAX_PEAKS`] are dropped as they come, so a decoder
    /// that returns more audio than expected can't grow the list without bound.
    fn close_window(&mut self) {
        if self.values.len() < MAX_PEAKS {
            self.values.push(self.window_max as f32 / 32768.0);
        }
        self.windows += 1;
        self.window_max = 0;
        self.window_has_samples = false;
    }

    /// The values, including a final partial window, at most [`MAX_PEAKS`]. Decoders often return
    /// a little more audio than the container's duration, which can tip a long input over.
    fn finish(mut self) -> Vec<f32> {
        if self.window_has_samples {
            self.close_window();
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peaks_rate_caps_long_inputs() {
        assert_eq!(peaks_rate(90.0), 100.0);
        assert_eq!(peaks_rate(7200.0), 200_000.0 / 7200.0);
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

    #[test]
    fn a_decoder_that_overruns_never_grows_the_values_past_the_cap() {
        let mut acc = PeakAccumulator::new(8000.0);
        for _ in 0..3 * MAX_PEAKS {
            acc.push(1);
        }
        assert!(acc.values.len() <= MAX_PEAKS, "{} values held", acc.values.len());
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
}
