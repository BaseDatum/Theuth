//! 221 D20 `lock_on_sleep` (macOS only): the Mac going to sleep, its displays sleeping and its screen
//! locking are reported to the webview as `power-lock { kind }` (`sleep` for both sleeps, `screen_lock`);
//! the webview forwards them to the engine (`ui/src/lib/power-lock.ts`), which locks every
//! external-secrets connection whose `lock_on_sleep` is on. The shell holds no engine connection of its
//! own, so the webview is the path, as for every other shell signal. The observers are held in
//! [`OBSERVERS`] and removed by [`shutdown`] at `RunEvent::Exit`.

use std::cell::RefCell;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{class, msg_send};
use objc2_app_kit::{
    NSWorkspace, NSWorkspaceScreensDidSleepNotification, NSWorkspaceWillSleepNotification,
};
use objc2_foundation::{
    NSNotification, NSNotificationCenter, NSNotificationName, NSObjectProtocol, NSString,
};
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

type Observer = (
    Retained<NSNotificationCenter>,
    Retained<ProtocolObject<dyn NSObjectProtocol>>,
);

thread_local! {
    /// The registered observers with their centres; main thread only (`setup`, `RunEvent::Exit`).
    static OBSERVERS: RefCell<Vec<Observer>> = const { RefCell::new(Vec::new()) };
}

/// Observe sleep, display sleep and screen lock until [`shutdown`]. On the main thread; a second call
/// while observing does nothing.
pub fn init(app: &AppHandle) {
    if OBSERVERS.with_borrow(|o| !o.is_empty()) {
        return;
    }
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

/// Remove every observer [`init`] added (`RunEvent::Exit`, main thread).
pub fn shutdown() {
    for (center, token) in OBSERVERS.take() {
        // SAFETY: `token` is the observer `addObserverForName…` returned on this centre.
        unsafe { center.removeObserver(token.as_ref()) };
    }
}

fn observe(
    center: &Retained<NSNotificationCenter>,
    name: &NSNotificationName,
    app: &AppHandle,
    kind: Kind,
) {
    let app = app.clone();
    let block = block2::RcBlock::new(move |_: NonNull<NSNotification>| {
        if let Err(e) = app.emit(EVENT, PowerLock { kind }) {
            tracing::warn!(error = %e, "emit power-lock");
        }
    });
    // SAFETY: no object filter, no queue (the block runs on the posting thread); the block owns only a
    // `Send` `AppHandle`; the centre holds the observer until `shutdown` removes it.
    let token = unsafe {
        center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &block)
    };
    OBSERVERS.with_borrow_mut(|o| o.push((center.clone(), token)));
}
