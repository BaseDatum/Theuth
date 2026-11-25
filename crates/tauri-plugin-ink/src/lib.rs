//! 218 §6: native Apple Pencil capture for the whiteboard.
//!
//! On iPadOS 26 a capture-only `PKCanvasView` sits above the main web view: the Pencil draws there with
//! PencilKit's latency, and at pen-idle the plugin hands the strokes to JS (`onStrokes`), which commits
//! them to the whiteboard's Y.Doc and acknowledges them; acknowledged strokes leave the overlay. The
//! system tool picker, Pencil double-tap and squeeze, and Vision handwriting recognition come with it.
//! The Swift side is `ios/Sources`; on iOS the eight commands go straight to it (Tauri forwards a plugin
//! command no Rust handler claims to the native plugin, as lowerCamelCase). Desktop builds get a stub:
//! `attach` answers `{ native: false, reason: "desktop" }`, the other commands do nothing.

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

/// The plugin: `plugin:ink|…` (218 §6). Register it on the iOS shell's builder.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    let builder = Builder::new("ink");
    #[cfg(desktop)]
    let builder = builder.invoke_handler(tauri::generate_handler![
        desktop::attach,
        desktop::detach,
        desktop::set_enabled,
        desktop::set_viewport,
        desktop::set_tool,
        desktop::set_picker_visible,
        desktop::ack,
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
