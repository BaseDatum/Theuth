//! Native child webviews for the web view tile (205 §4.3): a `web-<16 hex>` webview inside the caller's window,
//! positioned over a placeholder the UI measures. The page is a stranger (205 §5): http/https only, no
//! capability names it, its own data store shared by all web views where the platform has one.
//!
//! IPC fence (205 §5): Tauri attaches its IPC handler to every webview (`tauri-2.12.5/src/manager/webview.rs:555`)
//! and grants a child the capabilities of its *window* label when the request origin is local
//! (`ipc/authority.rs:476-478`), and lets app commands through for any local origin when the app has no ACL
//! manifest (`webview/mod.rs:2080`). "Local" is the app URL (`devUrl` in dev builds) or a registered scheme,
//! which on Windows is `http://<scheme>.localhost` (`webview/mod.rs:1961-2002`). [`allowed`] refuses exactly
//! those, so a web view only ever runs at a remote origin: the IPC handler is present, but no ACL entry
//! matches that origin and every command is rejected.
//!
//! `on_navigation` sees subframe navigations on macOS and Linux, but only top-level ones on Windows
//! (wry 0.57.1 `webview2/mod.rs:754-772` hooks `NavigationStarting` only). Subframes have no IPC script
//! (Tauri's are main-frame only, `manager/webview.rs:159-199`).
//!
//! The one `initialization_script` is the constant [`LINKS_SCRIPT`]. The macOS data store identifier needs
//! macOS 14+; older systems silently use the default store, shared with the app (wry `wkwebview/mod.rs:276-287`).
//! The `asset` scheme refuses `web-*` webviews (`crate::asset`).
//!
//! The browser profile (209 §4.5): on macOS `web_profile_read`, `web_profile_write`, `web_profile_clear` and
//! `web_storage_capture` go to WebKit directly through `apple_cookies` (the store watch starts with the
//! first web view); Windows and Linux answer all four with `PROFILE_UNSUPPORTED`, with the same declared
//! types. All four refuse a `web-*` caller.
//!
//! The agent's tile operations (209 §4.7): `web_view_call` runs one op on a web view for the main webview
//! of the window that holds it; on macOS `script` and `snapshot` go through `apple_tile`, the rest through
//! Tauri and `WKWebView`. Windows and Linux check the same arguments and answer `CALL_UNSUPPORTED` (the CDP
//! route of 209 §4.7 is not built).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use serde::Serialize;
use serde_json::Value;
use tauri::webview::{NewWindowResponse, PageLoadEvent, WebviewBuilder};
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Runtime, State, Url, Webview,
    WebviewUrl, Window,
};
use tauri_plugin_opener::OpenerExt;

#[cfg(target_os = "macos")]
use crate::web_rules::DATA_STORE;
#[cfg(not(target_os = "macos"))]
use crate::web_rules::PROFILE_UNSUPPORTED;
use crate::web_rules::call::{Op, check_holder};
use crate::web_rules::profile::{Cookie, CookieKey, Storage, Written, check_caller};
use crate::web_rules::{
    CLOSED_EVENT, Closed, STATE_EVENT, app_url, check_label, check_url, in_popout, may_open,
    navigation_allowed, no_view,
};

/// tauri-plugin-opener 2.7.2 injects `init-iife.js` into every webview's main frame: a bubble-phase `click`
/// listener on `window` that `preventDefault()`s `<a target=_blank>` and ctrl/shift clicks on http(s), mailto
/// and tel links and invokes `plugin:opener|open_url`, which the ACL rejects for a remote origin, so the link
/// is dead. This capture-phase listener on `window` runs before any bubble listener there (and before the
/// page's own target/bubble listeners), whatever the registration order (Tauri adds plugin scripts first,
/// `manager/webview.rs:204, 225`); for the same http(s) cases it stops the event and calls `window.open`,
/// which reaches `on_new_window` (the system browser). mailto/tel stay with the opener script (dead, as before).
const LINKS_SCRIPT: &str = r#"window.addEventListener("click",function(e){if(e.defaultPrevented||e.button!==0||e.metaKey||e.altKey)return;var a=e.composedPath().find(function(n){return n instanceof Node&&n.nodeName.toUpperCase()==="A"});if(!a||!a.href||(a.target!=="_blank"&&!e.ctrlKey&&!e.shiftKey))return;var p;try{p=new URL(a.href).protocol}catch(_){return}if(p!=="http:"&&p!=="https:")return;e.stopImmediatePropagation();e.preventDefault();window.open(a.href)},true);"#;

/// Last known state per label: `web-view://state` payloads carry the full record. An entry exists from
/// `web_view_attach` until the view is destroyed (`web_view_close` or its window's destruction).
#[derive(Default)]
pub struct Views(Mutex<HashMap<String, ViewState>>);

#[derive(Clone, Default, Serialize)]
struct ViewState {
    label: String,
    url: String,
    title: String,
    loading: bool,
    /// The hosting window's label, kept current across reparenting; not part of the payload.
    #[serde(skip)]
    window: String,
}

/// `web_view_attach` may not take a view out of a pop-out into another window (205 §4.3): the pop-out
/// owns it until it closes. `main` → pop-out and re-attaching in the same window are fine.
fn steals(parent: &str, caller: &str) -> bool {
    parent != caller && parent.starts_with(crate::commands::POPOUT_PREFIX)
}

fn webview<R: Runtime>(app: &AppHandle<R>, label: &str) -> Result<Webview<R>, String> {
    check_label(label)?;
    app.get_webview(label).ok_or_else(|| no_view(label))
}

fn lock(views: &Views) -> std::sync::MutexGuard<'_, HashMap<String, ViewState>> {
    views.0.lock().unwrap_or_else(|e| e.into_inner())
}

/// Record `label` as living in `window` (attach and reparent).
fn track<R: Runtime>(app: &AppHandle<R>, label: &str, window: &str) {
    lock(&app.state::<Views>())
        .entry(label.to_owned())
        .or_insert_with(|| ViewState {
            label: label.to_owned(),
            ..ViewState::default()
        })
        .window = window.to_owned();
}

/// Update the record for `label` and emit it; a label no longer tracked (closed) is ignored.
fn publish<R: Runtime>(app: &AppHandle<R>, label: &str, change: impl FnOnce(&mut ViewState)) {
    let state = {
        let views = app.state::<Views>();
        let mut map = lock(&views);
        let Some(entry) = map.get_mut(label) else {
            return;
        };
        change(entry);
        entry.clone()
    };
    if let Err(e) = app.emit(STATE_EVENT, state) {
        tracing::warn!("emit {STATE_EVENT}: {e}");
    }
}

fn emit_closed<R: Runtime>(app: &AppHandle<R>, label: String) {
    if let Err(e) = app.emit(CLOSED_EVENT, Closed { label }) {
        tracing::warn!("emit {CLOSED_EVENT}: {e}");
    }
}

/// `WindowEvent::Destroyed` of any window: the runtime has dropped its child webviews and the manager has
/// already forgotten them (`tauri-2.12.5/src/app.rs:2711-2714` runs `on_window_close` before the window
/// event handlers), so the hosting window comes from `Views`. Emits `web-view://closed` for each (205 §4.3).
pub fn on_window_destroyed<R: Runtime>(window: &Window<R>) {
    let app = window.app_handle();
    let Some(views) = app.try_state::<Views>() else {
        return;
    };
    let gone: Vec<String> = {
        let mut map = lock(&views);
        let gone: Vec<String> = map
            .values()
            .filter(|s| s.window == window.label())
            .map(|s| s.label.clone())
            .collect();
        for label in &gone {
            map.remove(label);
        }
        gone
    };
    for label in gone {
        emit_closed(app, label);
    }
}

fn place<R: Runtime>(
    view: &Webview<R>,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    view.set_position(LogicalPosition::new(x, y))
        .map_err(|e| e.to_string())?;
    view.set_size(LogicalSize::new(width, height))
        .map_err(|e| e.to_string())
}

fn builder<R: Runtime>(
    app: &AppHandle<R>,
    label: &str,
    url: Url,
) -> Result<WebviewBuilder<R>, String> {
    let fence = app_url(app);
    let opener = app.clone();
    let last_open = Mutex::new(None::<Instant>);
    let builder = WebviewBuilder::new(label, WebviewUrl::External(url))
        .initialization_script(LINKS_SCRIPT)
        .on_navigation(move |url| navigation_allowed(url, fence.as_ref()))
        // `target=_blank` / `window.open`: the system browser, never a window of ours.
        .on_new_window(move |url, _features| {
            if !matches!(url.scheme(), "http" | "https") {
                return NewWindowResponse::Deny;
            }
            let now = Instant::now();
            {
                let mut last = last_open.lock().unwrap_or_else(|e| e.into_inner());
                if !may_open(*last, now) {
                    tracing::warn!("web view new-window request throttled: {url}");
                    return NewWindowResponse::Deny;
                }
                *last = Some(now);
            }
            if let Err(e) = opener.opener().open_url(url.as_str(), None::<&str>) {
                tracing::warn!("open {url} in the browser: {e}");
            }
            NewWindowResponse::Deny
        })
        .on_page_load(|view, payload| {
            let loading = matches!(payload.event(), PageLoadEvent::Started);
            let url = payload.url().to_string();
            publish(view.app_handle(), view.label(), |s| {
                s.url = url;
                s.loading = loading;
            });
        })
        .on_document_title_changed(|view, title| {
            let url = view.url().map(|u| u.to_string()).ok();
            publish(view.app_handle(), view.label(), |s| {
                if let Some(url) = url {
                    s.url = url;
                }
                s.title = title;
            });
        });
    #[cfg(target_os = "macos")]
    let builder = builder.data_store_identifier(DATA_STORE);
    #[cfg(not(target_os = "macos"))]
    let builder = builder.data_directory(
        app.path()
            .app_data_dir()
            .map_err(|e| e.to_string())?
            .join("web-views"),
    );
    Ok(builder)
}

/// Create `label` in the caller's window at the rectangle, or move an existing one there and show it; an
/// existing web view is not navigated (205 §4.3).
#[tauri::command]
#[allow(clippy::too_many_arguments)] // the contract's argument list (205 §4.3)
pub async fn web_view_attach<R: Runtime>(
    app: AppHandle<R>,
    window: Window<R>,
    label: String,
    url: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    check_label(&label)?;
    if let Some(view) = app.get_webview(&label) {
        let parent = view.window().label().to_owned();
        if steals(&parent, window.label()) {
            return Err(in_popout(&parent));
        }
        if parent != window.label() {
            view.reparent(&window).map_err(|e| e.to_string())?;
            track(&app, &label, window.label());
        }
        place(&view, x, y, width, height)?;
        return view.show().map_err(|e| e.to_string());
    }
    let parsed = check_url(&app, &url)?;
    let builder = builder(&app, &label, parsed)?;
    track(&app, &label, window.label());
    let res = window.add_child(
        builder,
        LogicalPosition::new(x, y),
        LogicalSize::new(width, height),
    );
    match res {
        Ok(_) => {
            // 209 §4.5: from the first tile on, store changes reach the UI as `web-profile://changed`.
            #[cfg(target_os = "macos")]
            crate::apple_cookies::start(&app);
            Ok(())
        }
        Err(e) => {
            lock(&app.state::<Views>()).remove(&label);
            Err(e.to_string())
        }
    }
}

/// Move and size it (205 §4.3).
#[tauri::command]
pub async fn web_view_bounds<R: Runtime>(
    app: AppHandle<R>,
    label: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    place(&webview(&app, &label)?, x, y, width, height)
}

/// Show or hide it without destroying it (205 §4.3).
#[tauri::command]
pub async fn web_view_visible<R: Runtime>(
    app: AppHandle<R>,
    label: String,
    visible: bool,
) -> Result<(), String> {
    let view = webview(&app, &label)?;
    if visible { view.show() } else { view.hide() }.map_err(|e| e.to_string())
}

/// Load `url`, http/https only (205 §4.3).
#[tauri::command]
pub async fn web_view_navigate<R: Runtime>(
    app: AppHandle<R>,
    label: String,
    url: String,
) -> Result<(), String> {
    let view = webview(&app, &label)?;
    view.navigate(check_url(&app, &url)?)
        .map_err(|e| e.to_string())
}

/// Reload (205 §4.3).
#[tauri::command]
pub async fn web_view_reload<R: Runtime>(app: AppHandle<R>, label: String) -> Result<(), String> {
    webview(&app, &label)?.reload().map_err(|e| e.to_string())
}

/// Back (`-1`) or forward (`1`) (205 §4.3).
#[tauri::command]
pub async fn web_view_history<R: Runtime>(
    app: AppHandle<R>,
    label: String,
    delta: i32,
) -> Result<(), String> {
    let view = webview(&app, &label)?;
    let js = match delta {
        -1 => "history.back()",
        1 => "history.forward()",
        _ => return Err(format!("history delta must be -1 or 1, got {delta}")),
    };
    view.eval(js).map_err(|e| e.to_string())
}

/// Destroy it and emit `web-view://closed`; an unknown label is `Ok` (205 §4.3).
#[tauri::command]
pub async fn web_view_close<R: Runtime>(
    app: AppHandle<R>,
    views: State<'_, Views>,
    label: String,
) -> Result<(), String> {
    check_label(&label)?;
    lock(&views).remove(&label);
    let Some(view) = app.get_webview(&label) else {
        return Ok(());
    };
    view.close().map_err(|e| e.to_string())?;
    emit_closed(&app, label);
    Ok(())
}

/// Every cookie of the tiles' store, through WebKit on the named store (209 §4.5, D3). Never for a tile.
#[cfg(target_os = "macos")]
#[tauri::command]
pub async fn web_profile_read<R: Runtime>(caller: Webview<R>) -> Result<Vec<Cookie>, String> {
    check_caller(caller.label(), false)?;
    crate::apple_cookies::read(caller.app_handle().clone()).await
}

/// Windows and Linux: [`PROFILE_UNSUPPORTED`] (209 §4.5 fallback not built), same types as on macOS.
#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub async fn web_profile_read<R: Runtime>(caller: Webview<R>) -> Result<Vec<Cookie>, String> {
    check_caller(caller.label(), false)?;
    Err(PROFILE_UNSUPPORTED.to_owned())
}

/// Empty the tiles' store: every cookie, and localStorage, sessionStorage and IndexedDB (209 §4.5). Never
/// for a tile; emits nothing (the UI reads the store again).
#[cfg(target_os = "macos")]
#[tauri::command]
pub async fn web_profile_clear<R: Runtime>(caller: Webview<R>) -> Result<(), String> {
    check_caller(caller.label(), false)?;
    crate::apple_cookies::clear(caller.app_handle().clone()).await
}

/// Windows and Linux: [`PROFILE_UNSUPPORTED`], same types as on macOS.
#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub async fn web_profile_clear<R: Runtime>(caller: Webview<R>) -> Result<(), String> {
    check_caller(caller.label(), false)?;
    Err(PROFILE_UNSUPPORTED.to_owned())
}

/// Write into the tiles' store (209 §4.5, D5): each cookie of `set` replaces the store cookie of its key
/// and scope, each key of `remove` goes in every scope; each localStorage snapshot of `storage` goes to
/// the open web views of its origin. Counts only, values are never logged; when WebKit has not reported
/// everything within `MAIN_TIMEOUT` the answer is the counts reached with `timed_out`. A loaded page
/// keeps the cookies it has until it loads again: no view is reloaded. Never for a tile.
#[cfg(target_os = "macos")]
#[tauri::command]
pub async fn web_profile_write<R: Runtime>(
    caller: Webview<R>,
    set: Vec<Cookie>,
    remove: Vec<CookieKey>,
    storage: Option<Vec<Storage>>,
) -> Result<Written, String> {
    use std::sync::mpsc::sync_channel;

    use objc2::MainThreadMarker;
    use objc2_web_kit::WKWebView;

    use crate::apple_cookies::{MAIN_TIMEOUT, apply_storage, available, write, writer, written};
    use crate::web_rules::profile::{plan_write, storage_for};
    use crate::web_rules::valid_label;

    check_caller(caller.label(), false)?;
    available()?;
    let deadline = Instant::now() + MAIN_TIMEOUT;
    let plan = plan_write(set, remove, storage.unwrap_or_default());
    let origins = plan.storage.iter().map(|s| s.origin.clone()).collect();
    let app = caller.app_handle().clone();
    let (cookies, wait_cookies) = writer(plan.cookies);
    app.run_on_main_thread(move || match MainThreadMarker::new() {
        Some(mtm) => write(mtm, cookies),
        None => cookies.fail("run_on_main_thread ran off the main thread".to_owned()),
    })
    .map_err(|e| e.to_string())?;
    let mut pending = Vec::new();
    for view in app.webviews().into_values() {
        if !valid_label(view.label()) {
            continue;
        }
        let Ok(url) = view.url() else {
            continue;
        };
        for snapshot in storage_for(&plan.storage, url.as_str()) {
            let (tx, rx) = sync_channel(1);
            let origin = snapshot.origin.clone();
            let snapshot = snapshot.clone();
            let res = view.with_webview(move |platform| {
                let Some(mtm) = MainThreadMarker::new() else {
                    let _ = tx.try_send(Err("with_webview ran off the main thread".to_owned()));
                    return;
                };
                // SAFETY: on macOS `inner()` is the webview's live `WKWebView`, and `with_webview` runs on
                // the main thread.
                let view = unsafe { &*platform.inner().cast::<WKWebView>() };
                apply_storage(mtm, view, &snapshot, tx);
            });
            match res {
                Ok(()) => pending.push((origin, rx)),
                Err(e) => tracing::warn!(%origin, "web profile write: localStorage: {e}"),
            }
        }
    }
    written(wait_cookies, pending, origins, deadline).await
}

/// Windows and Linux: [`PROFILE_UNSUPPORTED`], same types as on macOS.
#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub async fn web_profile_write<R: Runtime>(
    caller: Webview<R>,
    set: Vec<Cookie>,
    remove: Vec<CookieKey>,
    storage: Option<Vec<Storage>>,
) -> Result<Written, String> {
    check_caller(caller.label(), false)?;
    let _ = (set, remove, storage);
    Err(PROFILE_UNSUPPORTED.to_owned())
}

/// The top-level origin's localStorage of `label`, read in the isolated world (209 §4.5). Never for a tile.
#[cfg(target_os = "macos")]
#[tauri::command]
pub async fn web_storage_capture<R: Runtime>(
    caller: Webview<R>,
    label: String,
) -> Result<Storage, String> {
    use objc2::MainThreadMarker;
    use objc2_web_kit::WKWebView;

    check_caller(caller.label(), false)?;
    crate::apple_cookies::available()?;
    let view = webview(caller.app_handle(), &label)?;
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    view.with_webview(move |platform| {
        let Some(mtm) = MainThreadMarker::new() else {
            let _ = tx.try_send(Err("with_webview ran off the main thread".to_owned()));
            return;
        };
        // SAFETY: on macOS `inner()` is the webview's live `WKWebView`, and `with_webview` runs on the main
        // thread.
        let view = unsafe { &*platform.inner().cast::<WKWebView>() };
        crate::apple_cookies::capture(mtm, view, tx);
    })
    .map_err(|e| e.to_string())?;
    crate::apple_cookies::captured(rx).await
}

/// Windows and Linux: [`PROFILE_UNSUPPORTED`] (209 §4.5 fallback not built), same types as on macOS.
#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub async fn web_storage_capture<R: Runtime>(
    caller: Webview<R>,
    label: String,
) -> Result<Storage, String> {
    check_caller(caller.label(), false)?;
    webview(caller.app_handle(), &label)?;
    Err(PROFILE_UNSUPPORTED.to_owned())
}

/// Run one tile op for the agent (209 §4.7, D8): `script`, `navigate`, `back`, `forward`, `reload` or
/// `snapshot` on `label`, with `args` as `web_rules::call::Op::parse` checks them. Only the main webview of
/// the window holding the tile may call it. One log line per call: op, label, duration, ok or error;
/// never a value.
#[tauri::command]
pub async fn web_view_call<R: Runtime>(
    caller: Webview<R>,
    label: String,
    op: String,
    args: Option<Value>,
) -> Result<Value, String> {
    let started = Instant::now();
    let outcome = run_call(&caller, &label, &op, args).await;
    crate::web_rules::call::log(&op, &label, started, &outcome);
    outcome
}

/// The op, once the arguments and the caller pass.
#[cfg(target_os = "macos")]
async fn run_call<R: Runtime>(
    caller: &Webview<R>,
    label: &str,
    op: &str,
    args: Option<Value>,
) -> Result<Value, String> {
    use std::sync::mpsc::sync_channel;

    use objc2::MainThreadMarker;
    use objc2_web_kit::WKWebView;
    use serde_json::json;

    use crate::web_rules::call::SNAPSHOT_TIMEOUT;

    /// `f` with the tile's `WKWebView` on the main thread; when it cannot run there, `f` is dropped with
    /// its sender and the waiting side reads `DROPPED`.
    fn on_tile<R: Runtime>(
        view: &Webview<R>,
        f: impl FnOnce(MainThreadMarker, &WKWebView) + Send + 'static,
    ) -> Result<(), String> {
        view.with_webview(move |platform| {
            if let Some(mtm) = MainThreadMarker::new() {
                // SAFETY: on macOS `inner()` is the webview's live `WKWebView`, and `with_webview` runs on
                // the main thread.
                f(mtm, unsafe { &*platform.inner().cast::<WKWebView>() });
            }
        })
        .map_err(|e| e.to_string())
    }

    let started = Instant::now();
    let op = Op::parse(op, args)?;
    let app = caller.app_handle();
    let view = webview(app, label)?;
    check_holder(
        label,
        caller.label(),
        caller.window().label(),
        view.window().label(),
    )?;
    match op {
        Op::Navigate(url) => {
            view.navigate(check_url(app, &url)?)
                .map_err(|e| e.to_string())?;
            Ok(json!({}))
        }
        Op::Reload => {
            view.reload().map_err(|e| e.to_string())?;
            Ok(json!({}))
        }
        Op::Back | Op::Forward => {
            let forward = op == Op::Forward;
            let (tx, rx) = sync_channel(1);
            on_tile(&view, move |_, wk| {
                // SAFETY: plain history calls on a live view, on the main thread.
                let moved = unsafe {
                    match (forward, wk.canGoForward(), wk.canGoBack()) {
                        (true, true, _) => wk.goForward().is_some(),
                        (false, _, true) => wk.goBack().is_some(),
                        _ => false,
                    }
                };
                let _ = tx.try_send(Ok(moved));
            })?;
            let moved = crate::apple_cookies::wait(rx).await?;
            Ok(json!({ "moved": moved }))
        }
        Op::Script(script) => {
            let (deadline, timeout) = (started + script.timeout, script.timeout);
            let (tx, rx) = sync_channel(1);
            on_tile(&view, move |mtm, wk| {
                crate::apple_tile::script(mtm, wk, &script, tx)
            })?;
            crate::apple_tile::script_done(rx, deadline, timeout).await
        }
        Op::Snapshot => {
            let deadline = started + SNAPSHOT_TIMEOUT;
            let (tx, rx) = sync_channel(1);
            on_tile(&view, move |_, wk| crate::apple_tile::snapshot(wk, tx))?;
            crate::apple_tile::snapshot_done(rx, deadline).await
        }
    }
}

/// Windows and Linux: the same checks, then `CALL_UNSUPPORTED`.
#[cfg(not(target_os = "macos"))]
async fn run_call<R: Runtime>(
    caller: &Webview<R>,
    label: &str,
    op: &str,
    args: Option<Value>,
) -> Result<Value, String> {
    Op::parse(op, args)?;
    let view = webview(caller.app_handle(), label)?;
    check_holder(
        label,
        caller.label(),
        caller.window().label(),
        view.window().label(),
    )?;
    Err(crate::web_rules::call::CALL_UNSUPPORTED.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webview_attach_steal_rule() {
        assert!(steals("popout-web-0123456789abcdef", "main"));
        assert!(steals("popout-web-0123456789abcdef", "popout-chat-x"));
        assert!(!steals("main", "popout-web-0123456789abcdef"));
        assert!(!steals(
            "popout-web-0123456789abcdef",
            "popout-web-0123456789abcdef"
        ));
        assert!(!steals("main", "main"));
        assert!(!steals("island", "main"));
    }

    #[test]
    fn webview_capabilities_name_no_web_view() {
        for (name, text) in [
            ("default.json", include_str!("../capabilities/default.json")),
            ("mobile.json", include_str!("../capabilities/mobile.json")),
        ] {
            let cap: serde_json::Value = serde_json::from_str(text).unwrap();
            assert!(cap.get("remote").is_none(), "{name}: has a `remote` block");
            for key in ["windows", "webviews"] {
                for pattern in cap
                    .get(key)
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                {
                    let pattern = pattern.as_str().unwrap();
                    assert!(
                        !glob_matches(pattern, "web-0123456789abcdef"),
                        "{name}: `{key}` entry `{pattern}` matches a web view label"
                    );
                }
            }
        }
    }

    /// Tauri's capability label patterns are globs; `*` and `?` are all the files use.
    fn glob_matches(pattern: &str, label: &str) -> bool {
        fn go(p: &[u8], s: &[u8]) -> bool {
            match (p.first(), s.first()) {
                (None, None) => true,
                (Some(b'*'), _) => go(&p[1..], s) || (!s.is_empty() && go(p, &s[1..])),
                (Some(b'?'), Some(_)) => go(&p[1..], &s[1..]),
                (Some(a), Some(b)) if a == b => go(&p[1..], &s[1..]),
                _ => false,
            }
        }
        go(pattern.as_bytes(), label.as_bytes())
    }

    #[test]
    fn webview_glob_sanity() {
        assert!(glob_matches("popout-*", "popout-x"));
        assert!(glob_matches("*", "web-0123456789abcdef"));
        assert!(!glob_matches("main", "web-0123456789abcdef"));
    }
}
