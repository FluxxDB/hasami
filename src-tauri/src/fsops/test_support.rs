//! Helpers shared by the file-safety tests.

use std::fs;
use std::path::Path;

use super::presence::Presence;

pub(super) fn test_dir() -> tempfile::TempDir {
    tempfile::Builder::new().prefix("hasami-test-").tempdir().unwrap()
}

/// Every name in `dir`, sorted.
pub(super) fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> =
        fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

/// A look at the path that always fails, like a delete-pending file held by
/// Explorer's thumbnailer, the indexer or antivirus.
pub(super) fn unreadable(_: &Path) -> Presence {
    Presence::Unknown
}

/// Put `path` into the legacy delete-pending state FAT/exFAT/SMB always use
/// (and NTFS without POSIX semantics): another program holds a share-delete
/// handle, and a second handle sets `FileDispositionInfo` (class 4) and
/// closes. The returned holder keeps the name in place until it is dropped.
#[cfg(windows)]
pub(super) fn make_delete_pending(path: &Path) -> fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{FileDispositionInfo, SetFileInformationByHandle, FILE_DISPOSITION_INFO};
    const SHARE_ALL: u32 = 0x1 | 0x2 | 0x4;
    const DELETE: u32 = 0x0001_0000;

    let holder = fs::OpenOptions::new().read(true).share_mode(SHARE_ALL).open(path).unwrap();
    let deleter = fs::OpenOptions::new().access_mode(DELETE).share_mode(SHARE_ALL).open(path).unwrap();
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    unsafe {
        SetFileInformationByHandle(
            HANDLE(deleter.as_raw_handle()),
            FileDispositionInfo,
            &info as *const FILE_DISPOSITION_INFO as *const std::ffi::c_void,
            std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
        .unwrap();
    }
    drop(deleter);
    holder
}
