//! The two windows Hasami opens: the small progress window for Compress and Convert (and export
//! failures once the editor is gone), and the Quick Trim editor.

use std::path::PathBuf;

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use super::{lock, AppState};

/// Window labels; window events tell the two apart by these.
pub(super) const PROGRESS_LABEL: &str = "progress";
pub(super) const EDITOR_LABEL: &str = "editor";

/// Open the progress window. `kind` (and a convert's `ext`) tell the page what to show until the
/// first job starts.
pub(super) fn open_progress(app: &AppHandle, kind: &str, ext: Option<&str>) -> tauri::Result<()> {
    let mut url = format!("progress.html?kind={kind}");
    if let Some(ext) = ext {
        url.push_str("&ext=");
        url.push_str(ext);
    }
    WebviewWindowBuilder::new(app, PROGRESS_LABEL, WebviewUrl::App(url.into()))
        .title("Hasami")
        .inner_size(440.0, 190.0)
        .resizable(false)
        .maximizable(false)
        .decorations(false)
        .center()
        .build()
        .map(|_| ())
}

/// Open the editor window for `files`; the page asks for them with `editor_files`.
pub(super) fn open_editor(app: &AppHandle, files: Vec<PathBuf>) -> tauri::Result<()> {
    *lock(&app.state::<AppState>().editor_files) = files.iter().map(|f| f.display().to_string()).collect();
    WebviewWindowBuilder::new(app, EDITOR_LABEL, WebviewUrl::App("editor.html".into()))
        .title("Hasami")
        .inner_size(1280.0, 800.0)
        .min_inner_size(960.0, 600.0)
        .resizable(true)
        .decorations(false)
        .center()
        .build()
        .map(|_| ())
}
