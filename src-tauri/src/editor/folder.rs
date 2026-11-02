//! Finding the "next" video for the editor's Next video button.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::fsops::CLIPPED_PREFIX;

/// The extensions Hasami treats as videos (the same ones its right-click menu is registered for).
pub const VIDEO_EXTS: [&str; 4] = ["mp4", "mov", "mkv", "webm"];

/// The next video after `current` in Explorer's "date, descending" order: siblings in the same folder with a
/// video extension, newest modified first, ties by file name (case-insensitive); `[CLIPPED] ` outputs skipped;
/// subfolders not listed. `current` may no longer exist: its position comes from `current_modified_ms` + its name.
pub fn next_video(current: &Path, current_modified_ms: u64) -> io::Result<Option<PathBuf>> {
    let folder = match current.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let current_name = lower_name(current);
    let clipped_prefix = CLIPPED_PREFIX.to_lowercase();
    // A file comes after `current` when it is older, or equally old and later by name.
    let comes_after = |modified_ms: u64, name: &str| {
        modified_ms < current_modified_ms || (modified_ms == current_modified_ms && name > current_name.as_str())
    };
    let mut next: Option<(u64, String, PathBuf)> = None;
    for entry in std::fs::read_dir(folder)? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if !is_video(&path) || lower_name(&path).starts_with(&clipped_prefix) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let Some(modified_ms) = meta.modified().ok().and_then(millis) else { continue };
        let name = lower_name(&path);
        if !comes_after(modified_ms, &name) {
            continue;
        }
        // The nearest one after `current`: the newest, then the first by name.
        let nearer = next.as_ref().map_or(true, |(best_ms, best_name, _)| {
            modified_ms > *best_ms || (modified_ms == *best_ms && name < *best_name)
        });
        if nearer {
            next = Some((modified_ms, name, path));
        }
    }
    Ok(next.map(|(_, _, path)| path))
}

fn lower_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| VIDEO_EXTS.iter().any(|v| v.eq_ignore_ascii_case(e)))
}

/// Milliseconds since the Unix epoch; `None` for times before it.
pub fn millis(time: SystemTime) -> Option<u64> {
    time.duration_since(UNIX_EPOCH).ok().map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::time::Duration;

    fn make(dir: &Path, name: &str, modified_ms: u64) -> PathBuf {
        let path = dir.join(name);
        let file = File::create(&path).unwrap();
        file.set_modified(UNIX_EPOCH + Duration::from_millis(modified_ms)).unwrap();
        path
    }

    #[test]
    fn walks_older_videos_then_stops() {
        let dir = tempfile::tempdir().unwrap();
        let a = make(dir.path(), "a.mp4", 300_000);
        let b = make(dir.path(), "b.mp4", 200_000);
        let c = make(dir.path(), "c.mp4", 100_000);
        assert_eq!(next_video(&a, 300_000).unwrap(), Some(b.clone()));
        assert_eq!(next_video(&b, 200_000).unwrap(), Some(c.clone()));
        assert_eq!(next_video(&c, 100_000).unwrap(), None);
    }

    #[test]
    fn skips_clipped_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let a = make(dir.path(), "a.mp4", 300_000);
        make(dir.path(), "[CLIPPED] x.mp4", 250_000);
        let b = make(dir.path(), "b.mp4", 200_000);
        assert_eq!(next_video(&a, 300_000).unwrap(), Some(b));
    }

    #[test]
    fn ignores_other_files_and_subfolders() {
        let dir = tempfile::tempdir().unwrap();
        let a = make(dir.path(), "a.mp4", 300_000);
        make(dir.path(), "notes.txt", 250_000);
        fs::create_dir(dir.path().join("Raw")).unwrap();
        // A folder with a video-looking name is still not a video.
        fs::create_dir(dir.path().join("folder.mp4")).unwrap();
        assert_eq!(next_video(&a, 300_000).unwrap(), None);
    }

    #[test]
    fn equal_times_order_by_name_ignoring_case() {
        let dir = tempfile::tempdir().unwrap();
        let a = make(dir.path(), "a.mp4", 100_000);
        let b = make(dir.path(), "B.mp4", 100_000);
        let c = make(dir.path(), "c.mp4", 100_000);
        assert_eq!(next_video(&a, 100_000).unwrap(), Some(b.clone()));
        assert_eq!(next_video(&b, 100_000).unwrap(), Some(c.clone()));
        assert_eq!(next_video(&c, 100_000).unwrap(), None);
    }

    #[test]
    fn still_finds_the_next_after_the_current_file_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let a = make(dir.path(), "a.mp4", 300_000);
        let b = make(dir.path(), "b.mp4", 200_000);
        fs::remove_file(&a).unwrap();
        assert_eq!(next_video(&a, 300_000).unwrap(), Some(b));
    }

    #[test]
    fn matches_extensions_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        let a = make(dir.path(), "a.mp4", 300_000);
        let b = make(dir.path(), "b.MP4", 200_000);
        let c = make(dir.path(), "c.MkV", 100_000);
        assert_eq!(next_video(&a, 300_000).unwrap(), Some(b.clone()));
        assert_eq!(next_video(&b, 200_000).unwrap(), Some(c));
    }

    #[test]
    fn an_unreadable_folder_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("missing").join("a.mp4");
        assert!(next_video(&gone, 1).is_err());
    }
}
