//! Writing a job's result safely: claiming a new file's hidden temp name up front, giving the
//! verified file its name, and a guard that deletes scratch files on every way out.

use std::fs;
use std::path::{Path, PathBuf};

use super::JobError;
use crate::fsops::{commit_new, temp_path_for, FsError, NO_VIDEOS_FOLDER};
use crate::media::run::CancelToken;

/// Shown when the Videos folder is there but a new file can't be written in it.
const NO_WRITE_VIDEOS: &str = "Couldn't write the new file in your Videos folder.";

/// Finds the folder a `[20MB]` file goes into; only called once something will be written.
pub(super) type VideosDirFn<'a> = &'a dyn Fn() -> Result<PathBuf, FsError>;

/// Start a new file that will be named `final_path` (in the Videos folder): create its hidden
/// temp file now, which proves the folder is there and writable before any encode runs. The
/// temp file is deleted when the guard drops, unless [`commit_claimed`] keeps it.
pub(super) fn claim_new_file(final_path: &Path) -> Result<RemoveOnDrop, JobError> {
    if !final_path.parent().is_some_and(Path::is_dir) {
        return Err(JobError::Message(NO_VIDEOS_FOLDER.to_owned()));
    }
    let temp = temp_path_for(final_path);
    fs::File::create(&temp).map_err(|_| JobError::Message(NO_WRITE_VIDEOS.to_owned()))?;
    Ok(RemoveOnDrop::file(temp))
}

/// Give a finished, verified [`claim_new_file`] temp file its name, never replacing anything.
/// Checks `cancel` one last time first. If the rename may have happened after all, the temp
/// file is kept, since it may be the only copy.
pub(super) fn commit_claimed(temp: &mut RemoveOnDrop, final_path: &Path, cancel: &CancelToken) -> Result<(), JobError> {
    if cancel.is_cancelled() {
        return Err(JobError::Cancelled);
    }
    if let Err(e) = commit_new(temp.path(), final_path) {
        if e.kept_new_file.is_some() {
            temp.keep();
        }
        return Err(e.into());
    }
    Ok(())
}

/// Deletes a file or folder when dropped, so every exit path (error, cancel, panic) cleans up.
/// Deleting something that is already gone (e.g. a temp file that was committed) is a no-op.
pub(super) struct RemoveOnDrop {
    path: PathBuf,
    is_dir: bool,
    kept: bool,
}

impl RemoveOnDrop {
    pub(super) fn file(path: PathBuf) -> Self {
        Self { path, is_dir: false, kept: false }
    }

    pub(super) fn dir(path: PathBuf) -> Self {
        Self { path, is_dir: true, kept: false }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    /// Don't delete it after all.
    pub(super) fn keep(&mut self) {
        self.kept = true;
    }
}

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        if self.kept {
            return;
        }
        let _ = if self.is_dir { fs::remove_dir_all(&self.path) } else { fs::remove_file(&self.path) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_on_drop_deletes_unless_kept() {
        let dir = tempfile::tempdir().unwrap();
        let (gone, kept) = (dir.path().join("gone"), dir.path().join("kept"));
        fs::write(&gone, b"x").unwrap();
        fs::write(&kept, b"x").unwrap();
        drop(RemoveOnDrop::file(gone.clone()));
        let mut guard = RemoveOnDrop::file(kept.clone());
        guard.keep();
        drop(guard);
        assert!(!gone.exists());
        assert!(kept.exists());
    }

    #[test]
    fn claim_new_file_needs_the_folder_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("gone").join("[20MB] a.mp4");
        let err = claim_new_file(&missing).err().unwrap();
        assert_eq!(err.to_string(), NO_VIDEOS_FOLDER);

        let final_path = dir.path().join("[20MB] a.mp4");
        let claim = claim_new_file(&final_path).unwrap();
        assert!(claim.path().is_file(), "the temp file is created up front");
        drop(claim);
        assert!(fs::read_dir(dir.path()).unwrap().next().is_none(), "and deleted when dropped");
    }
}
