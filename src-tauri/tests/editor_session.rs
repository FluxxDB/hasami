//! Integration tests for editor sessions: work folders, and closing letting go of the clip.

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

/// Discard recycles the clip as soon as its session has closed, so closing must not return while
/// a cancelled ffmpeg still has the clip open. Thumbnails write a picture only now and then, so
/// the work folder can be deleted while their ffmpeg is still reading the clip.
#[test]
fn close_returns_only_once_ffmpeg_has_let_go_of_the_clip() {
    use hasami_lib::editor::media::thumbnails;
    use std::time::Duration;

    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_big_h264(dir.path(), 20);
    let paths = FfmpegPaths::resolve().expect("bundled ffmpeg not found");
    let mut sessions = EditorSessions::new();
    let (id, media, work_dir) = sessions.open(&paths, &input).unwrap();
    let cancel = sessions.cancel_token(id).unwrap();

    let (run_paths, run_input) = (paths.clone(), input.clone());
    let run = std::thread::spawn(move || {
        thumbnails(&run_paths, &run_input, media.info.duration_s, &work_dir.join("thumbs"), &cancel)
    });
    // Decoding a 20 s 1080p clip for its pictures takes well over this.
    std::thread::sleep(Duration::from_millis(300));
    assert!(!run.is_finished(), "the thumbnails finished before the close; the test proves nothing");

    sessions.close(id);
    // What the Recycle Bin needs: nothing may hold the clip without allowing it to be moved.
    let moved = dir.path().join("moved.mp4");
    let result = std::fs::rename(&input, &moved);
    let _ = run.join();
    result.expect("the clip was still open after the session closed");
}
