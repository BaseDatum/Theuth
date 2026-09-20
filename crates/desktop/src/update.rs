//! 39 §3.3: the `.app` checks, downloads and verifies in the background and
//! installs at quit, after the sidecar is stopped. Nothing here opens a
//! window; the webview only receives `update-ready`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::time::Duration;

use tauri::plugin::TauriPlugin;
use tauri::{AppHandle, Emitter, Manager, Url, Wry};
use tauri_plugin_updater::{Config, Update, UpdaterExt};

/// A verified update waiting for `RunEvent::Exit` (39 §3.3: bytes stay in memory).
/// `restart`: Settings › General asked to install now; the quit handshake relaunches instead of exiting.
#[derive(Default)]
pub struct Pending {
    pub update: Mutex<Option<(Update, Vec<u8>)>>,
    pub restart: AtomicBool,
}

/// 39 §9: same schedule as the harness.
const FIRST_CHECK: Duration = Duration::from_secs(60);
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const JITTER_MAX_SECS: u64 = 5 * 60;

/// The updater plugin with the 39 §1.7 rule: never downgrade, never cross a major.
pub fn plugin() -> TauriPlugin<Wry, Config> {
    tauri_plugin_updater::Builder::new()
        .default_version_comparator(|current, release| {
            release.version > current && release.version.major == current.major
        })
        .build()
}

/// Spawn the check loop. Debug builds never check (39 §3.2).
pub fn start(app: &AppHandle) {
    if cfg!(debug_assertions) {
        tracing::debug!("update: debug build, not checking");
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            if let Err(e) = check(&app).await {
                tracing::debug!("update: {e}");
            }
            let jitter = Duration::from_secs(rand::random_range(0..=JITTER_MAX_SECS));
            tokio::time::sleep(EVERY + jitter).await;
        }
    });
}

/// One check: manifest → compare → download (minisign-verified) → park in [`Pending`].
/// `Some(version)` when an update is parked (now or from an earlier check), `None` when current.
async fn check(app: &AppHandle) -> anyhow::Result<Option<String>> {
    // 39 §7: `THEUTH_UPDATE_URL` points a scratch bucket; default is tauri.conf.json.
    let builder = match std::env::var("THEUTH_UPDATE_URL")
        .ok()
        .and_then(|s| s.parse::<Url>().ok())
    {
        Some(url) => app.updater_builder().endpoints(vec![url])?,
        None => app.updater_builder(),
    };
    let Some(update) = builder.build()?.check().await? else {
        return Ok(None);
    };
    let version = update.version.clone();
    let held = app
        .state::<Pending>()
        .update
        .lock()
        .is_ok_and(|p| p.as_ref().is_some_and(|(u, _)| u.version == update.version));
    if held {
        return Ok(Some(version));
    }
    // The plugin escalates with an AppleScript password prompt when the bundle
    // is not ours to rename (39 §1.1: no dialogs); a root-owned .app is skipped.
    let app_dir = bundle_dir()?;
    if !writable(&app_dir) {
        anyhow::bail!(
            "{} is not writable, skipping {}",
            app_dir.display(),
            update.version
        );
    }
    let bytes = update.download(|_, _| {}, || {}).await?;
    tracing::info!("update: {} verified, installs at quit", update.version);
    app.emit(
        "update-ready",
        serde_json::json!({ "version": update.version }),
    )?;
    if let Ok(mut p) = app.state::<Pending>().update.lock() {
        *p = Some((update, bytes));
    }
    Ok(Some(version))
}

/// Settings › General › Check for updates: the same check as the loop, on demand.
#[tauri::command]
pub async fn check_update(app: AppHandle) -> Result<Option<String>, String> {
    check(&app).await.map_err(|e| format!("{e:#}"))
}

/// Settings › General › Install and restart: run the quit handshake (RPC `shutdown`,
/// `quit_ack`), swap the bundle in `RunEvent::Exit`, then relaunch.
#[tauri::command]
pub fn install_update(app: AppHandle) -> Result<(), String> {
    let pending = app.state::<Pending>();
    if pending.update.lock().is_ok_and(|p| p.is_none()) {
        return Err("no update downloaded".into());
    }
    pending.restart.store(true, SeqCst);
    crate::desktop::request_quit(&app);
    Ok(())
}

/// `app.exit(0)`, or a relaunch when [`install_update`] asked for one.
pub fn exit(app: &AppHandle) {
    if app.state::<Pending>().restart.load(SeqCst) {
        app.request_restart();
    } else {
        app.exit(0);
    }
}

/// `RunEvent::Exit`, after `Launch::shutdown()`: swap the bundle (39 §3.3).
pub fn install_pending(app: &AppHandle) {
    let pending = app.state::<Pending>();
    let Some((update, bytes)) = pending.update.lock().ok().and_then(|mut p| p.take()) else {
        return;
    };
    match update.install(&bytes) {
        Ok(()) => tracing::info!("update: installed {}", update.version),
        Err(e) => tracing::error!("update: install: {e}"),
    }
}

/// `<x>.app` from `<x>.app/Contents/MacOS/theuth-desktop`.
fn bundle_dir() -> anyhow::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    exe.ancestors()
        .nth(3)
        .filter(|p| p.extension().is_some_and(|e| e == "app"))
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow::anyhow!("{} is not inside a .app", exe.display()))
}

/// The plugin renames `dir` aside and a new one in: needs write on `dir` and its parent.
fn writable(dir: &Path) -> bool {
    use nix::unistd::{AccessFlags, access};
    dir.parent()
        .is_some_and(|parent| access(parent, AccessFlags::W_OK).is_ok())
        && access(dir, AccessFlags::W_OK).is_ok()
}
