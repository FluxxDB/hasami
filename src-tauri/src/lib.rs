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

/// Entry point. Explorer launches `hasami.exe <verb> <files…>`; the first launch becomes the app
/// and later launches join it (see [`app`]).
pub fn run() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match cli::parse_args(&args) {
        Ok(action) => {
            // The installer runs `register`/`unregister`; they must exit without starting Tauri
            // (no window, and nothing joins a running instance).
            if let Some(code) = shell_menu::handle_action(
                &action,
                std::env::current_exe(),
                shell_menu::DEFAULT_CLASSES_ROOT,
            ) {
                std::process::exit(code);
            }
            app::run(action)
        }
        // Started from the Start menu or the installer's "Run Hasami": say where Hasami lives.
        Err(cli::CliError::MissingVerb) => show_menu_hint(),
        Err(e) => {
            eprintln!("hasami: {e}");
            std::process::exit(2);
        }
    }
}

/// Shown when Hasami is started with no arguments.
pub const MENU_HINT: &str = "Hasami lives in your right-click menu. Right-click a video → Hasami.";

/// A plain Windows message box with [`MENU_HINT`]; returns once the user closes it.
fn show_menu_hint() {
    use windows::core::HSTRING;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};
    unsafe {
        MessageBoxW(None, &HSTRING::from(MENU_HINT), &HSTRING::from("Hasami"), MB_OK | MB_ICONINFORMATION);
    }
}
