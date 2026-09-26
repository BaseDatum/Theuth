//! `#[tauri::command]`s the webview invokes. Everything that talks to the daemon
//! goes over the WebSocket instead (25 §0.4); these are launch, transcript and
//! the quit handshake. Attachments go through RPC `put_attachment` (31 §9) so
//! every container shares one path.

#[cfg(desktop)]
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};

#[cfg(desktop)]
use crate::launch::{Connection, Launch};

/// Window labels the UI opens on demand (a chat or an artifact in its own window). The label prefix is what
/// `capabilities/default.json` grants (`popout-*`) and what `on_window_event` lets close for real.
#[cfg(desktop)]
pub const POPOUT_PREFIX: &str = "popout-";

/// Open `url` (an app route with `?window=<label>`) in its own window, or focus the window already showing it.
/// Size and position come back per label from `tauri-plugin-window-state` once the window was closed once.
#[cfg(desktop)]
#[tauri::command]
pub fn open_window(app: AppHandle, label: String, url: String) -> Result<(), String> {
    if !label.starts_with(POPOUT_PREFIX) {
        return Err(format!("window label must start with `{POPOUT_PREFIX}`"));
    }
    if let Some(win) = app.get_webview_window(&label) {
        win.show().map_err(|e| e.to_string())?;
        return win.set_focus().map_err(|e| e.to_string());
    }
    // Tauri's file-drop interception swallows every in-page HTML5 drop in WKWebView (BLOCKNOTE-DRAG); the main
    // window turns it off in tauri.conf.json (`dragDropEnabled: false`), pop-outs here.
    WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(url.into()))
        .disable_drag_drop_handler()
        .inner_size(960.0, 720.0)
        .min_inner_size(480.0, 360.0)
        .on_document_title_changed(|win, title| {
            let _ = win.set_title(&title);
        })
        .build()
        .map(drop)
        .map_err(|e| e.to_string())
}

/// Connect (probe, else spawn on a pty and wait for the port); cached after the
/// first success. Blocks up to 15 s on a cold start.
#[cfg(desktop)]
#[tauri::command]
pub async fn connection(app: AppHandle) -> Result<Connection, String> {
    blocking(move || app.state::<Launch>().connection(&app)).await
}

/// No local daemon on this platform: the UI's `invoke('connection')` fails
/// cleanly and it uses the paired harnesses instead (31 §7). Same command name
/// so the frontend does not branch on the container.
#[cfg(mobile)]
#[tauri::command]
pub fn connection() -> Result<(), String> {
    Err("no local daemon on this platform".into())
}

/// The pty transcript so far; `launch-log` events carry the increments.
#[cfg(desktop)]
#[tauri::command]
pub fn launch_log(launch: State<'_, Launch>) -> String {
    launch.log()
}

/// Stop the daemon we own (if any) and connect again.
#[cfg(desktop)]
#[tauri::command]
pub async fn relaunch(app: AppHandle) -> Result<Connection, String> {
    blocking(move || app.state::<Launch>().relaunch(&app)).await
}

/// The webview finished its part of the quit sequence (RPC `shutdown` when it
/// owned the daemon); exit (or relaunch for an update install) now instead of at the 3 s fallback.
#[cfg(desktop)]
#[tauri::command]
pub fn quit_ack(app: AppHandle) {
    crate::update::exit(&app);
}

#[cfg(desktop)]
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))
}
