//! The companion helper (130 D10, §5.8; 134 D10): on macOS the nested Swift app `theuth.app/Contents/
//! Library/LoginItems/Theuth Companion.app`, bundle id [`BUNDLE_ID`]; on Windows the C# app
//! `<exe dir>\companion\Theuth.Companion.exe`. The shell launches it at start, watches it (bundle id ·
//! named mutex), tells every webview `companion-alive { alive }`, hides its own tray while the companion
//! owns the menu bar / notification area (D9), and terminates it in the quit handshake. Linux keeps
//! today's tray and notifications (130 §5.9).

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod other;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub use other::*;
#[cfg(target_os = "macos")]
pub mod login_item;

/// `<main id>.companion` (130 D1); the Info.plist `scripts/bundle-companion.sh` writes. macOS only — the
/// Windows companion is found by its mutex, not an id.
#[cfg(target_os = "macos")]
pub const BUNDLE_ID: &str = "io.theuth.mac.companion";
/// Event to every webview: `{ alive: bool }`, on every change and once at start.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub const ALIVE_EVENT: &str = "companion-alive";

/// Payload of [`ALIVE_EVENT`].
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct Alive {
    /// A process with [`BUNDLE_ID`] is running.
    pub alive: bool,
}

/// The webview's initial read (`lib/companion.ts`); [`ALIVE_EVENT`] carries the changes.
#[tauri::command]
pub fn companion_alive() -> bool {
    alive()
}

/// 130 W2: the login item's `SMAppService` status (`not_registered` · `enabled` · `requires_approval` ·
/// `not_found` · `unavailable`), for the main UI to show beside the companion's Launch-at-login toggle.
/// 134 D12: on Windows the companion owns the Run key and mirrors it to `companion.json`
/// (`enabled` · `not_registered`).
#[tauri::command]
pub fn companion_login_item_status() -> String {
    #[cfg(target_os = "macos")]
    {
        login_item::status().as_str().into()
    }
    #[cfg(target_os = "windows")]
    {
        login_item_status().into()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "unavailable".into()
    }
}
