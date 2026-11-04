//! Temp/final/unique file names, commit and recycle helpers (Task 4).
//!
//! This is the safety core: the user's original must never be lost. Every
//! function here either completes its whole job, leaves the original at its
//! original path with its original bytes, or (in the one case where neither is
//! possible) keeps the new file and says exactly where both files are.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

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

    fn with_source(message: impl Into<String>, source: impl std::error::Error + Send + Sync + 'static) -> Self {
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
    fn rename_unsure(temp: &Path, final_path: &Path, source: io::Error) -> Self {
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
    fn from_io(err: io::Error) -> Self {
        FsError::from_io_while(err, "replace the file")
    }

    /// Wrap an I/O error from giving a new file its name (no original involved).
    fn from_io_saving(err: io::Error) -> Self {
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

/// Prefix of every file the Quick Trim editor writes.
pub const CLIPPED_PREFIX: &str = "[CLIPPED] ";

/// Folder (next to the clips) that originals are moved into after a trim.
const RAW_FOLDER: &str = "Raw";

/// Where a trimmed copy of `input` goes: `[CLIPPED] <stem>.<ext>` in the same
/// folder, or in the folder above when `input` already sits in a `Raw` folder.
/// A `[CLIPPED] ` prefix already on the stem is not doubled. The name is free
/// (numbered like `unique_path` if it isn't).
pub fn clipped_path(input: &Path, ext: &str) -> PathBuf {
    let mut dir = input.parent().unwrap_or_else(|| Path::new(""));
    if in_raw_folder(input) {
        dir = dir.parent().unwrap_or(dir);
    }
    let stem = input.file_stem().unwrap_or_default();
    let stem = match stem.to_str().and_then(|s| s.strip_prefix(CLIPPED_PREFIX)) {
        Some(bare) => OsString::from(bare),
        None => stem.to_os_string(),
    };
    let mut name = OsString::from(CLIPPED_PREFIX);
    name.push(stem);
    name.push(".");
    name.push(ext.trim_start_matches('.'));
    unique_path(&dir.join(name), None)
}

/// Prefix of every file Compress (and the editor's 20 MB copy) writes.
pub const COMPRESSED_PREFIX: &str = "[20MB] ";

/// Where a 20 MB copy of `input` goes: `[20MB] <stem>.mp4` in `videos_dir`. Any `[CLIPPED] ` or
/// `[20MB] ` prefix already on the stem is dropped first, so the prefix is never doubled. The
/// name is free (numbered like `unique_path` if it isn't).
pub fn compressed_path(input: &Path, videos_dir: &Path) -> PathBuf {
    let stem = input.file_stem().unwrap_or_default();
    let stem = match stem.to_str() {
        Some(mut bare) => {
            while let Some(rest) = bare.strip_prefix(CLIPPED_PREFIX).or_else(|| bare.strip_prefix(COMPRESSED_PREFIX)) {
                bare = rest;
            }
            OsString::from(bare)
        }
        None => stem.to_os_string(),
    };
    let mut name = OsString::from(COMPRESSED_PREFIX);
    name.push(stem);
    name.push(".mp4");
    unique_path(&videos_dir.join(name), None)
}

/// Shown when Windows can't say where the Videos folder is (or it no longer exists).
pub const NO_VIDEOS_FOLDER: &str = "Couldn't find your Videos folder.";

/// The Windows Videos folder (`FOLDERID_Videos`), wherever the user has moved it, e.g. into
/// OneDrive. Fails if Windows has no such folder on record or it doesn't exist.
pub fn videos_dir() -> Result<PathBuf, FsError> {
    use std::os::windows::ffi::OsStringExt;
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Videos, SHGetKnownFolderPath, KF_FLAG_DEFAULT};

    let found = unsafe { SHGetKnownFolderPath(&FOLDERID_Videos, KF_FLAG_DEFAULT, None) }
        .map_err(|e| FsError::with_source(NO_VIDEOS_FOLDER, e))?;
    // Windows allocated the string; copy it out, then hand the memory back.
    let path = unsafe { PathBuf::from(OsString::from_wide(found.as_wide())) };
    unsafe { CoTaskMemFree(Some(found.0 as *const std::ffi::c_void)) };
    Ok(path)
}

/// True when the folder holding `path` is named `Raw` (any letter case).
pub fn in_raw_folder(path: &Path) -> bool {
    path.parent().and_then(Path::file_name).is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(RAW_FOLDER))
}

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
    if in_raw_folder(original) {
        return Ok(None);
    }
    let raw = original.parent().unwrap_or_else(|| Path::new("")).join(RAW_FOLDER);
    std::fs::create_dir_all(&raw).map_err(|e| FsError::with_source("Couldn't create the Raw folder.", e))?;
    let target = unique_path(&raw.join(original.file_name().unwrap_or_default()), None);
    rename_with_retry(&rename_no_replace, original, &target)
        .map_err(|e| FsError::with_source("Couldn't move the original into the Raw folder.", e))?;
    Ok(Some(target))
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
/// Sends a file to the Recycle Bin.
type RecycleFn<'a> = &'a dyn Fn(&Path) -> Result<(), FsError>;
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

/// What a look at a path found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Presence {
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
fn presence(path: &Path) -> Presence {
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

/// Win32 errors arrive as HRESULTs (0x8007xxxx); hand back the plain Windows
/// error code so `raw_os_error` matches what std::fs reports.
fn win32_io_error(e: windows::core::Error) -> io::Error {
    let hr = e.code().0 as u32;
    if hr & 0xFFFF_0000 == 0x8007_0000 {
        io::Error::from_raw_os_error((hr & 0xFFFF) as i32)
    } else {
        io::Error::other(e)
    }
}

/// Absolute, `\\?\`-prefixed, NUL-terminated UTF-16 form of `path`, so paths
/// past 260 characters work as they do through `std::fs`.
fn verbatim_wide(path: &Path) -> io::Result<Vec<u16>> {
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

/// Probe with the same access the Recycle Bin needs (DELETE, sharing everything
/// else): fails with a sharing violation if another program holds the file
/// without allowing deletion.
fn ensure_deletable(path: &Path) -> Result<(), FsError> {
    open_for_delete(path).map_err(FsError::from_io)
}

fn open_for_delete(path: &Path) -> io::Result<()> {
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
    use super::*;
    use std::fs;

    fn test_dir() -> tempfile::TempDir {
        tempfile::Builder::new().prefix("hasami-test-").tempdir().unwrap()
    }

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
        assert_eq!(with_extension(Path::new(r"C:\d\a.mkv"), "mp4"), Path::new(r"C:\d\a.mp4"));
    }

    /// Every name in `dir`, sorted.
    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> =
            fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
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

    /// A look at the path that always fails, like a delete-pending file held by
    /// Explorer's thumbnailer, the indexer or antivirus.
    fn unreadable(_: &Path) -> Presence {
        Presence::Unknown
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

    /// Put `path` into the legacy delete-pending state FAT/exFAT/SMB always use
    /// (and NTFS without POSIX semantics): another program holds a share-delete
    /// handle, and a second handle sets `FileDispositionInfo` (class 4) and
    /// closes. The returned holder keeps the name in place until it is dropped.
    #[cfg(windows)]
    fn make_delete_pending(path: &Path) -> fs::File {
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::Storage::FileSystem::{
            FileDispositionInfo, SetFileInformationByHandle, FILE_DISPOSITION_INFO,
        };
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

    #[test]
    fn clipped_path_prefixes_and_keeps_ext() {
        assert_eq!(clipped_path(Path::new(r"C:\d\Replay 1.mp4"), "mp4"), Path::new(r"C:\d\[CLIPPED] Replay 1.mp4"));
    }

    #[test]
    fn clipped_path_never_doubles_prefix() {
        let dir = test_dir();
        let input = dir.path().join("[CLIPPED] Replay 1.mp4");
        assert_eq!(clipped_path(&input, "mp4"), dir.path().join("[CLIPPED] Replay 1.mp4"));
        // Another extension keeps the single prefix too.
        assert_eq!(clipped_path(&input, "mkv"), dir.path().join("[CLIPPED] Replay 1.mkv"));
    }

    #[test]
    fn clipped_path_counts_collisions() {
        let dir = test_dir();
        fs::write(dir.path().join("[CLIPPED] a.mp4"), b"1").unwrap();
        assert_eq!(clipped_path(&dir.path().join("a.mp4"), "mp4"), dir.path().join("[CLIPPED] a (2).mp4"));
    }

    #[test]
    fn clipped_path_from_raw_folder_goes_to_parent() {
        let dir = test_dir();
        assert_eq!(clipped_path(&dir.path().join("raw").join("a.mov"), "mp4"), dir.path().join("[CLIPPED] a.mp4"));
    }

    #[test]
    fn compressed_path_drops_existing_prefixes_and_is_always_mp4() {
        let videos = test_dir();
        let v = videos.path();
        for (input, want) in [
            (r"C:\d\Replay 1.mkv", "[20MB] Replay 1.mp4"),
            (r"C:\d\[CLIPPED] Replay 1.mov", "[20MB] Replay 1.mp4"),
            (r"C:\d\[20MB] Replay 1.mp4", "[20MB] Replay 1.mp4"),
            (r"C:\d\[20MB] [CLIPPED] Replay 1.webm", "[20MB] Replay 1.mp4"),
            (r"C:\d\Raw\Replay [CLIPPED] 1.MP4", "[20MB] Replay [CLIPPED] 1.mp4"),
        ] {
            assert_eq!(compressed_path(Path::new(input), v), v.join(want), "{input}");
        }
    }

    #[test]
    fn compressed_path_counts_collisions() {
        let videos = test_dir();
        fs::write(videos.path().join("[20MB] a.mp4"), b"1").unwrap();
        fs::write(videos.path().join("[20MB] a (2).mp4"), b"2").unwrap();
        assert_eq!(
            compressed_path(Path::new(r"C:\d\[CLIPPED] a.mp4"), videos.path()),
            videos.path().join("[20MB] a (3).mp4")
        );
    }

    #[test]
    fn videos_dir_is_an_absolute_path() {
        // Only asks Windows where the folder is; nothing in it is read or written.
        assert!(videos_dir().unwrap().is_absolute());
    }

    #[test]
    fn in_raw_folder_ignores_case() {
        assert!(in_raw_folder(Path::new(r"C:\d\Raw\a.mp4")));
        assert!(in_raw_folder(Path::new(r"C:\d\RAW\a.mp4")));
        assert!(!in_raw_folder(Path::new(r"C:\d\Raw2\a.mp4")));
        assert!(!in_raw_folder(Path::new(r"C:\Raw\d\a.mp4")));
        assert!(!in_raw_folder(Path::new("a.mp4")));
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
    fn move_to_raw_fails_cleanly_when_the_file_is_missing() {
        let dir = test_dir();
        let err = move_to_raw(&dir.path().join("gone.mp4")).unwrap_err();
        assert!(err.original_intact);
        assert!(err.kept_new_file.is_none());
    }
}
