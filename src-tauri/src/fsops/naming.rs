//! Where each output goes: the hidden temp sibling, free numbered names, the `[CLIPPED] ` and
//! `[20MB] ` names, the `Raw` folder and the Windows Videos folder.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::FsError;

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
pub(super) fn same_path(a: &Path, b: &Path) -> bool {
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
pub(super) const RAW_FOLDER: &str = "Raw";

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

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::fsops::test_support::test_dir;

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
    fn a_raw_folder_at_a_drive_root_or_a_bare_name_is_handled() {
        assert!(in_raw_folder(Path::new(r"C:\Raw\a.mp4")));
        // Outputs of a clip in C:\Raw go to the drive root.
        assert_eq!(clipped_path(Path::new(r"C:\Raw\a.mp4"), "mp4"), Path::new(r"C:\[CLIPPED] a.mp4"));
        // A bare file name (no folder) stays relative to where Hasami was started.
        assert_eq!(clipped_path(Path::new("a.mov"), "mp4"), Path::new("[CLIPPED] a.mp4"));
        assert!(!in_raw_folder(Path::new("a.mp4")));
    }
}
