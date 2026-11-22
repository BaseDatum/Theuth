//! The web view tile's cookie store and page storage on macOS and iOS (209 §4.5, D3): WebKit directly on the
//! named store [`DATA_STORE`] that every tile shares, so neither a live view nor Tauri's `Webview::cookies()`
//! is needed (that API wants a live webview and has blocked the main thread, 209 D3).
//!
//! - [`read`]: `WKHTTPCookieStore.getAllCookies` in the contract's [`Cookie`] shape (`web_profile_read`).
//! - [`watch`]: one `WKHTTPCookieStoreObserver` on the store, kept alive in a main-thread registry until
//!   [`unwatch`]; a change starts a [`Debounce`] timer, and when it is due the store is read again and
//!   `web-profile://changed {}` is emitted if the jar differs from the last read ([`diff`]).
//! - [`capture`]: the top-level origin's localStorage, by `callAsyncJavaScript` in the isolated
//!   [`AGENT_WORLD`] content world of a tile's `WKWebView` (`web_storage_capture`).
//!
//! Every WebKit call runs on the main thread; callers wait for the reply at most [`MAIN_TIMEOUT`]. Cookie
//! and storage values never reach `tracing`.
//!
//! `objc2-web-kit` 0.3 declares some protocol methods for macOS only, so [`watch`] asserts in debug builds
//! that the observer answers its selector on either platform.

use std::cell::RefCell;
use std::ptr::NonNull;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_foundation::{
    NSArray, NSDictionary, NSError, NSHTTPCookie, NSNumber, NSObjectProtocol, NSString, NSUUID,
};
use objc2_web_kit::{
    WKContentWorld, WKHTTPCookieStore, WKHTTPCookieStoreObserver, WKWebsiteDataStore,
};
use tauri::{AppHandle, Emitter, Runtime};

use crate::web_rules::DATA_STORE;
use crate::web_rules::profile::{
    AGENT_WORLD, CAPTURE_SCRIPT, CHANGED_EVENT, Changed, Cookie, Debounce, Due, NativeCookie,
    QUIET, Storage, diff, parse_capture,
};

/// How long a command waits for the main thread (and WebKit's reply) before giving up.
pub(crate) const MAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// A main-thread reply, at most [`MAIN_TIMEOUT`] away.
fn recv<T>(rx: &Receiver<Result<T, String>>) -> Result<T, String> {
    rx.recv_timeout(MAIN_TIMEOUT).map_err(|e| match e {
        RecvTimeoutError::Timeout => format!(
            "the main thread did not run the web view call within {} s",
            MAIN_TIMEOUT.as_secs()
        ),
        RecvTimeoutError::Disconnected => "the main thread dropped the web view call".to_owned(),
    })?
}

/// [`recv`] off the async workers.
pub(crate) async fn wait<T: Send + 'static>(rx: Receiver<Result<T, String>>) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || recv(&rx))
        .await
        .map_err(|e| e.to_string())?
}

/// The tiles' cookie store.
fn cookie_store(mtm: MainThreadMarker) -> Retained<WKHTTPCookieStore> {
    // SAFETY: a non-zero identifier; `dataStoreForIdentifier:` exists from macOS 14 / iOS 17 and the app's
    // minimums are macOS 15.0 / iOS 17.4.
    unsafe {
        WKWebsiteDataStore::dataStoreForIdentifier(&NSUUID::from_bytes(DATA_STORE), mtm)
            .httpCookieStore()
    }
}

/// An `NSHTTPCookie`'s attributes as plain values.
fn native(cookie: &NSHTTPCookie) -> NativeCookie {
    let properties = cookie
        .properties()
        .map(|props| {
            let (keys, values) = props.to_vecs();
            keys.iter()
                .zip(values)
                .filter_map(|(k, v)| {
                    let v = if let Some(s) = v.downcast_ref::<NSString>() {
                        s.to_string()
                    } else {
                        v.downcast_ref::<NSNumber>()?.stringValue().to_string()
                    };
                    Some((k.to_string(), v))
                })
                .collect()
        })
        .unwrap_or_default();
    NativeCookie {
        name: cookie.name().to_string(),
        value: cookie.value().to_string(),
        domain: cookie.domain().to_string(),
        path: cookie.path().to_string(),
        expires: cookie.expiresDate().map(|d| d.timeIntervalSince1970()),
        session_only: cookie.isSessionOnly(),
        secure: cookie.isSecure(),
        http_only: cookie.isHTTPOnly(),
        same_site: cookie.sameSitePolicy().map(|s| s.to_string()),
        properties,
    }
}

/// Every cookie of the tiles' store (`web_profile_read`, 209 §4.5). Also starts [`watch`]. Blocks for the
/// main thread: never call it there.
pub(crate) fn read_blocking<R: Runtime>(app: &AppHandle<R>) -> Result<Vec<Cookie>, String> {
    let (tx, rx) = sync_channel(1);
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let Some(mtm) = MainThreadMarker::new() else {
            let _ = tx.send(Err("run_on_main_thread ran off the main thread".to_owned()));
            return;
        };
        watch(mtm, &handle);
        let reply = RcBlock::new(move |cookies: NonNull<NSArray<NSHTTPCookie>>| {
            // SAFETY: WebKit passes a live array for the duration of the call.
            let cookies = unsafe { cookies.as_ref() };
            let jar = cookies.iter().map(|c| Cookie::from(native(&c))).collect();
            let _ = tx.send(Ok(jar));
        });
        // SAFETY: the handler is called once, on the main thread, with the store's cookies.
        unsafe { cookie_store(mtm).getAllCookies(&reply) };
    })
    .map_err(|e| e.to_string())?;
    recv(&rx)
}

/// [`read_blocking`] off the async workers.
pub(crate) async fn read<R: Runtime>(app: AppHandle<R>) -> Result<Vec<Cookie>, String> {
    tauri::async_runtime::spawn_blocking(move || read_blocking(&app))
        .await
        .map_err(|e| e.to_string())?
}

/// Start the localStorage capture of `view`'s main frame (a `WKWebView`) in [`AGENT_WORLD`]; the result
/// goes to `tx` when the script finishes (`web_storage_capture`, 209 §4.5).
pub(crate) fn capture(
    mtm: MainThreadMarker,
    view: &AnyObject,
    tx: SyncSender<Result<Storage, String>>,
) {
    // SAFETY: a named world for this app's scripts; WebKit keeps one instance per name while it lives.
    let world = unsafe { WKContentWorld::worldWithName(&NSString::from_str(AGENT_WORLD), mtm) };
    let done = RcBlock::new(move |result: *mut AnyObject, error: *mut NSError| {
        // SAFETY: WebKit passes live (or nil) objects for the duration of the call.
        let reply = match unsafe { error.as_ref() } {
            Some(error) => Err(format!(
                "storage capture failed: {}",
                error.localizedDescription()
            )),
            None => match unsafe { result.as_ref() }.and_then(|r| r.downcast_ref::<NSString>()) {
                Some(json) => parse_capture(&json.to_string()),
                None => Err("storage capture returned no result".to_owned()),
            },
        };
        let _ = tx.send(reply);
    });
    let body = NSString::from_str(CAPTURE_SCRIPT);
    // SAFETY: `view` is a live `WKWebView` (macOS 11+ / iOS 14+ have this selector); nil arguments and
    // frame mean no arguments and the main frame; the handler is copied by WebKit and called once.
    unsafe {
        let () = msg_send![
            view,
            callAsyncJavaScript: &*body,
            arguments: None::<&NSDictionary<NSString, AnyObject>>,
            inFrame: None::<&AnyObject>,
            inContentWorld: &*world,
            completionHandler: &*done
        ];
    }
}

/// The debounce and the last jar the observer read, shared by the main thread and the timer thread.
#[derive(Default)]
struct Pending {
    debounce: Debounce,
    last: Option<Vec<Cookie>>,
}

fn lock(pending: &Mutex<Pending>) -> MutexGuard<'_, Pending> {
    pending.lock().unwrap_or_else(|e| e.into_inner())
}

struct ObserverIvars {
    on_change: Box<dyn Fn()>,
}

define_class!(
    /// `WKHTTPCookieStoreObserver` of the tiles' store.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ObserverIvars]
    struct Observer;

    unsafe impl NSObjectProtocol for Observer {}

    unsafe impl WKHTTPCookieStoreObserver for Observer {
        #[unsafe(method(cookiesDidChangeInCookieStore:))]
        fn cookies_did_change(&self, _store: &WKHTTPCookieStore) {
            (self.ivars().on_change)();
        }
    }
);

impl Observer {
    fn new(mtm: MainThreadMarker, ivars: ObserverIvars) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ivars);
        // SAFETY: `NSObject`'s `init` on a freshly allocated instance.
        unsafe { msg_send![super(this), init] }
    }
}

/// The store and the observer registered on it (which the store does not retain).
struct Watch {
    store: Retained<WKHTTPCookieStore>,
    observer: Retained<Observer>,
}

thread_local! {
    /// Main thread only.
    static WATCH: RefCell<Option<Watch>> = const { RefCell::new(None) };
}

/// Observe the tiles' store, once per process; a no-op when already watching.
pub(crate) fn watch<R: Runtime>(mtm: MainThreadMarker, app: &AppHandle<R>) {
    if WATCH.with_borrow(Option::is_some) {
        return;
    }
    let pending = Arc::new(Mutex::new(Pending::default()));
    let app = app.clone();
    let observer = Observer::new(
        mtm,
        ObserverIvars {
            on_change: Box::new(move || changed(&app, &pending)),
        },
    );
    debug_assert!(
        observer.respondsToSelector(sel!(cookiesDidChangeInCookieStore:)),
        "cookie store observer lacks cookiesDidChangeInCookieStore:"
    );
    let store = cookie_store(mtm);
    // SAFETY: the observer is kept alive in `WATCH` and removed in `unwatch` before it is released.
    unsafe { store.addObserver(ProtocolObject::from_ref(&*observer)) };
    WATCH.set(Some(Watch { store, observer }));
}

/// [`watch`] from any thread (the desktop's commands run off the main thread).
#[cfg(target_os = "macos")]
pub(crate) fn start<R: Runtime>(app: &AppHandle<R>) {
    let handle = app.clone();
    let hop = app.run_on_main_thread(move || {
        if let Some(mtm) = MainThreadMarker::new() {
            watch(mtm, &handle);
        }
    });
    if let Err(e) = hop {
        tracing::warn!("watch the web view cookie store: {e}");
    }
}

/// Remove the observer (app exit). Off the main thread, or when not watching, nothing happens.
pub(crate) fn unwatch() {
    if MainThreadMarker::new().is_none() {
        return;
    }
    if let Some(Watch { store, observer }) = WATCH.take() {
        // SAFETY: registered in `watch` on this store.
        unsafe { store.removeObserver(ProtocolObject::from_ref(&*observer)) };
    }
}

/// The observer fired: start the timer unless one is pending.
fn changed<R: Runtime>(app: &AppHandle<R>, pending: &Arc<Mutex<Pending>>) {
    if !lock(pending).debounce.change(Instant::now()) {
        return;
    }
    let (app, shared) = (app.clone(), pending.clone());
    let spawned = std::thread::Builder::new()
        .name("web-profile-changed".to_owned())
        .spawn(move || report(&app, &shared));
    if let Err(e) = spawned {
        tracing::warn!("start the cookie change timer: {e}");
        lock(pending).debounce = Debounce::default();
    }
}

/// Wait until the debounce is due, read the store, and emit `web-profile://changed` unless the jar is the
/// one last read (WebKit also reports writes that change nothing). A failed read emits anyway: the UI
/// reads again.
fn report<R: Runtime>(app: &AppHandle<R>, pending: &Mutex<Pending>) {
    let mut wait = QUIET;
    loop {
        std::thread::sleep(wait);
        match lock(pending).debounce.poll(Instant::now()) {
            Due::Wait(more) => wait = more,
            Due::Fire => break,
            Due::Idle => return,
        }
    }
    let changed = match read_blocking(app) {
        Ok(jar) => {
            let mut state = lock(pending);
            let changed = state
                .last
                .as_ref()
                .is_none_or(|last| !diff(last, &jar).is_empty());
            state.last = Some(jar);
            changed
        }
        Err(e) => {
            tracing::warn!("read the web view cookie store: {e}");
            true
        }
    };
    if changed && let Err(e) = app.emit(CHANGED_EVENT, Changed {}) {
        tracing::warn!("emit {CHANGED_EVENT}: {e}");
    }
}
