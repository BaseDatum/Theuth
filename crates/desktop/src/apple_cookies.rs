//! The web view tile's cookie store and page storage on macOS and iOS (209 §4.5, D3): WebKit directly on the
//! named store [`DATA_STORE`] that every tile shares, so neither a live view nor Tauri's `Webview::cookies()`
//! is needed (that API wants a live webview and has blocked the main thread, 209 D3).
//!
//! - [`available`]: the named store exists from macOS 14 / iOS 17 (wry's own check,
//!   `wry-0.57.1/src/wkwebview/mod.rs:264-287`). Before that the tiles use the app's default store, which
//!   is never exported: every profile command answers [`PROFILE_UNSUPPORTED`] and nothing is observed.
//! - [`read`]: `WKHTTPCookieStore.getAllCookies` in the contract's [`Cookie`] shape, sorted by key
//!   (`web_profile_read`).
//! - [`write`]: delete the store cookies a write replaces (same key and scope) or removes (the key in
//!   every scope) and set the given ones (`web_profile_write`), and [`apply_storage`]: a localStorage
//!   snapshot into a tile of its origin, by `callAsyncJavaScript` in [`AGENT_WORLD`]; [`written`] awaits
//!   both until the deadline and answers the counts reached, with `timed_out` when WebKit fell short.
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

use std::cell::{Cell, OnceCell, RefCell};
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_foundation::{
    NSArray, NSDate, NSDictionary, NSError, NSHTTPCookie, NSHTTPCookieDomain, NSHTTPCookieExpires,
    NSHTTPCookieName, NSHTTPCookiePath, NSHTTPCookieSameSiteLax, NSHTTPCookieSameSitePolicy,
    NSHTTPCookieSameSiteStrict, NSHTTPCookieSecure, NSHTTPCookieValue, NSNumber, NSObjectProtocol,
    NSProcessInfo, NSSet, NSString, NSURL, NSUUID,
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
    AGENT_WORLD, APPLY_STORAGE_SCRIPT, CAPTURE_SCRIPT, CHANGED_EVENT, Changed, Completion, Cookie,
    CookieWrite, Debounce, Due, NativeCookie, NativeSet, PROPERTIES, QUIET, SameSitePolicy,
    Storage, StorageApply, Written, diff, parse_capture, timeout_message,
};
use crate::web_rules::{DATA_STORE, PROFILE_UNSUPPORTED};

/// How long a command waits for the main thread (and WebKit's reply) before giving up.
pub(crate) const MAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// The window that runs the profile sync (`App.tsx`); the only target of `web-profile://changed`.
const MAIN_WINDOW: &str = "main";

/// A main-thread reply, at most [`MAIN_TIMEOUT`] away.
fn recv<T>(rx: &Receiver<Result<T, String>>) -> Result<T, String> {
    recv_until(rx, Instant::now() + MAIN_TIMEOUT)
}

/// Why a main-thread reply did not come: the time ran out (the budget it had), or the sender is gone.
pub(crate) enum Late {
    Timeout(Duration),
    Dropped,
}

pub(crate) const DROPPED: &str = "the main thread dropped the web view call";

/// A main-thread reply, or why there is none by `deadline`.
pub(crate) fn recv_by<T>(rx: &Receiver<T>, deadline: Instant) -> Result<T, Late> {
    let left = deadline.saturating_duration_since(Instant::now());
    rx.recv_timeout(left).map_err(|e| match e {
        RecvTimeoutError::Timeout => Late::Timeout(left),
        RecvTimeoutError::Disconnected => Late::Dropped,
    })
}

/// A main-thread reply, at most until `deadline`; a timeout names the time that was left.
fn recv_until<T>(rx: &Receiver<Result<T, String>>, deadline: Instant) -> Result<T, String> {
    recv_by(rx, deadline).map_err(|late| match late {
        Late::Timeout(left) => timeout_message(left),
        Late::Dropped => DROPPED.to_owned(),
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
///
/// One instance for the life of the process, kept in [`STORE`]: `dataStoreForIdentifier:` answers the live
/// store of that identifier if there is one, else a new one, and a `WKHTTPCookieStore` refers to its data
/// store only weakly. Without it, the UI's first read (before any tile) made a store that died after the
/// read, [`watch`] observed that store's cookie store, and the first tile got a second instance (seen on the
/// iPad simulator in the 209 P1 proof: the tile's cookie store was not the observed one).
pub(crate) fn data_store(mtm: MainThreadMarker) -> Option<Retained<WKWebsiteDataStore>> {
    if !named_stores() {
        return None;
    }
    STORE.with(|cell| {
        Some(
            cell.get_or_init(|| {
                // SAFETY: a non-zero identifier; the selector exists from macOS 14 / iOS 17, checked above.
                unsafe {
                    WKWebsiteDataStore::dataStoreForIdentifier(&NSUUID::from_bytes(DATA_STORE), mtm)
                }
            })
            .clone(),
        )
    })
}

thread_local! {
    /// Main thread only: the one [`data_store`] instance, never released.
    static STORE: OnceCell<Retained<WKWebsiteDataStore>> = const { OnceCell::new() };
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

/// The entries of `cookie.properties` named in [`PROPERTIES`], values as strings.
fn properties(cookie: &NSHTTPCookie) -> Vec<(String, String)> {
    cookie
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
        .unwrap_or_default()
}

/// An `NSHTTPCookie`'s attributes as plain values; of `properties`, only the keys the mapping reads.
fn native(cookie: &NSHTTPCookie) -> NativeCookie {
    let properties = properties(cookie);
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
    let mut cookies: Vec<Cookie> = recv(&rx)?.into_iter().map(Cookie::from).collect();
    // A deterministic order (209 §4.5): by key, host-only first for one key in both scopes.
    cookies.sort_by(|a, b| {
        (&a.name, &a.domain, &a.path, !a.host_only).cmp(&(
            &b.name,
            &b.domain,
            &b.path,
            !b.host_only,
        ))
    });
    Ok(cookies)
}

/// [`read_blocking`] off the async workers.
pub(crate) async fn read<R: Runtime>(app: AppHandle<R>) -> Result<Vec<Cookie>, String> {
    tauri::async_runtime::spawn_blocking(move || read_blocking(&app))
        .await
        .map_err(|e| e.to_string())?
}

/// An `NSHTTPCookie` of `cookie`'s properties (209 §4.5), `None` when Foundation refuses them. HttpOnly
/// has no exported key: `HttpOnly` is the one `isHTTPOnly` reads.
fn ns_cookie(cookie: &NativeSet) -> Option<Retained<NSHTTPCookie>> {
    fn object(value: Retained<NSString>) -> Retained<AnyObject> {
        Retained::into_super(Retained::into_super(value))
    }
    let yes = || object(NSString::from_str("TRUE"));
    let http_only = NSString::from_str("HttpOnly");
    // SAFETY: Foundation's exported property keys and policy names, immutable statics.
    let (mut keys, mut values): (Vec<&NSString>, Vec<Retained<AnyObject>>) = unsafe {
        (
            vec![
                NSHTTPCookieName,
                NSHTTPCookieValue,
                NSHTTPCookieDomain,
                NSHTTPCookiePath,
            ],
            vec![
                object(NSString::from_str(&cookie.name)),
                object(NSString::from_str(&cookie.value)),
                object(NSString::from_str(&cookie.domain)),
                object(NSString::from_str(&cookie.path)),
            ],
        )
    };
    if let Some(expires) = cookie.expires {
        keys.push(unsafe { NSHTTPCookieExpires });
        values.push(Retained::into_super(Retained::into_super(
            NSDate::dateWithTimeIntervalSince1970(expires as f64),
        )));
    }
    if cookie.secure {
        keys.push(unsafe { NSHTTPCookieSecure });
        values.push(yes());
    }
    if cookie.http_only {
        keys.push(&http_only);
        values.push(yes());
    }
    if let Some(policy) = cookie.same_site {
        // SAFETY: as above.
        let name = unsafe {
            match policy {
                SameSitePolicy::Lax => NSHTTPCookieSameSiteLax,
                SameSitePolicy::Strict => NSHTTPCookieSameSiteStrict,
            }
        };
        keys.push(unsafe { NSHTTPCookieSameSitePolicy });
        values.push(object(name.retain()));
    }
    let properties = NSDictionary::<NSString, AnyObject>::from_retained_objects(&keys, &values);
    // SAFETY: string keys of Foundation's cookie properties with string and date values.
    unsafe { NSHTTPCookie::cookieWithProperties(&properties) }
}

/// A cookie write for the main thread ([`write`]): the plan, its [`Completion`] shared with the waiting
/// side, and the signal that WebKit reported everything.
pub(crate) struct CookieWriter {
    cookies: CookieWrite,
    progress: Arc<Mutex<Completion>>,
    done: SyncSender<Result<(), String>>,
}

impl CookieWriter {
    /// The main thread could not start the write.
    pub(crate) fn fail(self, error: String) {
        let _ = self.done.try_send(Err(error));
    }
}

/// The waiting side of a [`CookieWriter`], for [`written`].
pub(crate) struct CookieWait {
    progress: Arc<Mutex<Completion>>,
    done: Receiver<Result<(), String>>,
}

/// The two sides of one cookie write.
pub(crate) fn writer(cookies: CookieWrite) -> (CookieWriter, CookieWait) {
    let progress = Arc::new(Mutex::new(Completion::new(cookies.skipped)));
    let (done, rx) = sync_channel(1);
    (
        CookieWriter {
            cookies,
            progress: progress.clone(),
            done,
        },
        CookieWait { progress, done: rx },
    )
}

/// Write the cookie half of `web_profile_write` into the tiles' store (209 §4.5), on the main thread: list
/// the store, delete every cookie the plan's [`Doom`] names (a replaced cookie in its own scope, a removed
/// key in every scope), then set each of `set`. Every WebKit completion handler counts in the
/// [`Completion`]; the one that completes it signals the waiting side. Main-thread work is copying keys
/// and building `NSHTTPCookie`s; the mapping was done before ([`plan_write`]). Values are never logged.
///
/// [`Doom`]: crate::web_rules::profile::Doom
/// [`plan_write`]: crate::web_rules::profile::plan_write
pub(crate) fn write(mtm: MainThreadMarker, writer: CookieWriter) {
    let store = match cookie_store(mtm) {
        Ok(store) => store,
        Err(e) => return writer.fail(e),
    };
    let jar = store.clone();
    let CookieWriter {
        cookies,
        progress,
        done,
    } = writer;
    let listed = RcBlock::new(move |all: NonNull<NSArray<NSHTTPCookie>>| {
        // SAFETY: WebKit passes a live array for the duration of the call.
        let all = unsafe { all.as_ref() };
        let doomed: Vec<Retained<NSHTTPCookie>> = all
            .iter()
            .filter(|c| {
                cookies.doom.dooms(
                    &c.name().to_string(),
                    &c.domain().to_string(),
                    &c.path().to_string(),
                    &properties(c),
                )
            })
            .collect();
        let fresh: Vec<Retained<NSHTTPCookie>> = cookies.set.iter().filter_map(ns_cookie).collect();
        lock(&progress).start(doomed.len(), fresh.len(), cookies.set.len() - fresh.len());
        for cookie in &doomed {
            let (progress, done) = (progress.clone(), done.clone());
            let handler = RcBlock::new(move || {
                if lock(&progress).removed() {
                    let _ = done.try_send(Ok(()));
                }
            });
            // SAFETY: a cookie of this store; the handler is called on the main thread.
            unsafe { jar.deleteCookie_completionHandler(cookie, Some(&*handler)) };
        }
        for cookie in &fresh {
            let (progress, done) = (progress.clone(), done.clone());
            let handler = RcBlock::new(move || {
                if lock(&progress).set() {
                    let _ = done.try_send(Ok(()));
                }
            });
            // SAFETY: a valid cookie; the handler is called on the main thread.
            unsafe { jar.setCookie_completionHandler(cookie, Some(&*handler)) };
        }
        if lock(&progress).issued() {
            let _ = done.try_send(Ok(()));
        }
    });
    // SAFETY: the handler is called once, on the main thread, with the store's cookies.
    unsafe { store.getAllCookies(&listed) };
}

/// Start applying `snapshot` to `view`'s main frame in [`AGENT_WORLD`] (209 §4.5): `localStorage.setItem`
/// per pair, no other key removed. The origin and entries are arguments, never script source. `tx` gets
/// `true` once written, `false` when the page is no longer at `snapshot.origin`.
pub(crate) fn apply_storage(
    mtm: MainThreadMarker,
    view: &WKWebView,
    snapshot: &StorageApply,
    tx: SyncSender<Result<bool, String>>,
) {
    // SAFETY: a named world for this app's scripts; WebKit keeps one instance per name while it lives.
    let world = unsafe { WKContentWorld::worldWithName(&NSString::from_str(AGENT_WORLD), mtm) };
    let (names, values): (Vec<_>, Vec<_>) = snapshot
        .arguments()
        .into_iter()
        .map(|(name, value)| {
            (
                NSString::from_str(name),
                Retained::into_super(Retained::into_super(NSString::from_str(value))),
            )
        })
        .unzip();
    let names: Vec<&NSString> = names.iter().map(|n| &**n).collect();
    let arguments = NSDictionary::<NSString, AnyObject>::from_retained_objects(&names, &values);
    let done = RcBlock::new(move |result: *mut AnyObject, error: *mut NSError| {
        // SAFETY: WebKit passes live (or nil) objects for the duration of the call.
        let reply = match unsafe { error.as_ref() } {
            // The code only: a script error's description may quote the page.
            Some(error) => Err(format!(
                "storage apply failed (WebKit error {})",
                error.code()
            )),
            None => Ok(unsafe { result.as_ref() }
                .and_then(|r| r.downcast_ref::<NSNumber>())
                .is_some_and(|n| n.boolValue())),
        };
        let _ = tx.try_send(reply);
    });
    // SAFETY: string arguments the body names; no frame means the main frame; WebKit copies the handler
    // and calls it once, on the main thread.
    unsafe {
        view.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
            &NSString::from_str(APPLY_STORAGE_SCRIPT),
            Some(&arguments),
            None,
            &world,
            Some(&*done),
        )
    };
}

/// Await [`write`] and every [`apply_storage`] (each with the tile's origin) until `deadline`, off the
/// async workers. `origins` are the plan's snapshot origins: each is in `storage_applied`, with the
/// number of tiles that took it. At the deadline the answer is `Ok` with `timed_out` and the counts
/// reached so far, so the caller reads the store again. A storage apply that fails is logged by origin
/// and does not fail the write. Refused cookies and a timeout are logged by count only.
pub(crate) async fn written(
    cookies: CookieWait,
    storage: Vec<(String, Receiver<Result<bool, String>>)>,
    origins: Vec<String>,
    deadline: Instant,
) -> Result<Written, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let timed_out = match recv_by(&cookies.done, deadline) {
            Ok(reply) => {
                reply?;
                false
            }
            Err(Late::Timeout(_)) => true,
            Err(Late::Dropped) => return Err(DROPPED.to_owned()),
        };
        let mut written = lock(&cookies.progress).written(timed_out);
        written.storage_applied = origins.into_iter().map(|o| (o, 0)).collect();
        for (origin, rx) in storage {
            match recv_by(&rx, deadline) {
                Ok(Ok(true)) => *written.storage_applied.entry(origin).or_default() += 1,
                Ok(Ok(false)) => {
                    tracing::debug!(%origin, "web profile write: the tile left the origin")
                }
                Ok(Err(e)) => tracing::warn!(%origin, "web profile write: localStorage: {e}"),
                Err(Late::Timeout(_)) => {
                    written.timed_out = true;
                    tracing::warn!(%origin, "web profile write: localStorage did not answer in time");
                }
                Err(Late::Dropped) => tracing::warn!(%origin, "web profile write: localStorage: {DROPPED}"),
            }
        }
        if written.skipped > 0 {
            tracing::warn!(skipped = written.skipped, "web profile write: cookies not set");
        }
        if written.timed_out {
            tracing::warn!(
                set = written.set,
                removed = written.removed,
                "web profile write: WebKit did not report everything in time"
            );
        }
        Ok(written)
    })
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

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
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
