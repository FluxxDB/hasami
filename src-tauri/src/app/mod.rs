//! Tauri setup, single-instance batch collection, the progress and editor windows, and commands.
//!
//! Explorer launches one `hasami.exe <verb> <file>` per selected file. The first process becomes
//! the app; every later launch hands its argv over through the single-instance plugin and exits.
//! Actions that arrive within [`COLLECT_WINDOW`] of each other become one `enqueue` per kind, and
//! the Trim files among them open the Quick Trim editor (or join the one already open).
//!
//! A single dispatcher thread owns the moving parts. Its decisions (batching, when to open the
//! progress and editor windows, where each job event goes, buffering events until a window
//! listens, when to exit) live in the pure [`Flow`](flow::Flow) struct, which turns inputs into
//! [`Effect`](flow::Effect)s; the dispatcher only carries them out.
//!
//! - [`collector`]: groups launches that arrive close together into one batch.
//! - [`flow`]: the dispatcher's decisions, free of Tauri and threads.
//! - [`replay`]: what a newly opened progress window needs to catch up.
//! - [`dispatcher`]: the thread that carries out those decisions.
//! - [`commands`]: the Tauri commands the progress and editor windows call.
//! - [`events`]: the events and data the windows are sent (the UI contract).
//! - [`runner`]: what runs each job.
//! - [`windows`]: opening the progress and editor windows.

mod collector;
mod commands;
mod dispatcher;
mod events;
mod flow;
mod replay;
mod runner;
mod windows;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

use tauri::{Manager, RunEvent, WindowEvent};

pub use collector::COLLECT_WINDOW;
pub use events::{EditorEvent, EditorMediaDto, EditorTrackDto, EditorVideoDto, EDITOR_EVENT, JOB_EVENT};

use crate::cli::{self, Action};
use crate::editor::session::{process_running, sweep_stale, EditorSessions};
use crate::jobs::queue::JobQueue;
use crate::media::FfmpegPaths;
use dispatcher::{Dispatcher, Msg};
use runner::job_runner;
use windows::{EDITOR_LABEL, PROGRESS_LABEL};

/// Every job fails with this when the bundled ffmpeg/ffprobe can't be found.
const TOOLS_MISSING: &str = "Hasami's video tools are missing. Reinstall Hasami.";

/// Start the app for the first launch's `action` (never `Register`/`Unregister`).
pub fn run(action: Action) {
    let cwd = std::env::current_dir().unwrap_or_default();
    let first = resolve_paths(action, &cwd);

    let paths = FfmpegPaths::resolve().map_err(|e| eprintln!("hasami: {e}")).ok();
    // Editor work folders and Compress pass logs left by a crashed or killed Hasami.
    let _ = thread::Builder::new()
        .name("hasami-sweep".into())
        .spawn(|| sweep_stale(&std::env::temp_dir(), process_running));
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
        recycling: Mutex::new(()),
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
            commands::cancel_job,
            commands::confirm_low_res,
            commands::progress_ready,
            commands::editor_files,
            commands::editor_open,
            commands::editor_adjacent_file,
            commands::editor_order_files,
            commands::editor_peaks,
            commands::editor_thumbs,
            commands::editor_proxy,
            commands::editor_export,
            commands::editor_discard,
            commands::editor_close,
            commands::editor_ready,
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
            let dispatcher = Dispatcher::new(app.handle().clone(), setup_queue, emitted);
            thread::Builder::new().name("hasami-dispatch".into()).spawn(move || dispatcher.run(rx))?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Hasami")
        .run(|app, event| match event {
            // Closing the last window must not end the process while jobs remain; the dispatcher
            // calls `exit` (which carries a code) once the queue has drained.
            RunEvent::ExitRequested { code: None, api, .. } => api.prevent_exit(),
            // Waits for an immediate discard still recycling its clip, then (briefly) for the
            // editor's cleanup, and catches a clip that finished opening after the editor closed.
            RunEvent::Exit => {
                let state = app.state::<AppState>();
                drop(lock(&state.recycling));
                lock(&state.sessions).close_all();
            }
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
    /// Held while an immediate discard (`editor_discard` with no `after`) recycles its clip, so
    /// the process doesn't exit halfway through. Flow tracks deferred discards itself.
    recycling: Mutex<()>,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn files(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn relative_paths_resolve_against_the_launch_cwd() {
        let cwd = Path::new("C:\\clips");
        let action = resolve_paths(Action::Compress(files(&["a.mp4", "D:\\b.mp4"])), cwd);
        assert_eq!(action, Action::Compress(files(&["C:\\clips\\a.mp4", "D:\\b.mp4"])));
    }
}
