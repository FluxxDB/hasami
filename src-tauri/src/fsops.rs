//! Temp/final/unique file names, commit and recycle helpers (Task 4).
//!
//! This is the safety core: the user's original must never be lost. Every
//! function here either completes its whole job or leaves the original at its
//! original path with its original bytes.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

/// A file-operation failure. `message` is plain language and goes straight to
/// the progress window; `source` keeps the underlying error for logs.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct FsError {
    pub message: String,
    #[source]
    pub source: Option<Box<dyn std::error::Error + Send + Sync + 'static>>,
}

impl FsError {
    fn new(message: impl Into<String>) -> Self {
        FsError { message: message.into(), source: None }
    }

    fn with_source(
        message: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        FsError { message: message.into(), source: Some(Box::new(source)) }
    }

    /// Wrap an I/O error, choosing the wording from what went wrong.
    fn from_io(err: io::Error) -> Self {
        // Windows: 32 = ERROR_SHARING_VIOLATION, 33 = ERROR_LOCK_VIOLATION,
        // 5 = ERROR_ACCESS_DENIED.
        let message = match err.raw_os_error() {
            Some(32) | Some(33) => {
                "Couldn't replace the file because another program is using it."
            }
            Some(5) => "Couldn't replace the file because Windows denied access to it.",
            _ if err.kind() == io::ErrorKind::PermissionDenied => {
                "Couldn't replace the file because Windows denied access to it."
            }
            _ if err.kind() == io::ErrorKind::NotFound => {
                "Couldn't replace the file because it could no longer be found."
            }
            _ => "Couldn't replace the file.",
        };
        FsError::with_source(message, err)
    }
}

/// Hidden sibling used while a result is being written: `.<name>.hasami-tmp`.
pub fn temp_path_for(final_path: &Path) -> PathBuf {
    sibling_with_suffix(final_path, ".hasami-tmp")
}

fn sibling_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(".");
    name.push(path.file_name().unwrap_or_default());
    name.push(suffix);
    path.with_file_name(name)
}

/// Windows paths are case-insensitive; treat `A.mp4` and `a.MP4` as the same file.
fn same_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    if cfg!(windows) {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        false
    }
}

/// `desired` if it is free (or is `allow_existing`); otherwise `<stem> (2).<ext>`,
/// `(3)`, … until a free name is found.
pub fn unique_path(desired: &Path, allow_existing: Option<&Path>) -> PathBuf {
    if let Some(allowed) = allow_existing {
        if same_path(desired, allowed) {
            return desired.to_path_buf();
        }
    }
    if !desired.exists() {
        return desired.to_path_buf();
    }
    let stem = desired.file_stem().unwrap_or_default().to_os_string();
    let ext = desired.extension().map(|e| e.to_os_string());
    let mut n: u32 = 2;
    loop {
        let mut name = stem.clone();
        name.push(format!(" ({n})"));
        if let Some(ext) = &ext {
            name.push(".");
            name.push(ext);
        }
        let candidate = desired.with_file_name(name);
        let allowed = allow_existing.is_some_and(|a| same_path(&candidate, a));
        if allowed || !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

/// Same folder and stem as `original`, with a new extension (`ext` without the dot).
pub fn with_extension(original: &Path, ext: &str) -> PathBuf {
    original.with_extension(ext.trim_start_matches('.'))
}

/// Replace `original` with the finished `temp` file, saved as `final_path`.
///
/// On any error the original is back at its own path with its own bytes and the
/// temp file is deleted.
pub fn commit_replace(temp: &Path, original: &Path, final_path: &Path) -> Result<(), FsError> {
    let result = if same_path(final_path, original) {
        replace_in_place(temp, original, final_path)
    } else {
        replace_with_new_name(temp, original, final_path)
    };
    if result.is_err() {
        // Best effort; if the temp already moved away this is a harmless no-op.
        let _ = std::fs::remove_file(temp);
    }
    result
}

/// Output has the same path as the original: park the original, move the new
/// file in, then recycle the parked original.
fn replace_in_place(temp: &Path, original: &Path, final_path: &Path) -> Result<(), FsError> {
    let old = free_old_path(original)?;

    std::fs::rename(original, &old).map_err(FsError::from_io)?;

    if let Err(e) = std::fs::rename(temp, final_path) {
        // Put the original back. If even that fails, the original still exists
        // at `old`; say so rather than pretend.
        return Err(match std::fs::rename(&old, original) {
            Ok(()) => FsError::from_io(e),
            Err(restore) => FsError::with_source(
                format!(
                    "Couldn't replace the file, and the original is saved as \"{}\".",
                    old.display()
                ),
                restore,
            ),
        });
    }

    if let Err(e) = recycle(&old) {
        // Undo: discard the new file and restore the original so the user's
        // folder looks exactly as it did before.
        if std::fs::remove_file(final_path).is_ok() && std::fs::rename(&old, original).is_ok() {
            return Err(e);
        }
        return Err(FsError::new(format!(
            "Couldn't move the old file to the Recycle Bin. The original is saved as \"{}\".",
            old.display()
        )));
    }
    Ok(())
}

/// Output has a different name (e.g. new extension): move the new file in, then
/// recycle the original.
fn replace_with_new_name(temp: &Path, original: &Path, final_path: &Path) -> Result<(), FsError> {
    // `rename` silently overwrites on Windows; never let it eat someone's file.
    if final_path.exists() {
        return Err(FsError::new(
            "Couldn't save the new file because a file with that name already exists.",
        ));
    }
    std::fs::rename(temp, final_path).map_err(FsError::from_io)?;

    if let Err(e) = recycle(original) {
        // Original is untouched; remove the new file so nothing is half-done.
        let _ = std::fs::remove_file(final_path);
        return Err(e);
    }
    Ok(())
}

/// `.<name>.hasami-old`, or a numbered variant if a leftover from an earlier
/// crashed run is sitting there (it may be someone's only copy; never overwrite it).
fn free_old_path(original: &Path) -> Result<PathBuf, FsError> {
    let first = sibling_with_suffix(original, ".hasami-old");
    if !first.exists() {
        return Ok(first);
    }
    for n in 2..1000 {
        let candidate = sibling_with_suffix(original, &format!(".hasami-old{n}"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(FsError::new("Couldn't replace the file because too many leftover backup files are in the folder."))
}

/// Send a file to the Recycle Bin without showing any Windows dialog.
///
/// `trash` on Windows uses `IFileOperation` with `FOF_NO_UI | FOF_ALLOWUNDO |
/// FOF_WANTNUKEWARNING`: no progress, confirmation or error UI, and a file that
/// would be permanently deleted (instead of recycled) fails rather than vanishing.
pub fn recycle(path: &Path) -> Result<(), FsError> {
    // trash 5.2 `unwrap()`s CoCreateInstance; don't let that take the app down.
    let outcome = std::panic::catch_unwind(|| trash::delete(path));
    match outcome {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(FsError::with_source(
            "Couldn't move the old file to the Recycle Bin.",
            e,
        )),
        Err(_) => Err(FsError::new("Couldn't move the old file to the Recycle Bin.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_dir() -> tempfile::TempDir {
        tempfile::Builder::new().prefix("hasami-test-").tempdir().unwrap()
    }

    #[test]
    fn temp_path_is_hidden_sibling() {
        let p = temp_path_for(Path::new(r"C:\d\Replay 1.mp4"));
        assert_eq!(p, Path::new(r"C:\d").join(".Replay 1.mp4.hasami-tmp"));
    }

    #[test]
    fn unique_path_appends_counter() {
        let dir = test_dir();
        let x = dir.path().join("x.mp4");
        fs::write(&x, b"1").unwrap();
        fs::write(dir.path().join("x (2).mp4"), b"2").unwrap();
        assert_eq!(unique_path(&x, None), dir.path().join("x (3).mp4"));
    }

    #[test]
    fn unique_path_returns_free_name_unchanged() {
        let dir = test_dir();
        let x = dir.path().join("free.mp4");
        assert_eq!(unique_path(&x, None), x);
    }

    #[test]
    fn unique_path_allows_original() {
        let dir = test_dir();
        let x = dir.path().join("x.mp4");
        fs::write(&x, b"1").unwrap();
        assert_eq!(unique_path(&x, Some(&x)), x);
    }

    #[test]
    fn with_extension_swaps_ext() {
        assert_eq!(
            with_extension(Path::new(r"C:\d\a.mkv"), "mp4"),
            Path::new(r"C:\d\a.mp4")
        );
    }

    #[test]
    fn commit_replace_same_name() {
        let dir = test_dir();
        let original = dir.path().join("hasami-test-same.mp4");
        let temp = temp_path_for(&original);
        fs::write(&original, b"old").unwrap();
        fs::write(&temp, b"new").unwrap();

        commit_replace(&temp, &original, &original).unwrap();

        assert_eq!(fs::read(&original).unwrap(), b"new");
        assert!(!temp.exists());
        assert!(!sibling_with_suffix(&original, ".hasami-old").exists());
    }

    #[test]
    fn commit_replace_new_ext() {
        let dir = test_dir();
        let original = dir.path().join("hasami-test-a.mkv");
        let final_path = with_extension(&original, "mp4");
        let temp = temp_path_for(&final_path);
        fs::write(&original, b"old").unwrap();
        fs::write(&temp, b"new").unwrap();

        commit_replace(&temp, &original, &final_path).unwrap();

        assert_eq!(fs::read(&final_path).unwrap(), b"new");
        assert!(!original.exists());
        assert!(!temp.exists());
    }

    #[test]
    fn commit_replace_refuses_to_overwrite_existing_final() {
        let dir = test_dir();
        let original = dir.path().join("hasami-test-b.mkv");
        let final_path = dir.path().join("hasami-test-b.mp4");
        let temp = temp_path_for(&final_path);
        fs::write(&original, b"old").unwrap();
        fs::write(&final_path, b"someone else's").unwrap();
        fs::write(&temp, b"new").unwrap();

        assert!(commit_replace(&temp, &original, &final_path).is_err());

        assert_eq!(fs::read(&original).unwrap(), b"old");
        assert_eq!(fs::read(&final_path).unwrap(), b"someone else's");
        assert!(!temp.exists());
    }

    #[test]
    fn commit_replace_keeps_leftover_old_file() {
        let dir = test_dir();
        let original = dir.path().join("hasami-test-c.mp4");
        let temp = temp_path_for(&original);
        let leftover = sibling_with_suffix(&original, ".hasami-old");
        fs::write(&original, b"old").unwrap();
        fs::write(&leftover, b"leftover").unwrap();
        fs::write(&temp, b"new").unwrap();

        commit_replace(&temp, &original, &original).unwrap();

        assert_eq!(fs::read(&original).unwrap(), b"new");
        assert_eq!(fs::read(&leftover).unwrap(), b"leftover");
    }

    #[cfg(windows)]
    #[test]
    fn commit_fails_cleanly_when_original_locked() {
        use std::os::windows::fs::OpenOptionsExt;

        let dir = test_dir();
        let original = dir.path().join("hasami-test-locked.mp4");
        let temp = temp_path_for(&original);
        fs::write(&original, b"precious").unwrap();
        fs::write(&temp, b"new").unwrap();

        let _lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&original)
            .unwrap();

        let err = commit_replace(&temp, &original, &original).unwrap_err();
        assert!(err.message.contains("another program"), "{}", err.message);

        drop(_lock);
        assert_eq!(fs::read(&original).unwrap(), b"precious");
        assert!(!temp.exists());
        assert!(!sibling_with_suffix(&original, ".hasami-old").exists());
    }

    #[cfg(windows)]
    #[test]
    fn commit_to_new_name_fails_cleanly_when_original_locked() {
        use std::os::windows::fs::OpenOptionsExt;

        let dir = test_dir();
        let original = dir.path().join("hasami-test-locked2.mkv");
        let final_path = with_extension(&original, "mp4");
        let temp = temp_path_for(&final_path);
        fs::write(&original, b"precious").unwrap();
        fs::write(&temp, b"new").unwrap();

        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&original)
            .unwrap();

        assert!(commit_replace(&temp, &original, &final_path).is_err());

        drop(lock);
        assert_eq!(fs::read(&original).unwrap(), b"precious");
        assert!(!final_path.exists());
        assert!(!temp.exists());
    }

    #[test]
    fn recycle_removes_file() {
        let dir = test_dir();
        let f = dir.path().join("hasami-test-recycle-me.txt");
        fs::write(&f, b"x").unwrap();
        recycle(&f).unwrap();
        assert!(!f.exists());
    }

    #[test]
    fn recycle_missing_file_is_an_error() {
        let dir = test_dir();
        assert!(recycle(&dir.path().join("hasami-test-nope.txt")).is_err());
    }
}
