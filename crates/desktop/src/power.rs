//! 221 D20 `lock_on_sleep`: the Mac going to sleep, its displays sleeping and its screen locking are
//! reported to the webview as `power-lock { kind }` (`sleep` | `screen_lock`); the webview forwards them
//! to the engine (`ui/src/lib/power-lock.ts`), which locks every external-secrets connection whose
//! `lock_on_sleep` is on. The shell holds no engine connection of its own, so the webview is the path, as
//! for every other shell signal.

use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::{class, msg_send};
use objc2_app_kit::{
    NSWorkspace, NSWorkspaceScreensDidSleepNotification, NSWorkspaceWillSleepNotification,
};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSNotificationName, NSString};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// The event the webview listens for.
pub const EVENT: &str = "power-lock";

/// `com.apple.screenIsLocked`: posted on the distributed centre when the login window locks the
/// screen (⌃⌘Q, the hot corner, the screen saver's password).
const SCREEN_LOCKED: &str = "com.apple.screenIsLocked";

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Sleep,
    ScreenLock,
}

#[derive(Clone, Serialize)]
struct PowerLock {
    kind: Kind,
}

/// Observe sleep, display sleep and screen lock for the life of the process.
pub fn init(app: &AppHandle) {
    let workspace = NSWorkspace::sharedWorkspace().notificationCenter();
    // SAFETY: AppKit's static names.
    observe(
        &workspace,
        unsafe { NSWorkspaceWillSleepNotification },
        app,
        Kind::Sleep,
    );
    observe(
        &workspace,
        unsafe { NSWorkspaceScreensDidSleepNotification },
        app,
        Kind::Sleep,
    );
    // SAFETY: `+[NSDistributedNotificationCenter defaultCenter]` returns the process's shared centre,
    // an `NSNotificationCenter` subclass, never nil.
    let distributed: Retained<NSNotificationCenter> =
        unsafe { msg_send![class!(NSDistributedNotificationCenter), defaultCenter] };
    observe(
        &distributed,
        &NSString::from_str(SCREEN_LOCKED),
        app,
        Kind::ScreenLock,
    );
}

fn observe(center: &NSNotificationCenter, name: &NSNotificationName, app: &AppHandle, kind: Kind) {
    let app = app.clone();
    let block = block2::RcBlock::new(move |_: NonNull<NSNotification>| {
        if let Err(e) = app.emit(EVENT, PowerLock { kind }) {
            tracing::warn!(error = %e, "emit power-lock");
        }
    });
    // SAFETY: no object filter, no queue (the block runs on the posting thread); the block owns only a
    // `Send` `AppHandle`; the centre holds the observer for the life of the process.
    let token = unsafe {
        center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &block)
    };
    std::mem::forget(token);
}
