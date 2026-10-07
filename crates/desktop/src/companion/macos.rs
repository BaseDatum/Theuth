//! macOS: launch the nested companion without activating it, follow it through `NSWorkspace`'s
//! launch/terminate notifications, hand the menu bar over (D9), and stop it at quit (130 §5.8).

use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::time::{Duration, Instant};

use objc2_app_kit::{
    NSRunningApplication, NSWorkspace, NSWorkspaceApplicationKey,
    NSWorkspaceDidLaunchApplicationNotification, NSWorkspaceDidTerminateApplicationNotification,
    NSWorkspaceOpenConfiguration,
};
use objc2_foundation::{NSError, NSNotification, NSString, NSURL};
use tauri::{AppHandle, Emitter};

use super::{ALIVE_EVENT, Alive, BUNDLE_ID};

/// Where the bundler-free packaging step (`scripts/package-macos.sh`) puts the helper, relative to
/// `Contents/`; TN2206's sanctioned login-item location, required by `SMAppService` (spike SA §4).
const NESTED: &str = "Library/LoginItems/Theuth Companion.app";
/// `tauri dev` has no bundle: point this at a built `Theuth Companion.app` to test the hand-over.
const DEV_ENV: &str = "THEUTH_COMPANION_APP";
/// `terminate()` is an Apple Event; the companion gets this long before `forceTerminate()`.
const QUIT_GRACE: Duration = Duration::from_secs(2);

/// Mirror of the last notification (plus the initial check); `companion_alive` reads it.
static ALIVE: AtomicBool = AtomicBool::new(false);

/// In `setup`, on the main thread: observe, take stock, launch if needed.
pub fn init(app: &AppHandle) -> anyhow::Result<()> {
    watch(app);
    let running = !running().is_empty();
    set_alive(app, running);
    if running {
        tracing::info!("companion: already running");
        return Ok(());
    }
    let Some(path) = locate() else {
        tracing::info!(
            "companion: no bundle at Contents/{NESTED} and ${DEV_ENV} unset; not launching"
        );
        return Ok(());
    };
    launch(&path);
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
fn launch(path: &std::path::Path) {
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    let config = NSWorkspaceOpenConfiguration::configuration();
    config.setActivates(false);
    config.setAddsToRecentItems(false);
    let shown = path.display().to_string();
    let done = block2::RcBlock::new(move |_: *mut NSRunningApplication, err: *mut NSError| {
        // SAFETY: AppKit hands a valid `NSError` or null.
        match unsafe { err.as_ref() } {
            Some(e) => tracing::warn!("companion: launch {shown}: {}", e.localizedDescription()),
            None => tracing::info!("companion: launched {shown}"),
        }
    });
    NSWorkspace::sharedWorkspace().openApplicationAtURL_configuration_completionHandler(
        &url,
        &config,
        Some(&done),
    );
}

/// Store, announce, and swap the menu bar presence (D9: one theuth in the menu bar).
fn set_alive(app: &AppHandle, alive: bool) {
    ALIVE.store(alive, SeqCst);
    tracing::info!("companion: alive={alive}; tray {}", if alive { "hidden" } else { "shown" });
    if let Err(e) = app.emit(ALIVE_EVENT, Alive { alive }) {
        tracing::warn!(error = %e, "emit {ALIVE_EVENT}");
    }
    if let Some(tray) = app.tray_by_id("main")
        && let Err(e) = tray.set_visible(!alive)
    {
        tracing::warn!(error = %e, "tray set_visible");
    }
}

/// `NSWorkspace.didLaunchApplicationNotification` / `didTerminateApplicationNotification` filtered to
/// [`BUNDLE_ID`]. The workspace centre posts on the main thread; the observers live for the process.
fn watch(app: &AppHandle) {
    let observe = |alive: bool, name: &'static objc2_foundation::NSNotificationName| {
        let app = app.clone();
        let block = block2::RcBlock::new(move |n: NonNull<NSNotification>| {
            // SAFETY: AppKit hands a live notification for the duration of the block.
            if unsafe { n.as_ref() }.userInfo().is_some_and(|info| {
                // SAFETY: AppKit's static key.
                info.objectForKey(unsafe { NSWorkspaceApplicationKey })
                    .and_then(|o| o.downcast::<NSRunningApplication>().ok())
                    .and_then(|a| a.bundleIdentifier())
                    .is_some_and(|id| id.to_string() == BUNDLE_ID)
            }) {
                set_alive(&app, alive);
            }
        });
        // SAFETY: AppKit's static name; no object filter, no queue; the block owns only a `Send` `AppHandle`.
        let token = unsafe {
            NSWorkspace::sharedWorkspace()
                .notificationCenter()
                .addObserverForName_object_queue_usingBlock(Some(name), None, None, &block)
        };
        std::mem::forget(token);
    };
    // SAFETY: reading AppKit's static notification names.
    unsafe {
        observe(true, NSWorkspaceDidLaunchApplicationNotification);
        observe(false, NSWorkspaceDidTerminateApplicationNotification);
    }
}
