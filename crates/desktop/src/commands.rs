//! `#[tauri::command]`s the webview invokes. Everything that talks to the daemon
//! goes over the WebSocket instead (25 §0.4); these are launch, transcript and
//! the quit handshake. Attachments go through RPC `put_attachment` (31 §9) so
//! every container shares one path.

#[cfg(desktop)]
use tauri::{AppHandle, Manager, State};

#[cfg(desktop)]
use crate::launch::{Connection, Launch};

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
