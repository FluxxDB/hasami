//! Sending a file to the Recycle Bin silently, and telling when another program holds it.

use std::io;
use std::path::Path;
use std::time::Duration;

use super::FsError;

/// Sends a file to the Recycle Bin.
pub(super) type RecycleFn<'a> = &'a dyn Fn(&Path) -> Result<(), FsError>;

pub(super) fn open_for_delete(path: &Path) -> io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    const DELETE: u32 = 0x0001_0000;
    const SHARE_ALL: u32 = 0x1 | 0x2 | 0x4; // READ | WRITE | DELETE
    std::fs::OpenOptions::new().access_mode(DELETE).share_mode(SHARE_ALL).open(path).map(|_| ())
}

/// True when another program has `path` open without allowing it to be moved or deleted.
fn in_use(path: &Path) -> bool {
    open_for_delete(path).is_err_and(|e| matches!(e.raw_os_error(), Some(32) | Some(33)))
}

/// How many times [`recycle_when_free`] tries while the file is in use, and the pause between.
const RECYCLE_ATTEMPTS: u32 = 5;
const RECYCLE_RETRY_DELAY: Duration = Duration::from_millis(200);

/// [`recycle_when_free`]'s message when another program kept the file open throughout.
pub const RECYCLE_IN_USE: &str = "Couldn't move the file to the Recycle Bin because another program is using it.";

/// [`recycle`], tried again for about a second while another program has the file open (a player
/// or a thumbnail maker often lets go in a moment). If it's still in use then, the error says so
/// ([`RECYCLE_IN_USE`]).
pub fn recycle_when_free(path: &Path) -> Result<(), FsError> {
    recycle_when_free_with(path, &recycle, &in_use, RECYCLE_RETRY_DELAY)
}

fn recycle_when_free_with(
    path: &Path,
    recycle: RecycleFn,
    in_use: &dyn Fn(&Path) -> bool,
    delay: Duration,
) -> Result<(), FsError> {
    let mut attempt = 1;
    loop {
        let err = match recycle(path) {
            Ok(()) => return Ok(()),
            Err(e) => e,
        };
        if !in_use(path) {
            return Err(err);
        }
        if attempt == RECYCLE_ATTEMPTS {
            return Err(FsError { message: RECYCLE_IN_USE.to_owned(), ..err });
        }
        attempt += 1;
        std::thread::sleep(delay);
    }
}

pub(super) const RECYCLE_FAILED: &str = "Couldn't move the original to the Recycle Bin.";

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
        FileOperation, IFileOperation, IShellItem, SHCreateItemFromParsingName, FOFX_RECYCLEONDELETE, FOF_ALLOWUNDO,
        FOF_NO_UI,
    };

    if !path.exists() {
        return Err(Error::from(ERROR_FILE_NOT_FOUND));
    }
    let absolute = std::path::absolute(path).map_err(|e| {
        Error::new(windows::core::HRESULT::from_win32(e.raw_os_error().unwrap_or(0) as u32), "could not resolve path")
    })?;
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
            return Err(Error::new(windows::Win32::Foundation::E_ABORT, "the operation was aborted"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::fsops::test_support::test_dir;

    /// A recycler that fails its first `fails` calls, counting every call.
    fn recycler(fails: u32, calls: &std::cell::Cell<u32>) -> impl Fn(&Path) -> Result<(), FsError> + '_ {
        move |_| {
            calls.set(calls.get() + 1);
            if calls.get() <= fails {
                Err(FsError::new(RECYCLE_FAILED))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn recycling_tries_again_while_another_program_has_the_file() {
        let calls = std::cell::Cell::new(0);
        let result = recycle_when_free_with(Path::new("a.mp4"), &recycler(2, &calls), &|_| true, Duration::ZERO);
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn recycling_says_when_the_file_stays_in_use() {
        let calls = std::cell::Cell::new(0);
        let err = recycle_when_free_with(Path::new("a.mp4"), &recycler(u32::MAX, &calls), &|_| true, Duration::ZERO)
            .unwrap_err();
        assert_eq!(calls.get(), RECYCLE_ATTEMPTS);
        assert_eq!(err.message, RECYCLE_IN_USE);
        assert!(err.original_intact);
    }

    #[test]
    fn recycling_fails_at_once_when_nothing_else_has_the_file() {
        let calls = std::cell::Cell::new(0);
        let err = recycle_when_free_with(Path::new("a.mp4"), &recycler(u32::MAX, &calls), &|_| false, Duration::ZERO)
            .unwrap_err();
        assert_eq!(calls.get(), 1);
        assert_eq!(err.message, RECYCLE_FAILED);
    }

    #[cfg(windows)]
    #[test]
    fn a_file_another_program_holds_is_in_use_and_stays() {
        use std::os::windows::fs::OpenOptionsExt;

        let dir = test_dir();
        let clip = dir.path().join("hasami-test-held.mp4");
        fs::write(&clip, b"precious").unwrap();
        assert!(!in_use(&clip));
        // How ffmpeg and most players open a file: reads and writes shared, deletion not.
        let held = fs::OpenOptions::new().read(true).share_mode(0x1 | 0x2).open(&clip).unwrap();
        assert!(in_use(&clip));

        let err = recycle_when_free(&clip).unwrap_err();
        assert_eq!(err.message, RECYCLE_IN_USE);
        drop(held);
        assert_eq!(fs::read(&clip).unwrap(), b"precious");
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
