//! What the progress and editor windows are sent: the Tauri event names, the editor's events and
//! the clip details `editor_open` returns. This is the contract with the UI (mirrored in
//! `src/lib/jobs.ts` and `src/editor/api.ts`).

use std::path::Path;

use serde::Serialize;

use crate::editor::media::{Crop, PreparedMedia};

/// Tauri event that carries every [`JobEvent`](crate::jobs::queue::JobEvent) to the progress window.
pub const JOB_EVENT: &str = "hasami://job";
/// Tauri event that carries every [`EditorEvent`] to the editor window.
pub const EDITOR_EVENT: &str = "hasami://editor";

/// What the editor window is told. Mirrored in `src/editor/api.ts`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type")]
pub enum EditorEvent {
    /// More files from a later Trim launch, to add to the editor's queue.
    Files { paths: Vec<String> },
    /// Export `id` of `input` began rendering.
    ExportStarted { id: u64, input: String },
    /// `fraction` covers the whole export, across all its passes.
    ExportProgress { id: u64, fraction: f64 },
    /// Export `id` was saved as `output` (`bytes` long).
    ExportDone {
        id: u64,
        input: String,
        output: String,
        bytes: u64,
        /// The export was saved but something else wasn't (the 20 MB copy); plain language.
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// Export `id` didn't finish; `message` is plain language, and `original_intact` says whether
    /// the original is still at its own path.
    ExportFailed { id: u64, input: String, message: String, original_intact: bool },
    /// How far the preview proxy of `session` has got.
    ProxyProgress { session: u64, fraction: f64 },
    /// A deferred discard sent the clip to the Recycle Bin.
    DiscardDone { path: String },
    /// A deferred discard left the clip where it was (an export it waited for didn't finish, or
    /// the recycle failed).
    DiscardKept { path: String, message: String },
}

/// A clip opened in the editor, as `editor_open` returns it. Paths are absolute; the page turns
/// them into asset URLs with `convertFileSrc`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EditorMediaDto {
    pub session: u64,
    pub path: String,
    pub file_name: String,
    pub size_bytes: u64,
    /// The input's modified time, milliseconds since the Unix epoch (what `editor_adjacent_file` orders by).
    pub modified_ms: u64,
    pub duration_s: f64,
    pub bit_rate: Option<u64>,
    pub video: Option<EditorVideoDto>,
    pub tracks: Vec<EditorTrackDto>,
    /// The picture area inside a fixed black border, in source pixels; `None` when there is none.
    pub border: Option<Crop>,
}

/// The video stream of a clip opened in the editor.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EditorVideoDto {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub codec: String,
}

/// One audio track of a clip opened in the editor (`index` is ffmpeg's `0:a:<index>`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EditorTrackDto {
    pub index: usize,
    pub empty: bool,
    /// The extracted audio; `None` when the track is empty.
    pub file: Option<String>,
}

impl EditorMediaDto {
    pub(super) fn new(session: u64, input: &Path, media: &PreparedMedia, modified_ms: u64) -> Self {
        let info = &media.info;
        Self {
            session,
            path: input.display().to_string(),
            file_name: input.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            size_bytes: info.size_bytes,
            modified_ms,
            duration_s: info.duration_s,
            bit_rate: info.bit_rate,
            video: info.video.as_ref().map(|v| EditorVideoDto {
                width: v.width,
                height: v.height,
                fps: v.fps,
                codec: v.codec.clone(),
            }),
            tracks: media
                .tracks
                .iter()
                .map(|t| EditorTrackDto {
                    index: t.index,
                    empty: t.empty,
                    file: t.file.as_ref().map(|f| f.display().to_string()),
                })
                .collect(),
            border: media.border,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn strings(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn discard_events_serialize_to_the_ui_contract() {
        assert_eq!(
            serde_json::to_value(EditorEvent::DiscardDone { path: "C:\\v\\a.mp4".into() }).unwrap(),
            serde_json::json!({"type": "DiscardDone", "path": "C:\\v\\a.mp4"})
        );
        assert_eq!(
            serde_json::to_value(EditorEvent::DiscardKept { path: "a.mp4".into(), message: "m".into() }).unwrap(),
            serde_json::json!({"type": "DiscardKept", "path": "a.mp4", "message": "m"})
        );
    }

    #[test]
    fn editor_events_serialize_to_the_ui_contract() {
        let failed = EditorEvent::ExportFailed {
            id: 5,
            input: "C:\\v\\a.mp4".into(),
            message: "boom".into(),
            original_intact: false,
        };
        assert_eq!(
            serde_json::to_value(&failed).unwrap(),
            serde_json::json!({"type": "ExportFailed", "id": 5, "input": "C:\\v\\a.mp4", "message": "boom",
                               "original_intact": false})
        );
        let files = EditorEvent::Files { paths: strings(&["a.mp4"]) };
        assert_eq!(serde_json::to_value(&files).unwrap(), serde_json::json!({"type": "Files", "paths": ["a.mp4"]}));
    }

    #[test]
    fn editor_media_border_serializes_to_the_ui_contract() {
        let info = crate::media::MediaInfo {
            path: PathBuf::from("a.mp4"),
            container: "mp4".into(),
            duration_s: 5.0,
            size_bytes: 1,
            bit_rate: None,
            video: None,
            audio: Vec::new(),
        };
        let border = Some(Crop { x: 0, y: 0, w: 1440, h: 810 });
        let media = PreparedMedia { info, tracks: Vec::new(), border };
        let dto = serde_json::to_value(EditorMediaDto::new(1, Path::new("a.mp4"), &media, 0)).unwrap();
        assert_eq!(dto["border"], serde_json::json!({"x": 0, "y": 0, "w": 1440, "h": 810}));
        let media = PreparedMedia { border: None, ..media };
        let dto = serde_json::to_value(EditorMediaDto::new(1, Path::new("a.mp4"), &media, 0)).unwrap();
        assert_eq!(dto["border"], serde_json::Value::Null);
    }
}
