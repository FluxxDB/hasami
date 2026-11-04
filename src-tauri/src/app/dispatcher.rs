//! The dispatcher thread: it receives every launch, job event and window message in order, asks
//! [`Flow`] what to do, and carries that out against Tauri and the job queue.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use tauri::{AppHandle, Emitter, Manager};

use super::commands::{recycle, DISCARD_FAILED};
use super::events::{EditorEvent, EDITOR_EVENT, JOB_EVENT};
use super::flow::{Effect, Flow};
use super::{lock, windows, AppState};
use crate::cli::Action;
use crate::editor::request::ExportRequest;
use crate::editor::session::next_session_id;
use crate::jobs::queue::{ExportJob, JobEvent, JobKind, JobQueue};

/// What the dispatcher is told, by the queue, the windows and the commands.
pub(super) enum Msg {
    Action(Action),
    Job(JobEvent),
    ProgressReady,
    WindowDestroyed,
    EditorReady,
    EditorDestroyed,
    /// An event from an editor command (proxy progress) for the editor window.
    Editor(EditorEvent),
    /// Enqueue an export and send back its job id. Enqueued by the dispatcher so it stays the only
    /// thread that enqueues (see [`Dispatcher::settled`]).
    Export(Box<ExportRequest>, Sender<u64>),
    /// Recycle `path` once every export in `after` is done; answers once it's recorded.
    Discard {
        path: String,
        after: Vec<u64>,
        reply: Sender<()>,
    },
    /// A deferred discard's recycle finished.
    Recycled {
        path: String,
        result: Result<(), String>,
    },
}

/// Carries out [`Flow`]'s decisions against Tauri and the queue.
pub(super) struct Dispatcher {
    app: AppHandle,
    queue: Arc<JobQueue>,
    /// Events the queue has emitted (counted by its emit callback) and those handled here.
    emitted: Arc<AtomicU64>,
    processed: u64,
    flow: Flow,
}

impl Dispatcher {
    /// A dispatcher for `queue`, whose emit callback counts every event it sends in `emitted`.
    pub(super) fn new(app: AppHandle, queue: Arc<JobQueue>, emitted: Arc<AtomicU64>) -> Self {
        Self { app, queue, emitted, processed: 0, flow: Flow::new() }
    }

    /// Handle messages until every sender is gone.
    pub(super) fn run(mut self, rx: Receiver<Msg>) {
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
            let effects = self.flow.on_tick(Instant::now());
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
            Msg::EditorReady => self.flow.on_editor_ready(),
            Msg::EditorDestroyed => {
                // Its clips go with it. Closing waits for their ffmpeg runs to stop, so it runs on
                // its own thread; `RunEvent::Exit` waits for it through the sessions lock. Only the
                // sessions reserved by now are this editor's: the next editor (opened after this
                // message) may reserve one before the cleanup thread gets the lock.
                let app = self.app.clone();
                let upto = next_session_id();
                let _ = thread::Builder::new()
                    .name("hasami-editor-cleanup".into())
                    .spawn(move || lock(&app.state::<AppState>().sessions).close_before(upto));
                let settled = self.settled();
                self.flow.on_editor_closed(settled)
            }
            Msg::Editor(event) => self.flow.on_editor_event(event),
            Msg::Export(request, reply) => {
                let input = request.input.clone();
                let id = self.queue.enqueue_one(JobKind::Export(ExportJob(Arc::from(request))), input.clone());
                self.flow.on_export_enqueued(id, input.display().to_string());
                let _ = reply.send(id);
                Vec::new()
            }
            Msg::Discard { path, after, reply } => {
                let effects = self.flow.on_discard(path, after);
                let _ = reply.send(());
                effects
            }
            Msg::Recycled { path, result } => {
                let settled = self.settled();
                self.flow.on_recycled(path, result, settled)
            }
        }
    }

    /// The queue is idle and every event it emitted (the last is always `Drained`) has been
    /// handled. Only this thread enqueues, so once idle, nothing new is emitted behind our back.
    fn settled(&self) -> bool {
        self.queue.is_idle() && self.processed == self.emitted.load(Ordering::SeqCst)
    }

    fn apply(&mut self, effects: Vec<Effect>) {
        let mut todo = std::collections::VecDeque::from(effects);
        while let Some(effect) = todo.pop_front() {
            match effect {
                Effect::OpenWindow { kind, ext } => {
                    if let Err(e) = windows::open_progress(&self.app, kind, ext) {
                        eprintln!("hasami: could not open the progress window: {e}");
                        // With no window coming, this may have been the last thing to wait for.
                        let settled = self.settled();
                        todo.extend(self.flow.on_open_failed(settled));
                    }
                }
                Effect::Emit(event) => {
                    if let Err(e) = self.app.emit(JOB_EVENT, &event) {
                        eprintln!("hasami: could not send {event:?}: {e}");
                    }
                }
                Effect::OpenEditor(files) => {
                    if let Err(e) = windows::open_editor(&self.app, files) {
                        eprintln!("hasami: could not open the editor window: {e}");
                        let settled = self.settled();
                        todo.extend(self.flow.on_editor_closed(settled));
                    }
                }
                Effect::EmitEditor(event) => {
                    if let Err(e) = self.app.emit(EDITOR_EVENT, &event) {
                        eprintln!("hasami: could not send {event:?}: {e}");
                    }
                }
                Effect::Enqueue(kind, files) => self.queue.enqueue(kind, files),
                Effect::Recycle(path) => {
                    // The shell's recycle can take a moment; its result comes back as a message.
                    let tx = self.app.state::<AppState>().tx.clone();
                    let target = path.clone();
                    let spawned = thread::Builder::new().name("hasami-recycle".into()).spawn(move || {
                        let result = recycle(&target);
                        let _ = tx.send(Msg::Recycled { path: target, result });
                    });
                    if let Err(e) = spawned {
                        // Resolve it anyway (the clip stays), so the app isn't kept waiting.
                        eprintln!("hasami: {DISCARD_FAILED} {e}");
                        let settled = self.settled();
                        todo.extend(self.flow.on_recycled(path, Err(DISCARD_FAILED.to_owned()), settled));
                    }
                }
                Effect::Decline(id) => self.queue.confirm(id, false),
                Effect::Exit => self.app.exit(0),
            }
        }
    }
}
