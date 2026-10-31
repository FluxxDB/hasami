//! Hasami: right-click video tools for Windows. All logic lives in this library crate;
//! `main.rs` only calls [`run`].

pub mod app;
pub mod cli;
pub mod compress;
pub mod fsops;
pub mod jobs;
pub mod media;
pub mod shell_menu;
pub mod verify;

/// Build the Tauri app. No windows are declared in `tauri.conf.json`; later tasks create them
/// at runtime. For now, with no windows to show, exit immediately instead of idling.
pub fn run() {
    tauri::Builder::default()
        .build(tauri::generate_context!())
        .expect("error while building Hasami")
        .run(|app_handle, event| {
            if let tauri::RunEvent::Ready = event {
                if tauri::Manager::webview_windows(app_handle).is_empty() {
                    app_handle.exit(0);
                }
            }
        });
}
