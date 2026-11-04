//! Looking at a path, and turning paths and Win32 errors into the forms the Windows API and
//! `std::io` use.

use std::io;
use std::path::Path;

/// What a look at a path found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Presence {
    /// Something is definitely there.
    Present,
    /// Definitely nothing there.
    Absent,
    /// The path couldn't be looked at (e.g. access denied on a delete-pending
    /// file). Never treated as "present": that would let a caller delete the
    /// new file while the original disappears.
    Unknown,
}

/// Look at `path` by opening it with no access rights (sharing everything, not
/// following links). `std::fs::symlink_metadata` can't be used: on a sharing or
/// access error it falls back to a directory listing, which still shows a
/// delete-pending file (FAT/exFAT/SMB, or any legacy delete) as present.
pub(super) fn presence(path: &Path) -> Presence {
    use std::ffi::c_void;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FileStandardInfo, GetFileInformationByHandleEx, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_STANDARD_INFO,
        OPEN_EXISTING,
    };

    let Ok(wide) = verbatim_wide(path) else {
        return Presence::Unknown;
    };
    let opened = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            None,
        )
    };
    let handle = match opened {
        Ok(handle) => handle,
        // 2 = ERROR_FILE_NOT_FOUND, 3 = ERROR_PATH_NOT_FOUND. Anything else,
        // including access denied on a delete-pending file, is unknown.
        Err(e) => {
            return match win32_io_error(e).raw_os_error() {
                Some(2) | Some(3) => Presence::Absent,
                _ => Presence::Unknown,
            }
        }
    };
    let mut info = FILE_STANDARD_INFO::default();
    let queried = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileStandardInfo,
            &mut info as *mut FILE_STANDARD_INFO as *mut c_void,
            std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
        )
    };
    let _ = unsafe { CloseHandle(handle) };
    match queried {
        Ok(()) if !info.DeletePending => Presence::Present,
        _ => Presence::Unknown,
    }
}

/// Win32 errors arrive as HRESULTs (0x8007xxxx); hand back the plain Windows
/// error code so `raw_os_error` matches what std::fs reports.
pub(super) fn win32_io_error(e: windows::core::Error) -> io::Error {
    let hr = e.code().0 as u32;
    if hr & 0xFFFF_0000 == 0x8007_0000 {
        io::Error::from_raw_os_error((hr & 0xFFFF) as i32)
    } else {
        io::Error::other(e)
    }
}

/// Absolute, `\\?\`-prefixed, NUL-terminated UTF-16 form of `path`, so paths
/// past 260 characters work as they do through `std::fs`.
pub(super) fn verbatim_wide(path: &Path) -> io::Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;

    let absolute = std::path::absolute(path)?;
    let wide: Vec<u16> = absolute.as_os_str().encode_wide().collect();
    let text = String::from_utf16_lossy(&wide);
    let (prefix, rest) = if text.starts_with(r"\\?\") || text.starts_with(r"\\.\") {
        ("", &wide[..])
    } else if text.starts_with(r"\\") {
        (r"\\?\UNC\", &wide[2..]) // \\server\share\… → \\?\UNC\server\share\…
    } else {
        (r"\\?\", &wide[..])
    };
    let mut out: Vec<u16> = prefix.encode_utf16().chain(rest.iter().copied()).collect();
    out.push(0);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::fsops::test_support::{make_delete_pending, test_dir};

    #[cfg(windows)]
    #[test]
    fn a_delete_pending_file_never_counts_as_present() {
        let dir = test_dir();
        let f = dir.path().join("hasami-test-pending.mp4");
        fs::write(&f, b"x").unwrap();
        assert_eq!(presence(&f), Presence::Present);
        assert_eq!(presence(&dir.path().join("nope.mp4")), Presence::Absent);
        assert_eq!(presence(&dir.path().join("no-dir").join("nope.mp4")), Presence::Absent);

        let holder = make_delete_pending(&f);
        // The trap this guards against: std still reports the file as there.
        assert!(fs::symlink_metadata(&f).is_ok(), "std's fallback sees the pending entry");
        assert_eq!(presence(&f), Presence::Unknown);
        drop(holder);
        assert_eq!(presence(&f), Presence::Absent);
    }
}
