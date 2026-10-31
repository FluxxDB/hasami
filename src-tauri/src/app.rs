//! Tauri setup, single-instance batch collection, progress window and commands (Task 9).
//!
//! Explorer launches one `hasami.exe <verb> <file>` per selected file. The first process becomes
//! the app; every later launch hands its argv over through the single-instance plugin and exits.
//! Actions that arrive within [`COLLECT_WINDOW`] of each other become one `enqueue` per kind.
//!
//! A single dispatcher thread owns the moving parts. Its decisions (batching, when to open the
//! progress window, buffering events until the window listens, when to exit) live in the pure
//! [`Flow`] struct, which turns inputs into [`Effect`]s; the dispatcher only carries them out.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WebviewUrl, WebviewWindowBuilder, WindowEvent};

use crate::cli::{self, Action};
use crate::compress::{plan_compress, CompressDecision};
use crate::jobs::compress::compress;
use crate::jobs::convert::convert;
use crate::jobs::queue::{JobEvent, JobKind, JobQueue, JobRunner, JobSpec, CANCELLED};
use crate::jobs::{JobError, JobOutcome, JobProgress};
use crate::media::run::CancelToken;
use crate::media::{probe, FfmpegPaths};

/// Tauri event that carries every [`JobEvent`].
pub const JOB_EVENT: &str = "hasami://job";
/// Launches closer together than this are combined into one batch.
pub const COLLECT_WINDOW: Duration = Duration::from_millis(400);
const PROGRESS_LABEL: &str = "progress";
/// Temporary, until Quick Trim lands in Plan 2.
const TRIM_NOTICE: &str = "Quick Trim arrives in the next version";
/// Every job fails with this when the bundled ffmpeg/ffprobe can't be found.
const TOOLS_MISSING: &str = "Hasami's video tools are missing. Reinstall Hasami.";

/// The one place that decides what runs a job.
fn job_runner() -> Box<dyn JobRunner> {
    let paths = FfmpegPaths::resolve()
        .map_err(|e| eprintln!("hasami: {e}"))
        .ok();
    Box::new(MediaRunner { paths })
}

/// Runs compress and convert jobs with the bundled ffmpeg.
struct MediaRunner {
    paths: Option<FfmpegPaths>,
}

impl JobRunner for MediaRunner {
    fn run(
        &self,
        spec: &JobSpec,
        confirmed: bool,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(JobProgress),
    ) -> Result<JobOutcome, JobError> {
        let paths = self.paths.as_ref().ok_or_else(|| JobError::Message(TOOLS_MISSING.into()))?;
        match &spec.kind {
            JobKind::Compress => compress(paths, &spec.input, confirmed, cancel, progress),
            JobKind::Convert(target) => convert(paths, &spec.input, target.clone(), cancel, progress),
        }
    }

    fn preview(&self, spec: &JobSpec) -> Option<(u32, u32, f64)> {
        if spec.kind != JobKind::Compress {
            return None;
        }
        let info = probe(self.paths.as_ref()?, &spec.input).ok()?;
        match plan_compress(&info).ok()? {
            CompressDecision::Encode(plan) if plan.width > 0 => Some((plan.width, plan.height, plan.fps)),
            _ => None,
        }
    }
}

/// Start the app for the first launch's `action` (never `Register`/`Unregister`).
pub fn run(action: Action) {
    let cwd = std::env::current_dir().unwrap_or_default();
    let first = resolve_paths(action, &cwd);

    let (tx, rx) = mpsc::channel::<Msg>();
    let queue_tx = tx.clone();
    // Counts events the queue has emitted, so the dispatcher knows when it has seen them all.
    let emitted = Arc::new(AtomicU64::new(0));
    let queue_emitted = emitted.clone();
    let queue = Arc::new(JobQueue::new(
        job_runner(),
        Box::new(move |event| {
            queue_emitted.fetch_add(1, Ordering::SeqCst);
            let _ = queue_tx.send(Msg::Job(event));
        }),
    ));

    let setup_tx = tx.clone();
    let setup_queue = queue.clone();
    tauri::Builder::default()
        // Must be the first plugin: a second instance forwards its argv here and exits.
        .plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            // argv includes the program name; `parse_args` wants what follows it.
            match cli::parse_args(argv.get(1..).unwrap_or_default()) {
                Ok(action) => {
                    let action = resolve_paths(action, Path::new(&cwd));
                    if let Some(state) = app.try_state::<AppState>() {
                        let _ = state.tx.send(Msg::Action(action));
                    }
                }
                Err(e) => eprintln!("hasami: ignoring launch {argv:?}: {e}"),
            }
        }))
        .manage(AppState { queue, tx })
        .invoke_handler(tauri::generate_handler![cancel_job, confirm_low_res, progress_ready])
        .on_window_event(|window, event| {
            if window.label() == PROGRESS_LABEL && matches!(event, WindowEvent::Destroyed) {
                let _ = window.state::<AppState>().tx.send(Msg::WindowDestroyed);
            }
        })
        .setup(move |app| {
            let _ = setup_tx.send(Msg::Action(first));
            let dispatcher = Dispatcher {
                app: app.handle().clone(),
                queue: setup_queue,
                emitted,
                processed: 0,
                flow: Flow::new(),
            };
            thread::Builder::new()
                .name("hasami-dispatch".into())
                .spawn(move || dispatcher.run(rx))?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Hasami")
        .run(|_app, event| {
            // Closing the last window must not end the process while jobs remain; the dispatcher
            // calls `exit` (which carries a code) once the queue has drained.
            if let RunEvent::ExitRequested { code: None, api, .. } = event {
                api.prevent_exit();
            }
        });
}

/// Explorer passes absolute paths; relative ones (from a terminal) are taken from the launch's cwd.
fn resolve_paths(action: Action, cwd: &Path) -> Action {
    let abs = |files: Vec<PathBuf>| files.into_iter().map(|f| cwd.join(f)).collect();
    match action {
        Action::Trim(files) => Action::Trim(abs(files)),
        Action::Compress(files) => Action::Compress(abs(files)),
        Action::Convert(target, files) => Action::Convert(target, abs(files)),
        other => other,
    }
}

struct AppState {
    queue: Arc<JobQueue>,
    tx: Sender<Msg>,
}

enum Msg {
    Action(Action),
    Job(JobEvent),
    ProgressReady,
    WindowDestroyed,
}

#[tauri::command]
fn cancel_job(id: u64, state: State<'_, AppState>) {
    state.queue.cancel(id);
}

#[tauri::command]
fn confirm_low_res(id: u64, proceed: bool, state: State<'_, AppState>) {
    state.queue.confirm(id, proceed);
}

/// Called by the progress window once its job-event listener is attached.
#[tauri::command]
fn progress_ready(state: State<'_, AppState>) {
    let _ = state.tx.send(Msg::ProgressReady);
}

/// Carries out [`Flow`]'s decisions against Tauri and the queue.
struct Dispatcher {
    app: AppHandle,
    queue: Arc<JobQueue>,
    emitted: Arc<AtomicU64>,
    processed: u64,
    flow: Flow,
}

impl Dispatcher {
    fn run(mut self, rx: Receiver<Msg>) {
        loop {
            let msg = match self.flow.deadline() {
                Some(deadline) => match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(msg) => Some(msg),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                },
                None => match rx.recv() {
                    Ok(msg) => Some(msg),
                    Err(_) => return,
                },
            };
            if let Some(msg) = msg {
                let effects = self.handle(msg);
                self.apply(effects);
            }
            let effects = self.flow.on_tick(Instant::now(), self.settled());
            self.apply(effects);
        }
    }

    fn handle(&mut self, msg: Msg) -> Vec<Effect> {
        match msg {
            Msg::Action(action) => {
                self.flow.on_action(action, Instant::now());
                Vec::new()
            }
            Msg::Job(event) => {
                self.processed += 1;
                let settled = self.settled();
                self.flow.on_job_event(event, settled)
            }
            Msg::ProgressReady => self.flow.on_ready(),
            Msg::WindowDestroyed => {
                let settled = self.settled();
                self.flow.on_window_closed(settled)
            }
        }
    }

    /// The queue is idle and every event it emitted (the last is always `Drained`) has been
    /// handled. Only this thread enqueues, so once idle, nothing new is emitted behind our back.
    fn settled(&self) -> bool {
        self.queue.is_idle() && self.processed == self.emitted.load(Ordering::SeqCst)
    }

    fn apply(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::OpenWindow { kind, ext } => {
                    if let Err(e) = self.open_window(kind, ext) {
                        eprintln!("hasami: could not open the progress window: {e}");
                        self.flow.on_open_failed();
                    }
                }
                Effect::Emit(event) => {
                    if let Err(e) = self.app.emit(JOB_EVENT, &event) {
                        eprintln!("hasami: could not send {event:?}: {e}");
                    }
                }
                Effect::Enqueue(kind, files) => self.queue.enqueue(kind, files),
                Effect::Decline(id) => self.queue.confirm(id, false),
                Effect::Exit => self.app.exit(0),
            }
        }
    }

    fn open_window(&self, kind: &str, ext: Option<&str>) -> tauri::Result<()> {
        let mut url = format!("progress.html?kind={kind}");
        if let Some(ext) = ext {
            url.push_str(&format!("&ext={ext}"));
        }
        WebviewWindowBuilder::new(&self.app, PROGRESS_LABEL, WebviewUrl::App(url.into()))
            .title("Hasami")
            .inner_size(440.0, 190.0)
            .resizable(false)
            .maximizable(false)
            .decorations(false)
            .center()
            .build()
            .map(|_| ())
    }
}

/// Something the dispatcher must do, decided by [`Flow`].
#[derive(Debug, PartialEq)]
pub(crate) enum Effect {
    OpenWindow { kind: &'static str, ext: Option<&'static str> },
    /// Send to the progress window (only produced once it listens).
    Emit(JobEvent),
    Enqueue(JobKind, Vec<PathBuf>),
    /// Answer a pending low-resolution question with "no".
    Decline(u64),
    Exit,
}

/// The dispatcher's decisions, free of Tauri and threads so they can be unit-tested.
///
/// `settled` arguments mean: the queue is idle and every event it emitted has been passed in.
pub(crate) struct Flow {
    collector: Collector,
    replay: Replay,
    /// `kind` (and `ext`) of the latest batch, for the window's URL. A `Started` that carries
    /// its own `kind`/`ext` overrides it in the window.
    hint: (&'static str, Option<&'static str>),
    window_open: bool,
    /// The window's listener is attached; until then events wait in `buffer`.
    ready: bool,
    buffer: Vec<JobEvent>,
    /// A Quick Trim notice waiting for the queue to drain, so it never hides a running job.
    held_trim: Option<PathBuf>,
}

impl Flow {
    pub(crate) fn new() -> Self {
        Self {
            collector: Collector::new(COLLECT_WINDOW),
            replay: Replay::default(),
            hint: ("compress", None),
            window_open: false,
            ready: false,
            buffer: Vec::new(),
            held_trim: None,
        }
    }

    /// When the open batch closes, if one is open.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.collector.deadline()
    }

    pub(crate) fn on_action(&mut self, action: Action, now: Instant) {
        self.collector.push(action, now);
    }

    /// Start the collected batch once it's due.
    pub(crate) fn on_tick(&mut self, now: Instant, settled: bool) -> Vec<Effect> {
        let mut out = Vec::new();
        let Some(batch) = self.collector.take_due(now) else {
            return out;
        };
        if let Some((kind, _)) = batch.jobs.first() {
            self.hint = (kind.name(), kind.ext());
        }
        let mut enqueued = false;
        for (kind, files) in batch.jobs.into_iter().filter(|(_, files)| !files.is_empty()) {
            enqueued = true;
            out.push(Effect::Enqueue(kind, files));
        }
        if let Some(first) = batch.trims.into_iter().next() {
            self.held_trim.get_or_insert(first);
        }
        if settled && !enqueued {
            self.show_held_trim(&mut out);
        }
        out
    }

    pub(crate) fn on_job_event(&mut self, event: JobEvent, settled: bool) -> Vec<Effect> {
        let mut out = Vec::new();
        let drained = event == JobEvent::Drained;
        let catch_up = self.replay.events();
        self.replay.apply(&event);
        if self.window_open {
            self.send(event, &mut out);
        } else if reopens_window(&event) {
            // The new window first learns which job is running, then gets this event.
            self.open_window(&mut out);
            self.buffer = catch_up;
            self.send(event, &mut out);
        }
        if drained && settled {
            self.show_held_trim(&mut out);
            self.exit_if_finished(settled, &mut out);
        }
        out
    }

    /// The window's listener is attached.
    pub(crate) fn on_ready(&mut self) -> Vec<Effect> {
        if !self.window_open {
            return Vec::new();
        }
        if self.ready {
            // The page reloaded and lost its state; bring it back up to date.
            return self.replay.events().into_iter().map(Effect::Emit).collect();
        }
        self.ready = true;
        self.buffer.drain(..).map(Effect::Emit).collect()
    }

    pub(crate) fn on_window_closed(&mut self, settled: bool) -> Vec<Effect> {
        let mut out = Vec::new();
        self.forget_window();
        // Nothing could answer the question any more: closing the window means "no".
        if let Some(id) = self.replay.awaiting_confirm() {
            out.push(Effect::Decline(id));
        }
        self.exit_if_finished(settled, &mut out);
        out
    }

    /// An `OpenWindow` effect could not be carried out; the next event tries again.
    pub(crate) fn on_open_failed(&mut self) {
        self.forget_window();
    }

    fn forget_window(&mut self) {
        self.window_open = false;
        self.ready = false;
        self.buffer.clear();
    }

    fn open_window(&mut self, out: &mut Vec<Effect>) {
        out.push(Effect::OpenWindow { kind: self.hint.0, ext: self.hint.1 });
        self.window_open = true;
        self.ready = false;
        self.buffer.clear();
    }

    fn send(&mut self, event: JobEvent, out: &mut Vec<Effect>) {
        if self.ready {
            out.push(Effect::Emit(event));
        } else {
            self.buffer.push(event);
        }
    }

    fn show_held_trim(&mut self, out: &mut Vec<Effect>) {
        let Some(first) = self.held_trim.take() else {
            return;
        };
        if !self.window_open {
            self.open_window(out);
        }
        let id = 0; // queue ids start at 1
        let started = JobEvent::Started {
            id,
            input: first.display().to_string(),
            input_bytes: 0, // not shown with a notice
            kind: Some("trim".into()),
            ext: None,
            width: None,
            height: None,
            fps: None,
        };
        self.send(started, out);
        self.send(JobEvent::Unchanged { id, reason: TRIM_NOTICE.into() }, out);
    }

    fn exit_if_finished(&self, settled: bool, out: &mut Vec<Effect>) {
        if settled && !self.window_open && self.collector.is_empty() && self.held_trim.is_none() {
            out.push(Effect::Exit);
        }
    }
}

/// Whether `event` brings back a window the user closed. Progress ticks would bring it back at
/// once, `Drained` alone has nothing to show, and the user already knows about their own cancel.
fn reopens_window(event: &JobEvent) -> bool {
    match event {
        JobEvent::Progress { .. } | JobEvent::Drained => false,
        JobEvent::Failed { message, .. } => message != CANCELLED,
        _ => true,
    }
}

/// Actions gathered during one collection window, grouped by kind in first-seen order.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Batch {
    pub jobs: Vec<(JobKind, Vec<PathBuf>)>,
    pub trims: Vec<PathBuf>,
}

/// Combines actions that arrive within `window` of each other. Time is passed in, so it's testable.
pub(crate) struct Collector {
    window: Duration,
    last: Option<Instant>,
    batch: Batch,
}

impl Collector {
    pub(crate) fn new(window: Duration) -> Self {
        Self { window, last: None, batch: Batch::default() }
    }

    pub(crate) fn push(&mut self, action: Action, now: Instant) {
        match action {
            Action::Compress(files) => self.add(JobKind::Compress, files),
            Action::Convert(target, files) => self.add(JobKind::Convert(target), files),
            Action::Trim(files) => self.batch.trims.extend(files),
            // Handled in `lib::run` before Tauri starts (Task 10), so they never reach the batch.
            Action::Register | Action::Unregister => return,
        }
        self.last = Some(now);
    }

    fn add(&mut self, kind: JobKind, files: Vec<PathBuf>) {
        match self.batch.jobs.iter_mut().find(|(k, _)| *k == kind) {
            Some((_, existing)) => existing.extend(files),
            None => self.batch.jobs.push((kind, files)),
        }
    }

    /// When the current batch closes, if one is open.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.last.map(|last| last + self.window)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.last.is_none()
    }

    /// The batch, once `window` has passed since the last action.
    pub(crate) fn take_due(&mut self, now: Instant) -> Option<Batch> {
        if now < self.deadline()? {
            return None;
        }
        self.last = None;
        Some(std::mem::take(&mut self.batch))
    }
}

/// What a freshly opened window needs to show the job that is already running.
#[derive(Default)]
struct Replay {
    queued: Option<JobEvent>,
    started: Option<JobEvent>,
    confirm: Option<JobEvent>,
}

impl Replay {
    fn events(&self) -> Vec<JobEvent> {
        [&self.queued, &self.started, &self.confirm].into_iter().flatten().cloned().collect()
    }

    /// Id of the job waiting on a `NeedsConfirm` answer.
    fn awaiting_confirm(&self) -> Option<u64> {
        self.confirm.as_ref().and_then(job_id)
    }

    fn apply(&mut self, event: &JobEvent) {
        let id = job_id(event);
        match event {
            JobEvent::Queued { .. } => self.queued = Some(event.clone()),
            JobEvent::Started { .. } => {
                self.started = Some(event.clone());
                self.confirm = None;
            }
            JobEvent::NeedsConfirm { .. } => self.confirm = Some(event.clone()),
            JobEvent::Progress { .. } => {
                if self.awaiting_confirm() == id {
                    self.confirm = None;
                }
            }
            JobEvent::Done { .. } | JobEvent::Unchanged { .. } | JobEvent::Failed { .. } => {
                if self.started.as_ref().and_then(job_id) == id {
                    self.started = None;
                }
                if self.awaiting_confirm() == id {
                    self.confirm = None;
                }
            }
            JobEvent::Drained => *self = Replay::default(),
        }
    }
}

fn job_id(event: &JobEvent) -> Option<u64> {
    match event {
        JobEvent::Started { id, .. }
        | JobEvent::Progress { id, .. }
        | JobEvent::NeedsConfirm { id, .. }
        | JobEvent::Done { id, .. }
        | JobEvent::Unchanged { id, .. }
        | JobEvent::Failed { id, .. } => Some(*id),
        JobEvent::Queued { .. } | JobEvent::Drained => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Target;
    use crate::jobs::queue::QueueSnapshot;

    fn files(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn launches_within_the_window_form_one_batch_per_kind() {
        let t0 = Instant::now();
        let mut c = Collector::new(COLLECT_WINDOW);
        c.push(Action::Compress(files(&["a.mp4"])), t0);
        c.push(Action::Convert(Target::Mov, files(&["b.mkv"])), t0 + ms(300));
        c.push(Action::Compress(files(&["c.mp4"])), t0 + ms(600));

        // Each launch extends the window: 400 ms after the last one, not the first.
        assert_eq!(c.take_due(t0 + ms(999)), None);
        let batch = c.take_due(t0 + ms(1000)).expect("batch due");
        assert_eq!(
            batch.jobs,
            vec![
                (JobKind::Compress, files(&["a.mp4", "c.mp4"])),
                (JobKind::Convert(Target::Mov), files(&["b.mkv"])),
            ]
        );
        assert!(batch.trims.is_empty());
        assert!(c.is_empty());
        assert_eq!(c.deadline(), None);
    }

    #[test]
    fn a_gap_longer_than_the_window_starts_a_new_batch() {
        let t0 = Instant::now();
        let mut c = Collector::new(COLLECT_WINDOW);
        c.push(Action::Compress(files(&["a.mp4"])), t0);
        assert_eq!(c.deadline(), Some(t0 + ms(400)));
        let first = c.take_due(t0 + ms(401)).unwrap();
        assert_eq!(first.jobs, vec![(JobKind::Compress, files(&["a.mp4"]))]);

        c.push(Action::Compress(files(&["b.mp4"])), t0 + ms(900));
        assert_eq!(c.take_due(t0 + ms(1200)), None);
        let second = c.take_due(t0 + ms(1300)).unwrap();
        assert_eq!(second.jobs, vec![(JobKind::Compress, files(&["b.mp4"]))]);
    }

    #[test]
    fn convert_targets_are_separate_kinds() {
        let t0 = Instant::now();
        let mut c = Collector::new(COLLECT_WINDOW);
        c.push(Action::Convert(Target::Mp4, files(&["a.mkv"])), t0);
        c.push(Action::Convert(Target::Mp3, files(&["b.mkv"])), t0 + ms(10));
        c.push(Action::Convert(Target::Mp4, files(&["c.mkv"])), t0 + ms(20));
        let batch = c.take_due(t0 + ms(420)).unwrap();
        assert_eq!(
            batch.jobs,
            vec![
                (JobKind::Convert(Target::Mp4), files(&["a.mkv", "c.mkv"])),
                (JobKind::Convert(Target::Mp3), files(&["b.mkv"])),
            ]
        );
    }

    #[test]
    fn trims_are_collected_apart_from_jobs() {
        let t0 = Instant::now();
        let mut c = Collector::new(COLLECT_WINDOW);
        c.push(Action::Trim(files(&["a.mp4"])), t0);
        c.push(Action::Trim(files(&["b.mp4"])), t0 + ms(100));
        let batch = c.take_due(t0 + ms(500)).unwrap();
        assert!(batch.jobs.is_empty());
        assert_eq!(batch.trims, files(&["a.mp4", "b.mp4"]));
    }

    #[test]
    fn register_never_opens_a_batch() {
        let t0 = Instant::now();
        let mut c = Collector::new(COLLECT_WINDOW);
        c.push(Action::Register, t0);
        c.push(Action::Unregister, t0);
        assert!(c.is_empty());
        assert_eq!(c.take_due(t0 + ms(1000)), None);
    }

    #[test]
    fn relative_paths_resolve_against_the_launch_cwd() {
        let cwd = Path::new("C:\\clips");
        let action = resolve_paths(Action::Compress(files(&["a.mp4", "D:\\b.mp4"])), cwd);
        assert_eq!(action, Action::Compress(files(&["C:\\clips\\a.mp4", "D:\\b.mp4"])));
    }

    // ---- Flow ----

    fn queued(index: usize, total: usize) -> JobEvent {
        JobEvent::Queued { snapshot: QueueSnapshot { current_index: index, total } }
    }

    fn started(id: u64) -> JobEvent {
        JobEvent::Started {
            id,
            input: format!("{id}.mp4"),
            input_bytes: 1,
            kind: Some("compress".into()),
            ext: None,
            width: None,
            height: None,
            fps: None,
        }
    }

    fn progress(id: u64) -> JobEvent {
        JobEvent::Progress { id, pass: 1, passes: 2, fraction: 0.5, eta_s: None }
    }

    fn failed(id: u64, message: &str) -> JobEvent {
        JobEvent::Failed { id, message: message.into(), original_intact: true }
    }

    fn done(id: u64) -> JobEvent {
        JobEvent::Done { id, output: format!("{id}.out.mp4"), bytes: 2 }
    }

    fn emits(events: &[JobEvent]) -> Vec<Effect> {
        events.iter().cloned().map(Effect::Emit).collect()
    }

    const OPEN_COMPRESS: Effect = Effect::OpenWindow { kind: "compress", ext: None };

    fn trim_notice(path: &str) -> Vec<JobEvent> {
        vec![
            JobEvent::Started {
                id: 0,
                input: path.into(),
                input_bytes: 0,
                kind: Some("trim".into()),
                ext: None,
                width: None,
                height: None,
                fps: None,
            },
            JobEvent::Unchanged { id: 0, reason: TRIM_NOTICE.into() },
        ]
    }

    /// A flow whose window is open and listening, showing job 1 of 2.
    fn showing_job_1() -> Flow {
        let mut f = Flow::new();
        assert_eq!(f.on_job_event(queued(1, 2), false), vec![OPEN_COMPRESS]);
        f.on_job_event(started(1), false);
        f.on_ready();
        f
    }

    #[test]
    fn a_batch_enqueues_each_kind_and_opens_the_window_on_its_first_event() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        f.on_action(Action::Convert(Target::Mov, files(&["a.mkv"])), t0);
        f.on_action(Action::Compress(files(&["b.mp4"])), t0 + ms(100));
        assert_eq!(f.deadline(), Some(t0 + ms(500)));
        assert_eq!(f.on_tick(t0 + ms(499), true), vec![]);
        assert_eq!(
            f.on_tick(t0 + ms(500), true),
            vec![
                Effect::Enqueue(JobKind::Convert(Target::Mov), files(&["a.mkv"])),
                Effect::Enqueue(JobKind::Compress, files(&["b.mp4"])),
            ]
        );
        assert_eq!(
            f.on_job_event(queued(1, 2), false),
            vec![Effect::OpenWindow { kind: "convert", ext: Some("mov") }]
        );
    }

    #[test]
    fn events_wait_for_the_listener_then_flush_in_order() {
        let mut f = Flow::new();
        assert_eq!(f.on_job_event(queued(1, 1), false), vec![OPEN_COMPRESS]);
        assert_eq!(f.on_job_event(started(1), false), vec![]);
        assert_eq!(f.on_job_event(failed(1, "boom"), false), vec![]);
        assert_eq!(f.on_ready(), emits(&[queued(1, 1), started(1), failed(1, "boom")]));
        assert_eq!(f.on_job_event(JobEvent::Drained, true), emits(&[JobEvent::Drained]));
    }

    #[test]
    fn a_reloaded_page_gets_the_running_job_again() {
        let mut f = showing_job_1();
        assert_eq!(f.on_ready(), emits(&[queued(1, 2), started(1)]));
    }

    #[test]
    fn closing_mid_job_keeps_the_queue_and_reopens_on_the_next_real_event() {
        let mut f = showing_job_1();
        assert_eq!(f.on_window_closed(false), vec![], "no exit while jobs remain");
        assert_eq!(f.on_job_event(progress(1), false), vec![], "progress ticks don't reopen");
        assert_eq!(f.on_job_event(done(1), false), vec![OPEN_COMPRESS]);
        // The new window learns which job ran before it hears how it ended.
        assert_eq!(f.on_ready(), emits(&[queued(1, 2), started(1), done(1)]));
    }

    #[test]
    fn closing_during_confirm_declines_and_then_exits() {
        let mut f = showing_job_1();
        let ask = JobEvent::NeedsConfirm { id: 1, width: 426, height: 240, fps: 15.0 };
        assert_eq!(f.on_job_event(ask.clone(), false), emits(&[ask]));
        assert_eq!(f.on_window_closed(false), vec![Effect::Decline(1)]);
        // The resulting cancel doesn't bring the window back; the drain ends the process.
        assert_eq!(f.on_job_event(failed(1, CANCELLED), false), vec![]);
        assert_eq!(f.on_job_event(JobEvent::Drained, true), vec![Effect::Exit]);
    }

    #[test]
    fn exit_waits_until_the_last_event_is_handled() {
        let mut f = showing_job_1();
        f.on_window_closed(false);
        assert_eq!(f.on_job_event(JobEvent::Drained, false), vec![], "not settled yet");
        assert_eq!(f.on_job_event(JobEvent::Drained, true), vec![Effect::Exit]);
    }

    #[test]
    fn no_exit_while_the_window_is_open() {
        let mut f = showing_job_1();
        assert_eq!(f.on_job_event(JobEvent::Drained, true), emits(&[JobEvent::Drained]));
        assert_eq!(f.on_window_closed(true), vec![Effect::Exit]);
    }

    #[test]
    fn no_exit_while_a_batch_is_being_collected() {
        let t0 = Instant::now();
        let mut f = showing_job_1();
        f.on_job_event(JobEvent::Drained, true);
        f.on_action(Action::Compress(files(&["c.mp4"])), t0);
        assert_eq!(f.on_window_closed(true), vec![]);
    }

    #[test]
    fn a_trim_during_a_job_waits_for_the_drain() {
        let t0 = Instant::now();
        let mut f = showing_job_1();
        f.on_action(Action::Trim(files(&["t.mp4"])), t0);
        assert_eq!(f.on_tick(t0 + ms(400), false), vec![], "the running job keeps the window");
        assert_eq!(f.on_job_event(failed(1, "boom"), false), emits(&[failed(1, "boom")]));
        let mut expected = emits(&[JobEvent::Drained]);
        expected.extend(emits(&trim_notice("t.mp4")));
        assert_eq!(f.on_job_event(JobEvent::Drained, true), expected);
    }

    #[test]
    fn a_trim_in_the_same_batch_as_jobs_waits_for_their_drain() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        f.on_action(Action::Compress(files(&["a.mp4"])), t0);
        f.on_action(Action::Trim(files(&["t.mp4"])), t0);
        assert_eq!(
            f.on_tick(t0 + ms(400), true),
            vec![Effect::Enqueue(JobKind::Compress, files(&["a.mp4"]))]
        );
    }

    #[test]
    fn a_trim_alone_shows_at_once_and_closing_it_exits() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        f.on_action(Action::Trim(files(&["t.mp4", "u.mp4"])), t0);
        assert_eq!(f.on_tick(t0 + ms(400), true), vec![OPEN_COMPRESS]);
        assert_eq!(f.on_ready(), emits(&trim_notice("t.mp4")));
        assert_eq!(f.on_window_closed(true), vec![Effect::Exit]);
    }

    #[test]
    fn a_failed_window_open_is_retried_on_the_next_event() {
        let mut f = Flow::new();
        assert_eq!(f.on_job_event(queued(1, 1), false), vec![OPEN_COMPRESS]);
        f.on_open_failed();
        assert_eq!(f.on_job_event(started(1), false), vec![OPEN_COMPRESS]);
        assert_eq!(f.on_ready(), emits(&[queued(1, 1), started(1)]));
    }

    #[test]
    fn replay_tracks_only_the_running_job() {
        let mut r = Replay::default();
        r.apply(&queued(2, 3));
        r.apply(&started(2));
        r.apply(&progress(2));
        assert_eq!(r.events(), vec![queued(2, 3), started(2)]);
        // A dropped waiting job's Failed must not clear the running job.
        r.apply(&failed(3, CANCELLED));
        assert_eq!(r.events(), vec![queued(2, 3), started(2)]);
        r.apply(&failed(2, "x"));
        assert_eq!(r.events(), vec![queued(2, 3)]);
        r.apply(&JobEvent::Drained);
        assert!(r.events().is_empty());
    }
}
