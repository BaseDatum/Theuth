//! 218: Apple Pencil gestures and handwriting recognition for the whiteboard, whose ink the web view draws.
//!
//! On iPadOS 17.5+ `pencil_listen` installs a `UIPencilInteraction` on the main web view and reports
//! double-tap and squeeze, with the user's preferred action, on its `onEvent` channel; `pencil_unlisten`
//! removes it. `recognize_text` runs Vision handwriting recognition on a PNG. The Swift side is
//! `ios/Sources`; on iOS the three commands go straight to it (Tauri forwards a plugin command no Rust
//! handler claims to the native plugin, as lowerCamelCase). Desktop builds get a stub: `pencil_listen`
//! answers `{ ok: false }`, `recognize_text` an empty recognition.

use tauri::{
    Runtime,
    plugin::{Builder, TauriPlugin},
};

#[cfg(desktop)]
mod desktop;
#[cfg(mobile)]
mod mobile;
mod models;

pub use models::*;

/// The plugin: `plugin:ink|…` (218). Register it on the iOS shell's builder.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    let builder = Builder::new("ink");
    #[cfg(desktop)]
    let builder = builder.invoke_handler(tauri::generate_handler![
        desktop::pencil_listen,
        desktop::pencil_unlisten,
        desktop::recognize_text,
    ]);
    builder
        .setup(|_app, _api| {
            #[cfg(mobile)]
            mobile::init(_app, _api)?;
            Ok(())
        })
        .build()
}
