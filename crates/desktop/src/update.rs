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

/// Fallback manifest URLs tried in order on download failure (mirrors `update::DEFAULT_URLS`
/// after GitHub and entries 2–3 of `tauri.conf.json`: Wasabi S3 first, then self-hosted S3).
const FALLBACK_URLS: &[&str] = &[
    "https://s3.us-central-1.wasabisys.com/theuth-releases/latest.json",
    "https://s3.rwx.dev/theuth/releases/latest.json",
];

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

fn is_held(app: &AppHandle, version: &str) -> bool {
    app.state::<Pending>()
        .update
        .lock()
        .is_ok_and(|p| p.as_ref().is_some_and(|(u, _)| u.version == version))
}

/// One check: manifest → compare → download (minisign-verified) → park in [`Pending`].
/// `Some(version)` when an update is parked (now or from an earlier check), `None` when current.
async fn check(app: &AppHandle) -> anyhow::Result<Option<String>> {
    // 39 §7: `THEUTH_UPDATE_URL` points a scratch bucket; default is tauri.conf.json.
    let override_url = std::env::var("THEUTH_UPDATE_URL")
        .ok()
        .and_then(|s| s.parse::<Url>().ok());
    let has_override = override_url.is_some();
    let builder = match override_url {
        Some(url) => app.updater_builder().endpoints(vec![url])?,
        None => app.updater_builder(),
    };
    let Some(mut update) = builder.build()?.check().await? else {
        return Ok(None);
    };
    if is_held(app, &update.version) {
        return Ok(Some(update.version));
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
    let bytes = match update.download(|_, _| {}, || {}).await {
        Ok(bytes) => bytes,
        Err(primary_err) => {
            if has_override {
                return Err(primary_err.into());
            }
            // Start after whichever fallback host already served the failed download,
            // preserving strict order: GitHub -> Wasabi S3 -> self-hosted S3.
            let failed_host = update.download_url.host_str().map(str::to_owned);
            let start_idx = FALLBACK_URLS
                .iter()
                .position(|raw| {
                    raw.parse::<Url>()
                        .ok()
                        .and_then(|u| u.host_str().map(str::to_owned))
                        == failed_host
                })
                .map_or(0, |i| i + 1);
            let remaining = &FALLBACK_URLS[start_idx..];
            if remaining.is_empty() {
                return Err(primary_err.into());
            }

            let mut fallback_errors = vec![format!("{}: {primary_err}", update.download_url)];
            let mut recovered: Option<(Update, Vec<u8>)> = None;

            for &fallback_str in remaining {
                let fallback_url: Url = fallback_str.parse()?;
                tracing::warn!(
                    "update: download of {} failed; falling back to {fallback_str}",
                    update.version
                );
                let fallback_update = match app
                    .updater_builder()
                    .endpoints(vec![fallback_url])?
                    .build()?
                    .check()
                    .await
                {
                    Ok(Some(u)) => u,
                    Ok(None) => {
                        fallback_errors.push(format!("{fallback_str}: reported no update"));
                        continue;
                    }
                    Err(e) => {
                        fallback_errors.push(format!("{fallback_str}: check failed: {e}"));
                        continue;
                    }
                };
                if is_held(app, &fallback_update.version) {
                    return Ok(Some(fallback_update.version));
                }
                match fallback_update.download(|_, _| {}, || {}).await {
                    Ok(fallback_bytes) => {
                        recovered = Some((fallback_update, fallback_bytes));
                        break;
                    }
                    Err(e) => {
                        fallback_errors.push(format!(
                            "{}: download failed: {e}",
                            fallback_update.download_url
                        ));
                    }
                }
            }

            let Some((fallback_update, fallback_bytes)) = recovered else {
                anyhow::bail!(
                    "all update download endpoints failed: {}",
                    fallback_errors.join("; ")
                );
            };
            update = fallback_update;
            fallback_bytes
        }
    };
    let version = update.version.clone();
    tracing::info!("update: {version} verified, installs at quit");
    app.emit("update-ready", serde_json::json!({ "version": version }))?;
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

/// Settings › General › Install and restart: the same quit as the menu's (163 D7: overlay titled "Restarting to
/// update", RPC `shutdown`, the quit worker), the bundle swap in `RunEvent::Exit`, then the relaunch.
#[tauri::command]
pub fn install_update(app: AppHandle) -> Result<(), String> {
    let pending = app.state::<Pending>();
    if pending.update.lock().is_ok_and(|p| p.is_none()) {
        return Err("no update downloaded".into());
    }
    pending.restart.store(true, SeqCst);
    crate::quit::request(&app);
    Ok(())
}

/// `app.exit(0)`, or a relaunch when [`install_update`] asked for one. Any thread: both only post to the event loop.
pub fn exit(app: &AppHandle) {
    if app.state::<Pending>().restart.load(SeqCst) {
        app.request_restart();
    } else {
        app.exit(0);
    }
}

/// `RunEvent::Exit`, after `Launch::shutdown()`: swap the bundle (39 §3.3). 134 D10: the companion goes
/// first — on Windows `install` starts the NSIS setup (`/UPDATE`, in-place copy over `companion\`) and
/// `process::exit(0)`s, and a running `Theuth.Companion.exe` would lock every file under it. Idempotent
/// after the `RunEvent::Exit` call.
pub fn install_pending(app: &AppHandle) {
    let pending = app.state::<Pending>();
    let Some((update, bytes)) = pending.update.lock().ok().and_then(|mut p| p.take()) else {
        return;
    };
    crate::companion::terminate();
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
    dir.parent().is_some_and(platform::files::writable) && platform::files::writable(dir)
}
