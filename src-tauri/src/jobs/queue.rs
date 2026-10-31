//! Serial job queue (Task 9).
//!
//! One worker thread runs jobs one at a time, in enqueue order. Everything the progress window
//! needs is reported through the `emit` callback as [`JobEvent`]s, which the app forwards as the
//! `"hasami://job"` Tauri event. Events are emitted while the queue's lock is held, so they arrive
//! in a consistent order even when `enqueue`, `cancel` and the worker race. `emit` must therefore
//! be quick and must not call back into the queue.

use std::collections::VecDeque;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::{JobError, JobOutcome, JobProgress};
use crate::cli::Target;
use crate::media::run::CancelToken;

/// `Failed.message` for every user cancel. The progress window keys on this exact string.
pub const CANCELLED: &str = "Cancelled";
/// `Failed.message` when a runner breaks its contract (panics, or asks to confirm twice).
const UNEXPECTED: &str = "Something went wrong while processing this file.";

/// What a job does to its input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobKind {
    Compress,
    Convert(Target),
}

impl JobKind {
    /// `"compress"` or `"convert"`, as the progress window expects in `Started.kind`.
    pub fn name(&self) -> &'static str {
        match self {
            JobKind::Compress => "compress",
            JobKind::Convert(_) => "convert",
        }
    }

    /// Output extension of a convert job, without the dot.
    pub fn ext(&self) -> Option<&'static str> {
        match self {
            JobKind::Compress => None,
            JobKind::Convert(Target::Mp4) => Some("mp4"),
            JobKind::Convert(Target::Mov) => Some("mov"),
            JobKind::Convert(Target::Mp3) => Some("mp3"),
        }
    }
}

/// One queued job.
#[derive(Debug, Clone, PartialEq)]
pub struct JobSpec {
    pub id: u64,
    pub kind: JobKind,
    pub input: PathBuf,
}

/// Position in the current batch: "File 3 of 12". `current_index` is 1-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct QueueSnapshot {
    pub current_index: usize,
    pub total: usize,
}

/// What the progress window is told. Mirrored in `src/lib/jobs.ts`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type")]
pub enum JobEvent {
    Queued {
        snapshot: QueueSnapshot,
    },
    Started {
        id: u64,
        input: String,
        input_bytes: u64,
        /// `"compress"` | `"convert"`.
        #[serde(skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        /// Convert target extension without the dot, e.g. `"mov"`.
        #[serde(skip_serializing_if = "Option::is_none")]
        ext: Option<String>,
        /// Planned output size and frame rate; only the compress job knows them.
        #[serde(skip_serializing_if = "Option::is_none")]
        width: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        height: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        fps: Option<f64>,
    },
    Progress {
        id: u64,
        pass: u8,
        passes: u8,
        fraction: f64,
        eta_s: Option<f64>,
    },
    NeedsConfirm {
        id: u64,
        width: u32,
        height: u32,
        fps: f64,
    },
    Done {
        id: u64,
        output: String,
        bytes: u64,
    },
    Unchanged {
        id: u64,
        reason: String,
    },
    Failed {
        id: u64,
        message: String,
    },
    Drained,
}

/// Runs one job. Implemented by the real compress/convert dispatcher and by test fakes.
///
/// `confirmed` is false on the first attempt. A runner that needs the user's go-ahead (a
/// low-resolution compress) returns `Ok(JobOutcome::NeedsConfirm)`; if the user agrees, the
/// queue calls `run` again with `confirmed = true`.
pub trait JobRunner: Send {
    fn run(
        &self,
        spec: &JobSpec,
        confirmed: bool,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(JobProgress),
    ) -> Result<JobOutcome, JobError>;

    /// Planned output `(width, height, fps)`, shown in the window as soon as the job starts.
    /// Called just before `Started`; `None` when unknown (the job itself reports any error).
    fn preview(&self, _spec: &JobSpec) -> Option<(u32, u32, f64)> {
        None
    }
}

/// The serial queue. Dropping it stops the worker once the current job finishes.
pub struct JobQueue {
    shared: Arc<Shared>,
}

impl JobQueue {
    pub fn new(runner: Box<dyn JobRunner>, emit: Box<dyn Fn(JobEvent) + Send>) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                emit,
                pending: VecDeque::new(),
                next_id: 1,
                batch_total: 0,
                batch_started: 0,
                current: None,
                shutdown: false,
            }),
            wake: Condvar::new(),
        });
        let worker = shared.clone();
        thread::Builder::new()
            .name("hasami-jobs".into())
            .spawn(move || worker.work(runner))
            .expect("failed to start the job worker thread");
        Self { shared }
    }

    /// Append one job per input. Emits `Queued` with the updated batch size.
    pub fn enqueue(&self, kind: JobKind, inputs: Vec<PathBuf>) {
        if inputs.is_empty() {
            return;
        }
        let mut st = self.shared.lock();
        for input in inputs {
            let id = st.next_id;
            st.next_id += 1;
            st.pending.push_back(JobSpec { id, kind: kind.clone(), input });
            st.batch_total += 1;
        }
        st.emit_snapshot();
        self.shared.wake.notify_all();
    }

    /// Cancel a job. The running job is stopped; a waiting job is dropped. Either way the queue
    /// emits `Failed { id, message: "Cancelled" }` and moves on. Unknown ids are ignored.
    pub fn cancel(&self, id: u64) {
        let mut st = self.shared.lock();
        if let Some(current) = st.current.as_ref().filter(|c| c.id == id) {
            // The worker reports the Failed event once the runner (or the confirm wait) returns.
            current.cancel.cancel();
            self.shared.wake.notify_all();
            return;
        }
        let Some(at) = st.pending.iter().position(|spec| spec.id == id) else {
            return;
        };
        st.pending.remove(at);
        st.batch_total -= 1;
        (st.emit)(JobEvent::Failed { id, message: CANCELLED.into() });
        if st.current.is_none() && st.pending.is_empty() {
            st.drain();
        } else {
            st.emit_snapshot();
        }
    }

    /// Answer a `NeedsConfirm`. `proceed = false` cancels the job.
    pub fn confirm(&self, id: u64, proceed: bool) {
        let mut st = self.shared.lock();
        if let Some(current) = st.current.as_mut().filter(|c| c.id == id) {
            if current.confirm == Confirm::Waiting {
                current.confirm = Confirm::Answered(proceed);
                self.shared.wake.notify_all();
            }
        }
    }

    /// True when nothing is running or waiting.
    pub fn is_idle(&self) -> bool {
        let st = self.shared.lock();
        st.current.is_none() && st.pending.is_empty()
    }
}

impl Drop for JobQueue {
    fn drop(&mut self) {
        let mut st = self.shared.lock();
        st.shutdown = true;
        if let Some(current) = &st.current {
            current.cancel.cancel();
        }
        self.shared.wake.notify_all();
    }
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

struct State {
    emit: Box<dyn Fn(JobEvent) + Send>,
    pending: VecDeque<JobSpec>,
    next_id: u64,
    /// Jobs in the current batch (since the last `Drained`), minus dropped ones.
    batch_total: usize,
    /// Jobs of the current batch that have started, including the running one.
    batch_started: usize,
    current: Option<Current>,
    shutdown: bool,
}

struct Current {
    id: u64,
    cancel: CancelToken,
    confirm: Confirm,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Confirm {
    NotAsked,
    Waiting,
    Answered(bool),
}

impl State {
    fn emit_snapshot(&self) {
        (self.emit)(JobEvent::Queued {
            snapshot: QueueSnapshot {
                current_index: self.batch_started.max(1),
                total: self.batch_total,
            },
        });
    }

    fn drain(&mut self) {
        self.batch_total = 0;
        self.batch_started = 0;
        (self.emit)(JobEvent::Drained);
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        // A panic while holding the lock can only come from `emit`; keep the queue usable.
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn work(&self, runner: Box<dyn JobRunner>) {
        loop {
            let (spec, cancel) = {
                let mut st = self.lock();
                let spec = loop {
                    if st.shutdown {
                        return;
                    }
                    if let Some(spec) = st.pending.pop_front() {
                        break spec;
                    }
                    st = self.wake.wait(st).unwrap_or_else(|p| p.into_inner());
                };
                let cancel = CancelToken::new();
                st.batch_started += 1;
                st.current = Some(Current { id: spec.id, cancel: cancel.clone(), confirm: Confirm::NotAsked });
                (spec, cancel)
            };

            // File system and probe work happens outside the lock; a cancel meanwhile still lands
            // on `current` and is honoured by the runner.
            let input_bytes = std::fs::metadata(&spec.input).map(|m| m.len()).unwrap_or(0);
            let preview = panic::catch_unwind(AssertUnwindSafe(|| runner.preview(&spec))).ok().flatten();
            {
                let st = self.lock();
                st.emit_snapshot();
                (st.emit)(JobEvent::Started {
                    id: spec.id,
                    input: spec.input.display().to_string(),
                    input_bytes,
                    kind: Some(spec.kind.name().to_string()),
                    ext: spec.kind.ext().map(str::to_string),
                    width: preview.map(|p| p.0),
                    height: preview.map(|p| p.1),
                    fps: preview.map(|p| p.2),
                });
            }

            let finished = match panic::catch_unwind(AssertUnwindSafe(|| self.run_job(&*runner, &spec, &cancel))) {
                Ok(event) => event,
                Err(_) => JobEvent::Failed { id: spec.id, message: UNEXPECTED.into() },
            };

            let mut st = self.lock();
            st.current = None;
            (st.emit)(finished);
            if st.pending.is_empty() {
                st.drain();
            }
        }
    }

    /// Run one job to its end, including the confirm round-trip. Returns the final event.
    fn run_job(&self, runner: &dyn JobRunner, spec: &JobSpec, cancel: &CancelToken) -> JobEvent {
        let id = spec.id;
        let cancelled = || JobEvent::Failed { id, message: CANCELLED.into() };
        let mut confirmed = false;
        loop {
            let mut eta = EtaTracker::new();
            let mut report = |p: JobProgress| {
                let eta_s = eta.update(p.pass, p.fraction, Instant::now());
                let st = self.lock();
                (st.emit)(JobEvent::Progress { id, pass: p.pass, passes: p.passes, fraction: p.fraction, eta_s });
            };
            let result = runner.run(spec, confirmed, cancel, &mut report);
            return match result {
                Ok(JobOutcome::NeedsConfirm { plan }) if !confirmed && !cancel.is_cancelled() => {
                    if self.await_confirm(id, plan.width, plan.height, plan.fps, cancel) {
                        confirmed = true;
                        continue;
                    }
                    cancelled()
                }
                Ok(JobOutcome::NeedsConfirm { .. }) if cancel.is_cancelled() => cancelled(),
                Ok(JobOutcome::NeedsConfirm { .. }) => JobEvent::Failed { id, message: UNEXPECTED.into() },
                Ok(JobOutcome::Done { output, bytes }) => {
                    JobEvent::Done { id, output: output.display().to_string(), bytes }
                }
                Ok(JobOutcome::Unchanged { reason }) => JobEvent::Unchanged { id, reason },
                Err(_) if cancel.is_cancelled() => cancelled(),
                Err(e) => JobEvent::Failed { id, message: e.to_string() },
            };
        }
    }

    /// Emit `NeedsConfirm` and block until the user answers or cancels. True means proceed.
    fn await_confirm(&self, id: u64, width: u32, height: u32, fps: f64, cancel: &CancelToken) -> bool {
        let mut st = self.lock();
        if let Some(current) = st.current.as_mut() {
            current.confirm = Confirm::Waiting;
        }
        (st.emit)(JobEvent::NeedsConfirm { id, width, height, fps });
        loop {
            if cancel.is_cancelled() || st.shutdown {
                return false;
            }
            if let Some(Confirm::Answered(proceed)) = st.current.as_ref().map(|c| c.confirm) {
                if let Some(current) = st.current.as_mut() {
                    current.confirm = Confirm::NotAsked;
                }
                return proceed;
            }
            st = self.wake.wait(st).unwrap_or_else(|p| p.into_inner());
        }
    }
}

/// Before this much of a pass has elapsed, the rate is too noisy to show an estimate.
const ETA_MIN_ELAPSED: Duration = Duration::from_secs(1);

/// Estimates the time left in the current pass from the fraction rate since the pass began.
pub(crate) struct EtaTracker {
    pass: u8,
    start: Option<(Instant, f64)>,
}

impl EtaTracker {
    pub(crate) fn new() -> Self {
        Self { pass: 0, start: None }
    }

    /// Seconds left in `pass`, or `None` while there's no usable rate yet.
    pub(crate) fn update(&mut self, pass: u8, fraction: f64, now: Instant) -> Option<f64> {
        let (t0, f0) = match self.start {
            Some(start) if self.pass == pass => start,
            _ => {
                self.pass = pass;
                self.start = Some((now, fraction));
                return None;
            }
        };
        let elapsed = now.saturating_duration_since(t0);
        let progressed = fraction - f0;
        if elapsed < ETA_MIN_ELAPSED || progressed <= 0.0 {
            return None;
        }
        let rate = progressed / elapsed.as_secs_f64();
        Some((1.0 - fraction).max(0.0) / rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compress::CompressPlan;
    use std::sync::atomic::{AtomicBool, Ordering};

    const WAIT: Duration = Duration::from_secs(5);

    /// Collects emitted events and lets a test wait for one.
    #[derive(Clone, Default)]
    struct Events(Arc<(Mutex<Vec<JobEvent>>, Condvar)>);

    impl Events {
        fn sink(&self) -> Box<dyn Fn(JobEvent) + Send> {
            let me = self.clone();
            Box::new(move |e| {
                let (lock, cv) = &*me.0;
                lock.lock().unwrap().push(e);
                cv.notify_all();
            })
        }

        fn all(&self) -> Vec<JobEvent> {
            self.0 .0.lock().unwrap().clone()
        }

        /// Wait until `pred` holds for some event; returns the full log at that moment.
        fn wait_for(&self, what: &str, pred: impl Fn(&JobEvent) -> bool) -> Vec<JobEvent> {
            let (lock, cv) = &*self.0;
            let deadline = Instant::now() + WAIT;
            let mut log = lock.lock().unwrap();
            while !log.iter().any(&pred) {
                let left = deadline.saturating_duration_since(Instant::now());
                assert!(!left.is_zero(), "timed out waiting for {what}; got {:?}", *log);
                log = cv.wait_timeout(log, left).unwrap().0;
            }
            log.clone()
        }

        fn wait_drained(&self) -> Vec<JobEvent> {
            self.wait_for("Drained", |e| matches!(e, JobEvent::Drained))
        }
    }

    fn pos(log: &[JobEvent], pred: impl Fn(&JobEvent) -> bool) -> usize {
        log.iter().position(pred).expect("event missing")
    }

    fn started(id: u64) -> impl Fn(&JobEvent) -> bool {
        move |e| matches!(e, JobEvent::Started { id: i, .. } if *i == id)
    }

    fn done(id: u64) -> impl Fn(&JobEvent) -> bool {
        move |e| matches!(e, JobEvent::Done { id: i, .. } if *i == id)
    }

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    type Script = dyn Fn(&JobSpec, bool, &CancelToken, &mut dyn FnMut(JobProgress)) -> Result<JobOutcome, JobError>
        + Send
        + Sync;

    /// One entry per `run` call: (job id, confirmed, start, stop).
    type RunLog = Arc<Mutex<Vec<(u64, bool, Instant, Instant)>>>;

    /// Records when each run starts and stops, and delegates the behaviour to a script.
    struct FakeRunner {
        log: RunLog,
        script: Arc<Script>,
    }

    impl JobRunner for FakeRunner {
        fn run(
            &self,
            spec: &JobSpec,
            confirmed: bool,
            cancel: &CancelToken,
            progress: &mut dyn FnMut(JobProgress),
        ) -> Result<JobOutcome, JobError> {
            let start = Instant::now();
            let result = (self.script)(spec, confirmed, cancel, progress);
            self.log.lock().unwrap().push((spec.id, confirmed, start, Instant::now()));
            result
        }
    }

    fn queue_with(
        script: impl Fn(&JobSpec, bool, &CancelToken, &mut dyn FnMut(JobProgress)) -> Result<JobOutcome, JobError>
            + Send
            + Sync
            + 'static,
    ) -> (JobQueue, Events, RunLog) {
        let events = Events::default();
        let log: RunLog = Arc::default();
        let runner = FakeRunner { log: log.clone(), script: Arc::new(script) };
        (JobQueue::new(Box::new(runner), events.sink()), events, log)
    }

    fn done_outcome(spec: &JobSpec) -> Result<JobOutcome, JobError> {
        Ok(JobOutcome::Done { output: spec.input.with_extension("out"), bytes: 10 })
    }

    /// Blocks until the cancel token is set, like a real encode would.
    fn until_cancelled(cancel: &CancelToken) -> Result<JobOutcome, JobError> {
        let deadline = Instant::now() + WAIT;
        while !cancel.is_cancelled() {
            assert!(Instant::now() < deadline, "job was never cancelled");
            thread::sleep(Duration::from_millis(5));
        }
        Err(JobError::Cancelled)
    }

    fn low_res_plan() -> CompressPlan {
        CompressPlan {
            width: 426,
            height: 240,
            fps: 15.0,
            video_bps: 100_000,
            audio_bps: 32_000,
            audio_mono: true,
            warn_low_res: true,
        }
    }

    #[test]
    fn queue_runs_serially() {
        let (queue, events, log) = queue_with(|spec, _, _, progress| {
            progress(JobProgress { pass: 1, passes: 1, fraction: 0.5 });
            thread::sleep(Duration::from_millis(30));
            done_outcome(spec)
        });
        queue.enqueue(JobKind::Compress, paths(&["a.mp4", "b.mp4", "c.mp4"]));
        let all = events.wait_drained();

        let runs = log.lock().unwrap().clone();
        assert_eq!(runs.iter().map(|r| r.0).collect::<Vec<_>>(), vec![1, 2, 3]);
        for pair in runs.windows(2) {
            assert!(pair[0].3 <= pair[1].2, "jobs overlapped: {pair:?}");
        }
        let inputs: Vec<_> = all
            .iter()
            .filter_map(|e| match e {
                JobEvent::Started { input, .. } => Some(input.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(inputs, vec!["a.mp4", "b.mp4", "c.mp4"]);
        // Each job finishes before the next one starts.
        assert!(pos(&all, done(1)) < pos(&all, started(2)));
        assert!(pos(&all, done(2)) < pos(&all, started(3)));
    }

    #[test]
    fn enqueue_while_running_appends() {
        let gate = Arc::new(AtomicBool::new(false));
        let open = gate.clone();
        let (queue, events, log) = queue_with(move |spec, _, _, _| {
            if spec.id == 1 {
                let deadline = Instant::now() + WAIT;
                while !open.load(Ordering::SeqCst) && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(5));
                }
            }
            done_outcome(spec)
        });
        queue.enqueue(JobKind::Compress, paths(&["a.mp4"]));
        events.wait_for("Started 1", started(1));
        queue.enqueue(JobKind::Convert(Target::Mov), paths(&["b.mkv"]));
        let grown = events.wait_for("Queued total 2", |e| {
            matches!(e, JobEvent::Queued { snapshot } if snapshot.total == 2)
        });
        assert!(
            grown.contains(&JobEvent::Queued { snapshot: QueueSnapshot { current_index: 1, total: 2 } }),
            "{grown:?}"
        );
        assert!(!grown.iter().any(started(2)), "job 2 started while job 1 was running");
        gate.store(true, Ordering::SeqCst);

        let all = events.wait_drained();
        assert!(pos(&all, done(1)) < pos(&all, started(2)));
        assert!(all.contains(&JobEvent::Queued { snapshot: QueueSnapshot { current_index: 2, total: 2 } }));
        let runs = log.lock().unwrap().clone();
        assert!(runs[0].3 <= runs[1].2);
        // The appended job keeps its own kind.
        let second = all.iter().find(|e| started(2)(e)).unwrap();
        assert!(
            matches!(second, JobEvent::Started { kind: Some(k), ext: Some(x), .. } if k == "convert" && x == "mov"),
            "{second:?}"
        );
    }

    #[test]
    fn cancel_current_moves_on() {
        let (queue, events, _log) = queue_with(|spec, _, cancel, _| {
            if spec.id == 1 {
                until_cancelled(cancel)
            } else {
                done_outcome(spec)
            }
        });
        queue.enqueue(JobKind::Compress, paths(&["a.mp4", "b.mp4"]));
        events.wait_for("Started 1", started(1));
        queue.cancel(1);
        let all = events.wait_drained();

        let failed = pos(&all, |e| *e == JobEvent::Failed { id: 1, message: CANCELLED.into() });
        assert!(failed < pos(&all, started(2)));
        assert!(all.iter().any(done(2)));
    }

    #[test]
    fn cancel_waiting_job_drops_it() {
        let (queue, events, log) = queue_with(|spec, _, cancel, _| {
            if spec.id == 1 {
                until_cancelled(cancel)
            } else {
                done_outcome(spec)
            }
        });
        queue.enqueue(JobKind::Compress, paths(&["a.mp4", "b.mp4", "c.mp4"]));
        events.wait_for("Started 1", started(1));
        queue.cancel(2);
        queue.cancel(1);
        let all = events.wait_drained();

        assert!(all.contains(&JobEvent::Failed { id: 2, message: CANCELLED.into() }));
        assert!(!all.iter().any(started(2)));
        assert!(all.iter().any(done(3)));
        assert_eq!(log.lock().unwrap().iter().map(|r| r.0).collect::<Vec<_>>(), vec![1, 3]);
        // Job 3 is now "File 2 of 2".
        assert!(all.contains(&JobEvent::Queued { snapshot: QueueSnapshot { current_index: 2, total: 2 } }));
    }

    #[test]
    fn needs_confirm_waits_then_proceeds() {
        let (queue, events, log) = queue_with(|spec, confirmed, _, _| {
            if confirmed {
                done_outcome(spec)
            } else {
                Ok(JobOutcome::NeedsConfirm { plan: low_res_plan() })
            }
        });
        queue.enqueue(JobKind::Compress, paths(&["tiny.mp4"]));
        let asked = events.wait_for("NeedsConfirm", |e| matches!(e, JobEvent::NeedsConfirm { .. }));
        assert!(asked.contains(&JobEvent::NeedsConfirm { id: 1, width: 426, height: 240, fps: 15.0 }));

        thread::sleep(Duration::from_millis(100));
        assert!(!events.all().iter().any(done(1)), "job finished without confirmation");
        assert!(!queue.is_idle());

        queue.confirm(1, true);
        let all = events.wait_drained();
        assert!(all.iter().any(done(1)));
        let runs: Vec<_> = log.lock().unwrap().iter().map(|r| (r.0, r.1)).collect();
        assert_eq!(runs, vec![(1, false), (1, true)]);
    }

    #[test]
    fn needs_confirm_declined_is_cancelled() {
        let (queue, events, log) = queue_with(|spec, confirmed, _, _| {
            if spec.id == 1 && !confirmed {
                Ok(JobOutcome::NeedsConfirm { plan: low_res_plan() })
            } else {
                done_outcome(spec)
            }
        });
        queue.enqueue(JobKind::Compress, paths(&["tiny.mp4", "big.mp4"]));
        events.wait_for("NeedsConfirm", |e| matches!(e, JobEvent::NeedsConfirm { .. }));
        queue.confirm(1, false);
        let all = events.wait_drained();

        assert!(all.contains(&JobEvent::Failed { id: 1, message: CANCELLED.into() }));
        assert!(all.iter().any(done(2)));
        let runs: Vec<_> = log.lock().unwrap().iter().map(|r| (r.0, r.1)).collect();
        assert_eq!(runs, vec![(1, false), (2, false)]);
    }

    #[test]
    fn cancel_while_awaiting_confirm_is_cancelled() {
        let (queue, events, _log) =
            queue_with(|_, _, _, _| Ok(JobOutcome::NeedsConfirm { plan: low_res_plan() }));
        queue.enqueue(JobKind::Compress, paths(&["tiny.mp4"]));
        events.wait_for("NeedsConfirm", |e| matches!(e, JobEvent::NeedsConfirm { .. }));
        queue.cancel(1);
        let all = events.wait_drained();
        assert!(all.contains(&JobEvent::Failed { id: 1, message: CANCELLED.into() }));
    }

    #[test]
    fn drained_emitted_once() {
        let (queue, events, _log) = queue_with(|spec, _, _, _| done_outcome(spec));
        queue.enqueue(JobKind::Compress, paths(&["a.mp4", "b.mp4", "c.mp4"]));
        events.wait_drained();
        thread::sleep(Duration::from_millis(100));
        let all = events.all();
        assert_eq!(all.iter().filter(|e| **e == JobEvent::Drained).count(), 1);
        assert_eq!(all.last(), Some(&JobEvent::Drained));
        assert!(queue.is_idle());
    }

    #[test]
    fn new_batch_after_drain_restarts_numbering() {
        let (queue, events, _log) = queue_with(|spec, _, _, _| done_outcome(spec));
        queue.enqueue(JobKind::Compress, paths(&["a.mp4", "b.mp4"]));
        events.wait_drained();
        queue.enqueue(JobKind::Compress, paths(&["c.mp4"]));
        let all = events.wait_for("Done 3", done(3));
        let after_drain: Vec<_> = all.iter().skip_while(|e| **e != JobEvent::Drained).skip(1).collect();
        assert_eq!(
            after_drain[0],
            &JobEvent::Queued { snapshot: QueueSnapshot { current_index: 1, total: 1 } }
        );
    }

    #[test]
    fn errors_become_failed_with_display_message() {
        let (queue, events, _log) =
            queue_with(|_, _, _, _| Err(JobError::Message("Not wired yet".into())));
        queue.enqueue(JobKind::Compress, paths(&["a.mp4"]));
        let all = events.wait_drained();
        assert!(all.contains(&JobEvent::Failed { id: 1, message: "Not wired yet".into() }));
    }

    #[test]
    fn progress_is_forwarded_for_the_running_job() {
        let (queue, events, _log) = queue_with(|spec, _, _, progress| {
            progress(JobProgress { pass: 2, passes: 2, fraction: 0.25 });
            done_outcome(spec)
        });
        queue.enqueue(JobKind::Compress, paths(&["a.mp4"]));
        let all = events.wait_drained();
        assert!(all.contains(&JobEvent::Progress { id: 1, pass: 2, passes: 2, fraction: 0.25, eta_s: None }));
    }

    #[test]
    fn preview_fills_the_started_event() {
        struct Previewing;
        impl JobRunner for Previewing {
            fn run(
                &self,
                spec: &JobSpec,
                _: bool,
                _: &CancelToken,
                _: &mut dyn FnMut(JobProgress),
            ) -> Result<JobOutcome, JobError> {
                done_outcome(spec)
            }
            fn preview(&self, spec: &JobSpec) -> Option<(u32, u32, f64)> {
                (spec.kind == JobKind::Compress).then_some((1280, 720, 60.0))
            }
        }
        let events = Events::default();
        let queue = JobQueue::new(Box::new(Previewing), events.sink());
        queue.enqueue(JobKind::Compress, paths(&["a.mp4"]));
        queue.enqueue(JobKind::Convert(Target::Mp4), paths(&["b.mkv"]));
        let all = events.wait_for("Done 2", done(2));
        let started: Vec<_> = all.iter().filter(|e| matches!(e, JobEvent::Started { .. })).collect();
        assert!(
            matches!(started[0], JobEvent::Started { width: Some(1280), height: Some(720), fps: Some(f), .. } if *f == 60.0),
            "{:?}",
            started[0]
        );
        assert!(matches!(started[1], JobEvent::Started { width: None, height: None, fps: None, .. }));
    }

    #[test]
    fn first_snapshot_is_one_based() {
        let (queue, events, _log) = queue_with(|spec, _, _, _| done_outcome(spec));
        queue.enqueue(JobKind::Compress, paths(&["a.mp4", "b.mp4"]));
        let all = events.wait_drained();
        assert_eq!(all[0], JobEvent::Queued { snapshot: QueueSnapshot { current_index: 1, total: 2 } });
    }

    #[test]
    fn events_serialize_to_the_ui_contract() {
        let started = JobEvent::Started {
            id: 3,
            input: "C:\\v\\a.mkv".into(),
            input_bytes: 412,
            kind: Some("convert".into()),
            ext: Some("mov".into()),
            width: None,
            height: None,
            fps: None,
        };
        assert_eq!(
            serde_json::to_value(&started).unwrap(),
            serde_json::json!({"type": "Started", "id": 3, "input": "C:\\v\\a.mkv", "input_bytes": 412,
                               "kind": "convert", "ext": "mov"})
        );
        let progress = JobEvent::Progress { id: 3, pass: 1, passes: 2, fraction: 0.5, eta_s: None };
        assert_eq!(
            serde_json::to_value(&progress).unwrap(),
            serde_json::json!({"type": "Progress", "id": 3, "pass": 1, "passes": 2, "fraction": 0.5, "eta_s": null})
        );
        let queued = JobEvent::Queued { snapshot: QueueSnapshot { current_index: 3, total: 12 } };
        assert_eq!(
            serde_json::to_value(&queued).unwrap(),
            serde_json::json!({"type": "Queued", "snapshot": {"current_index": 3, "total": 12}})
        );
        assert_eq!(serde_json::to_value(&JobEvent::Drained).unwrap(), serde_json::json!({"type": "Drained"}));
    }

    #[test]
    fn eta_comes_from_the_fraction_rate_of_the_current_pass() {
        let t0 = Instant::now();
        let mut eta = EtaTracker::new();
        assert_eq!(eta.update(1, 0.0, t0), None);
        // 20% in 4 s is 0.05 per second, so the 80% left takes 16 s.
        let left = eta.update(1, 0.2, t0 + Duration::from_secs(4)).unwrap();
        assert!((left - 16.0).abs() < 1e-9, "{left}");
        // A new pass restarts the measurement.
        assert_eq!(eta.update(2, 0.0, t0 + Duration::from_secs(5)), None);
        let left = eta.update(2, 0.5, t0 + Duration::from_secs(7)).unwrap();
        assert!((left - 2.0).abs() < 1e-9, "{left}");
    }

    #[test]
    fn eta_is_unknown_until_there_is_a_rate() {
        let t0 = Instant::now();
        let mut eta = EtaTracker::new();
        eta.update(1, 0.1, t0);
        assert_eq!(eta.update(1, 0.3, t0 + Duration::from_millis(500)), None, "too soon after start");
        assert_eq!(eta.update(1, 0.1, t0 + Duration::from_secs(3)), None, "no progress yet");
    }
}
