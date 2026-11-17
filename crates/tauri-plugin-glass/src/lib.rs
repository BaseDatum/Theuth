//! Design 198 (Liquid Glass) phase 1: the iPhone's chat chrome as native iOS 26 glass controls floating over the
//! webview — the top bar (drawer button, title pill, ⋯) and the scroll-to-bottom button. All of it is Swift
//! (`ios/Sources`); the web calls the commands as `plugin:glass|…` and Tauri forwards any command this crate does
//! not handle to the registered Swift plugin (`is_supported` → `isSupported`), so there is no Rust command layer.
//! The desktop crate registers this plugin on iOS only; the web reads `is_supported` before using any of it.

use tauri::{
    Runtime,
    plugin::{Builder, TauriPlugin},
};

#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_glass);

/// The `glass` plugin. On iOS, setup registers the Swift `GlassPlugin` (`@_cdecl("init_plugin_glass")`); on any
/// other target it registers nothing and every command is rejected as not found.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("glass")
        .setup(|_app, _api| {
            #[cfg(target_os = "ios")]
            _api.register_ios_plugin(init_plugin_glass)?;
            Ok(())
        })
        .build()
}
