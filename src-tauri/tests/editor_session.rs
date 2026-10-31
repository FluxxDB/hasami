mod common;

use hasami_lib::editor::session::EditorSessions;
use hasami_lib::media::FfmpegPaths;

#[test]
fn open_creates_and_close_removes_work_dir() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_multitrack_hevc(dir.path());
    let paths = FfmpegPaths::resolve().expect("bundled ffmpeg not found");
    let mut sessions = EditorSessions::new();

    let (id, media, work_dir) = sessions.open(&paths, &input).unwrap();
    let expected = std::env::temp_dir().join(format!("hasami-editor-{}-{id}", std::process::id()));
    assert_eq!(work_dir, expected);
    assert!(work_dir.is_dir());
    assert_eq!(sessions.work_dir(id), Some(work_dir.as_path()));
    assert_eq!(sessions.input(id), Some(input.as_path()));
    // The extracted tracks live in the work dir.
    assert_eq!(media.tracks.len(), 3);
    for file in media.tracks.iter().filter_map(|t| t.file.as_ref()) {
        assert!(file.starts_with(&work_dir), "{}", file.display());
    }

    let (second, _, second_dir) = sessions.open(&paths, &input).unwrap();
    assert!(second > id);
    assert_ne!(second_dir, work_dir);

    sessions.close(id);
    assert!(!work_dir.exists());
    assert_eq!(sessions.work_dir(id), None);
    assert_eq!(sessions.input(id), None);
    assert!(second_dir.is_dir(), "closing one session leaves the others");

    // Dropping closes whatever is still open.
    drop(sessions);
    assert!(!second_dir.exists());
}
