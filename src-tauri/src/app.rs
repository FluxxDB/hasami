//! Tauri setup, single-instance batch collection, the progress and editor windows, and commands.
//!
//! Explorer launches one `hasami.exe <verb> <file>` per selected file. The first process becomes
//! the app; every later launch hands its argv over through the single-instance plugin and exits.
//! Actions that arrive within [`COLLECT_WINDOW`] of each other become one `enqueue` per kind, and
//! the Trim files among them open the Quick Trim editor (or join the one already open).
//!
//! A single dispatcher thread owns the moving parts. Its decisions (batching, when to open the
//! progress and editor windows, where each job event goes, buffering events until a window
//! listens, when to exit) live in the pure [`Flow`] struct, which turns inputs into [`Effect`]s;
//! the dispatcher only carries them out.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WebviewUrl, WebviewWindowBuilder, WindowEvent};

use crate::cli::{self, Action};
use crate::compress::{plan_compress, CompressDecision};
use crate::editor::media::{make_proxy, peaks, thumbnails, Peaks, PreparedMedia, Thumb};
use crate::editor::request::ExportRequest;
use crate::editor::session::EditorSessions;
use crate::fsops;
use crate::jobs::compress::compress;
use crate::jobs::convert::convert;
use crate::jobs::export::export;
use crate::jobs::queue::{ExportJob, JobEvent, JobKind, JobQueue, JobRunner, JobSpec, CANCELLED};
use crate::jobs::{JobError, JobOutcome, JobProgress};
use crate::media::run::CancelToken;
use crate::media::{probe, FfmpegPaths};

/// Tauri event that carries every [`JobEvent`] to the progress window.
pub const JOB_EVENT: &str = "hasami://job";
/// Tauri event that carries every [`EditorEvent`] to the editor window.
pub const EDITOR_EVENT: &str = "hasami://editor";
/// Launches closer together than this are combined into one batch.
pub const COLLECT_WINDOW: Duration = Duration::from_millis(400);
const PROGRESS_LABEL: &str = "progress";
const EDITOR_LABEL: &str = "editor";
/// Every job fails with this when the bundled ffmpeg/ffprobe can't be found.
const TOOLS_MISSING: &str = "Hasami's video tools are missing. Reinstall Hasami.";

// What the editor's commands answer when they fail.
const OPEN_FAILED: &str = "Couldn't open this clip in the editor.";
const SESSION_GONE: &str = "This clip is no longer open in the editor.";
const NO_SUCH_TRACK: &str = "This audio track has no sound to show.";
const PEAKS_FAILED: &str = "Couldn't read this audio track.";
const THUMBS_FAILED: &str = "Couldn't make the timeline pictures for this clip.";
const PROXY_FAILED: &str = "Couldn't prepare a preview of this clip.";
const EXPORT_WONT_START: &str = "Couldn't start the export.";
const DISCARD_FAILED: &str = "Couldn't move this clip to the Recycle Bin.";
const COMMAND_FAILED: &str = "Something went wrong.";

/// The one place that decides what runs a job.
fn job_runner(paths: Option<FfmpegPaths>) -> Box<dyn JobRunner> {
    Box::new(MediaRunner { paths })
}

/// Runs compress, convert and export jobs with the bundled ffmpeg.
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
            JobKind::Export(job) => export(paths, &job.0, cancel, progress),
        }
    }

    /// Only compress knows its output size up front.
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

    let paths = FfmpegPaths::resolve().map_err(|e| eprintln!("hasami: {e}")).ok();
    let (tx, rx) = mpsc::channel::<Msg>();
    let queue_tx = tx.clone();
    // Counts events the queue has emitted, so the dispatcher knows when it has seen them all.
    let emitted = Arc::new(AtomicU64::new(0));
    let queue_emitted = emitted.clone();
    let queue = Arc::new(JobQueue::new(
        job_runner(paths.clone()),
        Box::new(move |event| {
            queue_emitted.fetch_add(1, Ordering::SeqCst);
            let _ = queue_tx.send(Msg::Job(event));
        }),
    ));

    let setup_tx = tx.clone();
    let setup_queue = queue.clone();
    let state = AppState {
        queue,
        tx,
        paths,
        sessions: Mutex::new(EditorSessions::new()),
        editor_files: Mutex::new(Vec::new()),
    };
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
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            cancel_job,
            confirm_low_res,
            progress_ready,
            editor_files,
            editor_open,
            editor_peaks,
            editor_thumbs,
            editor_proxy,
            editor_export,
            editor_discard,
            editor_close,
            editor_ready,
        ])
        .on_window_event(|window, event| {
            if !matches!(event, WindowEvent::Destroyed) {
                return;
            }
            let msg = match window.label() {
                PROGRESS_LABEL => Msg::WindowDestroyed,
                EDITOR_LABEL => Msg::EditorDestroyed,
                _ => return,
            };
            let _ = window.state::<AppState>().tx.send(msg);
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
        .run(|app, event| match event {
            // Closing the last window must not end the process while jobs remain; the dispatcher
            // calls `exit` (which carries a code) once the queue has drained.
            RunEvent::ExitRequested { code: None, api, .. } => api.prevent_exit(),
            // A clip still opening when the editor closed may have left a work folder.
            RunEvent::Exit => lock(&app.state::<AppState>().sessions).close_all(),
            _ => {}
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
    /// `None` when the bundled tools are missing; every job and editor command then says so.
    paths: Option<FfmpegPaths>,
    sessions: Mutex<EditorSessions>,
    /// The files the current editor window was opened with; later ones arrive as `Files` events.
    editor_files: Mutex<Vec<String>>,
}

impl AppState {
    fn tools(&self) -> Result<&FfmpegPaths, String> {
        self.paths.as_ref().ok_or_else(|| TOOLS_MISSING.to_owned())
    }
}

/// A mutex that stays usable after a panic elsewhere while it was held.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

enum Msg {
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
}

/// What the editor window is told. Mirrored in `src/editor/api.ts`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type")]
pub enum EditorEvent {
    /// More files from a later Trim launch, to add to the editor's queue.
    Files { paths: Vec<String> },
    ExportStarted { id: u64, input: String },
    /// `fraction` covers the whole export, across all its passes.
    ExportProgress { id: u64, fraction: f64 },
    ExportDone { id: u64, input: String, output: String, bytes: u64 },
    ExportFailed { id: u64, input: String, message: String, original_intact: bool },
    ProxyProgress { session: u64, fraction: f64 },
}

/// A clip opened in the editor, as `editor_open` returns it. Paths are absolute; the page turns
/// them into asset URLs with `convertFileSrc`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EditorMediaDto {
    pub session: u64,
    pub path: String,
    pub file_name: String,
    pub size_bytes: u64,
    pub duration_s: f64,
    pub bit_rate: Option<u64>,
    pub video: Option<EditorVideoDto>,
    pub tracks: Vec<EditorTrackDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EditorVideoDto {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub codec: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EditorTrackDto {
    pub index: usize,
    pub empty: bool,
    /// The extracted audio; `None` when the track is empty.
    pub file: Option<String>,
}

impl EditorMediaDto {
    fn new(session: u64, input: &Path, media: &PreparedMedia) -> Self {
        let info = &media.info;
        Self {
            session,
            path: input.display().to_string(),
            file_name: input.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            size_bytes: info.size_bytes,
            duration_s: info.duration_s,
            bit_rate: info.bit_rate,
            video: info.video.as_ref().map(|v| EditorVideoDto {
                width: v.width,
                height: v.height,
                fps: v.fps,
                codec: v.codec.clone(),
            }),
            tracks: media
                .tracks
                .iter()
                .map(|t| EditorTrackDto {
                    index: t.index,
                    empty: t.empty,
                    file: t.file.as_ref().map(|f| f.display().to_string()),
                })
                .collect(),
        }
    }
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

/// The text shown for `e`: its own when it is written for people, `fallback` otherwise. The
/// details go to stderr either way.
fn plain(e: &JobError, fallback: &str) -> String {
    eprintln!("hasami: {fallback} {e:?}");
    match e {
        JobError::Cancelled | JobError::Message(_) | JobError::Fs(_) | JobError::Encode { .. } => e.to_string(),
        _ => fallback.to_owned(),
    }
}

/// Run `work` on a blocking thread, away from the main thread and the async workers: editor
/// commands run ffmpeg and may wait on the sessions lock.
async fn blocking<T: Send + 'static>(
    app: AppHandle,
    work: impl FnOnce(&AppHandle, &AppState) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || work(&app, &app.state::<AppState>()))
        .await
        .unwrap_or_else(|e| {
            eprintln!("hasami: an editor command panicked: {e}");
            Err(COMMAND_FAILED.to_owned())
        })
}

/// The files the editor window was opened with.
#[tauri::command]
fn editor_files(state: State<'_, AppState>) -> Vec<String> {
    lock(&state.editor_files).clone()
}

/// Open `path` in a new session and let the page load its files through the asset protocol.
#[tauri::command]
async fn editor_open(app: AppHandle, path: String) -> Result<EditorMediaDto, String> {
    blocking(app, move |app, state| {
        let paths = state.tools()?;
        let input = PathBuf::from(path);
        let (session, media, work_dir) =
            lock(&state.sessions).open(paths, &input).map_err(|e| plain(&e, OPEN_FAILED))?;
        let scope = app.asset_protocol_scope();
        if let Err(e) = scope.allow_file(&input).and_then(|()| scope.allow_directory(&work_dir, true)) {
            eprintln!("hasami: could not allow {} in the asset scope: {e}", input.display());
            lock(&state.sessions).close(session);
            return Err(OPEN_FAILED.to_owned());
        }
        Ok(EditorMediaDto::new(session, &input, &media))
    })
    .await
}

/// Waveform of audio track `index` (ffmpeg's `0:a:<index>`) of a session.
#[tauri::command]
async fn editor_peaks(app: AppHandle, session: u64, index: usize) -> Result<Peaks, String> {
    blocking(app, move |_, state| {
        let paths = state.tools()?;
        let (file, duration_s) = {
            let sessions = lock(&state.sessions);
            let media = sessions.media(session).ok_or(SESSION_GONE)?;
            let track = media.tracks.iter().find(|t| t.index == index);
            let file = track.and_then(|t| t.file.clone()).ok_or(NO_SUCH_TRACK)?;
            (file, media.info.duration_s)
        };
        peaks(paths, &file, duration_s).map_err(|e| plain(&e.into(), PEAKS_FAILED))
    })
    .await
}

/// Timeline thumbnails of a session's clip, written into its work folder.
#[tauri::command]
async fn editor_thumbs(app: AppHandle, session: u64) -> Result<Vec<Thumb>, String> {
    blocking(app, move |_, state| {
        let paths = state.tools()?;
        let (input, work_dir, duration_s) = {
            let sessions = lock(&state.sessions);
            let duration_s = sessions.media(session).ok_or(SESSION_GONE)?.info.duration_s;
            let input = sessions.input(session).ok_or(SESSION_GONE)?.to_path_buf();
            let work_dir = sessions.work_dir(session).ok_or(SESSION_GONE)?.to_path_buf();
            (input, work_dir, duration_s)
        };
        let result = thumbnails(paths, &input, duration_s, &work_dir.join("thumbs"));
        sweep_if_closed(state, session, &work_dir);
        result.map_err(|e| plain(&e.into(), THUMBS_FAILED))
    })
    .await
}

/// Encode a session's H.264 preview proxy, reporting `ProxyProgress`. Stops when the session
/// closes. Returns the proxy's path.
#[tauri::command]
async fn editor_proxy(app: AppHandle, session: u64) -> Result<String, String> {
    blocking(app, move |_, state| {
        let paths = state.tools()?;
        let (input, work_dir, cancel) = {
            let sessions = lock(&state.sessions);
            let input = sessions.input(session).ok_or(SESSION_GONE)?.to_path_buf();
            let work_dir = sessions.work_dir(session).ok_or(SESSION_GONE)?.to_path_buf();
            (input, work_dir, sessions.cancel_token(session).ok_or(SESSION_GONE)?)
        };
        let mut report = |fraction| {
            let _ = state.tx.send(Msg::Editor(EditorEvent::ProxyProgress { session, fraction }));
        };
        let result = make_proxy(paths, &input, &work_dir, &cancel, &mut report);
        sweep_if_closed(state, session, &work_dir);
        result.map(|proxy| proxy.display().to_string()).map_err(|e| plain(&e, PROXY_FAILED))
    })
    .await
}

/// A session closed while ffmpeg was still writing into its folder may have left files there
/// that couldn't be deleted yet; delete them now.
fn sweep_if_closed(state: &AppState, session: u64, work_dir: &Path) {
    if lock(&state.sessions).work_dir(session).is_none() {
        let _ = std::fs::remove_dir_all(work_dir);
    }
}

/// Queue an export of the edit. Returns its job id; progress arrives as `Export*` events.
#[tauri::command]
async fn editor_export(app: AppHandle, request: ExportRequest) -> Result<u64, String> {
    blocking(app, move |_, state| {
        let (reply, id) = mpsc::channel();
        state.tx.send(Msg::Export(Box::new(request), reply)).map_err(|_| EXPORT_WONT_START)?;
        id.recv().map_err(|_| EXPORT_WONT_START.to_owned())
    })
    .await
}

/// Send a clip the user discarded to the Recycle Bin.
#[tauri::command]
async fn editor_discard(app: AppHandle, path: String) -> Result<(), String> {
    blocking(app, move |_, _| {
        fsops::recycle(Path::new(&path)).map_err(|e| {
            eprintln!("hasami: {DISCARD_FAILED} {e:?}");
            DISCARD_FAILED.to_owned()
        })
    })
    .await
}

/// The editor is done with a clip: stop its work and delete its work folder.
#[tauri::command]
async fn editor_close(app: AppHandle, session: u64) {
    let _ = blocking(app, move |_, state| {
        lock(&state.sessions).close(session);
        Ok(())
    })
    .await;
}

/// Called by the editor window once its editor-event listener is attached.
#[tauri::command]
fn editor_ready(state: State<'_, AppState>) {
    let _ = state.tx.send(Msg::EditorReady);
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
                // Its clips are gone with it; any proxy encode stops.
                lock(&self.app.state::<AppState>().sessions).close_all();
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
                    if let Err(e) = self.open_window(kind, ext) {
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
                    if let Err(e) = self.open_editor(files) {
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

    fn open_editor(&self, files: Vec<PathBuf>) -> tauri::Result<()> {
        *lock(&self.app.state::<AppState>().editor_files) = files.iter().map(|f| f.display().to_string()).collect();
        WebviewWindowBuilder::new(&self.app, EDITOR_LABEL, WebviewUrl::App("editor.html".into()))
            .title("Hasami")
            .inner_size(1280.0, 800.0)
            .min_inner_size(960.0, 600.0)
            .resizable(true)
            .decorations(false)
            .center()
            .build()
            .map(|_| ())
    }
}

/// Something the dispatcher must do, decided by [`Flow`].
#[derive(Debug, PartialEq)]
pub(crate) enum Effect {
    /// Open the progress window.
    OpenWindow { kind: &'static str, ext: Option<&'static str> },
    /// Send to the progress window (only produced once it listens).
    Emit(JobEvent),
    /// Open the editor window for these files.
    OpenEditor(Vec<PathBuf>),
    /// Send to the editor window (only produced once it listens).
    EmitEditor(EditorEvent),
    Enqueue(JobKind, Vec<PathBuf>),
    /// Answer a pending low-resolution question with "no".
    Decline(u64),
    Exit,
}

/// An export the flow is following: its input, and which editor window asked for it.
struct ExportInfo {
    input: String,
    editor: u64,
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
    /// The editor window exists, or is being created.
    editor_open: bool,
    /// The editor's listener is attached; until then its events wait in `editor_buffer`.
    editor_ready: bool,
    editor_buffer: Vec<EditorEvent>,
    /// Counts editor windows opened, so a later window never hears of an earlier one's exports.
    editor_count: u64,
    /// Exports not yet finished, by job id. Their events go to the editor, never the progress window.
    exports: HashMap<u64, ExportInfo>,
    /// `Queued` events of enqueued exports not seen yet. Those don't open the progress window.
    export_queued: usize,
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
            editor_open: false,
            editor_ready: false,
            editor_buffer: Vec::new(),
            editor_count: 0,
            exports: HashMap::new(),
            export_queued: 0,
        }
    }

    /// When the open batch closes, if one is open.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.collector.deadline()
    }

    pub(crate) fn on_action(&mut self, action: Action, now: Instant) {
        self.collector.push(action, now);
    }

    /// Start the collected batch once it's due: enqueue its jobs and hand its Trim files to the
    /// editor, opening it if needed.
    pub(crate) fn on_tick(&mut self, now: Instant) -> Vec<Effect> {
        let mut out = Vec::new();
        let Some(batch) = self.collector.take_due(now) else {
            return out;
        };
        if let Some((kind, _)) = batch.jobs.first() {
            self.hint = (kind.name(), kind.ext());
        }
        for (kind, files) in batch.jobs.into_iter().filter(|(_, files)| !files.is_empty()) {
            out.push(Effect::Enqueue(kind, files));
        }
        if batch.trims.is_empty() {
            return out;
        }
        if self.editor_open {
            let paths = batch.trims.iter().map(|f| f.display().to_string()).collect();
            self.send_editor(EditorEvent::Files { paths }, &mut out);
        } else {
            out.push(Effect::OpenEditor(batch.trims));
            self.editor_open = true;
            self.editor_ready = false;
            self.editor_buffer.clear();
            self.editor_count += 1;
        }
        out
    }

    /// The dispatcher enqueued export `id` of `input` for the current editor window.
    pub(crate) fn on_export_enqueued(&mut self, id: u64, input: String) {
        self.exports.insert(id, ExportInfo { input, editor: self.editor_count });
        self.export_queued += 1;
    }

    pub(crate) fn on_job_event(&mut self, event: JobEvent, settled: bool) -> Vec<Effect> {
        let mut out = Vec::new();
        let drained = event == JobEvent::Drained;
        match job_id(&event).filter(|id| self.exports.contains_key(id)) {
            Some(id) => self.on_export_event(id, event, &mut out),
            None => self.on_progress_event(event, &mut out),
        }
        if drained && settled {
            self.exit_if_finished(settled, &mut out);
        }
        out
    }

    /// A compress or convert job's event, or one about the whole queue.
    fn on_progress_event(&mut self, event: JobEvent, out: &mut Vec<Effect>) {
        // An export's `Queued` is the editor's business. When it can't be told apart from a job
        // starting at the same moment, that job's `Started` still opens the window right after.
        let reopens = match event {
            JobEvent::Queued { .. } if self.export_queued > 0 => {
                self.export_queued -= 1;
                false
            }
            ref event => reopens_window(event),
        };
        let catch_up = self.replay.events();
        self.replay.apply(&event);
        if self.window_open {
            self.send(event, out);
        } else if reopens {
            // The new window first learns which job is running, then gets this event.
            self.open_window(self.hint, out);
            self.buffer = catch_up;
            self.send(event, out);
        }
    }

    /// Route an export's event to the editor that asked for it. With that editor gone, only a
    /// failure is shown, in the progress window.
    fn on_export_event(&mut self, id: u64, event: JobEvent, out: &mut Vec<Effect>) {
        let info = &self.exports[&id];
        let to_editor = self.editor_open && info.editor == self.editor_count;
        let input = info.input.clone();
        let failure = |message: String, original_intact: bool| EditorEvent::ExportFailed {
            id,
            input: input.clone(),
            message,
            original_intact,
        };
        let editor_event = match event {
            JobEvent::Started { .. } => EditorEvent::ExportStarted { id, input: input.clone() },
            JobEvent::Progress { pass, passes, fraction, .. } => {
                let overall = (f64::from(pass.saturating_sub(1)) + fraction) / f64::from(passes.max(1));
                EditorEvent::ExportProgress { id, fraction: overall }
            }
            // Exports never ask; don't leave the queue waiting on an answer nobody can give.
            JobEvent::NeedsConfirm { .. } => {
                out.push(Effect::Decline(id));
                return;
            }
            JobEvent::Done { output, bytes, .. } => {
                self.exports.remove(&id);
                EditorEvent::ExportDone { id, input: input.clone(), output, bytes }
            }
            JobEvent::Unchanged { reason, .. } => {
                self.exports.remove(&id);
                failure(reason, true)
            }
            JobEvent::Failed { message, original_intact, .. } => {
                self.exports.remove(&id);
                failure(message, original_intact)
            }
            JobEvent::Queued { .. } | JobEvent::Drained => return,
        };
        if to_editor {
            self.send_editor(editor_event, out);
        } else if let EditorEvent::ExportFailed { message, original_intact, .. } = editor_event {
            if message != CANCELLED {
                self.show_export_failure(id, input, message, original_intact, out);
            }
        }
    }

    /// Show a failed export in the progress window, opening it if needed.
    fn show_export_failure(&mut self, id: u64, input: String, message: String, original_intact: bool, out: &mut Vec<Effect>) {
        if !self.window_open {
            self.open_window(("export", None), out);
        }
        let started = JobEvent::Started {
            id,
            input,
            input_bytes: 0, // not shown with a failure
            kind: Some("export".into()),
            ext: None,
            width: None,
            height: None,
            fps: None,
        };
        self.send(started, out);
        self.send(JobEvent::Failed { id, message, original_intact }, out);
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

    /// An `OpenWindow` effect could not be carried out; the next event tries again. If nothing
    /// else is coming (the window was for the final drain), exit instead of waiting forever with
    /// no window.
    pub(crate) fn on_open_failed(&mut self, settled: bool) -> Vec<Effect> {
        let mut out = Vec::new();
        self.forget_window();
        self.exit_if_finished(settled, &mut out);
        out
    }

    /// The editor's listener is attached. A reloaded editor gets nothing again: it asks for its
    /// files itself, and its earlier exports' progress is not replayed.
    pub(crate) fn on_editor_ready(&mut self) -> Vec<Effect> {
        if !self.editor_open || self.editor_ready {
            return Vec::new();
        }
        self.editor_ready = true;
        self.editor_buffer.drain(..).map(Effect::EmitEditor).collect()
    }

    /// An event from an editor command, for the editor window.
    pub(crate) fn on_editor_event(&mut self, event: EditorEvent) -> Vec<Effect> {
        let mut out = Vec::new();
        if self.editor_open {
            self.send_editor(event, &mut out);
        }
        out
    }

    /// The editor window closed, or could not be opened. Its exports keep running.
    pub(crate) fn on_editor_closed(&mut self, settled: bool) -> Vec<Effect> {
        let mut out = Vec::new();
        self.editor_open = false;
        self.editor_ready = false;
        self.editor_buffer.clear();
        self.exit_if_finished(settled, &mut out);
        out
    }

    fn forget_window(&mut self) {
        self.window_open = false;
        self.ready = false;
        self.buffer.clear();
    }

    fn open_window(&mut self, (kind, ext): (&'static str, Option<&'static str>), out: &mut Vec<Effect>) {
        out.push(Effect::OpenWindow { kind, ext });
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

    fn send_editor(&mut self, event: EditorEvent, out: &mut Vec<Effect>) {
        if self.editor_ready {
            out.push(Effect::EmitEditor(event));
        } else {
            self.editor_buffer.push(event);
        }
    }

    fn exit_if_finished(&self, settled: bool, out: &mut Vec<Effect>) {
        if settled && !self.window_open && !self.editor_open && self.collector.is_empty() {
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
    const OPEN_EXPORT: Effect = Effect::OpenWindow { kind: "export", ext: None };

    /// A flow whose window is open and listening, showing job 1 of 2.
    fn showing_job_1() -> Flow {
        let mut f = Flow::new();
        assert_eq!(f.on_job_event(queued(1, 2), false), vec![OPEN_COMPRESS]);
        f.on_job_event(started(1), false);
        f.on_ready();
        f
    }

    fn strings(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    fn to_editor(events: &[EditorEvent]) -> Vec<Effect> {
        events.iter().cloned().map(Effect::EmitEditor).collect()
    }

    /// A flow whose editor window is open and listening, opened for `t.mp4`.
    fn editing() -> Flow {
        let t0 = Instant::now();
        let mut f = Flow::new();
        f.on_action(Action::Trim(files(&["t.mp4"])), t0);
        assert_eq!(f.on_tick(t0 + ms(400)), vec![Effect::OpenEditor(files(&["t.mp4"]))]);
        assert_eq!(f.on_editor_ready(), vec![]);
        f
    }

    /// `editing()` with export 5 of `t.mp4` enqueued and started.
    fn exporting() -> Flow {
        let mut f = editing();
        f.on_export_enqueued(5, "t.mp4".into());
        assert_eq!(f.on_job_event(queued(1, 1), false), vec![], "an export's Queued opens nothing");
        f.on_job_event(export_started(5), false);
        f
    }

    fn export_started(id: u64) -> JobEvent {
        JobEvent::Started {
            id,
            input: "t.mp4".into(),
            input_bytes: 1,
            kind: Some("export".into()),
            ext: None,
            width: None,
            height: None,
            fps: None,
        }
    }

    #[test]
    fn a_batch_enqueues_each_kind_and_opens_the_window_on_its_first_event() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        f.on_action(Action::Convert(Target::Mov, files(&["a.mkv"])), t0);
        f.on_action(Action::Compress(files(&["b.mp4"])), t0 + ms(100));
        assert_eq!(f.deadline(), Some(t0 + ms(500)));
        assert_eq!(f.on_tick(t0 + ms(499)), vec![]);
        assert_eq!(
            f.on_tick(t0 + ms(500)),
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
    fn a_failed_window_open_is_retried_on_the_next_event() {
        let mut f = Flow::new();
        assert_eq!(f.on_job_event(queued(1, 1), false), vec![OPEN_COMPRESS]);
        assert_eq!(f.on_open_failed(false), vec![], "jobs remain, so no exit");
        assert_eq!(f.on_job_event(started(1), false), vec![OPEN_COMPRESS]);
        assert_eq!(f.on_ready(), emits(&[queued(1, 1), started(1)]));
    }

    #[test]
    fn a_failed_window_open_with_nothing_left_exits() {
        let mut f = Flow::new();
        assert_eq!(f.on_job_event(queued(1, 1), false), vec![OPEN_COMPRESS]);
        f.on_job_event(failed(1, "boom"), false);
        f.on_job_event(JobEvent::Drained, true);
        assert_eq!(f.on_open_failed(true), vec![Effect::Exit]);
    }

    #[test]
    fn a_failed_window_open_waits_while_a_batch_is_collected() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        assert_eq!(f.on_job_event(queued(1, 1), false), vec![OPEN_COMPRESS]);
        f.on_action(Action::Compress(files(&["a.mp4"])), t0);
        assert_eq!(f.on_open_failed(true), vec![]);
    }

    // ---- Editor ----

    #[test]
    fn a_trim_batch_opens_the_editor() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        f.on_action(Action::Trim(files(&["a.mp4"])), t0);
        f.on_action(Action::Compress(files(&["c.mp4"])), t0 + ms(10));
        f.on_action(Action::Trim(files(&["b.mp4"])), t0 + ms(20));
        assert_eq!(
            f.on_tick(t0 + ms(420)),
            vec![
                Effect::Enqueue(JobKind::Compress, files(&["c.mp4"])),
                Effect::OpenEditor(files(&["a.mp4", "b.mp4"])),
            ]
        );
    }

    #[test]
    fn a_trim_while_the_editor_is_open_appends_files() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        f.on_action(Action::Trim(files(&["a.mp4"])), t0);
        f.on_tick(t0 + ms(400));
        // The page isn't listening yet: the new files wait for it.
        f.on_action(Action::Trim(files(&["b.mp4", "c.mp4"])), t0 + ms(500));
        assert_eq!(f.on_tick(t0 + ms(900)), vec![]);
        let appended = EditorEvent::Files { paths: strings(&["b.mp4", "c.mp4"]) };
        assert_eq!(f.on_editor_ready(), to_editor(&[appended]));
        // Once it listens, they go straight to it.
        f.on_action(Action::Trim(files(&["d.mp4"])), t0 + ms(1000));
        assert_eq!(
            f.on_tick(t0 + ms(1400)),
            to_editor(&[EditorEvent::Files { paths: strings(&["d.mp4"]) }])
        );
    }

    #[test]
    fn export_events_go_to_the_editor_not_the_progress_window() {
        let mut f = editing();
        f.on_export_enqueued(5, "t.mp4".into());
        assert_eq!(f.on_job_event(queued(1, 1), false), vec![]);
        assert_eq!(
            f.on_job_event(export_started(5), false),
            to_editor(&[EditorEvent::ExportStarted { id: 5, input: "t.mp4".into() }])
        );
        // Pass 2 of 3, half done: halfway through the whole export.
        let tick = JobEvent::Progress { id: 5, pass: 2, passes: 3, fraction: 0.5, eta_s: None };
        assert_eq!(
            f.on_job_event(tick, false),
            to_editor(&[EditorEvent::ExportProgress { id: 5, fraction: 0.5 }])
        );
        assert_eq!(
            f.on_job_event(done(5), false),
            to_editor(&[EditorEvent::ExportDone {
                id: 5,
                input: "t.mp4".into(),
                output: "5.out.mp4".into(),
                bytes: 2
            }])
        );
        assert_eq!(f.on_job_event(JobEvent::Drained, true), vec![]);
    }

    #[test]
    fn an_export_failure_goes_to_the_editor() {
        let mut f = exporting();
        assert_eq!(
            f.on_job_event(failed(5, "boom"), false),
            to_editor(&[EditorEvent::ExportFailed {
                id: 5,
                input: "t.mp4".into(),
                message: "boom".into(),
                original_intact: true
            }])
        );
    }

    #[test]
    fn export_failure_without_editor_shows_in_progress_window() {
        let mut f = exporting();
        assert_eq!(f.on_editor_closed(false), vec![]);
        assert_eq!(f.on_job_event(progress(5), false), vec![], "nothing shows a running export");
        assert_eq!(f.on_job_event(failed(5, "boom"), false), vec![OPEN_EXPORT]);
        let shown = JobEvent::Started {
            id: 5,
            input: "t.mp4".into(),
            input_bytes: 0,
            kind: Some("export".into()),
            ext: None,
            width: None,
            height: None,
            fps: None,
        };
        assert_eq!(f.on_ready(), emits(&[shown, failed(5, "boom")]));
        assert_eq!(f.on_job_event(JobEvent::Drained, true), emits(&[JobEvent::Drained]));
        assert_eq!(f.on_window_closed(true), vec![Effect::Exit]);
    }

    #[test]
    fn a_later_editor_hears_nothing_of_an_earlier_ones_exports() {
        let t0 = Instant::now();
        let mut f = exporting();
        f.on_editor_closed(false);
        f.on_action(Action::Trim(files(&["u.mp4"])), t0);
        assert_eq!(f.on_tick(t0 + ms(400)), vec![Effect::OpenEditor(files(&["u.mp4"]))]);
        f.on_editor_ready();
        assert_eq!(f.on_job_event(done(5), false), vec![]);
    }

    #[test]
    fn no_exit_while_the_editor_is_open() {
        let mut f = exporting();
        assert_eq!(f.on_job_event(done(5), false).len(), 1);
        assert_eq!(f.on_job_event(JobEvent::Drained, true), vec![]);
        // A progress window closing doesn't end the app under the editor either.
        f.on_job_event(queued(2, 2), false);
        assert_eq!(f.on_window_closed(true), vec![]);
        assert_eq!(f.on_editor_closed(true), vec![Effect::Exit]);
    }

    #[test]
    fn exit_after_editor_closes_and_exports_drain() {
        let mut f = exporting();
        assert_eq!(f.on_editor_closed(false), vec![], "the export is still running");
        assert_eq!(f.on_job_event(done(5), false), vec![]);
        assert_eq!(f.on_job_event(JobEvent::Drained, true), vec![Effect::Exit]);
    }

    #[test]
    fn a_failed_editor_open_with_nothing_left_exits() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        f.on_action(Action::Trim(files(&["t.mp4"])), t0);
        f.on_tick(t0 + ms(400));
        assert_eq!(f.on_editor_closed(true), vec![Effect::Exit]);
    }

    #[test]
    fn proxy_progress_waits_for_the_editor() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        f.on_action(Action::Trim(files(&["t.mp4"])), t0);
        f.on_tick(t0 + ms(400));
        let tick = EditorEvent::ProxyProgress { session: 1, fraction: 0.25 };
        assert_eq!(f.on_editor_event(tick.clone()), vec![]);
        assert_eq!(f.on_editor_ready(), vec![Effect::EmitEditor(tick.clone())]);
        assert_eq!(f.on_editor_event(tick.clone()), vec![Effect::EmitEditor(tick)]);
    }

    #[test]
    fn an_export_that_asks_to_confirm_is_declined() {
        let mut f = exporting();
        let ask = JobEvent::NeedsConfirm { id: 5, width: 426, height: 240, fps: 15.0 };
        assert_eq!(f.on_job_event(ask, false), vec![Effect::Decline(5)]);
    }

    #[test]
    fn editor_events_serialize_to_the_ui_contract() {
        let failed = EditorEvent::ExportFailed {
            id: 5,
            input: "C:\\v\\a.mp4".into(),
            message: "boom".into(),
            original_intact: false,
        };
        assert_eq!(
            serde_json::to_value(&failed).unwrap(),
            serde_json::json!({"type": "ExportFailed", "id": 5, "input": "C:\\v\\a.mp4", "message": "boom",
                               "original_intact": false})
        );
        let files = EditorEvent::Files { paths: strings(&["a.mp4"]) };
        assert_eq!(serde_json::to_value(&files).unwrap(), serde_json::json!({"type": "Files", "paths": ["a.mp4"]}));
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
