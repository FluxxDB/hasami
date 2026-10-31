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
    commit_replace_with(temp, original, final_path, &recycle)
}

/// `commit_replace` with the recycle step injectable, so the rollback paths can
/// be tested deterministically.
fn commit_replace_with(
    temp: &Path,
    original: &Path,
    final_path: &Path,
    recycle: &dyn Fn(&Path) -> Result<(), FsError>,
) -> Result<(), FsError> {
    let result = if same_path(final_path, original) {
        replace_in_place(temp, original, final_path, recycle)
    } else {
        replace_with_new_name(temp, original, final_path, recycle)
    };
    if result.is_err() {
        // Best effort; if the temp already moved away this is a harmless no-op.
        let _ = std::fs::remove_file(temp);
    }
    result
}

/// Output has the same path as the original: park the original, move the new
/// file in, then recycle the parked original.
fn replace_in_place(
    temp: &Path,
    original: &Path,
    final_path: &Path,
    recycle: &dyn Fn(&Path) -> Result<(), FsError>,
) -> Result<(), FsError> {
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
        if !old.exists() {
            // The original is already gone, so the replacement is complete.
            return Ok(());
        }
        // Undo: discard the new file and restore the original so the user's
        // folder looks exactly as it did before.
        if std::fs::remove_file(final_path).is_ok() && std::fs::rename(&old, original).is_ok() {
            return Err(e);
        }
        return Err(FsError::new(format!(
            "Couldn't move the original to the Recycle Bin. It is saved as \"{}\".",
            old.display()
        )));
    }
    Ok(())
}

/// Output has a different name (e.g. new extension): move the new file in, then
/// recycle the original.
fn replace_with_new_name(
    temp: &Path,
    original: &Path,
    final_path: &Path,
    recycle: &dyn Fn(&Path) -> Result<(), FsError>,
) -> Result<(), FsError> {
    // `rename` silently overwrites on Windows; never let it eat someone's file.
    if final_path.exists() {
        return Err(FsError::new(
            "Couldn't save the new file because a file with that name already exists.",
        ));
    }
    // Find out now, before anything moves, whether the original can be deleted.
    ensure_deletable(original)?;

    std::fs::rename(temp, final_path).map_err(FsError::from_io)?;

    if let Err(e) = recycle(original) {
        if !original.exists() {
            // The original is already gone, so the replacement is complete.
            return Ok(());
        }
        // Original is untouched; remove the new file so nothing is half-done.
        let _ = std::fs::remove_file(final_path);
        return Err(e);
    }
    Ok(())
}

/// Probe with the same access the Recycle Bin needs (DELETE, sharing everything
/// else): fails with a sharing violation if another program holds the file
/// without allowing deletion.
fn ensure_deletable(path: &Path) -> Result<(), FsError> {
    use std::os::windows::fs::OpenOptionsExt;
    const DELETE: u32 = 0x0001_0000;
    const SHARE_ALL: u32 = 0x1 | 0x2 | 0x4; // READ | WRITE | DELETE
    std::fs::OpenOptions::new()
        .access_mode(DELETE)
        .share_mode(SHARE_ALL)
        .open(path)
        .map(|_| ())
        .map_err(FsError::from_io)
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

const RECYCLE_FAILED: &str = "Couldn't move the original to the Recycle Bin.";

/// Send a file to the Recycle Bin without showing any Windows dialog.
///
/// Calls `IFileOperation` directly with `FOF_NO_UI | FOF_ALLOWUNDO |
/// FOFX_RECYCLEONDELETE` and deliberately without `FOF_WANTNUKEWARNING` (which
/// `trash` hard-codes and which pops a dialog when a file cannot be recycled).
/// When recycling is impossible (file too big for the bin, drive without a
/// bin) Windows deletes permanently and silently, as the user chose.
pub fn recycle(path: &Path) -> Result<(), FsError> {
    recycle_impl(path).map_err(|e| FsError::with_source(RECYCLE_FAILED, e))
}

fn recycle_impl(path: &Path) -> windows::core::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{Error, PCWSTR};
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, RPC_E_CHANGED_MODE};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{
        FileOperation, IFileOperation, IShellItem, SHCreateItemFromParsingName, FOFX_RECYCLEONDELETE,
        FOF_ALLOWUNDO, FOF_NO_UI,
    };

    if !path.exists() {
        return Err(Error::from(ERROR_FILE_NOT_FOUND));
    }
    let absolute = std::path::absolute(path).map_err(|e| Error::new(
        windows::core::HRESULT::from_win32(e.raw_os_error().unwrap_or(0) as u32),
        "could not resolve path",
    ))?;
    let wide: Vec<u16> = absolute.as_os_str().encode_wide().chain(std::iter::once(0)).collect();

    // Initialise COM for this call; undo it only if we are the ones who did.
    struct ComGuard(bool);
    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let _com = if hr == RPC_E_CHANGED_MODE {
        // Thread already runs COM in another mode; usable as is, nothing to undo.
        ComGuard(false)
    } else {
        hr.ok()?;
        ComGuard(true)
    };

    unsafe {
        let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)?;
        op.SetOperationFlags(FOF_NO_UI | FOF_ALLOWUNDO | FOFX_RECYCLEONDELETE)?;
        let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None)?;
        op.DeleteItem(&item, None)?;
        op.PerformOperations()?;
        if op.GetAnyOperationsAborted()?.as_bool() {
            return Err(Error::new(
                windows::Win32::Foundation::E_ABORT,
                "the operation was aborted",
            ));
        }
    }
    Ok(())
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
        assert!(!sibling_with_suffix(&original, ".hasami-old2").exists());
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

        let err = commit_replace(&temp, &original, &final_path).unwrap_err();
        assert!(err.message.contains("another program"), "{}", err.message);

        drop(lock);
        assert_eq!(fs::read(&original).unwrap(), b"precious");
        assert!(!final_path.exists());
        assert!(!temp.exists());
    }

    fn failing_recycle(_: &Path) -> Result<(), FsError> {
        Err(FsError::new(RECYCLE_FAILED))
    }

    /// Recycler that deletes the file and then still reports failure.
    fn delete_then_fail(p: &Path) -> Result<(), FsError> {
        fs::remove_file(p).unwrap();
        Err(FsError::new(RECYCLE_FAILED))
    }

    #[test]
    fn in_place_restores_original_when_second_rename_fails() {
        let dir = test_dir();
        let original = dir.path().join("hasami-test-r1.mp4");
        let temp = temp_path_for(&original); // never created, so the rename fails
        fs::write(&original, b"precious").unwrap();

        assert!(commit_replace(&temp, &original, &original).is_err());

        assert_eq!(fs::read(&original).unwrap(), b"precious");
        assert!(!sibling_with_suffix(&original, ".hasami-old").exists());
    }

    #[test]
    fn in_place_rolls_back_when_recycle_fails() {
        let dir = test_dir();
        let original = dir.path().join("hasami-test-r2.mp4");
        let temp = temp_path_for(&original);
        fs::write(&original, b"precious").unwrap();
        fs::write(&temp, b"new").unwrap();

        let err = commit_replace_with(&temp, &original, &original, &failing_recycle).unwrap_err();
        assert!(err.message.contains("Recycle Bin"), "{}", err.message);

        assert_eq!(fs::read(&original).unwrap(), b"precious");
        assert!(!temp.exists());
        assert!(!sibling_with_suffix(&original, ".hasami-old").exists());
    }

    #[test]
    fn in_place_counts_as_done_if_original_already_gone() {
        let dir = test_dir();
        let original = dir.path().join("hasami-test-r3.mp4");
        let temp = temp_path_for(&original);
        fs::write(&original, b"old").unwrap();
        fs::write(&temp, b"new").unwrap();

        commit_replace_with(&temp, &original, &original, &delete_then_fail).unwrap();

        assert_eq!(fs::read(&original).unwrap(), b"new");
    }

    #[test]
    fn new_name_rolls_back_when_recycle_fails() {
        let dir = test_dir();
        let original = dir.path().join("hasami-test-r4.mkv");
        let final_path = with_extension(&original, "mp4");
        let temp = temp_path_for(&final_path);
        fs::write(&original, b"precious").unwrap();
        fs::write(&temp, b"new").unwrap();

        assert!(commit_replace_with(&temp, &original, &final_path, &failing_recycle).is_err());

        assert_eq!(fs::read(&original).unwrap(), b"precious");
        assert!(!final_path.exists());
        assert!(!temp.exists());
    }

    #[test]
    fn new_name_counts_as_done_if_original_already_gone() {
        let dir = test_dir();
        let original = dir.path().join("hasami-test-r5.mkv");
        let final_path = with_extension(&original, "mp4");
        let temp = temp_path_for(&final_path);
        fs::write(&original, b"old").unwrap();
        fs::write(&temp, b"new").unwrap();

        commit_replace_with(&temp, &original, &final_path, &delete_then_fail).unwrap();

        assert_eq!(fs::read(&final_path).unwrap(), b"new");
        assert!(!original.exists());
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
