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
        Ok(cli::Action::Register | cli::Action::Unregister) => {
            // TODO(Task 10): run the shell-menu register/unregister here, then return without
            // starting Tauri, so no window opens and nothing joins a running instance.
        }
        Ok(action) => app::run(action),
        Err(e) => {
            eprintln!("hasami: {e}");
            std::process::exit(2);
        }
    }
}
