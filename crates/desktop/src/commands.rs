//! `#[tauri::command]`s the webview invokes. Everything that talks to the daemon
//! goes over the WebSocket instead (25 §0.4); these are launch, transcript,
//! attachment writes and the quit handshake.

use sha2::{Digest, Sha256};
use tauri::ipc::{InvokeBody, Request};
use tauri::{AppHandle, Manager, State};

use crate::launch::{self, Connection, Launch};

/// Connect (probe, else spawn on a pty and wait for the port); cached after the
/// first success. Blocks up to 15 s on a cold start.
#[tauri::command]
pub async fn connection(app: AppHandle) -> Result<Connection, String> {
    blocking(move || app.state::<Launch>().connection(&app)).await
}

/// The pty transcript so far; `launch-log` events carry the increments.
#[tauri::command]
pub fn launch_log(launch: State<'_, Launch>) -> String {
    launch.log()
}

/// Stop the daemon we own (if any) and connect again.
#[tauri::command]
pub async fn relaunch(app: AppHandle) -> Result<Connection, String> {
    blocking(move || app.state::<Launch>().relaunch(&app)).await
}

/// Write a pasted/dropped file to `<data_dir>/attachments/<sha256[..16]>.<ext>`
/// (the `rpc/acp.rs` convention) and return its absolute path for `send_input`.
/// Invoke with a binary body and an `ext` header:
/// `invoke('save_attachment', bytes, { headers: { ext: 'png' } })`.
#[tauri::command]
pub fn save_attachment(request: Request<'_>) -> Result<String, String> {
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err("save_attachment: expected a binary body".into());
    };
    let ext = request
        .headers()
        .get("ext")
        .and_then(|v| v.to_str().ok())
        .ok_or("save_attachment: missing `ext` header")?;
    if !matches!(ext, "png" | "jpg" | "jpeg" | "gif" | "webp" | "pdf") {
        return Err(format!("save_attachment: unsupported type `{ext}`"));
    }
    let hex = format!("{:x}", Sha256::digest(bytes));
    let dir = launch::data_dir().join("attachments");
    let path = dir.join(format!("{}.{ext}", &hex[..16]));
    if !path.exists() {
        std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(&path, bytes))
            .map_err(|e| format!("save_attachment: {e}"))?;
    }
    Ok(path.to_string_lossy().into_owned())
}

/// The webview finished its part of the quit sequence (RPC `shutdown` when it
/// owned the daemon); exit now instead of at the 3 s fallback.
#[tauri::command]
pub fn quit_ack(app: AppHandle) {
    app.exit(0);
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))
}
