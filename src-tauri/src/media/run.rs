//! Run ffmpeg with `-progress` parsing and cancellation.
//!
//! Every encode (compress, convert, editor exports) goes through [`run_ffmpeg`], which drives the
//! progress bar and the Cancel button.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::{hidden_command, FfmpegPaths};

/// Number of trailing stderr lines kept for error reports.
const STDERR_TAIL_LINES: usize = 20;
/// How often the cancel token is checked while waiting for ffmpeg output. Must stay under 200 ms.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Shared flag the UI sets to stop a running encode. Cloning shares the same flag.
///
/// It also counts the ffmpeg runs started under it ([`start_run`](Self::start_run)), so whoever
/// cancels can wait until every one of them has let go of its files
/// ([`wait_for_runs`](Self::wait_for_runs)).
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<TokenState>);

#[derive(Debug, Default)]
struct TokenState {
    cancelled: AtomicBool,
    runs: AtomicUsize,
}

/// One ffmpeg run under a [`CancelToken`], from [`CancelToken::start_run`]. Hold it until the
/// child process has exited and been reaped; dropping it ends the run.
#[derive(Debug)]
pub struct RunGuard(Arc<TokenState>);

impl Drop for RunGuard {
    fn drop(&mut self) {
        self.0.runs.fetch_sub(1, Ordering::SeqCst);
    }
}

/// How often [`CancelToken::wait_for_runs`] looks again.
const RUNS_POLL: Duration = Duration::from_millis(5);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// Request cancellation. Safe to call from any thread, any number of times.
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::SeqCst)
    }

    /// Call before spawning ffmpeg: `None` once cancelled (don't spawn it), otherwise the run's
    /// guard. The run is counted before the cancel is checked, so one starting just as the token
    /// is cancelled either stops here or is seen by [`wait_for_runs`](Self::wait_for_runs).
    pub fn start_run(&self) -> Option<RunGuard> {
        self.0.runs.fetch_add(1, Ordering::SeqCst);
        let guard = RunGuard(Arc::clone(&self.0));
        // Dropping the guard uncounts the run.
        (!self.is_cancelled()).then_some(guard)
    }

    /// Wait until no run started under this token is still going, or until `deadline`. True when
    /// none is. Cancel first, or a long run keeps it waiting to the deadline.
    pub fn wait_for_runs(&self, deadline: Instant) -> bool {
        loop {
            if self.0.runs.load(Ordering::SeqCst) == 0 {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            thread::sleep(RUNS_POLL.min(deadline - now));
        }
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
    // Declared first, so it ends after the child is reaped (locals drop in reverse order).
    let _run = cancel.start_run().ok_or(RunError::Cancelled)?;
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
        for_each_line(stdout, |line| {
            if let Some(fraction) = parse_progress_line(line, total_s) {
                // A closed receiver means the run is over; keep draining so ffmpeg never blocks.
                let _ = tx.send(fraction);
            }
        });
    });
    let stderr_thread: JoinHandle<String> = thread::spawn(move || collect_tail(stderr, STDERR_TAIL_LINES));

    let mut last = 0.0_f64;
    let outcome = loop {
        if cancel.is_cancelled() {
            child.kill_and_reap();
            break Err(RunError::Cancelled);
        }
        match rx.recv_timeout(POLL_INTERVAL) {
            Ok(fraction) => {
                // `progress=end` repeats 1.0 after the final out_time; don't report it twice.
                if fraction > last {
                    last = fraction;
                    on_progress(last);
                }
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
    let stderr_tail = stderr_thread.join().unwrap_or_default();

    let status = outcome?;
    if status.success() {
        Ok(())
    } else {
        Err(RunError::Failed { stderr_tail })
    }
}

/// Call `f` with each line of `reader` until EOF. Lines are decoded lossily: ffmpeg echoes
/// metadata in legacy codepages, and a non-UTF-8 line must not stop the drain (the child would
/// block or hit a broken pipe on its next write).
fn for_each_line(reader: impl Read, mut f: impl FnMut(&str)) {
    let mut reader = BufReader::new(reader);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => f(String::from_utf8_lossy(&buf).trim_end_matches(['\r', '\n'])),
        }
    }
}

/// Read `reader` to EOF and return its last `max_lines` lines joined by `\n`.
fn collect_tail(reader: impl Read, max_lines: usize) -> String {
    let mut tail: VecDeque<String> = VecDeque::with_capacity(max_lines);
    for_each_line(reader, |line| {
        if tail.len() == max_lines {
            tail.pop_front();
        }
        tail.push_back(line.to_owned());
    });
    tail.into_iter().collect::<Vec<_>>().join("\n")
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
    fn tail_survives_invalid_utf8_and_keeps_last_lines() {
        let mut input = b"first\n".to_vec();
        input.extend_from_slice(b"title : caf\xe9 \x93bad\x94\r\n"); // legacy codepage bytes
        input.extend_from_slice(b"valid line\n");
        let tail = super::collect_tail(&input[..], 20);
        assert!(tail.contains("valid line"), "tail was {tail:?}");
        assert!(tail.contains("caf"), "invalid line should be kept lossily: {tail:?}");
        assert_eq!(tail.lines().count(), 3);

        let many: String = (0..30).map(|i| format!("l{i}\n")).collect();
        let tail = super::collect_tail(many.as_bytes(), 20);
        assert_eq!(tail.lines().count(), 20);
        assert_eq!(tail.lines().next(), Some("l10"));
        assert_eq!(tail.lines().last(), Some("l29"));
    }

    #[test]
    fn progress_survives_invalid_utf8_line() {
        let input = b"junk \xff\xfe\nout_time_us=2500000\n";
        let mut got = Vec::new();
        super::for_each_line(&input[..], |l| got.extend(parse(l, 5.0)));
        assert_eq!(got, vec![0.5]);
    }

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
