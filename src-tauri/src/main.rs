//! The `hasami` executable; everything lives in the `hasami_lib` crate.

// Hide the console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    hasami_lib::run()
}
