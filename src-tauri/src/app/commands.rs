//! The Tauri commands the progress and editor windows call. Editor commands that run ffmpeg or
//! wait on the sessions lock run on a blocking thread (see [`blocking`]).

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use tauri::{AppHandle, Manager, State};

use super::dispatcher::Msg;
use super::events::{EditorEvent, EditorMediaDto};
use super::{lock, AppState};
use crate::editor::folder::{self, adjacent_video, order_videos};
use crate::editor::media::{make_proxy, peaks, prepare, thumbnails, Peaks, Thumb};
use crate::editor::request::ExportRequest;
use crate::fsops;
use crate::jobs::JobError;

// What the editor's commands answer when they fail.
const OPEN_FAILED: &str = "Couldn't open this clip in the editor.";
const SESSION_GONE: &str = "This clip is no longer open in the editor.";
const NO_SUCH_TRACK: &str = "This audio track has no sound to show.";
const PEAKS_FAILED: &str = "Couldn't read this audio track.";
const THUMBS_FAILED: &str = "Couldn't make the timeline pictures for this clip.";
const PROXY_FAILED: &str = "Couldn't prepare a preview of this clip.";
const EXPORT_WONT_START: &str = "Couldn't start the export.";
const NEXT_FAILED: &str = "Couldn't look for the next video in this folder.";
const PREV_FAILED: &str = "Couldn't look for the previous video in this folder.";
pub(super) const DISCARD_FAILED: &str = "Couldn't move this clip to the Recycle Bin.";
const DISCARD_IN_USE: &str =
    "Couldn't move this clip to the Recycle Bin because another program is using it. Close it there, then try Discard again.";
const DISCARD_WONT_WAIT: &str = "Couldn't set this clip aside for the Recycle Bin.";
const COMMAND_FAILED: &str = "Something went wrong.";

#[tauri::command]
pub(super) fn cancel_job(id: u64, state: State<'_, AppState>) {
    state.queue.cancel(id);
}

#[tauri::command]
pub(super) fn confirm_low_res(id: u64, proceed: bool, state: State<'_, AppState>) {
    state.queue.confirm(id, proceed);
}

/// Called by the progress window once its job-event listener is attached.
#[tauri::command]
pub(super) fn progress_ready(state: State<'_, AppState>) {
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
    tauri::async_runtime::spawn_blocking(move || work(&app, &app.state::<AppState>())).await.unwrap_or_else(|e| {
        eprintln!("hasami: an editor command panicked: {e}");
        Err(COMMAND_FAILED.to_owned())
    })
}

/// The files the editor window was opened with.
#[tauri::command]
pub(super) fn editor_files(state: State<'_, AppState>) -> Vec<String> {
    lock(&state.editor_files).clone()
}

/// Open `path` in a new session and let the page load its files through the asset protocol.
#[tauri::command]
pub(super) async fn editor_open(app: AppHandle, path: String) -> Result<EditorMediaDto, String> {
    blocking(app, move |app, state| {
        let paths = state.tools()?;
        let input = PathBuf::from(path);
        // Prepared without holding the lock, so closing the editor never waits on it.
        let reservation = lock(&state.sessions).reserve(&input);
        let prepared = prepare(paths, &input, &reservation.work_dir, &reservation.cancel);
        let media = lock(&state.sessions).finish(reservation.id, prepared).map_err(|e| plain(&e, OPEN_FAILED))?;
        let (session, work_dir) = (reservation.id, reservation.work_dir);
        let scope = app.asset_protocol_scope();
        if let Err(e) = scope.allow_file(&input).and_then(|()| scope.allow_directory(&work_dir, true)) {
            eprintln!("hasami: could not allow {} in the asset scope: {e}", input.display());
            lock(&state.sessions).close(session);
            return Err(OPEN_FAILED.to_owned());
        }
        let modified_ms =
            std::fs::metadata(&input).and_then(|m| m.modified()).ok().and_then(folder::millis).unwrap_or(0);
        Ok(EditorMediaDto::new(session, &input, &media, modified_ms))
    })
    .await
}

/// The video next to `path` in its folder, ordered by modified time as Explorer sorts by date (see
/// [`adjacent_video`]): the next newer one, or the next older one when `newer` is false. `modified_ms` is
/// the open clip's modified time, so this works after the clip has gone. `None` when there is nothing
/// further that way. Only names a path: `editor_open` grants access to it.
#[tauri::command]
pub(super) async fn editor_adjacent_file(
    app: AppHandle,
    path: String,
    modified_ms: u64,
    newer: bool,
) -> Result<Option<String>, String> {
    blocking(app, move |_, _| {
        adjacent_video(Path::new(&path), modified_ms, newer)
            .map(|found| found.map(|p| p.display().to_string()))
            .map_err(|e| {
                eprintln!("hasami: could not look for the neighbouring video of {path}: {e}");
                if newer { NEXT_FAILED } else { PREV_FAILED }.to_owned()
            })
    })
    .await
}

/// `paths` oldest first by modified time, ties by name (see [`order_videos`]): the trim queue's order.
/// Unreadable paths go last, in the order given.
#[tauri::command]
pub(super) async fn editor_order_files(app: AppHandle, paths: Vec<String>) -> Vec<String> {
    let fallback = paths.clone();
    // Ordering is only a convenience: if the worker fails, keep the selection as it was.
    blocking(app, move |_, _| {
        let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
        Ok(order_videos(&paths).into_iter().map(|p| p.display().to_string()).collect())
    })
    .await
    .unwrap_or(fallback)
}

/// Waveform of audio track `index` (ffmpeg's `0:a:<index>`) of a session. Stops when the session
/// closes.
#[tauri::command]
pub(super) async fn editor_peaks(app: AppHandle, session: u64, index: usize) -> Result<Peaks, String> {
    blocking(app, move |_, state| {
        let paths = state.tools()?;
        let (file, duration_s, cancel) = {
            let sessions = lock(&state.sessions);
            let media = sessions.media(session).ok_or(SESSION_GONE)?;
            let track = media.tracks.iter().find(|t| t.index == index);
            let file = track.and_then(|t| t.file.clone()).ok_or(NO_SUCH_TRACK)?;
            (file, media.info.duration_s, sessions.cancel_token(session).ok_or(SESSION_GONE)?)
        };
        peaks(paths, &file, duration_s, &cancel).map_err(|e| plain(&e, PEAKS_FAILED))
    })
    .await
}

/// Timeline thumbnails of a session's clip, written into its work folder. Stops when the session
/// closes.
#[tauri::command]
pub(super) async fn editor_thumbs(app: AppHandle, session: u64) -> Result<Vec<Thumb>, String> {
    blocking(app, move |_, state| {
        let paths = state.tools()?;
        let (input, work_dir, duration_s, cancel) = {
            let sessions = lock(&state.sessions);
            let duration_s = sessions.media(session).ok_or(SESSION_GONE)?.info.duration_s;
            let input = sessions.input(session).ok_or(SESSION_GONE)?.to_path_buf();
            let work_dir = sessions.work_dir(session).ok_or(SESSION_GONE)?.to_path_buf();
            (input, work_dir, duration_s, sessions.cancel_token(session).ok_or(SESSION_GONE)?)
        };
        let result = thumbnails(paths, &input, duration_s, &work_dir.join("thumbs"), &cancel);
        sweep_if_closed(state, session, &work_dir);
        result.map_err(|e| plain(&e, THUMBS_FAILED))
    })
    .await
}

/// Encode a session's H.264 preview proxy, reporting `ProxyProgress`. Stops when the session
/// closes. Returns the proxy's path.
#[tauri::command]
pub(super) async fn editor_proxy(app: AppHandle, session: u64) -> Result<String, String> {
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

/// Closing a session waits only so long for its ffmpeg runs to let go of its folder, and a run
/// that was just starting may have recreated it; delete whatever is left now.
fn sweep_if_closed(state: &AppState, session: u64, work_dir: &Path) {
    if lock(&state.sessions).work_dir(session).is_none() {
        let _ = std::fs::remove_dir_all(work_dir);
    }
}

/// Queue an export of the edit. Returns its job id; progress arrives as `Export*` events.
#[tauri::command]
pub(super) async fn editor_export(app: AppHandle, request: ExportRequest) -> Result<u64, String> {
    blocking(app, move |_, state| {
        let (reply, id) = mpsc::channel();
        state.tx.send(Msg::Export(Box::new(request), reply)).map_err(|_| EXPORT_WONT_START)?;
        id.recv().map_err(|_| EXPORT_WONT_START.to_owned())
    })
    .await
}

/// Send a clip the user discarded to the Recycle Bin: now when `after` is empty, otherwise once
/// every export in `after` is done (if one fails, the clip stays; `Discard*` events say which).
/// A deferred discard is recorded before this returns, so it happens even if the window closes.
#[tauri::command]
pub(super) async fn editor_discard(app: AppHandle, path: String, after: Vec<u64>) -> Result<(), String> {
    blocking(app, move |_, state| {
        if after.is_empty() {
            let _recycling = lock(&state.recycling);
            return recycle(&path);
        }
        let (reply, recorded) = mpsc::channel();
        state.tx.send(Msg::Discard { path, after, reply }).map_err(|_| DISCARD_WONT_WAIT)?;
        recorded.recv().map_err(|_| DISCARD_WONT_WAIT.to_owned())
    })
    .await
}

pub(super) fn recycle(path: &str) -> Result<(), String> {
    fsops::recycle_when_free(Path::new(path)).map_err(|e| {
        eprintln!("hasami: {DISCARD_FAILED} {e:?}");
        discard_failure(&e).to_owned()
    })
}

/// What the editor says when a discarded clip couldn't be recycled.
fn discard_failure(e: &fsops::FsError) -> &'static str {
    if e.message == fsops::RECYCLE_IN_USE {
        DISCARD_IN_USE
    } else {
        DISCARD_FAILED
    }
}

/// The editor is done with a clip: stop its work and delete its work folder.
#[tauri::command]
pub(super) async fn editor_close(app: AppHandle, session: u64) {
    let _ = blocking(app, move |_, state| {
        lock(&state.sessions).close(session);
        Ok(())
    })
    .await;
}

/// Called by the editor window once its editor-event listener is attached.
#[tauri::command]
pub(super) fn editor_ready(state: State<'_, AppState>) {
    let _ = state.tx.send(Msg::EditorReady);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_discard_that_failed_because_the_clip_was_in_use_says_so() {
        let in_use = fsops::FsError::new(fsops::RECYCLE_IN_USE);
        assert_eq!(discard_failure(&in_use), DISCARD_IN_USE);
        let other = fsops::FsError::new("Couldn't move the original to the Recycle Bin.");
        assert_eq!(discard_failure(&other), DISCARD_FAILED);
    }
}
