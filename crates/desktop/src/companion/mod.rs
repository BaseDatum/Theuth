//! The nested Swift companion (130 D10, §5.8): `theuth.app/Contents/Library/LoginItems/Theuth
//! Companion.app`, bundle id [`BUNDLE_ID`]. The shell launches it at start, watches it by bundle id,
//! tells every webview `companion-alive { alive }`, hides its own tray while the companion owns the
//! menu bar (D9), and terminates it in the quit handshake. macOS only; the other backends are no-ops
//! so Windows and Linux keep today's tray and notifications (§5.9).

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;
#[cfg(not(target_os = "macos"))]
mod other;
#[cfg(not(target_os = "macos"))]
pub use other::*;
#[cfg(target_os = "macos")]
pub mod login_item;

/// `<main id>.companion` (D1); the Info.plist `scripts/bundle-companion.sh` writes.
pub const BUNDLE_ID: &str = "io.theuth.mac.companion";
/// Event to every webview: `{ alive: bool }`, on every change and once at start.
pub const ALIVE_EVENT: &str = "companion-alive";

/// Payload of [`ALIVE_EVENT`].
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
#[tauri::command]
pub fn companion_login_item_status() -> String {
    #[cfg(target_os = "macos")]
    {
        login_item::status().as_str().into()
    }
    #[cfg(not(target_os = "macos"))]
    {
        "unavailable".into()
    }
}
