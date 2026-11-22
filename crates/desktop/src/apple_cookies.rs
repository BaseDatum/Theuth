//! The web view tile's cookie store and page storage on macOS and iOS (209 §4.5, D3): WebKit directly on the
//! named store [`DATA_STORE`] that every tile shares, so neither a live view nor Tauri's `Webview::cookies()`
//! is needed (that API wants a live webview and has blocked the main thread, 209 D3).
//!
//! - [`available`]: the named store exists from macOS 14 / iOS 17 (wry's own check,
//!   `wry-0.57.1/src/wkwebview/mod.rs:264-287`). Before that the tiles use the app's default store, which
//!   is never exported: every profile command answers [`PROFILE_UNSUPPORTED`] and nothing is observed.
//! - [`read`]: `WKHTTPCookieStore.getAllCookies` in the contract's [`Cookie`] shape (`web_profile_read`).
//! - [`clear`]: every cookie of the store, and its localStorage, sessionStorage and IndexedDB data
//!   (`web_profile_clear`).
//! - [`watch`]: one `WKHTTPCookieStoreObserver` on the store, kept alive in a main-thread registry until
//!   [`unwatch`]; a change starts a [`Debounce`] timer, and when it is due the store is read again and
//!   `web-profile://changed {}` is emitted to the main window if the jar differs from the last read ([`diff`]).
//! - [`capture`]: the top-level origin's localStorage, by `callAsyncJavaScript` in the isolated
//!   [`AGENT_WORLD`] content world of a tile's `WKWebView` (`web_storage_capture`).
//!
//! Every WebKit call runs on the main thread, and nothing heavier than copying WebKit's objects into plain
//! values: completion handlers hand those over with `try_send` (never blocking the main thread), and the
//! waiting thread maps cookies and parses the capture JSON. Callers wait at most [`MAIN_TIMEOUT`]. Cookie
//! and storage values never reach `tracing`.
//!
//! `objc2-web-kit` 0.3 declares some protocol methods for macOS only, so [`watch`] asserts in debug builds
//! that the observer answers its selector on either platform.

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_foundation::{
    NSArray, NSDate, NSError, NSHTTPCookie, NSNumber, NSObjectProtocol, NSProcessInfo, NSSet,
    NSString, NSURL, NSUUID,
};
#[cfg(target_os = "macos")]
use objc2_web_kit::WKWebView;
use objc2_web_kit::{
    WKContentWorld, WKHTTPCookieStore, WKHTTPCookieStoreObserver, WKWebsiteDataStore,
    WKWebsiteDataTypeIndexedDBDatabases, WKWebsiteDataTypeLocalStorage,
    WKWebsiteDataTypeSessionStorage,
};
use tauri::{AppHandle, Emitter, Runtime};

#[cfg(target_os = "ios")]
use crate::ios_webview::WKWebView;
use crate::web_rules::profile::{
    AGENT_WORLD, CAPTURE_SCRIPT, CHANGED_EVENT, Changed, Cookie, Debounce, Due, NativeCookie,
    PROPERTIES, QUIET, Storage, diff, parse_capture,
};
use crate::web_rules::{DATA_STORE, PROFILE_UNSUPPORTED};

/// How long a command waits for the main thread (and WebKit's reply) before giving up.
pub(crate) const MAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// The window that runs the profile sync (`App.tsx`); the only target of `web-profile://changed`.
const MAIN_WINDOW: &str = "main";

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

/// Whether this system has named data stores, so the tiles use [`DATA_STORE`]: macOS 14 / iOS 17, as wry
/// decides it (`wry-0.57.1/src/wkwebview/mod.rs:264-268`).
fn named_stores() -> bool {
    let major = NSProcessInfo::processInfo()
        .operatingSystemVersion()
        .majorVersion;
    major >= if cfg!(target_os = "ios") { 17 } else { 14 }
}

/// `Ok` when the profile commands may run here, else [`PROFILE_UNSUPPORTED`] (209 §4.5).
pub(crate) fn available() -> Result<(), String> {
    if named_stores() {
        Ok(())
    } else {
        Err(PROFILE_UNSUPPORTED.to_owned())
    }
}

/// The tiles' data store, `None` where [`available`] fails (the tiles then use the default store).
pub(crate) fn data_store(mtm: MainThreadMarker) -> Option<Retained<WKWebsiteDataStore>> {
    // SAFETY: a non-zero identifier; the selector exists from macOS 14 / iOS 17, checked first.
    named_stores().then(|| unsafe {
        WKWebsiteDataStore::dataStoreForIdentifier(&NSUUID::from_bytes(DATA_STORE), mtm)
    })
}

/// The tiles' cookie store, or the refusal to send.
fn cookie_store(mtm: MainThreadMarker) -> Result<Retained<WKHTTPCookieStore>, String> {
    // SAFETY: a plain getter on a live store.
    data_store(mtm)
        .map(|store| unsafe { store.httpCookieStore() })
        .ok_or_else(|| PROFILE_UNSUPPORTED.to_owned())
}

/// A string value of `properties` (an `NSString`, an `NSNumber`'s description, an `NSURL`'s absolute string).
fn property_string(value: &AnyObject) -> Option<String> {
    if let Some(s) = value.downcast_ref::<NSString>() {
        Some(s.to_string())
    } else if let Some(n) = value.downcast_ref::<NSNumber>() {
        Some(n.stringValue().to_string())
    } else {
        Some(value.downcast_ref::<NSURL>()?.absoluteString()?.to_string())
    }
}

/// An `NSHTTPCookie`'s attributes as plain values; of `properties`, only the keys the mapping reads.
fn native(cookie: &NSHTTPCookie) -> NativeCookie {
    let properties = cookie
        .properties()
        .map(|props| {
            let (keys, values) = props.to_vecs();
            keys.iter()
                .zip(values)
                .filter(|(k, _)| {
                    let k = k.to_string();
                    PROPERTIES.iter().any(|p| p.eq_ignore_ascii_case(&k))
                })
                .filter_map(|(k, v)| Some((k.to_string(), property_string(&v)?)))
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
    available()?;
    let (tx, rx) = sync_channel::<Result<Vec<NativeCookie>, String>>(1);
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let Some(mtm) = MainThreadMarker::new() else {
            let _ = tx.try_send(Err("run_on_main_thread ran off the main thread".to_owned()));
            return;
        };
        watch(mtm, &handle);
        let store = match cookie_store(mtm) {
            Ok(store) => store,
            Err(e) => {
                let _ = tx.try_send(Err(e));
                return;
            }
        };
        let reply = RcBlock::new(move |cookies: NonNull<NSArray<NSHTTPCookie>>| {
            // SAFETY: WebKit passes a live array for the duration of the call.
            let cookies = unsafe { cookies.as_ref() };
            let _ = tx.try_send(Ok(cookies.iter().map(|c| native(&c)).collect()));
        });
        // SAFETY: the handler is called once, on the main thread, with the store's cookies.
        unsafe { store.getAllCookies(&reply) };
    })
    .map_err(|e| e.to_string())?;
    Ok(recv(&rx)?.into_iter().map(Cookie::from).collect())
}

/// [`read_blocking`] off the async workers.
pub(crate) async fn read<R: Runtime>(app: AppHandle<R>) -> Result<Vec<Cookie>, String> {
    tauri::async_runtime::spawn_blocking(move || read_blocking(&app))
        .await
        .map_err(|e| e.to_string())?
}

/// Empty the tiles' store (`web_profile_clear`, 209 §4.5): delete every cookie, then remove the
/// localStorage, sessionStorage and IndexedDB data of every site. Returns once WebKit reports both done,
/// at most [`MAIN_TIMEOUT`]; emits nothing (the observer reports the cookie change, the UI reads again).
pub(crate) async fn clear<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    available()?;
    let (tx, rx) = sync_channel::<Result<(), String>>(1);
    app.run_on_main_thread(move || {
        let Some(mtm) = MainThreadMarker::new() else {
            let _ = tx.try_send(Err("run_on_main_thread ran off the main thread".to_owned()));
            return;
        };
        clear_on_main(mtm, tx);
    })
    .map_err(|e| e.to_string())?;
    wait(rx).await
}

fn clear_on_main(mtm: MainThreadMarker, tx: SyncSender<Result<(), String>>) {
    let Some(store) = data_store(mtm) else {
        let _ = tx.try_send(Err(PROFILE_UNSUPPORTED.to_owned()));
        return;
    };
    // SAFETY: a plain getter on a live store.
    let cookies = unsafe { store.httpCookieStore() };
    let jar = cookies.clone();
    let listed = RcBlock::new(move |all: NonNull<NSArray<NSHTTPCookie>>| {
        // SAFETY: WebKit passes a live array for the duration of the call.
        let all = unsafe { all.as_ref() };
        // One tick per cookie deleted plus one for the loop itself; the page data goes at zero.
        let left = Rc::new(Cell::new(all.count() + 1));
        let tick = {
            let (store, tx) = (store.clone(), tx.clone());
            Rc::new(move |left: &Cell<usize>| match left.get() {
                0 => {}
                1 => {
                    left.set(0);
                    remove_page_data(&store, tx.clone());
                }
                n => left.set(n - 1),
            })
        };
        for cookie in all.iter() {
            let (left, tick) = (left.clone(), tick.clone());
            let done = RcBlock::new(move || tick(&left));
            // SAFETY: a cookie of this store; the handler is called on the main thread.
            unsafe { jar.deleteCookie_completionHandler(&cookie, Some(&*done)) };
        }
        tick(&left);
    });
    // SAFETY: the handler is called once, on the main thread, with the store's cookies.
    unsafe { cookies.getAllCookies(&listed) };
}

/// Remove the page storage data types of every site of `store`, then answer `tx`.
fn remove_page_data(store: &WKWebsiteDataStore, tx: SyncSender<Result<(), String>>) {
    // SAFETY: WebKit's exported data type constants.
    let types = unsafe {
        NSSet::from_slice(&[
            WKWebsiteDataTypeLocalStorage,
            WKWebsiteDataTypeSessionStorage,
            WKWebsiteDataTypeIndexedDBDatabases,
        ])
    };
    let done = RcBlock::new(move || {
        let _ = tx.try_send(Ok(()));
    });
    // SAFETY: data types WebKit names; `distantPast` covers every modification; the handler runs on the
    // main thread.
    unsafe {
        store.removeDataOfTypes_modifiedSince_completionHandler(
            &types,
            &NSDate::distantPast(),
            &done,
        )
    };
}

/// Start the localStorage capture of `view`'s main frame in [`AGENT_WORLD`]; the script's JSON goes to `tx`
/// as it is, for [`captured`] to parse off the main thread (`web_storage_capture`, 209 §4.5).
pub(crate) fn capture(
    mtm: MainThreadMarker,
    view: &WKWebView,
    tx: SyncSender<Result<String, String>>,
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
                Some(json) => Ok(json.to_string()),
                None => Err("storage capture returned no result".to_owned()),
            },
        };
        let _ = tx.try_send(reply);
    });
    // SAFETY: no arguments and no frame mean none and the main frame; WebKit copies the handler and calls
    // it once, on the main thread.
    unsafe {
        view.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
            &NSString::from_str(CAPTURE_SCRIPT),
            None,
            None,
            &world,
            Some(&*done),
        )
    };
}

/// Wait for [`capture`]'s JSON and parse it here, off the main thread.
pub(crate) async fn captured(rx: Receiver<Result<String, String>>) -> Result<Storage, String> {
    parse_capture(&wait(rx).await?)
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

/// Observe the tiles' store, once per process; a no-op when already watching, and where [`available`]
/// fails (the default store is never observed).
pub(crate) fn watch<R: Runtime>(mtm: MainThreadMarker, app: &AppHandle<R>) {
    if WATCH.with_borrow(Option::is_some) {
        return;
    }
    let Ok(store) = cookie_store(mtm) else {
        return;
    };
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

/// Remove the observer (app exit), on the main thread: directly when called there, else by a hop.
pub(crate) fn unwatch<R: Runtime>(app: &AppHandle<R>) {
    fn remove() {
        if let Some(Watch { store, observer }) = WATCH.take() {
            // SAFETY: registered in `watch` on this store.
            unsafe { store.removeObserver(ProtocolObject::from_ref(&*observer)) };
        }
    }
    if MainThreadMarker::new().is_some() {
        remove();
    } else if let Err(e) = app.run_on_main_thread(remove) {
        tracing::warn!("unwatch the web view cookie store: {e}");
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

/// Wait until the debounce is due, read the store, and emit `web-profile://changed` to the main window
/// unless the jar is the one last read (WebKit also reports writes that change nothing). A failed read
/// emits anyway: the UI reads again.
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
    if changed && let Err(e) = app.emit_to(MAIN_WINDOW, CHANGED_EVENT, Changed {}) {
        tracing::warn!("emit {CHANGED_EVENT}: {e}");
    }
}
