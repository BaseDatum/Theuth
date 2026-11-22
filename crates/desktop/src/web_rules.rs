//! The web view tile's pure rules (205 §4.3, §5, §9), shared by the desktop child webviews (`webview.rs`) and the
//! iOS shell's raw `WKWebView`s (`ios_webview.rs`): labels, events, error prefixes, the load / navigation
//! allowlist, the browser-open throttle and the data store identifier; and, in [`profile`], the browser
//! profile's cookie shape, jar diff, change debounce and storage-capture parsing (209 §4.1, §4.2, §4.5).

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

/// `web_profile_read` / `web_storage_capture` on Windows and Linux: the store is not read there yet
/// (209 §4.5 allows Tauri's `Webview::cookies()`; it has no host-only flag and no script result).
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
pub(crate) const PROFILE_UNSUPPORTED: &str =
    "unsupported: the browser profile is read on macOS and iOS only";

/// The browser profile's client-side rules (209 §4.5): what `web_profile_read` returns, what
/// `web_storage_capture` returns, and when `web-profile://changed` fires. Apple platforms only: Windows and
/// Linux answer both commands with [`PROFILE_UNSUPPORTED`].
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) mod profile {
    use std::time::{Duration, Instant};

    use serde::{Deserialize, Serialize};

    /// Event to every window when the tile store's cookies changed, payload `{}` (209 §4.5).
    pub(crate) const CHANGED_EVENT: &str = "web-profile://changed";

    /// A change is reported once the store has been quiet this long (209 §4.5, D3).
    pub(crate) const QUIET: Duration = Duration::from_secs(2);

    /// …or at the latest this long after the first unreported change, so a page that sets a cookie every
    /// second is still reported.
    pub(crate) const MAX_WAIT: Duration = Duration::from_secs(10);

    /// `web-profile://changed` payload: empty, the UI reads the store itself (209 §4.5).
    #[derive(Clone, Serialize)]
    pub(crate) struct Changed {}

    /// One cookie in the contract's shape (209 §4.1). `domain` has no leading dot; `host_only` says whether
    /// it was set without a `Domain` attribute. `expires` is Unix seconds, `None` for a session cookie.
    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub(crate) struct Cookie {
        pub(crate) name: String,
        pub(crate) value: String,
        pub(crate) domain: String,
        pub(crate) host_only: bool,
        pub(crate) path: String,
        pub(crate) expires: Option<i64>,
        pub(crate) secure: bool,
        pub(crate) http_only: bool,
        /// `"Strict" | "Lax" | "None" | ""`.
        pub(crate) same_site: String,
        pub(crate) partitioned: bool,
    }

    /// The identity of a cookie, `(name, domain, path)` (209 §4.2).
    #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
    pub(crate) struct CookieKey {
        pub(crate) name: String,
        pub(crate) domain: String,
        pub(crate) path: String,
    }

    impl Cookie {
        pub(crate) fn key(&self) -> CookieKey {
            CookieKey {
                name: self.name.clone(),
                domain: self.domain.clone(),
                path: self.path.clone(),
            }
        }
    }

    /// An `NSHTTPCookie`'s attributes as plain values, read by `apple_cookies`.
    pub(crate) struct NativeCookie {
        pub(crate) name: String,
        pub(crate) value: String,
        /// As `NSHTTPCookie.domain` gives it: a leading dot for a `Domain` cookie.
        pub(crate) domain: String,
        pub(crate) path: String,
        /// `expiresDate.timeIntervalSince1970`; ignored when `session_only`.
        pub(crate) expires: Option<f64>,
        pub(crate) session_only: bool,
        pub(crate) secure: bool,
        pub(crate) http_only: bool,
        /// `sameSitePolicy`: `"lax"`, `"strict"`, or absent.
        pub(crate) same_site: Option<String>,
        /// Every entry of `properties`, values as their description.
        pub(crate) properties: Vec<(String, String)>,
    }

    impl From<NativeCookie> for Cookie {
        fn from(n: NativeCookie) -> Self {
            let (domain, host_only) = match n.domain.strip_prefix('.') {
                Some(rest) => (rest.to_owned(), false),
                None => (n.domain, true),
            };
            Cookie {
                name: n.name,
                value: n.value,
                domain: domain.to_ascii_lowercase(),
                host_only,
                path: if n.path.is_empty() {
                    "/".to_owned()
                } else {
                    n.path
                },
                expires: if n.session_only {
                    None
                } else {
                    n.expires.map(|s| s.floor() as i64)
                },
                secure: n.secure,
                http_only: n.http_only,
                same_site: same_site(n.same_site.as_deref()).to_owned(),
                partitioned: partitioned(&n.properties),
            }
        }
    }

    /// `sameSitePolicy` → the contract's spelling; anything unknown is `""` (unspecified).
    fn same_site(policy: Option<&str>) -> &'static str {
        match policy.map(str::to_ascii_lowercase).as_deref() {
            Some("strict") => "Strict",
            Some("lax") => "Lax",
            Some("none") => "None",
            _ => "",
        }
    }

    /// `NSHTTPCookie` has no partition attribute (209 D1); a `Partitioned` property that WebKit may carry
    /// is honoured when it is truthy, else the cookie is unpartitioned.
    fn partitioned(properties: &[(String, String)]) -> bool {
        properties.iter().any(|(k, v)| {
            k.eq_ignore_ascii_case("partitioned")
                && matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes")
        })
    }

    /// Cookies set (new or any attribute changed) and removed between two reads of the store (209 §4.2).
    #[derive(Debug, Default, PartialEq, Eq)]
    pub(crate) struct Delta {
        pub(crate) set: Vec<Cookie>,
        pub(crate) removed: Vec<CookieKey>,
    }

    impl Delta {
        pub(crate) fn is_empty(&self) -> bool {
            self.set.is_empty() && self.removed.is_empty()
        }
    }

    /// `new` against `old`, keyed `(name, domain, path)`; for a key present twice in one list the last wins.
    pub(crate) fn diff(old: &[Cookie], new: &[Cookie]) -> Delta {
        use std::collections::BTreeMap;
        let old: BTreeMap<CookieKey, &Cookie> = old.iter().map(|c| (c.key(), c)).collect();
        let new: BTreeMap<CookieKey, &Cookie> = new.iter().map(|c| (c.key(), c)).collect();
        Delta {
            set: new
                .iter()
                .filter(|(k, c)| old.get(*k) != Some(*c))
                .map(|(_, c)| (*c).clone())
                .collect(),
            removed: old
                .keys()
                .filter(|k| !new.contains_key(*k))
                .cloned()
                .collect(),
        }
    }

    /// Trailing debounce of store changes: due [`QUIET`] after the last change, or [`MAX_WAIT`] after the
    /// first unreported one, whichever comes first.
    #[derive(Debug, Default)]
    pub(crate) struct Debounce {
        first: Option<Instant>,
        last: Option<Instant>,
    }

    #[derive(Debug, PartialEq, Eq)]
    pub(crate) enum Due {
        /// Nothing unreported.
        Idle,
        /// Report now; the debounce is reset.
        Fire,
        /// Ask again after this long.
        Wait(Duration),
    }

    impl Debounce {
        /// Record a change at `now`. `true` when nothing was pending: the caller starts the timer.
        pub(crate) fn change(&mut self, now: Instant) -> bool {
            let idle = self.first.is_none();
            self.first.get_or_insert(now);
            self.last = Some(now);
            idle
        }

        pub(crate) fn poll(&mut self, now: Instant) -> Due {
            let (Some(first), Some(last)) = (self.first, self.last) else {
                return Due::Idle;
            };
            let quiet = QUIET.saturating_sub(now.saturating_duration_since(last));
            let max = MAX_WAIT.saturating_sub(now.saturating_duration_since(first));
            let wait = quiet.min(max);
            if wait.is_zero() {
                *self = Debounce::default();
                Due::Fire
            } else {
                Due::Wait(wait)
            }
        }
    }

    /// Name of the isolated `WKContentWorld` the shell's scripts run in (209 §4.7).
    pub(crate) const AGENT_WORLD: &str = "theuth-agent";

    /// Body of the `callAsyncJavaScript` that `web_storage_capture` runs in [`AGENT_WORLD`], in the main
    /// frame: the top-level origin and its localStorage as JSON.
    pub(crate) const CAPTURE_SCRIPT: &str = "const s=[];for(let i=0;i<localStorage.length;i++){const k=localStorage.key(i);if(k!==null){s.push([k,localStorage.getItem(k)??\"\"]);}}return JSON.stringify({origin:location.origin,local_storage:s});";

    /// `web_storage_capture` result (209 §4.5).
    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub(crate) struct Storage {
        pub(crate) origin: String,
        pub(crate) local_storage: Vec<(String, String)>,
    }

    /// The capture script's JSON, refused unless the origin is http(s) (an `about:blank` or opaque page
    /// has no origin to store under).
    pub(crate) fn parse_capture(json: &str) -> Result<Storage, String> {
        // The error names the position only: serde's message can quote a storage value.
        let storage: Storage = serde_json::from_str(json).map_err(|e| {
            format!(
                "storage capture returned an unexpected shape (at column {})",
                e.column()
            )
        })?;
        let web = tauri::Url::parse(&storage.origin)
            .is_ok_and(|u| matches!(u.scheme(), "http" | "https"));
        if !web {
            return Err(format!("{NO_ORIGIN} `{}`", storage.origin));
        }
        Ok(storage)
    }

    /// Prefix of `web_storage_capture`'s refusal for a page without an http(s) origin.
    const NO_ORIGIN: &str = "no web origin";

    #[cfg(test)]
    mod tests {
        use super::*;

        fn native(domain: &str) -> NativeCookie {
            NativeCookie {
                name: "__Host-sid".into(),
                value: "v".into(),
                domain: domain.into(),
                path: "/".into(),
                expires: Some(1_795_000_000.75),
                session_only: false,
                secure: true,
                http_only: true,
                same_site: Some("lax".into()),
                properties: vec![],
            }
        }

        fn cookie(name: &str, value: &str) -> Cookie {
            Cookie::from(NativeCookie {
                name: name.into(),
                value: value.into(),
                ..native("app.example.com")
            })
        }

        #[test]
        fn profile_cookie_mapping() {
            let c = Cookie::from(native("App.Example.com"));
            assert_eq!(
                c,
                Cookie {
                    name: "__Host-sid".into(),
                    value: "v".into(),
                    domain: "app.example.com".into(),
                    host_only: true,
                    path: "/".into(),
                    expires: Some(1_795_000_000),
                    secure: true,
                    http_only: true,
                    same_site: "Lax".into(),
                    partitioned: false,
                }
            );
            let d = Cookie::from(NativeCookie {
                session_only: true,
                same_site: None,
                path: String::new(),
                properties: vec![("Partitioned".into(), "TRUE".into())],
                ..native(".example.com")
            });
            assert_eq!((d.domain.as_str(), d.host_only), ("example.com", false));
            assert_eq!(d.expires, None);
            assert_eq!(d.same_site, "");
            assert_eq!(d.path, "/");
            assert!(d.partitioned);
            for (policy, want) in [("strict", "Strict"), ("None", "None"), ("bogus", "")] {
                let c = Cookie::from(NativeCookie {
                    same_site: Some(policy.into()),
                    ..native("a.test")
                });
                assert_eq!(c.same_site, want, "{policy}");
            }
            let json = serde_json::to_value(&c).unwrap();
            let keys: Vec<&str> = json
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            for key in [
                "name",
                "value",
                "domain",
                "host_only",
                "path",
                "expires",
                "secure",
                "http_only",
                "same_site",
                "partitioned",
            ] {
                assert!(keys.contains(&key), "{key}");
            }
            assert_eq!(keys.len(), 10);
        }

        #[test]
        fn profile_jar_diff() {
            let a = cookie("a", "1");
            let b = cookie("b", "2");
            let b2 = cookie("b", "3");
            let c = cookie("c", "4");
            assert!(diff(&[a.clone(), b.clone()], &[b.clone(), a.clone()]).is_empty());
            let d = diff(&[a.clone(), b], &[b2.clone(), c.clone()]);
            assert_eq!(d.set, vec![b2, c]);
            assert_eq!(d.removed, vec![a.key()]);
            // An attribute other than the value is a change too.
            let mut a_strict = a.clone();
            a_strict.same_site = "Strict".into();
            assert_eq!(diff(&[a.clone()], &[a_strict.clone()]).set, vec![a_strict]);
            // The same name under another path is another cookie.
            let mut a_sub = a.clone();
            a_sub.path = "/sub".into();
            let d = diff(&[a.clone()], &[a.clone(), a_sub.clone()]);
            assert_eq!(d.set, vec![a_sub]);
            assert!(d.removed.is_empty());
            assert_eq!(diff(&[a.clone()], &[]).removed, vec![a.key()]);
        }

        #[test]
        fn profile_change_debounce() {
            let t0 = Instant::now();
            let mut d = Debounce::default();
            assert_eq!(d.poll(t0), Due::Idle);
            assert!(d.change(t0));
            assert!(!d.change(t0 + Duration::from_millis(500)));
            assert_eq!(
                d.poll(t0 + Duration::from_millis(1500)),
                Due::Wait(Duration::from_secs(1))
            );
            assert_eq!(d.poll(t0 + Duration::from_millis(2500)), Due::Fire);
            assert_eq!(d.poll(t0 + Duration::from_secs(3)), Due::Idle);
            // A change every second never goes quiet: due MAX_WAIT after the first.
            let mut d = Debounce::default();
            assert!(d.change(t0));
            for s in 1..=9 {
                d.change(t0 + Duration::from_secs(s));
                assert!(matches!(d.poll(t0 + Duration::from_secs(s)), Due::Wait(_)));
            }
            assert_eq!(
                d.poll(t0 + Duration::from_millis(9500)),
                Due::Wait(Duration::from_millis(500))
            );
            assert_eq!(d.poll(t0 + MAX_WAIT), Due::Fire);
            // A clock that did not advance never panics.
            let mut d = Debounce::default();
            d.change(t0 + QUIET);
            assert_eq!(d.poll(t0), Due::Wait(QUIET));
        }

        #[test]
        fn profile_storage_capture() {
            let s = parse_capture(
                r#"{"origin":"https://app.example.com","local_storage":[["k","v"],["e",""]]}"#,
            )
            .unwrap();
            assert_eq!(s.origin, "https://app.example.com");
            assert_eq!(
                s.local_storage,
                vec![("k".into(), "v".into()), ("e".into(), String::new())]
            );
            assert_eq!(
                serde_json::to_value(&s).unwrap(),
                serde_json::json!({"origin": "https://app.example.com", "local_storage": [["k", "v"], ["e", ""]]})
            );
            for origin in ["null", "file://", "about:blank"] {
                let json = format!(r#"{{"origin":"{origin}","local_storage":[]}}"#);
                assert!(
                    parse_capture(&json)
                        .unwrap_err()
                        .starts_with("no web origin"),
                    "{origin}"
                );
            }
            assert!(parse_capture("[]").is_err());
        }
    }
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
