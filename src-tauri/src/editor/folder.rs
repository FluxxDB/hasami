//! Finding the neighbouring video for the editor's Prev / Next video buttons, and ordering a selection by date.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::fsops::{CLIPPED_PREFIX, COMPRESSED_PREFIX};
use crate::media::VIDEO_EXTENSIONS;

/// The video next to `current` in the folder's date order: siblings in the same folder with a video extension,
/// ordered by modified time (oldest first), ties by file name (case-insensitive); `[CLIPPED] ` and `[20MB] `
/// outputs skipped; subfolders not listed. `newer` picks the nearest one after `current` in that order, otherwise
/// the nearest one before it. `current` may no longer exist: its position comes from `current_modified_ms` + its name.
pub fn adjacent_video(current: &Path, current_modified_ms: u64, newer: bool) -> io::Result<Option<PathBuf>> {
    let folder = match current.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let here = (current_modified_ms, lower_name(current));
    let skipped = [CLIPPED_PREFIX.to_lowercase(), COMPRESSED_PREFIX.to_lowercase()];
    let mut nearest: Option<((u64, String), PathBuf)> = None;
    for entry in std::fs::read_dir(folder)? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        let name = lower_name(&path);
        if !is_video(&path) || skipped.iter().any(|prefix| name.starts_with(prefix.as_str())) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let Some(modified_ms) = meta.modified().ok().and_then(millis) else { continue };
        let key = (modified_ms, name);
        // Only files on the wanted side of `current`.
        if (newer && key <= here) || (!newer && key >= here) {
            continue;
        }
        // The nearest one on that side: the smallest key going newer, the largest going older.
        let nearer = nearest.as_ref().is_none_or(|(best, _)| if newer { key < *best } else { key > *best });
        if nearer {
            nearest = Some((key, path));
        }
    }
    Ok(nearest.map(|(_, path)| path))
}

/// `paths` oldest first by modified time, ties by file name (case-insensitive), the order [`adjacent_video`]
/// walks. Paths whose modified time can't be read go last, in the order given.
pub fn order_videos(paths: &[PathBuf]) -> Vec<PathBuf> {
    let (mut known, unknown): (Vec<_>, Vec<_>) = paths
        .iter()
        .map(|path| {
            let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(millis);
            (modified, path)
        })
        .partition(|(modified, _)| modified.is_some());
    known.sort_by_cached_key(|(modified, path)| (*modified, lower_name(path)));
    known.into_iter().chain(unknown).map(|(_, path)| path.clone()).collect()
}

fn lower_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| VIDEO_EXTENSIONS.iter().any(|v| v.eq_ignore_ascii_case(e)))
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
    fn walks_newer_and_older_videos_then_stops() {
        let dir = tempfile::tempdir().unwrap();
        let a = make(dir.path(), "a.mp4", 300_000);
        let b = make(dir.path(), "b.mp4", 200_000);
        let c = make(dir.path(), "c.mp4", 100_000);
        assert_eq!(adjacent_video(&c, 100_000, true).unwrap(), Some(b.clone()));
        assert_eq!(adjacent_video(&b, 200_000, true).unwrap(), Some(a.clone()));
        assert_eq!(adjacent_video(&a, 300_000, true).unwrap(), None);
        assert_eq!(adjacent_video(&a, 300_000, false).unwrap(), Some(b.clone()));
        assert_eq!(adjacent_video(&b, 200_000, false).unwrap(), Some(c.clone()));
        assert_eq!(adjacent_video(&c, 100_000, false).unwrap(), None);
    }

    #[test]
    fn skips_clipped_and_compressed_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let a = make(dir.path(), "a.mp4", 300_000);
        make(dir.path(), "[CLIPPED] x.mp4", 250_000);
        make(dir.path(), "[20mb] y.mp4", 225_000);
        let b = make(dir.path(), "b.mp4", 200_000);
        assert_eq!(adjacent_video(&a, 300_000, false).unwrap(), Some(b.clone()));
        assert_eq!(adjacent_video(&b, 200_000, true).unwrap(), Some(a));
    }

    #[test]
    fn ignores_other_files_and_subfolders() {
        let dir = tempfile::tempdir().unwrap();
        let a = make(dir.path(), "a.mp4", 300_000);
        make(dir.path(), "notes.txt", 250_000);
        fs::create_dir(dir.path().join("Raw")).unwrap();
        // A folder with a video-looking name is still not a video.
        fs::create_dir(dir.path().join("folder.mp4")).unwrap();
        assert_eq!(adjacent_video(&a, 300_000, false).unwrap(), None);
        assert_eq!(adjacent_video(&a, 300_000, true).unwrap(), None);
    }

    #[test]
    fn equal_times_order_by_name_ignoring_case() {
        let dir = tempfile::tempdir().unwrap();
        let a = make(dir.path(), "a.mp4", 100_000);
        let b = make(dir.path(), "B.mp4", 100_000);
        let c = make(dir.path(), "c.mp4", 100_000);
        assert_eq!(adjacent_video(&a, 100_000, true).unwrap(), Some(b.clone()));
        assert_eq!(adjacent_video(&b, 100_000, true).unwrap(), Some(c.clone()));
        assert_eq!(adjacent_video(&c, 100_000, true).unwrap(), None);
        assert_eq!(adjacent_video(&c, 100_000, false).unwrap(), Some(b.clone()));
        assert_eq!(adjacent_video(&a, 100_000, false).unwrap(), None);
    }

    #[test]
    fn still_finds_neighbours_after_the_current_file_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let a = make(dir.path(), "a.mp4", 300_000);
        let b = make(dir.path(), "b.mp4", 200_000);
        let c = make(dir.path(), "c.mp4", 100_000);
        fs::remove_file(&b).unwrap();
        assert_eq!(adjacent_video(&b, 200_000, true).unwrap(), Some(a));
        assert_eq!(adjacent_video(&b, 200_000, false).unwrap(), Some(c));
    }

    #[test]
    fn matches_extensions_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        let a = make(dir.path(), "a.mp4", 300_000);
        let b = make(dir.path(), "b.MP4", 200_000);
        let c = make(dir.path(), "c.MkV", 100_000);
        assert_eq!(adjacent_video(&a, 300_000, false).unwrap(), Some(b.clone()));
        assert_eq!(adjacent_video(&b, 200_000, false).unwrap(), Some(c));
    }

    #[test]
    fn an_unreadable_folder_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("missing").join("a.mp4");
        assert!(adjacent_video(&gone, 1, false).is_err());
        assert!(adjacent_video(&gone, 1, true).is_err());
    }

    #[test]
    fn orders_videos_oldest_first_with_unreadable_paths_last() {
        let dir = tempfile::tempdir().unwrap();
        let a = make(dir.path(), "a.mp4", 300_000);
        let b = make(dir.path(), "b.mp4", 200_000);
        let c = make(dir.path(), "c.mp4", 100_000);
        let gone1 = dir.path().join("gone1.mp4");
        let gone2 = dir.path().join("gone2.mp4");
        let ordered = order_videos(&[gone2.clone(), a.clone(), gone1.clone(), c.clone(), b.clone()]);
        assert_eq!(ordered, vec![c, b, a, gone2, gone1]);
    }

    #[test]
    fn order_videos_breaks_ties_by_name_ignoring_case() {
        let dir = tempfile::tempdir().unwrap();
        let c = make(dir.path(), "c.mp4", 100_000);
        let b = make(dir.path(), "B.mp4", 100_000);
        let a = make(dir.path(), "a.mp4", 100_000);
        assert_eq!(order_videos(&[c.clone(), b.clone(), a.clone()]), vec![a, b, c]);
    }
}
