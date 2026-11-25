//! 218 §6: the plugin's permissions (one `allow-<command>` each, `ink:default` allows all three) and, for
//! iOS, the Swift package in `ios/` linked into the app (`tauri_utils::build::link_apple_library`).

/// The commands JS may call as `plugin:ink|<command>`. On iOS they run in Swift (`InkPlugin`, the
/// lowerCamelCase method of the same name); on desktop in `src/desktop.rs`.
const COMMANDS: &[&str] = &["pencil_listen", "pencil_unlisten", "recognize_text"];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .global_api_script_path("./api-iife.js")
        .ios_path("ios")
        .try_build()
        .expect("tauri-plugin-ink build");
}
