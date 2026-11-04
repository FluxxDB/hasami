//! Editor sessions: one per clip open in the Quick Trim editor, each with its own work folder
//! (`%TEMP%\hasami-editor-<pid>-<id>`) for the extracted audio, thumbnails and preview proxy.
//! Closing a session stops its ffmpeg runs and deletes its folder; dropping [`EditorSessions`]
//! closes them all. Folders left behind by a crash are removed by [`sweep_stale`] on a later start.
//!
//! Preparing a clip takes seconds, so the app doesn't hold its sessions lock through it:
//! [`EditorSessions::reserve`] registers the session, `prepare` runs unlocked with the session's
//! cancel token, and [`EditorSessions::finish`] records the result (or discards it when the
//! session was closed meanwhile). [`EditorSessions::open`] does all three for single-threaded use.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use super::media::{prepare, PreparedMedia};
use crate::jobs::compress::PASSLOG_DIR_PREFIX;
use crate::jobs::JobError;
use crate::media::run::CancelToken;
use crate::media::FfmpegPaths;

/// How long closing waits for cancelled ffmpeg runs to let go of their files before it gives up
/// on deleting a work folder (the next start's [`sweep_stale`] removes it then).
pub const CLOSE_WAIT: Duration = Duration::from_secs(2);
const WORK_DIR_PREFIX: &str = "hasami-editor-";
/// Session ids are unique within the process, so two `EditorSessions` never share a work folder.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

struct Session {
    input: PathBuf,
    work_dir: PathBuf,
    /// `None` while the clip is still being prepared.
    media: Option<PreparedMedia>,
    /// Cancelled when the session closes, so its ffmpeg runs stop.
    cancel: CancelToken,
}

/// A session being prepared, from [`EditorSessions::reserve`].
#[derive(Debug, Clone)]
pub struct Reservation {
    pub id: u64,
    pub work_dir: PathBuf,
    pub cancel: CancelToken,
}

/// The open sessions, by id. Ids start at 1 and are never reused within the process.
pub struct EditorSessions {
    open: HashMap<u64, Session>,
}

impl Default for EditorSessions {
    fn default() -> Self {
        Self::new()
    }
}

impl EditorSessions {
    /// No sessions open.
    pub fn new() -> Self {
        Self { open: HashMap::new() }
    }

    /// Prepare `input` (see [`prepare`]) in a new work folder. Returns the session id, the
    /// prepared media and the folder. On failure nothing is left behind.
    pub fn open(&mut self, paths: &FfmpegPaths, input: &Path) -> Result<(u64, PreparedMedia, PathBuf), JobError> {
        let r = self.reserve(input);
        let result = prepare(paths, input, &r.work_dir, &r.cancel);
        self.finish(r.id, result).map(|media| (r.id, media, r.work_dir))
    }

    /// Register a session for `input` that is about to be prepared in the returned work folder.
    pub fn reserve(&mut self, input: &Path) -> Reservation {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let work_dir = work_dir_for(id);
        // A folder from an earlier process that had the same id would mix its files into ours.
        let _ = std::fs::remove_dir_all(&work_dir);
        let cancel = CancelToken::new();
        let session =
            Session { input: input.to_path_buf(), work_dir: work_dir.clone(), media: None, cancel: cancel.clone() };
        self.open.insert(id, session);
        Reservation { id, work_dir, cancel }
    }

    /// Record how preparing reserved session `id` went. A failure closes the session; a session
    /// closed while it was being prepared stays closed, and this returns [`JobError::Cancelled`].
    pub fn finish(&mut self, id: u64, result: Result<PreparedMedia, JobError>) -> Result<PreparedMedia, JobError> {
        let Some(session) = self.open.get_mut(&id) else {
            // Closed meanwhile; `prepare` may have written more after the close deleted the folder.
            remove_dirs(vec![work_dir_for(id)], Instant::now() + CLOSE_WAIT);
            return Err(JobError::Cancelled);
        };
        match result {
            Ok(media) => {
                session.media = Some(media.clone());
                Ok(media)
            }
            Err(e) => {
                self.close(id);
                Err(e)
            }
        }
    }

    /// The work folder of session `id`, prepared or still being prepared.
    pub fn work_dir(&self, id: u64) -> Option<&Path> {
        self.open.get(&id).map(|s| s.work_dir.as_path())
    }

    /// The clip of session `id`, prepared or still being prepared.
    pub fn input(&self, id: u64) -> Option<&Path> {
        self.open.get(&id).map(|s| s.input.as_path())
    }

    /// The prepared media of session `id`; `None` while it is still being prepared.
    pub fn media(&self, id: u64) -> Option<&PreparedMedia> {
        self.open.get(&id).and_then(|s| s.media.as_ref())
    }

    /// The token cancelled when session `id` closes.
    pub fn cancel_token(&self, id: u64) -> Option<CancelToken> {
        self.open.get(&id).map(|s| s.cancel.clone())
    }

    /// Stop the session's ffmpeg runs and delete its folder, waiting up to [`CLOSE_WAIT`] for
    /// them to let go of it and of the clip (a discard recycles the clip right after this).
    /// Unknown ids are ignored.
    pub fn close(&mut self, id: u64) {
        self.close_where(|open| open == id);
    }

    /// [`close`](Self::close) every session, waiting at most [`CLOSE_WAIT`] in all.
    pub fn close_all(&mut self) {
        self.close_where(|_| true);
    }

    /// [`close`](Self::close) every session reserved before [`next_session_id`] returned `upto`,
    /// waiting at most [`CLOSE_WAIT`] in all. A closed editor's cleanup uses this, so a session
    /// the next editor reserved before the cleanup got the lock is left open.
    pub fn close_before(&mut self, upto: u64) {
        self.close_where(|id| id < upto);
    }

    fn close_where(&mut self, pick: impl Fn(u64) -> bool) {
        let ids: Vec<u64> = self.open.keys().copied().filter(|&id| pick(id)).collect();
        let sessions: Vec<Session> = ids.iter().filter_map(|id| self.open.remove(id)).collect();
        for session in &sessions {
            session.cancel.cancel();
        }
        let deadline = Instant::now() + CLOSE_WAIT;
        // A run's work folder can often be deleted while its ffmpeg still reads the clip (the
        // thumbnails write a picture only now and then), so wait for the runs themselves.
        for session in &sessions {
            session.cancel.wait_for_runs(deadline);
        }
        remove_dirs(sessions.into_iter().map(|s| s.work_dir).collect(), deadline);
    }
}

impl Drop for EditorSessions {
    fn drop(&mut self) {
        self.close_all();
    }
}

/// The id the next [`EditorSessions::reserve`] will hand out; every session reserved so far has a
/// smaller one. Reading it takes no lock.
pub fn next_session_id() -> u64 {
    NEXT_ID.load(Ordering::Relaxed)
}

fn work_dir_for(id: u64) -> PathBuf {
    std::env::temp_dir().join(format!("{WORK_DIR_PREFIX}{}-{id}", std::process::id()))
}

/// Delete `dirs`, retrying until `deadline` while a cancelled ffmpeg still holds a file in one of
/// them.
fn remove_dirs(mut dirs: Vec<PathBuf>, deadline: Instant) {
    loop {
        dirs.retain(|dir| std::fs::remove_dir_all(dir).is_err() && dir.exists());
        if dirs.is_empty() || Instant::now() >= deadline {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// The pid in the name of a Hasami scratch folder: an editor work folder,
/// `hasami-editor-<pid>-<id>`, or a Compress pass-log folder, `hasami-2pass-<pid>-<nanos>-<n>`.
fn work_dir_pid(name: &str) -> Option<u32> {
    let rest = [WORK_DIR_PREFIX, PASSLOG_DIR_PREFIX].iter().find_map(|prefix| name.strip_prefix(prefix))?;
    let (pid, ids) = rest.split_once('-')?;
    ids.split('-').all(|id| id.parse::<u64>().is_ok()).then_some(())?;
    pid.parse().ok()
}

/// Delete the scratch folders in `temp` (editor work folders and Compress pass logs) left by Hasami
/// processes that are no longer running (a crash, or an exit that couldn't wait for ffmpeg). This
/// process's own folders are left alone.
pub fn sweep_stale(temp: &Path, is_running: impl Fn(u32) -> bool) {
    let Ok(entries) = std::fs::read_dir(temp) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(work_dir_pid) else {
            continue;
        };
        if pid != std::process::id() && !is_running(pid) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Whether a process with this id is running. A process Hasami may not inspect still counts.
pub fn process_running(pid: u32) -> bool {
    use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, STILL_ACTIVE};
    use windows::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    // SAFETY: plain Win32 calls; the handle is closed before returning.
    unsafe {
        let handle = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(handle) => handle,
            Err(e) => return e.code() == ERROR_ACCESS_DENIED.to_hresult(),
        };
        let mut code = 0u32;
        let running = GetExitCodeProcess(handle, &mut code).is_ok() && code == STILL_ACTIVE.0 as u32;
        let _ = CloseHandle(handle);
        running
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::MediaInfo;

    fn media() -> PreparedMedia {
        let info = MediaInfo {
            path: PathBuf::from("a.mp4"),
            container: "mp4".into(),
            duration_s: 1.0,
            size_bytes: 1,
            bit_rate: None,
            video: None,
            audio: Vec::new(),
        };
        PreparedMedia { info, tracks: Vec::new(), border: None }
    }

    #[test]
    fn a_session_closed_while_preparing_is_discarded() {
        let mut sessions = EditorSessions::new();
        let r = sessions.reserve(Path::new("a.mp4"));
        std::fs::create_dir_all(&r.work_dir).unwrap();
        assert_eq!(sessions.media(r.id), None, "not prepared yet");
        assert_eq!(sessions.work_dir(r.id), Some(r.work_dir.as_path()));

        sessions.close(r.id);
        assert!(r.cancel.is_cancelled(), "the running prepare is told to stop");
        // `prepare` wrote one more file after the close.
        std::fs::create_dir_all(&r.work_dir).unwrap();
        std::fs::write(r.work_dir.join("a0.m4a"), b"x").unwrap();

        assert!(matches!(sessions.finish(r.id, Ok(media())), Err(JobError::Cancelled)));
        assert!(!r.work_dir.exists());
        assert_eq!(sessions.work_dir(r.id), None);
    }

    #[test]
    fn a_failed_prepare_closes_the_session() {
        let mut sessions = EditorSessions::new();
        let r = sessions.reserve(Path::new("a.mp4"));
        std::fs::create_dir_all(&r.work_dir).unwrap();
        let result = sessions.finish(r.id, Err(JobError::Message("nope".into())));
        assert!(matches!(result, Err(JobError::Message(m)) if m == "nope"));
        assert!(!r.work_dir.exists());
        assert_eq!(sessions.input(r.id), None);
    }

    #[test]
    fn closing_an_editor_spares_sessions_reserved_after_it() {
        let mut sessions = EditorSessions::new();
        let old = sessions.reserve(Path::new("a.mp4"));
        std::fs::create_dir_all(&old.work_dir).unwrap();
        // The editor window is destroyed here; the next editor opens a clip before the cleanup runs.
        let upto = next_session_id();
        let new = sessions.reserve(Path::new("b.mp4"));
        std::fs::create_dir_all(&new.work_dir).unwrap();

        sessions.close_before(upto);
        assert!(old.cancel.is_cancelled());
        assert!(!old.work_dir.exists());
        assert_eq!(sessions.input(old.id), None);
        assert!(!new.cancel.is_cancelled(), "the next editor's clip is left alone");
        assert!(new.work_dir.exists());
        assert_eq!(sessions.input(new.id), Some(Path::new("b.mp4")));
        sessions.close(new.id);
    }

    #[test]
    fn finish_records_the_media() {
        let mut sessions = EditorSessions::new();
        let r = sessions.reserve(Path::new("a.mp4"));
        assert_eq!(sessions.finish(r.id, Ok(media())).unwrap(), media());
        assert_eq!(sessions.media(r.id), Some(&media()));
        sessions.close(r.id);
    }

    #[test]
    fn close_waits_for_a_held_file_to_be_released() {
        use std::os::windows::fs::OpenOptionsExt;
        let mut sessions = EditorSessions::new();
        let r = sessions.reserve(Path::new("a.mp4"));
        std::fs::create_dir_all(&r.work_dir).unwrap();
        // Like an ffmpeg still writing: no sharing, so nothing can delete the file meanwhile.
        let held = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .share_mode(0)
            .open(r.work_dir.join("proxy.mp4"))
            .unwrap();
        let releaser = thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            drop(held);
        });
        let started = Instant::now();
        sessions.close(r.id);
        releaser.join().unwrap();
        assert!(!r.work_dir.exists(), "deleted once the file was let go");
        assert!(started.elapsed() < CLOSE_WAIT);
    }

    #[test]
    fn stale_work_dirs_of_dead_processes_are_swept() {
        let temp = tempfile::tempdir().unwrap();
        let me = std::process::id();
        let names = [
            "hasami-editor-111-1".to_owned(),     // dead
            "hasami-editor-222-3".to_owned(),     // still running
            format!("hasami-editor-{me}-1"),      // ours
            "hasami-editor-notapid-1".to_owned(), // not a work folder
            "hasami-export-111".to_owned(),       // someone else's
            "hasami-2pass-111-5-0".to_owned(),    // a dead Compress's pass log
            "hasami-2pass-222-5-1".to_owned(),    // a running one's
        ];
        for name in &names {
            std::fs::create_dir(temp.path().join(name)).unwrap();
        }
        sweep_stale(temp.path(), |pid| pid == 222);
        let left: Vec<bool> = names.iter().map(|n| temp.path().join(n).exists()).collect();
        assert_eq!(left, [false, true, true, true, true, false, true]);
    }

    #[test]
    fn this_process_is_running() {
        assert!(process_running(std::process::id()));
        assert!(!process_running(0xFFFF_FFF0));
    }
}
