//! Giving a finished, verified temp file its final name: as a new file ([`commit_new`]), in place
//! of the original ([`commit_replace`]), and moving the original into `Raw` ([`move_to_raw`]).

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::naming::{in_raw_folder, same_path, unique_path, RAW_FOLDER};
use super::presence::{presence, verbatim_wide, win32_io_error, Presence};
use super::recycle::{open_for_delete, recycle, RecycleFn};
use super::FsError;

/// Give the finished `temp` file its name, `final_path`, without ever replacing
/// anything: if the name is taken the commit fails. No original is touched.
///
/// On a definite failure the temp file is deleted. When the rename may have
/// happened anyway (the temp can no longer be seen where it was) it is kept and
/// the error says so (`kept_new_file`).
pub fn commit_new(temp: &Path, final_path: &Path) -> Result<(), FsError> {
    commit_new_with(temp, final_path, &rename_no_replace, &presence)
}

/// `commit_new` with the rename and presence check injectable, so the failure
/// paths can be tested deterministically.
fn commit_new_with(temp: &Path, final_path: &Path, rename: RenameFn, presence: PresenceFn) -> Result<(), FsError> {
    if !temp.is_file() {
        return Err(FsError::new("Couldn't save the new file because it could no longer be found."));
    }
    let Err(e) = rename_with_retry(rename, temp, final_path) else {
        return Ok(());
    };
    // Only a definite "still there" lets the temp file be deleted; otherwise the
    // rename may have gone through and the file must be kept.
    if presence(temp) == Presence::Present {
        let _ = std::fs::remove_file(temp);
        return Err(FsError::from_io_saving(e));
    }
    Err(FsError::rename_unsure(temp, final_path, e))
}

/// Move `original` into a `Raw` folder next to it (created if missing), under a
/// free name, without replacing anything. `None` means it already sits in a
/// `Raw` folder and was left where it is; otherwise the new path.
pub fn move_to_raw(original: &Path) -> Result<Option<PathBuf>, FsError> {
    move_to_raw_with(original, &rename_no_replace, &presence)
}

/// `move_to_raw` with the rename and presence check injectable. A rename that reports failure
/// although the original is gone from its path and something is at the target counts as done.
fn move_to_raw_with(original: &Path, rename: RenameFn, presence: PresenceFn) -> Result<Option<PathBuf>, FsError> {
    if in_raw_folder(original) {
        return Ok(None);
    }
    let raw = original.parent().unwrap_or_else(|| Path::new("")).join(RAW_FOLDER);
    std::fs::create_dir_all(&raw).map_err(|e| FsError::with_source("Couldn't create the Raw folder.", e))?;
    let target = unique_path(&raw.join(original.file_name().unwrap_or_default()), None);
    match rename_with_retry(rename, original, &target) {
        Ok(()) => Ok(Some(target)),
        Err(_) if presence(original) == Presence::Absent && presence(&target) == Presence::Present => Ok(Some(target)),
        Err(e) => Err(FsError::with_source("Couldn't move the original into the Raw folder.", e)),
    }
}

/// Replace `original` with the finished `temp` file, saved as `final_path`.
///
/// On any error the original is back at its own path with its own bytes and the
/// temp file is deleted, except in one case: the original was already recycled
/// and the new file then couldn't take its name. Then the temp file is kept and
/// the error says where it is (`kept_new_file`, `original_intact == false`).
pub fn commit_replace(temp: &Path, original: &Path, final_path: &Path) -> Result<(), FsError> {
    commit_replace_with(temp, original, final_path, &recycle, &rename_no_replace, &presence)
}

/// Renames a file, failing (never overwriting) if the destination exists.
type RenameFn<'a> = &'a dyn Fn(&Path, &Path) -> io::Result<()>;
/// Looks at whether something is at a path.
type PresenceFn<'a> = &'a dyn Fn(&Path) -> Presence;

/// `commit_replace` with the recycle, rename and presence checks injectable, so
/// every failure path can be tested deterministically.
fn commit_replace_with(
    temp: &Path,
    original: &Path,
    final_path: &Path,
    recycle: RecycleFn,
    rename: RenameFn,
    presence: PresenceFn,
) -> Result<(), FsError> {
    let result = if !temp.is_file() {
        // Never touch the original unless there is a finished file to replace it with.
        Err(FsError::new("Couldn't save the new file because it could no longer be found."))
    } else if same_path(final_path, original) {
        replace_in_place(temp, original, final_path, recycle, rename, presence)
    } else {
        replace_with_new_name(temp, original, final_path, recycle, rename, presence)
    };
    if let Err(e) = &result {
        if e.kept_new_file.is_none() {
            // Best effort; if the temp already moved away this is a harmless no-op.
            let _ = std::fs::remove_file(temp);
        }
    }
    result
}

/// Output has the same path as the original: recycle the original under its
/// real name (so Restore brings back the right file), then move the new file
/// in with a rename that can never overwrite anything.
fn replace_in_place(
    temp: &Path,
    original: &Path,
    final_path: &Path,
    recycle: RecycleFn,
    rename: RenameFn,
    presence: PresenceFn,
) -> Result<(), FsError> {
    // Find out now, before anything moves, whether the original can be deleted.
    ensure_deletable(original)?;

    // Only a definite "it's there" lets the new file be deleted. A path that
    // can't be looked at may be an original that is already on its way out
    // (delete-pending while another program holds it); the non-replacing
    // rename below can't harm it, so carry on and keep the new file.
    let recycled = recycle(original);
    if let Err(e) = recycled {
        if presence(original) == Presence::Present {
            // Nothing moved: the original is untouched.
            return Err(e);
        }
        // The original is gone (or going) anyway; finish the replacement,
        // without claiming to know it is in the Recycle Bin.
        return rename_with_retry(rename, temp, final_path).map_err(|e| FsError::stranded_unsure(temp, Some(e)));
    }
    if presence(original) == Presence::Present {
        // Recycling reported success, yet something is still at the path. A
        // rename would fail at best. The original has probably gone to the
        // Recycle Bin, so keep the new file too and stop.
        return Err(FsError::stranded_unsure(temp, None));
    }

    // From here the original is in the Recycle Bin, so the new file must never
    // be deleted: if it can't take its name, keep it where it is and say so.
    rename_with_retry(rename, temp, final_path).map_err(|e| FsError::stranded(temp, e))
}

/// Output has a different name (e.g. new extension): move the new file in, then
/// recycle the original.
fn replace_with_new_name(
    temp: &Path,
    original: &Path,
    final_path: &Path,
    recycle: RecycleFn,
    rename: RenameFn,
    presence: PresenceFn,
) -> Result<(), FsError> {
    // Find out now, before anything moves, whether the original can be deleted.
    ensure_deletable(original)?;

    // Non-replacing: if another file took the final name meanwhile, this fails
    // instead of overwriting it.
    rename_with_retry(rename, temp, final_path).map_err(FsError::from_io)?;

    if let Err(e) = recycle(original) {
        if presence(original) != Presence::Present {
            // The original is gone, or can't be looked at (it may be
            // delete-pending): keep the new file, the replacement is complete.
            return Ok(());
        }
        // Original is definitely untouched; remove the new file so nothing is half-done.
        let _ = std::fs::remove_file(final_path);
        return Err(e);
    }
    Ok(())
}

/// Attempts at the final rename, and the pause between them. Antivirus scanners
/// and Explorer's thumbnailer briefly open fresh files; a moment later they let go.
const RENAME_ATTEMPTS: u32 = 5;
const RENAME_RETRY_DELAY: Duration = Duration::from_millis(200);

/// `rename`, retried a few times while the file is briefly held by someone else.
fn rename_with_retry(rename: RenameFn, from: &Path, to: &Path) -> io::Result<()> {
    let mut attempt = 1;
    loop {
        match rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if attempt < RENAME_ATTEMPTS && is_transient(&e) => {
                attempt += 1;
                std::thread::sleep(RENAME_RETRY_DELAY);
            }
            Err(e) => return Err(e),
        }
    }
}

/// Sharing, lock or access errors: what another program holding the file looks like.
fn is_transient(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(5) | Some(32) | Some(33))
}

/// Rename `from` to `to` with `MoveFileExW` and no flags, which fails if `to`
/// exists. (`std::fs::rename` replaces existing files on Windows.)
fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVE_FILE_FLAGS};

    let (from, to) = (verbatim_wide(from)?, verbatim_wide(to)?);
    unsafe { MoveFileExW(PCWSTR(from.as_ptr()), PCWSTR(to.as_ptr()), MOVE_FILE_FLAGS(0)) }.map_err(win32_io_error)
}

/// Probe with the same access the Recycle Bin needs (DELETE, sharing everything
/// else): fails with a sharing violation if another program holds the file
/// without allowing deletion.
fn ensure_deletable(path: &Path) -> Result<(), FsError> {
    open_for_delete(path).map_err(FsError::from_io)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::fsops::naming::{temp_path_for, with_extension};
    use crate::fsops::recycle::RECYCLE_FAILED;
    use crate::fsops::test_support::{make_delete_pending, names, test_dir, unreadable};

    #[test]
    fn commit_replace_same_name() {
        let dir = test_dir();
        let original = dir.path().join("hasami-test-same.mp4");
        let temp = temp_path_for(&original);
        fs::write(&original, b"old").unwrap();
        fs::write(&temp, b"new").unwrap();

        commit_replace(&temp, &original, &original).unwrap();

        assert_eq!(fs::read(&original).unwrap(), b"new");
        assert_eq!(names(dir.path()), vec!["hasami-test-same.mp4"], "no temp or parked file left");
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

        let err = commit_replace(&temp, &original, &final_path).unwrap_err();
        assert!(err.message.contains("already exists"), "{}", err.message);
        assert!(err.original_intact);

        assert_eq!(fs::read(&original).unwrap(), b"old");
        assert_eq!(fs::read(&final_path).unwrap(), b"someone else's");
        assert!(!temp.exists());
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

        let _lock = fs::OpenOptions::new().read(true).share_mode(0).open(&original).unwrap();

        let err = commit_replace(&temp, &original, &original).unwrap_err();
        assert!(err.message.contains("another program"), "{}", err.message);
        assert!(err.original_intact);

        drop(_lock);
        assert_eq!(fs::read(&original).unwrap(), b"precious");
        assert_eq!(names(dir.path()), vec!["hasami-test-locked.mp4"]);
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

        let lock = fs::OpenOptions::new().read(true).share_mode(0).open(&original).unwrap();

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

    /// Recycler that deletes the file (standing in for a real move to the Bin).
    fn delete_ok(p: &Path) -> Result<(), FsError> {
        fs::remove_file(p).unwrap();
        Ok(())
    }

    fn never_rename(_: &Path, _: &Path) -> io::Result<()> {
        panic!("rename must not be called");
    }

    #[test]
    fn in_place_recycles_the_original_under_its_real_name() {
        let dir = test_dir();
        let original = dir.path().join("hasami-test-real-name.mp4");
        let temp = temp_path_for(&original);
        fs::write(&original, b"old").unwrap();
        fs::write(&temp, b"new").unwrap();

        let recycled = std::cell::RefCell::new(Vec::new());
        let record = |p: &Path| {
            recycled.borrow_mut().push((p.to_path_buf(), fs::read(p).unwrap()));
            delete_ok(p)
        };
        commit_replace_with(&temp, &original, &original, &record, &rename_no_replace, &presence).unwrap();

        assert_eq!(*recycled.borrow(), vec![(original.clone(), b"old".to_vec())]);
        assert_eq!(fs::read(&original).unwrap(), b"new");
        assert_eq!(names(dir.path()), vec!["hasami-test-real-name.mp4"]);
    }

    #[test]
    fn in_place_never_touches_the_original_without_a_new_file() {
        let dir = test_dir();
        let original = dir.path().join("hasami-test-r1.mp4");
        let temp = temp_path_for(&original); // never created
        fs::write(&original, b"precious").unwrap();

        let recycle_called = |_: &Path| -> Result<(), FsError> { panic!("must not recycle") };
        let err =
            commit_replace_with(&temp, &original, &original, &recycle_called, &never_rename, &presence).unwrap_err();
        assert!(err.original_intact);

        assert_eq!(fs::read(&original).unwrap(), b"precious");
        assert_eq!(names(dir.path()), vec!["hasami-test-r1.mp4"]);
    }

    #[test]
    fn in_place_rolls_back_when_recycle_fails() {
        let dir = test_dir();
        let original = dir.path().join("hasami-test-r2.mp4");
        let temp = temp_path_for(&original);
        fs::write(&original, b"precious").unwrap();
        fs::write(&temp, b"new").unwrap();

        let err =
            commit_replace_with(&temp, &original, &original, &failing_recycle, &never_rename, &presence).unwrap_err();
        assert!(err.message.contains("Recycle Bin"), "{}", err.message);
        assert!(err.original_intact);

        assert_eq!(fs::read(&original).unwrap(), b"precious");
        assert_eq!(names(dir.path()), vec!["hasami-test-r2.mp4"], "temp deleted");
    }

    #[test]
    fn in_place_counts_as_done_if_original_already_gone() {
        let dir = test_dir();
        let original = dir.path().join("hasami-test-r3.mp4");
        let temp = temp_path_for(&original);
        fs::write(&original, b"old").unwrap();
        fs::write(&temp, b"new").unwrap();

        commit_replace_with(&temp, &original, &original, &delete_then_fail, &rename_no_replace, &presence).unwrap();

        assert_eq!(fs::read(&original).unwrap(), b"new");
        assert!(!temp.exists());
    }

    #[test]
    fn in_place_stops_if_the_original_survives_recycling() {
        let dir = test_dir();
        let original = dir.path().join("hasami-test-survivor.mp4");
        let temp = temp_path_for(&original);
        fs::write(&original, b"precious").unwrap();
        fs::write(&temp, b"new").unwrap();

        // Reports success but leaves the file: a rename now could overwrite it.
        let pretend = |_: &Path| -> Result<(), FsError> { Ok(()) };
        let err = commit_replace_with(&temp, &original, &original, &pretend, &never_rename, &presence).unwrap_err();
        // Recycling claimed success, so the original may well be in the Bin: keep the new file.
        assert!(!err.original_intact);
        assert_eq!(err.kept_new_file.as_deref(), Some(temp.as_path()));
        assert!(err.message.contains("may be in the Recycle Bin"), "{}", err.message);

        assert_eq!(fs::read(&original).unwrap(), b"precious");
        assert_eq!(fs::read(&temp).unwrap(), b"new");
    }

    #[test]
    fn in_place_retries_a_rename_while_the_file_is_busy() {
        let dir = test_dir();
        let original = dir.path().join("hasami-test-busy.mp4");
        let temp = temp_path_for(&original);
        fs::write(&original, b"old").unwrap();
        fs::write(&temp, b"new").unwrap();

        let calls = std::cell::Cell::new(0);
        let busy_twice = |from: &Path, to: &Path| {
            calls.set(calls.get() + 1);
            if calls.get() <= 2 {
                Err(io::Error::from_raw_os_error(32)) // ERROR_SHARING_VIOLATION
            } else {
                rename_no_replace(from, to)
            }
        };
        commit_replace_with(&temp, &original, &original, &delete_ok, &busy_twice, &presence).unwrap();

        assert_eq!(calls.get(), 3);
        assert_eq!(fs::read(&original).unwrap(), b"new");
        assert_eq!(names(dir.path()), vec!["hasami-test-busy.mp4"]);
    }

    #[test]
    fn in_place_keeps_the_new_file_when_it_cant_take_its_name() {
        let dir = test_dir();
        let original = dir.path().join("hasami-test-stranded.mp4");
        let temp = temp_path_for(&original);
        fs::write(&original, b"old").unwrap();
        fs::write(&temp, b"new").unwrap();

        let calls = std::cell::Cell::new(0);
        let always_denied = |_: &Path, _: &Path| {
            calls.set(calls.get() + 1);
            Err(io::Error::from_raw_os_error(5)) // ERROR_ACCESS_DENIED
        };
        let err = commit_replace_with(&temp, &original, &original, &delete_ok, &always_denied, &presence).unwrap_err();

        assert_eq!(calls.get(), RENAME_ATTEMPTS, "retried before giving up");
        assert!(!err.original_intact);
        assert_eq!(err.kept_new_file.as_deref(), Some(temp.as_path()));
        assert_eq!(
            err.message,
            format!(
                "Couldn't finish saving the new file. It's saved as \"{}\", and the original is in the Recycle Bin.",
                temp.display()
            )
        );
        // The only copy of the new file must survive the error path.
        assert_eq!(fs::read(&temp).unwrap(), b"new");
        assert!(!original.exists());
    }

    #[test]
    fn new_name_rolls_back_when_recycle_fails() {
        let dir = test_dir();
        let original = dir.path().join("hasami-test-r4.mkv");
        let final_path = with_extension(&original, "mp4");
        let temp = temp_path_for(&final_path);
        fs::write(&original, b"precious").unwrap();
        fs::write(&temp, b"new").unwrap();

        let err = commit_replace_with(&temp, &original, &final_path, &failing_recycle, &rename_no_replace, &presence)
            .unwrap_err();
        assert!(err.original_intact);

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

        commit_replace_with(&temp, &original, &final_path, &delete_then_fail, &rename_no_replace, &presence).unwrap();

        assert_eq!(fs::read(&final_path).unwrap(), b"new");
        assert!(!original.exists());
    }

    #[test]
    fn new_name_failed_rename_leaves_the_original_alone() {
        let dir = test_dir();
        let original = dir.path().join("hasami-test-r6.mkv");
        let final_path = with_extension(&original, "mp4");
        let temp = temp_path_for(&final_path);
        fs::write(&original, b"precious").unwrap();
        fs::write(&temp, b"new").unwrap();

        let recycle_called = |_: &Path| -> Result<(), FsError> { panic!("must not recycle") };
        let refuse = |_: &Path, _: &Path| Err(io::Error::from_raw_os_error(183)); // ERROR_ALREADY_EXISTS
        let err = commit_replace_with(&temp, &original, &final_path, &recycle_called, &refuse, &presence).unwrap_err();
        assert!(err.message.contains("already exists"), "{}", err.message);
        assert!(err.original_intact);
        assert!(err.kept_new_file.is_none());

        assert_eq!(fs::read(&original).unwrap(), b"precious");
        assert_eq!(names(dir.path()), vec!["hasami-test-r6.mkv"]);
    }

    #[test]
    fn in_place_keeps_the_new_file_when_the_original_is_unreadable_after_a_failed_recycle() {
        let dir = test_dir();
        let original = dir.path().join("hasami-test-u1.mp4");
        let temp = temp_path_for(&original);
        fs::write(&original, b"old").unwrap();
        fs::write(&temp, b"new").unwrap();

        // Delete-pending original: recycling fails, then the path can't be looked at.
        commit_replace_with(&temp, &original, &original, &delete_then_fail, &rename_no_replace, &unreadable).unwrap();
        assert_eq!(fs::read(&original).unwrap(), b"new");

        // If the original was in fact still there, the non-replacing rename can't
        // touch it, and the new file is kept rather than deleted.
        fs::write(&temp, b"new2").unwrap();
        let err = commit_replace_with(&temp, &original, &original, &failing_recycle, &rename_no_replace, &unreadable)
            .unwrap_err();
        assert_eq!(err.kept_new_file.as_deref(), Some(temp.as_path()));
        assert_eq!(fs::read(&temp).unwrap(), b"new2");
        assert_eq!(fs::read(&original).unwrap(), b"new", "never overwritten");
    }

    #[test]
    fn in_place_keeps_the_new_file_when_the_original_is_unreadable_after_recycling() {
        let dir = test_dir();
        let original = dir.path().join("hasami-test-u2.mp4");
        let temp = temp_path_for(&original);
        fs::write(&original, b"old").unwrap();
        fs::write(&temp, b"new").unwrap();

        commit_replace_with(&temp, &original, &original, &delete_ok, &rename_no_replace, &unreadable).unwrap();

        assert_eq!(fs::read(&original).unwrap(), b"new");
        assert_eq!(names(dir.path()), vec!["hasami-test-u2.mp4"]);
    }

    #[test]
    fn new_name_keeps_the_new_file_when_the_original_is_unreadable() {
        let dir = test_dir();
        let original = dir.path().join("hasami-test-u3.mkv");
        let final_path = with_extension(&original, "mp4");
        let temp = temp_path_for(&final_path);
        fs::write(&original, b"old").unwrap();
        fs::write(&temp, b"new").unwrap();

        commit_replace_with(&temp, &original, &final_path, &failing_recycle, &rename_no_replace, &unreadable).unwrap();

        assert_eq!(fs::read(&final_path).unwrap(), b"new");
        assert!(!temp.exists());
    }

    /// The re-review case end to end, with the real presence check and rename:
    /// recycling leaves the original delete-pending while another program
    /// holds it. Deterministic outcome: the holder stays open for the whole
    /// commit, so every rename attempt is refused and the new file is kept.
    #[cfg(windows)]
    #[test]
    fn in_place_keeps_the_new_file_when_the_original_is_left_delete_pending() {
        let dir = test_dir();
        let original = dir.path().join("hasami-test-pending2.mp4");
        let temp = temp_path_for(&original);
        fs::write(&original, b"old").unwrap();
        fs::write(&temp, b"new").unwrap();

        let holder = std::cell::RefCell::new(None);
        let pending_recycle = |p: &Path| -> Result<(), FsError> {
            *holder.borrow_mut() = Some(make_delete_pending(p));
            Ok(())
        };
        let err = commit_replace_with(&temp, &original, &original, &pending_recycle, &rename_no_replace, &presence)
            .unwrap_err();

        assert_eq!(err.kept_new_file.as_deref(), Some(temp.as_path()));
        assert!(!err.original_intact);
        assert_eq!(fs::read(&temp).unwrap(), b"new", "the new file survives");
        drop(holder.borrow_mut().take());
        assert!(!original.exists(), "the original was really deleted");
        assert_eq!(fs::read(&temp).unwrap(), b"new");
    }

    #[test]
    fn rename_no_replace_moves_but_never_overwrites() {
        let dir = test_dir();
        let a = dir.path().join("a [x] 'é'.bin");
        let b = dir.path().join("b.bin");
        fs::write(&a, b"a").unwrap();
        rename_no_replace(&a, &b).unwrap();
        assert_eq!(fs::read(&b).unwrap(), b"a");
        assert!(!a.exists());

        fs::write(&a, b"a2").unwrap();
        let err = rename_no_replace(&a, &b).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists, "{err:?}");
        assert_eq!(fs::read(&b).unwrap(), b"a");
        assert_eq!(fs::read(&a).unwrap(), b"a2");
    }

    #[test]
    fn commit_new_moves_and_never_overwrites() {
        let dir = test_dir();
        let final_path = dir.path().join("[CLIPPED] a.mp4");
        let temp = temp_path_for(&final_path);
        fs::write(&temp, b"new").unwrap();
        commit_new(&temp, &final_path).unwrap();
        assert_eq!(fs::read(&final_path).unwrap(), b"new");
        assert!(!temp.exists());

        // The name is taken now: refuse, keep the existing bytes, clean up the temp.
        fs::write(&temp, b"newer").unwrap();
        let err = commit_new(&temp, &final_path).unwrap_err();
        assert!(err.original_intact);
        assert!(err.kept_new_file.is_none());
        assert_eq!(fs::read(&final_path).unwrap(), b"new");
        assert!(!temp.exists(), "the temp file is deleted");
    }

    #[test]
    fn a_failed_commit_new_speaks_of_saving_not_replacing() {
        let dir = test_dir();
        let final_path = dir.path().join("x.mp4");
        let temp = temp_path_for(&final_path);
        fs::write(&temp, b"new").unwrap();
        let busy = |_: &Path, _: &Path| Err(io::Error::from_raw_os_error(32)); // ERROR_SHARING_VIOLATION
        let err = commit_new_with(&temp, &final_path, &busy, &presence).unwrap_err();
        assert_eq!(err.message, "Couldn't save the new file because another program is using it.");
        assert!(!temp.exists());

        fs::write(&temp, b"new").unwrap();
        let odd = |_: &Path, _: &Path| Err(io::Error::other("odd"));
        let err = commit_new_with(&temp, &final_path, &odd, &presence).unwrap_err();
        assert_eq!(err.message, "Couldn't save the new file.");
    }

    #[test]
    fn commit_new_with_a_missing_temp_is_an_error() {
        let dir = test_dir();
        let err = commit_new(&dir.path().join(".x.hasami-tmp"), &dir.path().join("x.mp4")).unwrap_err();
        assert!(err.kept_new_file.is_none());
        assert!(!dir.path().join("x.mp4").exists());
    }

    #[test]
    fn commit_new_retries_a_rename_while_the_file_is_busy() {
        let dir = test_dir();
        let final_path = dir.path().join("x.mp4");
        let temp = temp_path_for(&final_path);
        fs::write(&temp, b"new").unwrap();
        let calls = std::cell::Cell::new(0);
        let flaky = |from: &Path, to: &Path| {
            calls.set(calls.get() + 1);
            if calls.get() < 3 {
                Err(io::Error::from_raw_os_error(32))
            } else {
                rename_no_replace(from, to)
            }
        };
        commit_new_with(&temp, &final_path, &flaky, &presence).unwrap();
        assert_eq!(calls.get(), 3);
        assert_eq!(fs::read(&final_path).unwrap(), b"new");
    }

    #[test]
    fn commit_new_keeps_the_temp_when_the_rename_may_have_happened() {
        let dir = test_dir();
        let final_path = dir.path().join("x.mp4");
        let temp = temp_path_for(&final_path);
        fs::write(&temp, b"new").unwrap();
        // The rename reports failure, yet the temp is no longer where it was.
        let moved_then_failed = |from: &Path, to: &Path| {
            rename_no_replace(from, to)?;
            Err(io::Error::other("lost the reply"))
        };
        let err = commit_new_with(&temp, &final_path, &moved_then_failed, &presence).unwrap_err();
        assert!(err.original_intact, "commit_new never touches an original");
        assert_eq!(err.kept_new_file.as_deref(), Some(temp.as_path()));
        assert_eq!(fs::read(&final_path).unwrap(), b"new", "nothing was deleted");
    }

    #[test]
    fn commit_new_keeps_the_temp_when_it_cant_be_looked_at() {
        let dir = test_dir();
        let final_path = dir.path().join("x.mp4");
        let temp = temp_path_for(&final_path);
        fs::write(&temp, b"new").unwrap();
        let denied = |_: &Path, _: &Path| Err(io::Error::from_raw_os_error(5));
        let err = commit_new_with(&temp, &final_path, &denied, &unreadable).unwrap_err();
        assert_eq!(err.kept_new_file.as_deref(), Some(temp.as_path()));
        assert_eq!(fs::read(&temp).unwrap(), b"new");
    }

    #[test]
    fn commit_new_deletes_the_temp_on_a_definite_failure() {
        let dir = test_dir();
        let final_path = dir.path().join("x.mp4");
        let temp = temp_path_for(&final_path);
        fs::write(&temp, b"new").unwrap();
        let refused = |_: &Path, _: &Path| Err(io::Error::other("refused"));
        let err = commit_new_with(&temp, &final_path, &refused, &presence).unwrap_err();
        assert!(err.kept_new_file.is_none());
        assert!(!temp.exists());
        assert!(!final_path.exists());
    }

    #[test]
    fn move_to_raw_creates_folder_and_numbers() {
        let dir = test_dir();
        let a = dir.path().join("a.mp4");
        fs::write(&a, b"1").unwrap();
        let moved = move_to_raw(&a).unwrap();
        assert_eq!(moved, Some(dir.path().join("Raw").join("a.mp4")));
        assert!(!a.exists());
        assert_eq!(fs::read(dir.path().join("Raw").join("a.mp4")).unwrap(), b"1");

        fs::write(&a, b"2").unwrap();
        let moved = move_to_raw(&a).unwrap();
        assert_eq!(moved, Some(dir.path().join("Raw").join("a (2).mp4")));
        assert_eq!(fs::read(dir.path().join("Raw").join("a.mp4")).unwrap(), b"1");
        assert_eq!(fs::read(dir.path().join("Raw").join("a (2).mp4")).unwrap(), b"2");
    }

    #[test]
    fn move_to_raw_leaves_files_already_in_raw() {
        let dir = test_dir();
        let raw = dir.path().join("Raw");
        fs::create_dir(&raw).unwrap();
        let a = raw.join("a.mp4");
        fs::write(&a, b"1").unwrap();
        assert_eq!(move_to_raw(&a).unwrap(), None);
        assert_eq!(fs::read(&a).unwrap(), b"1");
        assert_eq!(names(&raw), vec!["a.mp4"]);
    }

    #[test]
    fn move_to_raw_counts_a_move_that_reported_failure_but_happened() {
        let dir = test_dir();
        let a = dir.path().join("a.mp4");
        fs::write(&a, b"1").unwrap();
        let moved_then_failed = |from: &Path, to: &Path| {
            rename_no_replace(from, to)?;
            Err(io::Error::other("lost the reply"))
        };
        let moved = move_to_raw_with(&a, &moved_then_failed, &presence).unwrap();
        assert_eq!(moved, Some(dir.path().join("Raw").join("a.mp4")));
        assert_eq!(fs::read(dir.path().join("Raw").join("a.mp4")).unwrap(), b"1");

        // A move that really failed is still an error, and the original stays.
        fs::write(&a, b"2").unwrap();
        let refused = |_: &Path, _: &Path| Err(io::Error::other("refused"));
        assert!(move_to_raw_with(&a, &refused, &presence).is_err());
        assert_eq!(fs::read(&a).unwrap(), b"2");
    }

    #[test]
    fn move_to_raw_fails_cleanly_when_the_file_is_missing() {
        let dir = test_dir();
        let err = move_to_raw(&dir.path().join("gone.mp4")).unwrap_err();
        assert!(err.original_intact);
        assert!(err.kept_new_file.is_none());
    }
}
