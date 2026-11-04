//! File safety: temp, final and unique file names, committing a finished file, and recycling.
//!
//! This is the safety core: the user's original must never be lost. Every function here either
//! completes its whole job, leaves the original at its original path with its original bytes, or
//! (in the one case where neither is possible) keeps the new file and says exactly where both
//! files are.
//!
//! - `naming`: where each output goes (`[CLIPPED] `, `[20MB] `, `Raw\`, the hidden temp name).
//! - `commit`: giving a verified temp file its name, replacing or moving the original.
//! - `presence`: looking at a path without being fooled by a file that is being deleted.
//! - `recycle`: sending a file to the Recycle Bin without any Windows dialog.

mod commit;
mod naming;
mod presence;
mod recycle;
#[cfg(test)]
mod test_support;

use std::io;
use std::path::{Path, PathBuf};

pub use commit::{commit_new, commit_replace, move_to_raw};
pub use naming::{
    clipped_path, compressed_path, in_raw_folder, temp_path_for, unique_path, videos_dir, with_extension,
    CLIPPED_PREFIX, COMPRESSED_PREFIX, NO_VIDEOS_FOLDER,
};
pub use recycle::{recycle, recycle_when_free, RECYCLE_IN_USE};

/// A file-operation failure. `message` is plain language and goes straight to
/// the progress window; `source` keeps the underlying error for logs.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct FsError {
    pub message: String,
    /// False when the original is no longer at its own path (it went to the
    /// Recycle Bin). True for every failure that left the original untouched.
    pub original_intact: bool,
    /// The finished new file, kept because it couldn't be given its final name
    /// after the original was already recycled. Callers must not delete it.
    pub kept_new_file: Option<PathBuf>,
    #[source]
    pub source: Option<Box<dyn std::error::Error + Send + Sync + 'static>>,
}

impl FsError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        FsError { message: message.into(), original_intact: true, kept_new_file: None, source: None }
    }

    pub(super) fn with_source(
        message: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        FsError { source: Some(Box::new(source)), ..FsError::new(message) }
    }

    /// The original is in the Recycle Bin but the new file couldn't be moved to
    /// its final name: it stays at `temp`.
    pub(crate) fn stranded(temp: &Path, source: io::Error) -> Self {
        FsError {
            message: format!(
                "Couldn't finish saving the new file. It's saved as \"{}\", and the original is in the Recycle Bin.",
                temp.display()
            ),
            original_intact: false,
            kept_new_file: Some(temp.to_path_buf()),
            source: Some(Box::new(source)),
        }
    }

    /// Like [`FsError::stranded`], for when Hasami can't be sure where the
    /// original went (recycling reported a failure, or something unexpected is
    /// still at its name). The new file is kept either way.
    pub(crate) fn stranded_unsure(temp: &Path, source: Option<io::Error>) -> Self {
        FsError {
            message: format!(
                "Couldn't finish saving the new file. It's saved as \"{}\". The original may be in the Recycle Bin.",
                temp.display()
            ),
            original_intact: false,
            kept_new_file: Some(temp.to_path_buf()),
            source: source.map(|e| Box::new(e) as _),
        }
    }

    /// A new file's final rename failed, and afterwards the temp file couldn't
    /// be seen where it was: the rename may have worked after all. No original
    /// is involved, so it is untouched; the new file is kept either way.
    pub(super) fn rename_unsure(temp: &Path, final_path: &Path, source: io::Error) -> Self {
        FsError {
            message: format!(
                "Couldn't confirm the new file was saved. It may be saved as \"{}\" or as \"{}\".",
                final_path.display(),
                temp.display()
            ),
            original_intact: true,
            kept_new_file: Some(temp.to_path_buf()),
            source: Some(Box::new(source)),
        }
    }

    /// Wrap an I/O error from replacing an original, choosing the wording from what went wrong.
    pub(super) fn from_io(err: io::Error) -> Self {
        FsError::from_io_while(err, "replace the file")
    }

    /// Wrap an I/O error from giving a new file its name (no original involved).
    pub(super) fn from_io_saving(err: io::Error) -> Self {
        FsError::from_io_while(err, "save the new file")
    }

    /// `Couldn't <doing> because <why>.`, the reason read from the error.
    fn from_io_while(err: io::Error, doing: &str) -> Self {
        // Windows: 32 = ERROR_SHARING_VIOLATION, 33 = ERROR_LOCK_VIOLATION, 5 = ERROR_ACCESS_DENIED.
        let why = match err.raw_os_error() {
            Some(32) | Some(33) => Some("another program is using it"),
            Some(5) => Some("Windows denied access to it"),
            _ => match err.kind() {
                io::ErrorKind::PermissionDenied => Some("Windows denied access to it"),
                io::ErrorKind::NotFound => Some("it could no longer be found"),
                io::ErrorKind::AlreadyExists => {
                    return FsError::with_source(
                        "Couldn't save the new file because a file with that name already exists.",
                        err,
                    )
                }
                _ => None,
            },
        };
        let message = match why {
            Some(why) => format!("Couldn't {doing} because {why}."),
            None => format!("Couldn't {doing}."),
        };
        FsError::with_source(message, err)
    }
}
