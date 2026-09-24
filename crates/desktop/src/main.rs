//! Desktop binary: the app lives in `desktop_lib` so the iOS bridge can link
//! it as a static library (31 §8.3).
//!
//! Release builds use the Windows GUI subsystem, so launching the app does not
//! open a console window beside it (2026-09-24); debug builds keep the console
//! for logs. No effect on other targets.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    desktop_lib::run()
}
