//! Run ffmpeg with `-progress` parsing and cancellation.
//!
//! Every encode (compress, convert, editor exports) goes through [`run_ffmpeg`], which drives the
//! progress bar and the Cancel button.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::{hidden_command, FfmpegPaths};

/// Number of trailing stderr lines kept for error reports.
const STDERR_TAIL_LINES: usize = 20;
/// How often the cancel token is checked while waiting for ffmpeg output. Must stay under 200 ms.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Shared flag the UI sets to stop a running encode. Cloning shares the same flag.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// Request cancellation. Safe to call from any thread, any number of times.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Why [`run_ffmpeg`] did not finish successfully.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// The cancel token was set; the child was killed and reaped.
    #[error("cancelled")]
    Cancelled,

    /// ffmpeg exited with a failure status. `stderr_tail` holds its last 20 stderr lines.
    #[error("ffmpeg failed:\n{stderr_tail}")]
    Failed { stderr_tail: String },

    /// Spawning or waiting on the child process failed.
    #[error("I/O error running ffmpeg: {0}")]
    Io(#[from] std::io::Error),
}

/// Kills and reaps the child on drop unless it already exited, so no run (cancelled, failed,
/// or unwound by a panicking callback) can leave an ffmpeg process behind.
struct ChildGuard(Child);

impl ChildGuard {
    fn kill_and_reap(&mut self) {
        // Errors mean the child already exited; wait() still reaps it.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            self.kill_and_reap();
        }
    }
}

/// Run ffmpeg with `args`, reporting progress as a fraction in `[0, 1]` of `total_s` seconds.
///
/// `-hide_banner -nostdin -y -progress pipe:1 -nostats` is prepended to `args`. The cancel token is
/// polled at least every 200 ms; once set, the child is killed and reaped and
/// [`RunError::Cancelled`] is returned. `on_progress` runs on the calling thread and receives
/// non-decreasing values.
pub fn run_ffmpeg(
    paths: &FfmpegPaths,
    args: &[OsString],
    total_s: f64,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(f64),
) -> Result<(), RunError> {
    let mut child = hidden_command(&paths.ffmpeg)
        .args(["-hide_banner", "-nostdin", "-y", "-progress", "pipe:1", "-nostats"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");
    let mut child = ChildGuard(child);

    let (tx, rx) = mpsc::channel::<f64>();
    let stdout_thread = thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(fraction) = parse_progress_line(&line, total_s) {
                if tx.send(fraction).is_err() {
                    break;
                }
            }
        }
    });
    let stderr_thread: JoinHandle<VecDeque<String>> = thread::spawn(move || {
        let mut tail = VecDeque::with_capacity(STDERR_TAIL_LINES);
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if tail.len() == STDERR_TAIL_LINES {
                tail.pop_front();
            }
            tail.push_back(line);
        }
        tail
    });

    let mut last = 0.0_f64;
    let outcome = loop {
        if cancel.is_cancelled() {
            child.kill_and_reap();
            break Err(RunError::Cancelled);
        }
        match rx.recv_timeout(POLL_INTERVAL) {
            Ok(fraction) => {
                last = last.max(fraction);
                on_progress(last);
            }
            Err(RecvTimeoutError::Timeout) => {}
            // stdout closed: ffmpeg is exiting. Keep polling try_wait so cancel stays responsive.
            Err(RecvTimeoutError::Disconnected) => match child.0.try_wait()? {
                Some(status) => break Ok(status),
                None => thread::sleep(POLL_INTERVAL),
            },
        }
    };

    // The child is dead or exiting, so both pipes reach EOF and the readers finish.
    let _ = stdout_thread.join();
    let tail = stderr_thread.join().unwrap_or_default();

    let status = outcome?;
    if status.success() {
        Ok(())
    } else {
        Err(RunError::Failed {
            stderr_tail: tail.into_iter().collect::<Vec<_>>().join("\n"),
        })
    }
}

/// Interpret one line of `-progress` output as a fraction of `total_s` in `[0, 1]`.
///
/// `out_time_us=N/A` (emitted before the first frame) and unrelated keys yield `None`;
/// `progress=end` yields `1.0`.
fn parse_progress_line(line: &str, total_s: f64) -> Option<f64> {
    let line = line.trim();
    if line == "progress=end" {
        return Some(1.0);
    }
    let micros: f64 = line.strip_prefix("out_time_us=")?.trim().parse::<i64>().ok()? as f64;
    if !(total_s > 0.0) {
        return None;
    }
    Some((micros / 1_000_000.0 / total_s).clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::parse_progress_line as parse;

    #[test]
    fn parses_out_time_us() {
        assert_eq!(parse("out_time_us=2500000", 5.0), Some(0.5));
    }

    #[test]
    fn clamps_to_unit_range() {
        assert_eq!(parse("out_time_us=9000000", 5.0), Some(1.0));
        assert_eq!(parse("out_time_us=-1000", 5.0), Some(0.0));
    }

    #[test]
    fn na_and_other_keys_are_no_update() {
        assert_eq!(parse("out_time_us=N/A", 5.0), None);
        assert_eq!(parse("out_time_ms=2500000", 5.0), None);
        assert_eq!(parse("frame=12", 5.0), None);
        assert_eq!(parse("progress=continue", 5.0), None);
    }

    #[test]
    fn end_is_one_and_zero_total_is_no_update() {
        assert_eq!(parse("progress=end", 5.0), Some(1.0));
        assert_eq!(parse("out_time_us=1000", 0.0), None);
    }
}
