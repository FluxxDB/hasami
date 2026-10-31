mod common;

use std::path::Path;

use hasami_lib::editor::request::{
    build_graph, encoder_available, AudioPiece, AudioTrackRequest, ExportMode, ExportRequest, VideoPiece,
};
use hasami_lib::media::{hidden_command, probe, FfmpegPaths};

/// Render `req`'s graph over `input` into `out` with plain libx264/aac, panicking with ffmpeg's
/// stderr on failure.
fn render_graph(paths: &FfmpegPaths, input: &Path, req: &ExportRequest, out: &Path) {
    let graph = build_graph(req);
    let mut cmd = hidden_command(&paths.ffmpeg);
    cmd.args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(input)
        .args(["-filter_complex", &graph.filter]);
    if let Some(v) = &graph.video_out {
        cmd.args(["-map", v, "-c:v", "libx264", "-pix_fmt", "yuv420p"]);
    }
    if let Some(a) = &graph.audio_out {
        cmd.args(["-map", a, "-c:a", "aac"]);
    }
    let output = cmd.arg(out).output().expect("failed to spawn ffmpeg");
    assert!(
        output.status.success(),
        "ffmpeg failed ({}) for filter {}:\n{}",
        output.status,
        graph.filter,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn render_graph_cuts_duration() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_multitrack_hevc(dir.path());
    let paths = FfmpegPaths::resolve().unwrap();
    let req = ExportRequest {
        session: 1,
        input: input.clone(),
        mode: ExportMode::Quick,
        fit_20mb: false,
        duration_s: 3.0,
        video: vec![
            VideoPiece { src_start: 0.0, src_end: 1.0 },
            VideoPiece { src_start: 2.0, src_end: 4.0 },
        ],
        audio: vec![AudioTrackRequest {
            index: 0,
            gain_db: 0.0,
            pieces: vec![
                AudioPiece { at: 0.0, src_start: 0.0, src_end: 1.0 },
                AudioPiece { at: 1.0, src_start: 2.0, src_end: 4.0 },
            ],
        }],
    };

    let out = dir.path().join("cut.mp4");
    render_graph(&paths, &input, &req, &out);

    let info = probe(&paths, &out).unwrap();
    assert!((info.duration_s - 3.0).abs() < 0.1, "duration was {}", info.duration_s);
    assert!(info.video.is_some());
    assert_eq!(info.audio.len(), 1);
}

#[test]
fn encoder_available_tests_with_a_real_encode() {
    let paths = FfmpegPaths::resolve().unwrap();
    assert!(encoder_available(&paths, "libx264"));
    assert!(!encoder_available(&paths, "no_such_encoder"));
    // Second call is served from the cache and agrees.
    assert!(encoder_available(&paths, "libx264"));
}
