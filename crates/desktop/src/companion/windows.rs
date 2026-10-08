//! Windows (134 D10, §5.5–5.6): launch `<exe dir>\companion\Theuth.Companion.exe`, follow it by the
//! named mutex it holds, hand the notification area over (D9), and stop it at quit — the Windows form
//! of `macos.rs`. Alive = `Local\io.theuth.companion` can be opened; the companion is the only writer.
//! Terminate = `WM_CLOSE` to its rail window class, [`QUIT_GRACE`], then `TerminateProcess`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows_sys::Win32::System::Threading::{
    OpenMutexW, OpenProcess, PROCESS_TERMINATE, SYNCHRONIZATION_SYNCHRONIZE, TerminateProcess,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetWindowThreadProcessId, PostMessageW, WM_CLOSE,
};
use windows_sys::core::{PCWSTR, w};

use super::{ALIVE_EVENT, Alive};

/// `Local\io.theuth.companion`: created by the companion at start, released by the kernel at exit (§5.5).
const MUTEX: PCWSTR = w!("Local\\io.theuth.companion");
/// The rail's top-level window class; `WM_CLOSE` here is the companion's quit (§5.5).
const RAIL_CLASS: PCWSTR = w!("TheuthCompanionRail");
/// Where `package-windows.ps1` puts the publish folder (a Tauri resource dir, §5.7), relative to the exe.
const NESTED: &str = "companion";
const EXE: &str = "Theuth.Companion.exe";
/// `cargo tauri dev` has no `companion\` beside the exe: point this at a publish folder to test the hand-over.
const DEV_ENV: &str = "THEUTH_COMPANION_APP";
/// `WM_CLOSE` first; `TerminateProcess` for whatever still holds the mutex after this.
const QUIT_GRACE: Duration = Duration::from_secs(2);
/// How often [`watch`] opens the mutex; the bound on two tray icons overlapping after the companion
/// starts or exits on its own (our own launch flips it immediately).
const POLL: Duration = Duration::from_secs(2);

/// Last observed state (initial check, our launch, or the poll); `companion_alive` reads it. The tray
/// starts visible, matching `false`.
static ALIVE: AtomicBool = AtomicBool::new(false);

/// In `setup`, on the main thread: take stock, launch if needed, keep watching.
pub fn init(app: &AppHandle) -> anyhow::Result<()> {
    let running = mutex_held();
    set_alive(app, running);
    if running {
        tracing::info!("companion: already running");
    } else if let Some(path) = locate() {
        launch(app, &path);
    } else {
        tracing::info!(
            "companion: no {NESTED}\\{EXE} beside the exe and %{DEV_ENV}% unset; not launching"
        );
    }
    watch(app);
    Ok(())
}

pub fn alive() -> bool {
    ALIVE.load(SeqCst)
}

/// Quit handshake, after the engine's `shutdown` and before the sidecar stops (and before the NSIS
/// updater replaces `companion\`, which it cannot do while the exe runs). Blocks the exiting main thread.
pub fn terminate() {
    if !mutex_held() {
        return;
    }
    // SAFETY: plain Win32 queries; a null HWND is handled.
    let hwnd: HWND = unsafe { FindWindowW(RAIL_CLASS, std::ptr::null()) };
    let mut pid = 0u32;
    if !hwnd.is_null() {
        // SAFETY: `hwnd` was just found; `pid` is a valid out-pointer.
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut pid);
            PostMessageW(hwnd, WM_CLOSE, 0, 0);
        }
    }
    let deadline = Instant::now() + QUIT_GRACE;
    while Instant::now() < deadline && mutex_held() {
        std::thread::sleep(Duration::from_millis(50));
    }
    if !mutex_held() {
        return;
    }
    tracing::warn!("companion: did not quit in {QUIT_GRACE:?}; TerminateProcess");
    if pid != 0 {
        // SAFETY: `OpenProcess` returns null on failure, checked before use; the handle is closed.
        unsafe {
            let h: HANDLE = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if !h.is_null() {
                TerminateProcess(h, 1);
                CloseHandle(h);
            }
        }
    } else {
        // ponytail: no rail window to name a pid (the companion is mid-start or headless) → taskkill by
        // image name; a Toolhelp32 walk is the upgrade if shelling out at quit ever shows up.
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/IM", EXE])
            .status();
    }
}

/// Whether the companion holds its mutex. `OpenMutexW` on a name nobody created fails with
/// `ERROR_FILE_NOT_FOUND`; same user, same session, so `SYNCHRONIZE` is always granted when it exists.
fn mutex_held() -> bool {
    // SAFETY: the name is a static wide string; a non-null handle is closed at once.
    unsafe {
        let h = OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, MUTEX);
        if h.is_null() {
            return false;
        }
        CloseHandle(h);
        true
    }
}

/// `<exe dir>\companion\Theuth.Companion.exe`; else `%THEUTH_COMPANION_APP%\Theuth.Companion.exe`.
fn locate() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let nested = exe.parent()?.join(NESTED).join(EXE);
    if nested.is_file() {
        return Some(nested);
    }
    std::env::var_os(DEV_ENV)
        .map(|dir| PathBuf::from(dir).join(EXE))
        .filter(|p| p.is_file())
}

/// `CreateProcess` through std with `--launched-by-shell` (§5.5); a WinExe opens no console. The child
/// handle is dropped — std does not kill on drop — so the companion outlives nothing but [`terminate`].
fn launch(app: &AppHandle, path: &std::path::Path) {
    match std::process::Command::new(path)
        .arg("--launched-by-shell")
        .spawn()
    {
        Ok(_) => {
            tracing::info!("companion: launched {}", path.display());
            set_alive(app, true);
        }
        Err(e) => tracing::warn!("companion: launch {}: {e}", path.display()),
    }
}

/// On a change: store, announce, and swap the notification-area presence (D9: one theuth in the tray).
fn set_alive(app: &AppHandle, alive: bool) {
    if ALIVE.swap(alive, SeqCst) == alive {
        return;
    }
    tracing::info!(
        "companion: alive={alive}; tray {}",
        if alive { "hidden" } else { "shown" }
    );
    if let Err(e) = app.emit(ALIVE_EVENT, Alive { alive }) {
        tracing::warn!(error = %e, "emit {ALIVE_EVENT}");
    }
    if let Some(tray) = app.tray_by_id("main")
        && let Err(e) = tray.set_visible(!alive)
    {
        tracing::warn!(error = %e, "tray set_visible");
    }
}

/// Open the mutex on the main thread every [`POLL`] and announce changes. ponytail: a 2 s poll, as on the
/// Mac; `WaitForSingleObject` on a process handle is the push upgrade if the overlap ever matters.
fn watch(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(POLL);
        loop {
            tick.tick().await;
            let handle = app.clone();
            let run = app.run_on_main_thread(move || set_alive(&handle, mutex_held()));
            if run.is_err() {
                return;
            }
        }
    });
}

/// `companion_login_item_status` (D12): the companion owns the Run key and mirrors the toggle into
/// `<data_dir>\companion.json` (`{ "launch_at_login": bool }`, the file `login_item.rs` reads on macOS).
pub fn login_item_status() -> &'static str {
    #[derive(serde::Deserialize, Default)]
    struct Wish {
        #[serde(default)]
        launch_at_login: bool,
    }
    let wanted = std::fs::read(crate::launch::data_dir().join("companion.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Wish>(&b).ok())
        .unwrap_or_default()
        .launch_at_login;
    if wanted { "enabled" } else { "not_registered" }
}
