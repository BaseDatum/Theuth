//! `#[tauri::command]`s the webview invokes. Everything that talks to the daemon
//! goes over the WebSocket instead (25 §0.4); these are launch, transcript and
//! the quit handshake. Attachments go through RPC `put_attachment` (31 §9) so
//! every container shares one path.

#[cfg(any(desktop, target_os = "ios"))]
use std::path::PathBuf;

#[cfg(any(desktop, target_os = "ios"))]
use serde::Serialize;
#[cfg(any(desktop, target_os = "ios"))]
use tauri::{AppHandle, Manager};
#[cfg(desktop)]
use tauri::{State, WebviewUrl, WebviewWindowBuilder};

#[cfg(desktop)]
use crate::launch::{Launch, StartupProgress};

/// What the webview needs to open the WebSocket (18 §2 step 5; Phase 2 iOS embedded engine).
#[cfg(any(desktop, target_os = "ios"))]
#[derive(Clone, Serialize)]
pub struct Connection {
    /// `ws://<listen>/?token=<token>`.
    pub url: String,
    /// Contents of `<data_dir>/token`.
    pub token: String,
    /// The daemon's data directory (attachments are written under it).
    pub data_dir: PathBuf,
    /// True when this app started the daemon and owns its lifecycle.
    pub owned: bool,
}

/// 221 §9.1 Copy (`ui/src/lib/secret-clipboard.ts`): the general pasteboard's change count right after the
/// webview wrote a secret to it; `None` off macOS (the webview then compares the text itself).
#[cfg(desktop)]
#[tauri::command]
pub fn pasteboard_change_count() -> Option<i64> {
    #[cfg(target_os = "macos")]
    {
        Some(objc2_app_kit::NSPasteboard::generalPasteboard().changeCount() as i64)
    }
    #[cfg(not(target_os = "macos"))]
    None
}

/// Clear the general pasteboard when nothing was copied since [`pasteboard_change_count`] read `count`:
/// the secret is still on it. `false` when something else was copied since (left alone) or off macOS.
#[cfg(desktop)]
#[tauri::command]
pub fn pasteboard_clear_if(count: i64) -> bool {
    #[cfg(target_os = "macos")]
    {
        let board = objc2_app_kit::NSPasteboard::generalPasteboard();
        if board.changeCount() as i64 != count {
            return false;
        }
        board.clearContents();
        true
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = count;
        false
    }
}

/// Window labels the UI opens on demand (a chat or an artifact in its own window). The label prefix is what
/// `capabilities/default.json` grants (`popout-*`) and what `on_window_event` lets close for real.
#[cfg(desktop)]
pub const POPOUT_PREFIX: &str = "popout-";

/// What every window but main is called after: `"<content> — theuth"`.
#[cfg(desktop)]
const APP_NAME: &str = "theuth";

/// The native title (title bar, Window menu, Dock menu, Windows taskbar) for a window showing `content`: `None` while
/// there is nothing to name yet — an empty `document.title`, or the URL WebView2 reports in place of a missing one —
/// so the window keeps the title it has rather than falling back to Tauri's default ("Tauri App").
#[cfg(desktop)]
pub(crate) fn window_title(content: &str) -> Option<String> {
    let content = content.trim();
    let url = content.contains("window=popout-")
        || [
            "tauri://",
            "http://tauri.localhost",
            "https://tauri.localhost",
            "tauri.localhost",
            "http://localhost:1420",
        ]
        .iter()
        .any(|p| content.starts_with(p));
    if content.is_empty() || url {
        return None;
    }
    if content == APP_NAME || content.ends_with(&format!(" — {APP_NAME}")) {
        return Some(content.to_owned());
    }
    Some(format!("{content} — {APP_NAME}"))
}

/// Open `url` (an app route with `?window=<label>`) in its own window, or focus the window already showing it.
/// Size and position come back per label from `tauri-plugin-window-state` once the window was closed once.
/// Notes (and a meeting's note pane) open portrait, 880×1000 — the 800 px sheet plus the canvas padding, and a
/// height most laptop screens still fit; everything else opens 960×720.
/// `title` (`popoutTitle` in `ui/src/lib/popout.ts`) names the window until its page sets `document.title`, which
/// it follows from then on; both go through [`window_title`].
#[cfg(desktop)]
#[tauri::command]
pub fn open_window(
    app: AppHandle,
    label: String,
    url: String,
    title: Option<String>,
) -> Result<(), String> {
    if !label.starts_with(POPOUT_PREFIX) {
        return Err(format!("window label must start with `{POPOUT_PREFIX}`"));
    }
    if let Some(win) = app.get_webview_window(&label) {
        let _ = win.unminimize();
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
    let initial = title
        .as_deref()
        .and_then(window_title)
        .unwrap_or_else(|| APP_NAME.to_owned());
    WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(url.into()))
        .title(initial)
        .background_color(crate::page_background(dark))
        .disable_drag_drop_handler()
        .inner_size(w, h)
        .min_inner_size(480.0, 360.0)
        .on_document_title_changed(|win, title| {
            if let Some(title) = window_title(&title) {
                let _ = win.set_title(&title);
            }
        })
        .build()
        .map(drop)
        .map_err(|e| e.to_string())
}

/// Connect (probe, else spawn on a pty and wait for the port); cached after the
/// first success. 222: no fixed cap while the engine reports progress; a start
/// already in flight is joined, never restarted.
#[cfg(desktop)]
#[tauri::command]
pub async fn connection(app: AppHandle) -> Result<Connection, String> {
    blocking(move || app.state::<Launch>().connection(&app)).await
}

/// 222: the last `startup-progress` payload (`{status, alive, stalled, waited_ms}`), for a
/// start screen that mounts after the start began.
#[cfg(desktop)]
#[tauri::command]
pub fn startup_status(launch: State<'_, Launch>) -> StartupProgress {
    launch.startup_status()
}

/// Connect to the in-process embedded iOS engine (starting it lazily if needed) and return its
/// loopback WebSocket URL, token, and data directory.
#[cfg(target_os = "ios")]
#[tauri::command]
pub async fn connection(app: AppHandle) -> Result<Connection, String> {
    app.state::<crate::ios_engine::IosEngine>()
        .connection(&app)
        .await
}

/// No local daemon on Android: the UI's `invoke('connection')` fails
/// cleanly and it uses the paired harnesses instead (31 §7). Same command name
/// so the frontend does not branch on the container.
#[cfg(target_os = "android")]
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

/// 221 D22/B10: the files the bundle carries in its resources (`tauri.conf.json` › `bundle.resources`).
#[cfg(desktop)]
const LEGAL_FILES: [&str; 3] = ["LICENSE", "NOTICE", "THIRD-PARTY-NOTICES"];

/// The text of one bundled legal file (`LICENSE`, `NOTICE`, `THIRD-PARTY-NOTICES`), for Settings › App ›
/// Licences. Any other name is refused; a dev build without the files says so.
#[cfg(desktop)]
#[tauri::command]
pub async fn legal_text(app: AppHandle, name: String) -> Result<String, String> {
    if !LEGAL_FILES.contains(&name.as_str()) {
        return Err(format!("{name} is not a bundled legal file"));
    }
    let path = app
        .path()
        .resource_dir()
        .map_err(|e| e.to_string())?
        .join(&name);
    tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Stop the daemon we own (if any) and connect again. 222: an engine still making progress is
/// left alone unless `force` — the error is `starting: <label>`.
#[cfg(desktop)]
#[tauri::command]
pub async fn relaunch(app: AppHandle, force: Option<bool>) -> Result<Connection, String> {
    let force = force.unwrap_or(false);
    blocking(move || app.state::<Launch>().relaunch(&app, force)).await
}

/// The webview finished its part of the quit sequence (RPC `shutdown` when it owned the daemon): start the
/// quit worker now instead of at the 3 s fallback (163). Returns at once; the worker exits the app.
#[cfg(desktop)]
#[tauri::command]
pub fn quit_ack(app: AppHandle) {
    crate::quit::start(&app);
}

/// 210 §7.4 item 2: the webview's live public-share count; the tray menu shows "Stop public sharing (N)" while it
/// is above zero. Its click is handled in the shell (`share::stop_all`), without the webview.
#[cfg(desktop)]
#[tauri::command]
pub fn share_tray(app: AppHandle, live: u32) -> Result<(), String> {
    crate::desktop::set_sharing(&app, live).map_err(|e| e.to_string())
}

/// 210 §7.4 item 6: the island's `● LIVE N · Stop` — the tray's native Stop all ([`crate::share::stop_all`]), so the
/// outcome (notification, `share-stopped-all`) is the same and no webview's connection is involved.
#[cfg(desktop)]
#[tauri::command]
pub fn share_stop_all(app: AppHandle) {
    crate::share::stop_all(&app);
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

#[cfg(all(test, desktop))]
mod tests {
    use super::window_title;

    #[test]
    fn a_window_is_named_after_its_content_then_the_app() {
        assert_eq!(
            window_title("Projects").as_deref(),
            Some("Projects — theuth")
        );
        assert_eq!(
            window_title("  fix the build  ").as_deref(),
            Some("fix the build — theuth")
        );
        // Already named: not twice.
        assert_eq!(
            window_title("Notes — theuth").as_deref(),
            Some("Notes — theuth")
        );
        assert_eq!(window_title("theuth").as_deref(), Some("theuth"));
    }

    #[test]
    fn nothing_to_name_keeps_the_current_title() {
        assert_eq!(window_title(""), None);
        assert_eq!(window_title("   "), None);
        // WebView2 reports the URL while the page has no `<title>`.
        assert_eq!(
            window_title("http://tauri.localhost/c/abc?window=popout-chat-abc"),
            None
        );
        assert_eq!(window_title("tauri.localhost/notes/d1"), None);
        assert_eq!(window_title("tauri://localhost/c/abc"), None);
    }
}
