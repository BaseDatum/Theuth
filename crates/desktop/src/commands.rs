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
/// Notes (and a meeting's note pane) open portrait, 880×1000 — the 800 px sheet plus the canvas padding, and a
/// height most laptop screens still fit; everything else opens 960×720.
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
    // Background: the page colour for the system appearance (DARK-FLASH), read off main since this window
    // does not exist yet; `ThemeChanged` keeps it current afterwards.
    let dark = app
        .get_webview_window("main")
        .and_then(|w| w.theme().ok())
        .is_some_and(|t| t == tauri::Theme::Dark);
    let (w, h): (f64, f64) =
        if label.starts_with("popout-note-") || label.starts_with("popout-meeting-note-") {
            (880.0, 1000.0)
        } else if label.starts_with("popout-notebook-") {
            (1100.0, 720.0) // 144 wave 2: three columns, landscape; the OS clamps to the screen
        } else {
            (960.0, 720.0)
        };
    WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(url.into()))
        .background_color(crate::page_background(dark))
        .disable_drag_drop_handler()
        .inner_size(w, h)
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

/// The webview finished its part of the quit sequence (RPC `shutdown` when it owned the daemon): start the
/// quit worker now instead of at the 3 s fallback (163). Returns at once; the worker exits the app.
#[cfg(desktop)]
#[tauri::command]
pub fn quit_ack(app: AppHandle) {
    crate::quit::start(&app);
}

/// 210 §7.4 item 2: the webview's live public-share count; the tray menu shows "Stop public sharing (N)" while it
/// is above zero. Its click comes back as the `share-stop-all` event (the webview owns the RPC connection).
#[cfg(desktop)]
#[tauri::command]
pub fn share_tray(app: AppHandle, live: u32) -> Result<(), String> {
    crate::desktop::set_sharing(&app, live).map_err(|e| e.to_string())
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
