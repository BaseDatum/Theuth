//! 39 §3.3: the desktop app checks, downloads and verifies in the background
//! and installs at quit, after the sidecar is stopped. Nothing here opens a
//! window; the webview only receives `update-ready`.

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux", test))]
use std::path::Path;
#[cfg(any(target_os = "macos", target_os = "linux", test))]
use std::path::PathBuf;
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
    verify_install_target(&update.version)?;
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

/// Exit the process so `RunEvent::Exit` runs [`install_pending`].
///
/// - macOS: always `app.exit(0)`; [`install_pending`] spawns a detached post-exit
///   relaunch helper when [`install_update`] asked for a restart, avoiding Tauri's
///   synchronous `open -n` while the old PID is still alive.
/// - Windows: always `app.exit(0)`; [`install_pending`] configures NSIS `/UPDATE`
///   via `restart_after_install`, and `update.install` exits directly.
/// - Linux (and other desktops): `app.request_restart()` when [`install_update`]
///   asked for a restart so Tauri relaunches `$APPIMAGE`, otherwise `app.exit(0)`.
///
/// Any thread: both methods only post to the event loop.
pub fn exit(app: &AppHandle) {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        app.exit(0);
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if app.state::<Pending>().restart.load(SeqCst) {
            app.request_restart();
        } else {
            app.exit(0);
        }
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
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let restart = pending.restart.swap(false, SeqCst);
    #[cfg(target_os = "macos")]
    let bundle = bundle_dir().ok();
    #[cfg(target_os = "windows")]
    let update = update.restart_after_install(restart);
    crate::companion::terminate();
    match update.install(&bytes) {
        Ok(()) => {
            tracing::info!("update: installed {}", update.version);
            #[cfg(target_os = "macos")]
            if restart {
                match bundle {
                    Some(bundle) => relaunch_macos_after_exit(&bundle),
                    None => tracing::error!(
                        "update: cannot relaunch after install — bundle path unavailable"
                    ),
                }
            }
        }
        Err(e) => tracing::error!("update: install: {e}"),
    }
}

/// Verify that the current installation can be replaced in place before downloading.
#[cfg(target_os = "macos")]
fn verify_install_target(version: &str) -> anyhow::Result<()> {
    // The plugin escalates with an AppleScript password prompt when the bundle
    // is not ours to rename (39 §1.1: no dialogs); a root-owned .app is skipped.
    let app_dir = bundle_dir()?;
    if !bundle_writable(&app_dir) {
        anyhow::bail!("{} is not writable, skipping {version}", app_dir.display());
    }
    Ok(())
}

/// Verify that the current installation can be replaced in place before downloading.
#[cfg(target_os = "windows")]
fn verify_install_target(version: &str) -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    verify_windows_install_target(&exe, version, platform::files::writable)
}

/// Verify that the current installation can be replaced in place before downloading.
#[cfg(target_os = "linux")]
fn verify_install_target(version: &str) -> anyhow::Result<()> {
    // Inside an AppImage, `current_exe()` resolves inside the read-only FUSE mount
    // (`/tmp/.mount_...`), so inspect `$APPIMAGE` directly instead.
    let appimage = std::env::var_os("APPIMAGE");
    verify_linux_appimage_target(appimage.as_deref(), version, platform::files::writable)
}

/// Fallback for other desktop targets without an in-app update target.
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn verify_install_target(version: &str) -> anyhow::Result<()> {
    anyhow::bail!("in-app updates are not supported on this platform, skipping {version}");
}

/// `<x>.app` from `<x>.app/Contents/MacOS/theuth-desktop`.
#[cfg(target_os = "macos")]
fn bundle_dir() -> anyhow::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    bundle_dir_from(&exe)
}

#[cfg(any(target_os = "macos", test))]
fn bundle_dir_from(exe: &Path) -> anyhow::Result<PathBuf> {
    exe.ancestors()
        .nth(3)
        .filter(|p| p.extension().is_some_and(|e| e == "app"))
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow::anyhow!("{} is not inside a .app", exe.display()))
}

/// The plugin renames `dir` aside and a new one in: needs write on `dir` and its parent.
#[cfg(target_os = "macos")]
fn bundle_writable(dir: &Path) -> bool {
    dir.parent().is_some_and(platform::files::writable) && platform::files::writable(dir)
}

#[cfg(any(target_os = "windows", test))]
fn verify_windows_install_target(
    exe: &Path,
    version: &str,
    is_writable: impl Fn(&Path) -> bool,
) -> anyhow::Result<()> {
    let install_dir = exe
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| anyhow::anyhow!("{} has no parent directory", exe.display()))?;
    if !is_writable(install_dir) {
        anyhow::bail!(
            "{} is not writable, skipping {version}",
            install_dir.display()
        );
    }
    Ok(())
}

#[cfg(any(target_os = "linux", test))]
fn verify_linux_appimage_target(
    appimage: Option<&std::ffi::OsStr>,
    version: &str,
    is_writable: impl Fn(&Path) -> bool,
) -> anyhow::Result<()> {
    let Some(raw) = appimage.filter(|s| !s.is_empty()) else {
        anyhow::bail!(
            "Installed outside an AppImage — update through your package manager (apt/dnf) or download the latest package"
        );
    };
    let path = PathBuf::from(raw);
    if !path.is_file() {
        anyhow::bail!(
            "AppImage {} does not exist, skipping {version}",
            path.display()
        );
    }
    let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) else {
        anyhow::bail!(
            "AppImage {} has no parent directory, skipping {version}",
            path.display()
        );
    };
    if !is_writable(parent) || !is_writable(&path) {
        anyhow::bail!("{} is not writable, skipping {version}", path.display());
    }
    Ok(())
}

/// Detached macOS relaunch helper script: wait for the exiting PID to disappear (up to 10s),
/// re-register the newly swapped `.app` with LaunchServices, then `open` without `-n` or
/// forwarded `--args`/`--autostart` so LaunchServices restores the pinned Dock tile and a
/// clean TCC responsible-process identity.
#[cfg(any(target_os = "macos", test))]
const MACOS_RELAUNCH_SCRIPT: &str = concat!(
    r#"pid="$1"; app="$2"; i=0; "#,
    r#"while kill -0 "$pid" 2>/dev/null && [ "$i" -lt 200 ]; do sleep 0.05; i=$((i + 1)); done; "#,
    r#"/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$app" >/dev/null 2>&1 || true; "#,
    r#"exec /usr/bin/open "$app""#,
);

#[cfg(any(target_os = "macos", test))]
fn macos_relaunch_command(pid: u32, bundle: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(MACOS_RELAUNCH_SCRIPT)
        .arg("theuth-relaunch")
        .arg(pid.to_string())
        .arg(bundle.as_os_str());
    cmd
}

#[cfg(target_os = "macos")]
fn relaunch_macos_after_exit(bundle: &Path) {
    use std::os::unix::process::CommandExt as _;
    use std::process::Stdio;

    let mut cmd = macos_relaunch_command(std::process::id(), bundle);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: `setsid` is async-signal-safe and touches no parent state; putting the helper in
    // its own session lets it outlive this exiting process cleanly.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    match cmd.spawn().map(drop) {
        Ok(()) => {
            tracing::info!(
                bundle = %bundle.display(),
                "update: scheduled post-exit macOS relaunch"
            );
        }
        Err(e) => {
            tracing::error!(
                bundle = %bundle.display(),
                "update: failed to spawn macOS relaunch helper: {e}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("desktop-update-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn bundle_dir_from_extracts_app_bundle() {
        assert_eq!(
            bundle_dir_from(Path::new(
                "/Applications/theuth.app/Contents/MacOS/theuth-desktop"
            ))
            .unwrap(),
            PathBuf::from("/Applications/theuth.app")
        );
        assert_eq!(
            bundle_dir_from(Path::new(
                "/Users/sky/Applications/theuth beta.app/Contents/MacOS/theuth-desktop"
            ))
            .unwrap(),
            PathBuf::from("/Users/sky/Applications/theuth beta.app")
        );
        let err = bundle_dir_from(Path::new("/usr/bin/theuth-desktop")).unwrap_err();
        assert!(err.to_string().contains("not inside a .app"), "{err}");
        assert!(
            bundle_dir_from(Path::new("/Applications/theuth.app/theuth-desktop")).is_err(),
            "wrong nesting depth must fail"
        );
    }

    #[test]
    fn verify_windows_install_target_checks_parent_writability() {
        let exe = Path::new("/Program Files/theuth/theuth-desktop.exe");
        assert!(
            verify_windows_install_target(exe, "2.33.0", |d| d
                == Path::new("/Program Files/theuth"))
            .is_ok()
        );

        let err = verify_windows_install_target(exe, "2.33.0", |_| false).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("theuth"), "{msg}");
        assert!(msg.contains("2.33.0"), "{msg}");
        assert!(msg.contains("not writable"), "{msg}");

        let no_parent =
            verify_windows_install_target(Path::new("theuth-desktop.exe"), "2.33.0", |_| true)
                .unwrap_err();
        assert!(
            no_parent.to_string().contains("no parent directory"),
            "{no_parent}"
        );
    }

    #[test]
    fn verify_linux_appimage_target_rejects_non_appimage() {
        for value in [None, Some(OsStr::new(""))] {
            let err = verify_linux_appimage_target(value, "2.33.0", |_| true).unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("AppImage"), "{msg}");
            assert!(!msg.contains(".app"), "must not mention macOS .app: {msg}");
        }
    }

    #[test]
    fn verify_linux_appimage_target_validates_existence_and_writability() {
        let dir = scratch("appimage");
        std::fs::create_dir_all(&dir).unwrap();
        let appimage = dir.join("theuth.AppImage");

        // Missing file fails before download.
        let missing = verify_linux_appimage_target(Some(appimage.as_os_str()), "2.33.0", |_| true)
            .unwrap_err();
        assert!(missing.to_string().contains("does not exist"), "{missing}");
        assert!(missing.to_string().contains("2.33.0"), "{missing}");

        // A directory path is rejected as not a file.
        let is_dir =
            verify_linux_appimage_target(Some(dir.as_os_str()), "2.33.0", |_| true).unwrap_err();
        assert!(is_dir.to_string().contains("does not exist"), "{is_dir}");

        std::fs::write(&appimage, b"fake-appimage").unwrap();

        // Both parent and file writable -> ok.
        assert!(
            verify_linux_appimage_target(Some(appimage.as_os_str()), "2.33.0", |_| true).is_ok()
        );

        // Parent not writable -> error.
        let err = verify_linux_appimage_target(Some(appimage.as_os_str()), "2.33.0", |p| {
            p != dir.as_path()
        })
        .unwrap_err();
        assert!(err.to_string().contains("not writable"), "{err}");
        assert!(err.to_string().contains("2.33.0"), "{err}");

        // AppImage file itself not writable -> error.
        let err = verify_linux_appimage_target(Some(appimage.as_os_str()), "2.33.0", |p| {
            p != appimage.as_path()
        })
        .unwrap_err();
        assert!(err.to_string().contains("not writable"), "{err}");
        assert!(err.to_string().contains("2.33.0"), "{err}");

        // On Unix, verify against the real `platform::files::writable` (`libc::access(W_OK)`).
        #[cfg(unix)]
        assert!(
            verify_linux_appimage_target(
                Some(appimage.as_os_str()),
                "2.33.0",
                platform::files::writable,
            )
            .is_ok()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn macos_relaunch_command_uses_positional_args_without_open_n() {
        let bundle = Path::new("/Applications/theuth's \"beta\" $HOME.app");
        let cmd = macos_relaunch_command(4242, bundle);
        assert_eq!(cmd.get_program(), OsStr::new("/bin/sh"));

        let args: Vec<&OsStr> = cmd.get_args().collect();
        assert_eq!(args.len(), 5);
        assert_eq!(args[0], OsStr::new("-c"));
        let script = args[1].to_str().expect("UTF-8 script");
        assert!(script.contains(r#"lsregister -f "$app""#), "{script}");
        assert!(script.contains(r#"exec /usr/bin/open "$app""#), "{script}");
        assert!(!script.contains("open -n"), "must not pass -n: {script}");
        assert!(
            !script.contains("--autostart") && !script.contains("--args"),
            "must not forward launch args: {script}"
        );
        assert!(
            !script.contains("theuth's"),
            "bundle path must never be interpolated into shell code: {script}"
        );
        assert_eq!(args[2], OsStr::new("theuth-relaunch"));
        assert_eq!(args[3], OsStr::new("4242"));
        assert_eq!(args[4], bundle.as_os_str());
    }
}
