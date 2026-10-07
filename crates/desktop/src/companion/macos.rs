//! macOS: launch the nested companion without activating it, follow it by bundle id, hand the menu
//! bar over (D9), and stop it at quit (130 §5.8). Not `NSWorkspace.didLaunch/didTerminateApplication`
//! as §5.8 planned: AppKit does not post those for `LSUIElement` apps (Apple, NSWorkspace docs; verified
//! here against the spike bundle), so the companion is polled by bundle id instead.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::time::{Duration, Instant};

use objc2_app_kit::{NSRunningApplication, NSWorkspace, NSWorkspaceOpenConfiguration};
use objc2_foundation::{NSError, NSString, NSURL};
use tauri::{AppHandle, Emitter};

use super::{ALIVE_EVENT, Alive, BUNDLE_ID};

/// Where the bundler-free packaging step (`scripts/package-macos.sh`) puts the helper, relative to
/// `Contents/`; TN2206's sanctioned login-item location, required by `SMAppService` (spike SA §4).
const NESTED: &str = "Library/LoginItems/Theuth Companion.app";
/// `tauri dev` has no bundle: point this at a built `Theuth Companion.app` to test the hand-over.
const DEV_ENV: &str = "THEUTH_COMPANION_APP";
/// `terminate()` is an Apple Event; the companion gets this long before `forceTerminate()`.
const QUIT_GRACE: Duration = Duration::from_secs(2);
/// How often [`watch`] asks LaunchServices; the bound on how long two menu bar icons can overlap after
/// the companion starts or exits on its own (our own launch flips it immediately).
const POLL: Duration = Duration::from_secs(2);

/// Last observed state (initial check, our launch, or the poll); `companion_alive` reads it. The tray
/// starts visible, matching `false`.
static ALIVE: AtomicBool = AtomicBool::new(false);

/// In `setup`, on the main thread: take stock, launch if needed, keep watching.
pub fn init(app: &AppHandle) -> anyhow::Result<()> {
    let running = !running().is_empty();
    set_alive(app, running);
    if running {
        tracing::info!("companion: already running");
    } else if let Some(path) = locate() {
        launch(app, &path);
    } else {
        tracing::info!(
            "companion: no bundle at Contents/{NESTED} and ${DEV_ENV} unset; not launching"
        );
    }
    watch(app);
    Ok(())
}

pub fn alive() -> bool {
    ALIVE.load(SeqCst)
}

/// Quit handshake, after the engine's `shutdown` and before the sidecar stops: `terminate()`, then
/// `forceTerminate()` on whatever is still there after [`QUIT_GRACE`]. Blocks the exiting main thread.
pub fn terminate() {
    let apps = running();
    if apps.is_empty() {
        return;
    }
    for app in &apps {
        app.terminate();
    }
    let deadline = Instant::now() + QUIT_GRACE;
    // `isTerminated` only updates from the main run loop, which is not spinning here; ask the kernel.
    let gone = |app: &NSRunningApplication| {
        // SAFETY: signal 0 checks for existence only.
        unsafe { libc::kill(app.processIdentifier(), 0) != 0 }
    };
    while Instant::now() < deadline && !apps.iter().all(|a| gone(a)) {
        std::thread::sleep(Duration::from_millis(50));
    }
    for app in apps.iter().filter(|a| !gone(a)) {
        tracing::warn!("companion: did not quit in {QUIT_GRACE:?}; forceTerminate");
        app.forceTerminate();
    }
}

fn running() -> Vec<objc2::rc::Retained<NSRunningApplication>> {
    NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(BUNDLE_ID))
        .to_vec()
}

/// `<bundle>/Contents/MacOS/theuth-desktop` → `<bundle>/Contents/<NESTED>`; else [`DEV_ENV`].
fn locate() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let nested = exe.parent()?.parent()?.join(NESTED);
    if nested.is_dir() {
        return Some(nested);
    }
    std::env::var_os(DEV_ENV)
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
}

/// `openApplication(at:configuration:)` with `activates = false`: the rail must never take focus (D4).
/// The completion handler (a concurrent queue) flips alive at once; everything it touches is `Send`.
fn launch(app: &AppHandle, path: &std::path::Path) {
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    let config = NSWorkspaceOpenConfiguration::configuration();
    config.setActivates(false);
    config.setAddsToRecentItems(false);
    let shown = path.display().to_string();
    let app = app.clone();
    let done = block2::RcBlock::new(move |_: *mut NSRunningApplication, err: *mut NSError| {
        // SAFETY: AppKit hands a valid `NSError` or null.
        match unsafe { err.as_ref() } {
            Some(e) => tracing::warn!("companion: launch {shown}: {}", e.localizedDescription()),
            None => {
                tracing::info!("companion: launched {shown}");
                set_alive(&app, true);
            }
        }
    });
    NSWorkspace::sharedWorkspace().openApplicationAtURL_configuration_completionHandler(
        &url,
        &config,
        Some(&done),
    );
}

/// On a change: store, announce, and swap the menu bar presence (D9: one theuth in the menu bar).
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

/// Poll `runningApplications(withBundleIdentifier:)` on the main thread every [`POLL`] and announce
/// changes. ponytail: a 2 s poll; KVO on `NSWorkspace.runningApplications` (an observer class via
/// `define_class!`) is the push upgrade if the overlap ever matters.
fn watch(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(POLL);
        loop {
            tick.tick().await;
            let handle = app.clone();
            let run = app.run_on_main_thread(move || set_alive(&handle, !running().is_empty()));
            if run.is_err() {
                return;
            }
        }
    });
}
