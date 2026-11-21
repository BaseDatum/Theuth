//! The web view tile's pure rules (205 §4.3, §5, §9), shared by the desktop child webviews (`webview.rs`) and the
//! iOS shell's raw `WKWebView`s (`ios_webview.rs`): labels, events, error prefixes, the load / navigation
//! allowlist, the browser-open throttle and the data store identifier.

use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Runtime, Url};

/// At most one system-browser open per web view per this interval (205 §4.3): WebView2 needs no user
/// gesture for `window.open`, so a page loop could otherwise spam tabs.
pub(crate) const OPEN_INTERVAL: Duration = Duration::from_secs(1);

/// Whether a new-window request at `now` may open the browser, given the last one that did.
pub(crate) fn may_open(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|last| now.saturating_duration_since(last) >= OPEN_INTERVAL)
}

/// Label prefix of every web view (205 D3).
const PREFIX: &str = "web-";

/// Event to every window on navigation start, page load finished and title change (205 §4.3).
pub(crate) const STATE_EVENT: &str = "web-view://state";

/// Event to every window when a web view is destroyed, payload `{ label }` (205 §4.3).
pub(crate) const CLOSED_EVENT: &str = "web-view://closed";

/// Prefix of the error of every command on a label that does not exist (205 §4.3).
const NO_VIEW: &str = "no web view";

/// Prefix of `web_view_attach`'s refusal to take a view out of a pop-out; the parent label follows (205 §4.3).
/// Unreachable on iOS: one window, no pop-outs (205 §9).
#[cfg_attr(target_os = "ios", allow(dead_code))]
const IN_POPOUT: &str = "in-popout: ";

/// `WKWebsiteDataStore` identifier shared by all web views, separate from the app's default store
/// (205 D3, §9). A fixed random UUID; changing it forgets every web view's cookies.
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) const DATA_STORE: [u8; 16] = [
    0x6f, 0x1c, 0x2e, 0x95, 0x4b, 0x7a, 0x4d, 0x0e, 0x9a, 0x53, 0x2d, 0xc8, 0x71, 0x0b, 0xe4, 0x3f,
];

/// Schemes this app serves; on Windows they appear as `http://<scheme>.localhost`, which Tauri treats as a
/// local origin (205 §5). `ipc`, `asset` and `tauri` are Tauri's own, `theuth-page` is `pages::SCHEME`.
const APP_SCHEMES: [&str; 4] = ["tauri", "ipc", "asset", crate::pages::SCHEME];

/// `web-view://closed` payload.
#[derive(Clone, Serialize)]
pub(crate) struct Closed {
    pub(crate) label: String,
}

pub(crate) fn no_view(label: &str) -> String {
    format!("{NO_VIEW} `{label}`")
}

#[cfg_attr(target_os = "ios", allow(dead_code))]
pub(crate) fn in_popout(parent: &str) -> String {
    format!("{IN_POPOUT}{parent}")
}

/// `^web-[0-9a-f]{16}$`.
pub(crate) fn valid_label(label: &str) -> bool {
    label.strip_prefix(PREFIX).is_some_and(|hex| {
        hex.len() == 16
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

pub(crate) fn check_label(label: &str) -> Result<(), String> {
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
pub(crate) fn allowed(url: &Url, app_url: Option<&Url>) -> bool {
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
pub(crate) fn navigation_allowed(url: &Url, app_url: Option<&Url>) -> bool {
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
pub(crate) fn app_url<R: Runtime>(app: &AppHandle<R>) -> Option<Url> {
    if tauri::is_dev() {
        app.config().build.dev_url.clone()
    } else {
        None
    }
}

/// Parse and check a URL for attach/navigate.
pub(crate) fn check_url<R: Runtime>(app: &AppHandle<R>, url: &str) -> Result<Url, String> {
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
    fn webview_new_window_throttle() {
        let t0 = Instant::now();
        assert!(may_open(None, t0));
        assert!(!may_open(Some(t0), t0));
        assert!(!may_open(Some(t0), t0 + Duration::from_millis(999)));
        assert!(may_open(Some(t0), t0 + OPEN_INTERVAL));
        // A clock that did not advance never panics and stays closed.
        assert!(!may_open(Some(t0 + OPEN_INTERVAL), t0));
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
}
