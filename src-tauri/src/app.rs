//! Tauri setup, single-instance batch collection, progress window and commands (Task 9).
//!
//! Explorer launches one `hasami.exe <verb> <file>` per selected file. The first process becomes
//! the app; every later launch hands its argv over through the single-instance plugin and exits.
//! Actions that arrive within [`COLLECT_WINDOW`] of each other become one `enqueue` per kind.
//!
//! A single dispatcher thread owns the moving parts: it collects actions, feeds the queue, opens
//! the progress window, buffers job events until the window's listener is attached, and decides
//! when the process exits (queue drained and no window open).

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WebviewUrl, WebviewWindowBuilder, WindowEvent};

use crate::cli::{self, Action};
use crate::jobs::queue::{JobEvent, JobKind, JobQueue, JobRunner, JobSpec};
use crate::jobs::{JobError, JobOutcome, JobProgress};
use crate::media::run::CancelToken;

/// Tauri event that carries every [`JobEvent`].
pub const JOB_EVENT: &str = "hasami://job";
/// Launches closer together than this are combined into one batch.
pub const COLLECT_WINDOW: Duration = Duration::from_millis(400);
const PROGRESS_LABEL: &str = "progress";
/// Temporary, until Quick Trim lands in Plan 2.
const TRIM_NOTICE: &str = "Quick Trim arrives in the next version";

/// The one place that decides what runs a job.
///
/// Follow-up: replace `PlaceholderRunner` with a runner that matches on `spec.kind` and calls the
/// compress job for `JobKind::Compress` and the convert job for `JobKind::Convert(target)`, with
/// `FfmpegPaths::resolve()`.
fn job_runner() -> Box<dyn JobRunner> {
    Box::new(PlaceholderRunner)
}

/// Fails every job, so the whole queue and window flow can be exercised before the real jobs exist.
struct PlaceholderRunner;

impl JobRunner for PlaceholderRunner {
    fn run(
        &self,
        _spec: &JobSpec,
        _confirmed: bool,
        _cancel: &CancelToken,
        _progress: &mut dyn FnMut(JobProgress),
    ) -> Result<JobOutcome, JobError> {
        Err(JobError::Message("Not wired yet".into()))
    }
}

/// Start the app for the first launch's `action` (never `Register`/`Unregister`).
pub fn run(action: Action) {
    let cwd = std::env::current_dir().unwrap_or_default();
    let first = resolve_paths(action, &cwd);

    let (tx, rx) = mpsc::channel::<Msg>();
    let queue_tx = tx.clone();
    let queue = Arc::new(JobQueue::new(
        job_runner(),
        Box::new(move |event| {
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
            let dispatcher = Dispatcher::new(app.handle().clone(), setup_queue);
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

    fn apply(&mut self, event: &JobEvent) {
        match event {
            JobEvent::Queued { .. } => self.queued = Some(event.clone()),
            JobEvent::Started { .. } => {
                self.started = Some(event.clone());
                self.confirm = None;
            }
            JobEvent::NeedsConfirm { .. } => self.confirm = Some(event.clone()),
            JobEvent::Progress { .. } => self.confirm = None,
            JobEvent::Done { .. } | JobEvent::Unchanged { .. } | JobEvent::Failed { .. } => {
                self.started = None;
                self.confirm = None;
            }
            JobEvent::Drained => *self = Replay::default(),
        }
    }
}

struct Dispatcher {
    app: AppHandle,
    queue: Arc<JobQueue>,
    collector: Collector,
    replay: Replay,
    /// `kind` (and `ext`) of the latest batch, for the window's URL. A `Started` that carries its
    /// own `kind`/`ext` overrides it in the window.
    hint: (&'static str, Option<&'static str>),
    window_open: bool,
    /// The window's listener is attached; until then events wait in `buffer`.
    ready: bool,
    buffer: Vec<JobEvent>,
}

impl Dispatcher {
    fn new(app: AppHandle, queue: Arc<JobQueue>) -> Self {
        Self {
            app,
            queue,
            collector: Collector::new(COLLECT_WINDOW),
            replay: Replay::default(),
            hint: ("compress", None),
            window_open: false,
            ready: false,
            buffer: Vec::new(),
        }
    }

    fn run(mut self, rx: Receiver<Msg>) {
        loop {
            let msg = match self.collector.deadline() {
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
                self.handle(msg);
            }
            if let Some(batch) = self.collector.take_due(Instant::now()) {
                self.start_batch(batch);
            }
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Action(action) => self.collector.push(action, Instant::now()),
            Msg::Job(event) => {
                let drained = event == JobEvent::Drained;
                self.on_job_event(event);
                if drained {
                    self.exit_if_finished();
                }
            }
            Msg::ProgressReady => {
                if !self.window_open {
                    return;
                }
                if self.ready {
                    // The page reloaded and lost its state; bring it back up to date.
                    for event in self.replay.events() {
                        self.send(event);
                    }
                } else {
                    self.ready = true;
                    for event in std::mem::take(&mut self.buffer) {
                        self.send(event);
                    }
                }
            }
            Msg::WindowDestroyed => {
                self.window_open = false;
                self.ready = false;
                self.buffer.clear();
                self.exit_if_finished();
            }
        }
    }

    fn start_batch(&mut self, batch: Batch) {
        if let Some((kind, _)) = batch.jobs.first() {
            self.hint = (kind.name(), kind.ext());
        }
        for (kind, files) in batch.jobs {
            self.queue.enqueue(kind, files);
        }
        if let Some(first) = batch.trims.first() {
            self.show_trim_notice(first);
        }
    }

    fn on_job_event(&mut self, event: JobEvent) {
        let catch_up = self.replay.events();
        self.replay.apply(&event);
        if !self.window_open {
            // A closed window comes back for the next meaningful event, not for every progress
            // tick of the job it was closed on, and not just to show "Drained".
            if matches!(event, JobEvent::Progress { .. } | JobEvent::Drained) || !self.open_window() {
                return;
            }
            self.buffer = catch_up;
        }
        self.send(event);
    }

    /// Show the Plan 1 stand-in for Quick Trim in the progress window.
    fn show_trim_notice(&mut self, first: &Path) {
        if !self.window_open && !self.open_window() {
            return;
        }
        self.send(JobEvent::Started {
            id: 0, // queue ids start at 1
            input: first.display().to_string(),
            input_bytes: std::fs::metadata(first).map(|m| m.len()).unwrap_or(0),
            kind: Some("trim".into()),
            ext: None,
            width: None,
            height: None,
            fps: None,
        });
        self.send(JobEvent::Unchanged { id: 0, reason: TRIM_NOTICE.into() });
    }

    fn send(&mut self, event: JobEvent) {
        if self.ready {
            if let Err(e) = self.app.emit(JOB_EVENT, &event) {
                eprintln!("hasami: could not send {event:?}: {e}");
            }
        } else {
            self.buffer.push(event);
        }
    }

    /// Open the progress window. Events are buffered until it reports `progress_ready`.
    fn open_window(&mut self) -> bool {
        let (kind, ext) = self.hint;
        let mut url = format!("progress.html?kind={kind}");
        if let Some(ext) = ext {
            url.push_str(&format!("&ext={ext}"));
        }
        let built = WebviewWindowBuilder::new(&self.app, PROGRESS_LABEL, WebviewUrl::App(url.into()))
            .title("Hasami")
            .inner_size(440.0, 190.0)
            .resizable(false)
            .maximizable(false)
            .decorations(false)
            .center()
            .build();
        match built {
            Ok(_) => {
                self.window_open = true;
                self.ready = false;
                self.buffer.clear();
                true
            }
            Err(e) => {
                eprintln!("hasami: could not open the progress window: {e}");
                false
            }
        }
    }

    fn exit_if_finished(&self) {
        if !self.window_open && self.collector.is_empty() && self.queue.is_idle() {
            self.app.exit(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Target;

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

    #[test]
    fn replay_brings_a_new_window_to_the_running_job() {
        let queued = JobEvent::Queued { snapshot: crate::jobs::queue::QueueSnapshot { current_index: 2, total: 3 } };
        let started = JobEvent::Started {
            id: 2,
            input: "b.mp4".into(),
            input_bytes: 1,
            kind: Some("compress".into()),
            ext: None,
            width: None,
            height: None,
            fps: None,
        };
        let mut r = Replay::default();
        r.apply(&queued);
        r.apply(&started);
        r.apply(&JobEvent::Progress { id: 2, pass: 1, passes: 2, fraction: 0.5, eta_s: None });
        assert_eq!(r.events(), vec![queued.clone(), started]);
        r.apply(&JobEvent::Failed { id: 2, message: "x".into() });
        assert_eq!(r.events(), vec![queued]);
        r.apply(&JobEvent::Drained);
        assert!(r.events().is_empty());
    }
}
