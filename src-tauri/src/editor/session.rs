//! Editor sessions: one per clip open in the Quick Trim editor, each with its own work folder
//! (`%TEMP%\hasami-editor-<pid>-<id>`) for the extracted audio, thumbnails and preview proxy.
//! Closing a session deletes its folder; dropping [`EditorSessions`] closes them all.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::media::{prepare, PreparedMedia};
use crate::jobs::JobError;
use crate::media::run::CancelToken;
use crate::media::FfmpegPaths;

struct Session {
    input: PathBuf,
    work_dir: PathBuf,
    media: PreparedMedia,
    /// Cancelled when the session closes, so a running proxy encode stops.
    cancel: CancelToken,
}

/// The open sessions, by id. Ids start at 1 and are never reused.
pub struct EditorSessions {
    next_id: u64,
    open: HashMap<u64, Session>,
}

impl Default for EditorSessions {
    fn default() -> Self {
        Self::new()
    }
}

impl EditorSessions {
    pub fn new() -> Self {
        Self { next_id: 1, open: HashMap::new() }
    }

    /// Prepare `input` (see [`prepare`]) in a new work folder. Returns the session id, the
    /// prepared media and the folder. On failure nothing is left behind.
    pub fn open(&mut self, paths: &FfmpegPaths, input: &Path) -> Result<(u64, PreparedMedia, PathBuf), JobError> {
        let id = self.next_id;
        self.next_id += 1;
        let work_dir = std::env::temp_dir().join(format!("hasami-editor-{}-{id}", std::process::id()));
        // A folder from an earlier process that had the same id would mix its files into ours.
        let _ = std::fs::remove_dir_all(&work_dir);
        match prepare(paths, input, &work_dir) {
            Ok(media) => {
                let session = Session {
                    input: input.to_path_buf(),
                    work_dir: work_dir.clone(),
                    media: media.clone(),
                    cancel: CancelToken::new(),
                };
                self.open.insert(id, session);
                Ok((id, media, work_dir))
            }
            Err(e) => {
                let _ = std::fs::remove_dir_all(&work_dir);
                Err(e)
            }
        }
    }

    pub fn work_dir(&self, id: u64) -> Option<&Path> {
        self.open.get(&id).map(|s| s.work_dir.as_path())
    }

    pub fn input(&self, id: u64) -> Option<&Path> {
        self.open.get(&id).map(|s| s.input.as_path())
    }

    pub fn media(&self, id: u64) -> Option<&PreparedMedia> {
        self.open.get(&id).map(|s| &s.media)
    }

    /// The token cancelled when session `id` closes.
    pub fn cancel_token(&self, id: u64) -> Option<CancelToken> {
        self.open.get(&id).map(|s| s.cancel.clone())
    }

    /// Stop the session's work and delete its folder. Unknown ids are ignored.
    pub fn close(&mut self, id: u64) {
        if let Some(session) = self.open.remove(&id) {
            session.cancel.cancel();
            let _ = std::fs::remove_dir_all(&session.work_dir);
        }
    }

    pub fn close_all(&mut self) {
        let ids: Vec<u64> = self.open.keys().copied().collect();
        for id in ids {
            self.close(id);
        }
    }
}

impl Drop for EditorSessions {
    fn drop(&mut self) {
        self.close_all();
    }
}
