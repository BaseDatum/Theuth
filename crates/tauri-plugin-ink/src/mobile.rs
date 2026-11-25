//! iOS: the Swift `InkPlugin` (`ios/Sources`) is registered with Tauri; the three commands reach it
//! directly from JS. Android has no native side: its commands fail and the UI goes without Pencil gestures.

use serde::de::DeserializeOwned;
use tauri::{AppHandle, Runtime, plugin::PluginApi};

#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_ink);

/// Registers the Swift plugin, which installs its Pencil interaction on `pencil_listen`.
pub(crate) fn init<R: Runtime, C: DeserializeOwned>(
    _app: &AppHandle<R>,
    api: PluginApi<R, C>,
) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "ios")]
    api.register_ios_plugin(init_plugin_ink)?;
    #[cfg(not(target_os = "ios"))]
    let _ = api;
    Ok(())
}
