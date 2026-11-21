//! Native child webviews for the web view tile (205 §4.3): a `web-<16 hex>` webview inside the caller's window,
//! positioned over a placeholder the UI measures. The page is a stranger (205 §5): http/https only, no
//! capability names it, no `initialization_script`, its own data store shared by all web views.
//!
//! IPC fence (205 §5): Tauri attaches its IPC handler to every webview and grants a child the capabilities of
//! its *window* label when the request origin is local (`tauri-2.12.5/src/ipc/authority.rs:476-478`), and
//! lets app commands through for any local origin when the app has no ACL manifest
//! (`tauri-2.12.5/src/webview/mod.rs:2080`). "Local" is the app URL (`devUrl` in dev builds) or a registered
//! scheme, which on Windows is `http://<scheme>.localhost` (`webview/mod.rs:1961-2002`). [`allowed`] refuses
//! exactly those, so a web view only ever runs at a remote origin, where every command is rejected.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Serialize;
use tauri::webview::{NewWindowResponse, PageLoadEvent, WebviewBuilder};
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Runtime, State, Url, Webview,
    WebviewUrl, Window,
};
use tauri_plugin_opener::OpenerExt;

/// Label prefix of every web view (205 D3).
const PREFIX: &str = "web-";

/// Event to every window on navigation start, page load finished and title change (205 §4.3).
const STATE_EVENT: &str = "web-view://state";

/// Event to every window when a web view is destroyed, payload `{ label }` (205 §4.3).
const CLOSED_EVENT: &str = "web-view://closed";

/// Prefix of the error of every command on a label that does not exist (205 §4.3).
const NO_VIEW: &str = "no web view";

/// Prefix of `web_view_attach`'s refusal to take a view out of a pop-out; the parent label follows (205 §4.3).
const IN_POPOUT: &str = "in-popout: ";

/// macOS `WKWebsiteDataStore` identifier shared by all web views, separate from the app's default store
/// (205 D3). A fixed random UUID; changing it forgets every web view's cookies.
#[cfg(target_os = "macos")]
const DATA_STORE: [u8; 16] = [
    0x6f, 0x1c, 0x2e, 0x95, 0x4b, 0x7a, 0x4d, 0x0e, 0x9a, 0x53, 0x2d, 0xc8, 0x71, 0x0b, 0xe4, 0x3f,
];

/// Schemes this app serves; on Windows they appear as `http://<scheme>.localhost`, which Tauri treats as a
/// local origin (205 §5). `ipc`, `asset` and `tauri` are Tauri's own, `theuth-page` is `pages::SCHEME`.
const APP_SCHEMES: [&str; 4] = ["tauri", "ipc", "asset", crate::pages::SCHEME];

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

#[derive(Clone, Serialize)]
struct Closed {
    label: String,
}

/// `web_view_attach` may not take a view out of a pop-out into another window (205 §4.3): the pop-out
/// owns it until it closes. `main` → pop-out and re-attaching in the same window are fine.
fn steals(parent: &str, caller: &str) -> bool {
    parent != caller && parent.starts_with(crate::commands::POPOUT_PREFIX)
}

fn no_view(label: &str) -> String {
    format!("{NO_VIEW} `{label}`")
}

fn in_popout(parent: &str) -> String {
    format!("{IN_POPOUT}{parent}")
}

/// `^web-[0-9a-f]{16}$`.
fn valid_label(label: &str) -> bool {
    label.strip_prefix(PREFIX).is_some_and(|hex| {
        hex.len() == 16
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

fn check_label(label: &str) -> Result<(), String> {
    if valid_label(label) {
        Ok(())
    } else {
        Err(format!(
            "web view label must match ^web-[0-9a-f]{{16}}$, got `{label}`"
        ))
    }
}

/// The load check of `web_view_attach` / `web_view_navigate` (205 D3, §5): http/https, and never an origin
/// Tauri would call local. `app_url` is the `devUrl` in dev builds (the only http app origin); `None` in release.
fn allowed(url: &Url, app_url: Option<&Url>) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if host
        .strip_suffix(".localhost")
        .is_some_and(|sub| APP_SCHEMES.contains(&sub))
    {
        return false;
    }
    !app_url.is_some_and(|app| {
        app.scheme() == url.scheme()
            && app.host() == url.host()
            && app.port_or_known_default() == url.port_or_known_default()
    })
}

/// The `on_navigation` predicate (205 §4.3, §5): wry applies it to every frame, so on top of [`allowed`] it
/// lets through the subframe URLs real pages create, none of which can carry the app's origin:
/// `about:blank` / `about:srcdoc` and `blob:` inherit the creator's origin, `data:` is opaque. A `blob:` whose
/// inner part parses as a URL must pass [`allowed`] (`blob:http://tauri.localhost/…` is refused); an opaque
/// creator (`blob:null/…`) has no inner URL and passes.
fn navigation_allowed(url: &Url, app_url: Option<&Url>) -> bool {
    match url.scheme() {
        "about" => matches!(url.path(), "blank" | "srcdoc"),
        "data" => true,
        "blob" => Url::parse(url.path())
            .ok()
            .is_none_or(|inner| allowed(&inner, app_url)),
        _ => allowed(url, app_url),
    }
}

/// The app origin Tauri treats as local besides its schemes: `devUrl`, in dev builds only
/// (`tauri-2.12.5/src/manager/mod.rs:345-358`).
fn app_url<R: Runtime>(app: &AppHandle<R>) -> Option<Url> {
    if tauri::is_dev() {
        app.config().build.dev_url.clone()
    } else {
        None
    }
}

/// Parse and check a URL for attach/navigate.
fn check_url<R: Runtime>(app: &AppHandle<R>, url: &str) -> Result<Url, String> {
    let parsed = Url::parse(url).map_err(|e| format!("invalid url `{url}`: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!("web view loads only http/https, got `{url}`"));
    }
    if !allowed(&parsed, app_url(app).as_ref()) {
        return Err(format!(
            "`{url}` is an app origin; a web view cannot load it"
        ));
    }
    Ok(parsed)
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
    let builder = WebviewBuilder::new(label, WebviewUrl::External(url))
        .on_navigation(move |url| navigation_allowed(url, fence.as_ref()))
        // `target=_blank` / `window.open`: the system browser, never a window of ours.
        .on_new_window(move |url, _features| {
            if matches!(url.scheme(), "http" | "https")
                && let Err(e) = opener.opener().open_url(url.as_str(), None::<&str>)
            {
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
        Ok(_) => Ok(()),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn webview_label_validation() {
        assert!(valid_label("web-0123456789abcdef"));
        assert!(!valid_label("web-0123456789ABCDEF"));
        assert!(!valid_label("web-0123456789abcde"));
        assert!(!valid_label("web-0123456789abcdef0"));
        assert!(!valid_label("web-0123456789abcdeg"));
        assert!(!valid_label("popout-0123456789abcdef"));
        assert!(!valid_label("main"));
        assert!(!valid_label(""));
    }

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
    fn webview_error_prefixes() {
        assert!(no_view("web-0123456789abcdef").starts_with("no web view"));
        assert_eq!(
            in_popout("popout-web-0123456789abcdef"),
            "in-popout: popout-web-0123456789abcdef"
        );
    }

    #[test]
    fn webview_navigation_allowlist() {
        let dev = url("http://localhost:1420");
        let ok_both = [
            "http://localhost:3000/",
            "https://example.com/a?b#c",
            "http://127.0.0.1:1420/",
            "http://app.localhost/",
            "https://localhost:1420/",
        ];
        let bad_both = [
            "http://localhost:1420/",
            "http://localhost:1420/sessions?x=1",
            "http://tauri.localhost/",
            "https://tauri.localhost/",
            "http://ipc.localhost/cmd",
            "http://asset.localhost/x",
            "http://theuth-page.localhost/abc",
            "http://THEUTH-PAGE.localhost/abc",
            "tauri://localhost/",
            "asset://localhost/x",
            "theuth-page://localhost/abc",
            "ipc://localhost/cmd",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "about:config",
            "blob:http://tauri.localhost/0b8e",
            "blob:http://localhost:1420/0b8e",
            "blob:tauri://localhost/0b8e",
            "blob:theuth-page://localhost/0b8e",
        ];
        // Subframe URLs: fine to navigate to, never a load target for attach/navigate.
        let ok_navigation_only = [
            "about:blank",
            "about:srcdoc",
            "data:text/html,hi",
            "blob:https://example.com/0b8e",
            "blob:http://localhost:3000/0b8e",
            "blob:null/0b8e",
        ];
        for ok in ok_both {
            assert!(allowed(&url(ok), Some(&dev)), "load {ok}");
            assert!(navigation_allowed(&url(ok), Some(&dev)), "navigate {ok}");
        }
        for bad in bad_both {
            assert!(!allowed(&url(bad), Some(&dev)), "load {bad}");
            assert!(!navigation_allowed(&url(bad), Some(&dev)), "navigate {bad}");
        }
        for sub in ok_navigation_only {
            assert!(!allowed(&url(sub), Some(&dev)), "load {sub}");
            assert!(navigation_allowed(&url(sub), Some(&dev)), "navigate {sub}");
        }
        // Release: no http app origin.
        assert!(allowed(&url("http://localhost:1420/"), None));
        assert!(navigation_allowed(
            &url("blob:http://localhost:1420/0b8e"),
            None
        ));
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
