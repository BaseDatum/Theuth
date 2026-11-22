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

/// Error of every browser profile command where the tile store cannot be read (209 §4.5): Windows and Linux
/// (Tauri's `Webview::cookies()` fallback is not built: it has no host-only flag and no script result), and
/// macOS < 14 / iOS < 17, where the tiles use the app's own default store, which is never exported.
pub(crate) const PROFILE_UNSUPPORTED: &str =
    "unsupported: the browser profile needs macOS 14 or iOS 17 and later";

/// The browser profile's client-side rules (209 §4.5): who may call the profile commands, what
/// `web_profile_read` returns, what `web_storage_capture` returns, and when `web-profile://changed` fires.
/// Used on Apple platforms only: Windows and Linux answer every profile command with
/// [`PROFILE_UNSUPPORTED`], with the same declared types.
#[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
pub(crate) mod profile {
    use std::fmt;
    use std::time::{Duration, Instant};

    use serde::{Deserialize, Serialize};

    /// Prefix of every profile command's refusal of a caller that may not reach the store.
    const NO_ACCESS: &str = "no access to the browser profile from";

    /// The profile commands answer the app's own webviews only (209 §4.5): never a tile (`web-*`, whose page
    /// is a stranger), and on iOS (`main_only`) only the main webview, the one window there is.
    pub(crate) fn check_caller(caller: &str, main_only: bool) -> Result<(), String> {
        if caller.starts_with(super::PREFIX) || (main_only && caller != "main") {
            Err(format!("{NO_ACCESS} `{caller}`"))
        } else {
            Ok(())
        }
    }

    /// Event to the main window when the tile store's cookies changed, payload `{}` (209 §4.5).
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
    /// `Debug` never prints `value`.
    #[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
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

    impl fmt::Debug for Cookie {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("Cookie")
                .field("name", &self.name)
                .field("value", &"[redacted]")
                .field("domain", &self.domain)
                .field("host_only", &self.host_only)
                .field("path", &self.path)
                .field("expires", &self.expires)
                .field("secure", &self.secure)
                .field("http_only", &self.http_only)
                .field("same_site", &self.same_site)
                .field("partitioned", &self.partitioned)
                .finish()
        }
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

    /// An `NSHTTPCookie`'s attributes as plain values, read by `apple_cookies` on the main thread and mapped
    /// to a [`Cookie`] off it. No `Debug`: it holds the value.
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
        /// The entries of `properties` named in [`PROPERTIES`], values as strings (an `NSURL` as its
        /// absolute string).
        pub(crate) properties: Vec<(String, String)>,
    }

    /// The `NSHTTPCookie.properties` keys the mapping reads (`NSHTTPCookieDomain`,
    /// `NSHTTPCookieOriginURL`, and a `Partitioned` flag WebKit may carry); compared ignoring case.
    pub(crate) const PROPERTIES: [&str; 3] = ["Domain", "OriginURL", "Partitioned"];

    fn property<'a>(properties: &'a [(String, String)], key: &str) -> Option<&'a str> {
        properties
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    /// Which rule of [`scope`] decided a cookie's host-only flag.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum ScopeRule {
        /// A `__Host-` name: the browser accepted it only without `Domain`, so host-only.
        HostPrefix,
        /// The domain is an IP address: no other host can domain-match it, so host-only.
        IpHost,
        /// `NSHTTPCookie.domain` or the `Domain` property has a leading dot: WebKit's mark of a
        /// `Domain=` attribute, so domain-wide.
        Dot,
        /// No dot, but the cookie's `OriginURL` is a subdomain of its domain: a host-only cookie's domain
        /// is its origin's host, so this one was set with `Domain=` and is domain-wide.
        OriginSubdomain,
        /// No dot and no origin elsewhere: host-only.
        Bare,
    }

    /// A cookie's domain (lower-case, no leading dot) and whether it is host-only (209 §4.1, §4.3).
    /// `NSHTTPCookie` has no host-only flag; WebKit keeps the leading dot of a `Domain=` cookie, and that
    /// is cross-checked against `properties` (`Domain`, `OriginURL`) rather than trusted alone.
    pub(crate) fn scope(
        name: &str,
        domain: &str,
        properties: &[(String, String)],
    ) -> (String, bool, ScopeRule) {
        let declared = property(properties, "Domain").unwrap_or(domain);
        let dotted = domain.starts_with('.') || declared.starts_with('.');
        let bare = domain.trim_start_matches('.').to_ascii_lowercase();
        let ip = bare
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .is_ok();
        let origin_sub = property(properties, "OriginURL")
            .and_then(|u| tauri::Url::parse(u).ok())
            .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
            .is_some_and(|host| host.ends_with(&format!(".{bare}")));
        let rule = if name.starts_with("__Host-") {
            ScopeRule::HostPrefix
        } else if ip {
            ScopeRule::IpHost
        } else if dotted {
            ScopeRule::Dot
        } else if origin_sub {
            ScopeRule::OriginSubdomain
        } else {
            ScopeRule::Bare
        };
        let host_only = matches!(
            rule,
            ScopeRule::HostPrefix | ScopeRule::IpHost | ScopeRule::Bare
        );
        (bare, host_only, rule)
    }

    impl From<NativeCookie> for Cookie {
        fn from(n: NativeCookie) -> Self {
            let (domain, host_only, rule) = scope(&n.name, &n.domain, &n.properties);
            // No values: which rule decided, to confirm the leading-dot behaviour on a device.
            tracing::debug!(
                name_len = n.name.len(),
                dotted = n.domain.starts_with('.'),
                has_origin = property(&n.properties, "OriginURL").is_some(),
                host_only,
                rule = ?rule,
                "web profile cookie scope"
            );
            Cookie {
                name: n.name,
                value: n.value,
                domain,
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

    /// One origin's localStorage snapshot at most this many bytes, measured as UTF-8 of
    /// `JSON.stringify(local_storage)` (209 §4.1; the UI's `STORAGE_MAX_BYTES`).
    pub(crate) const STORAGE_MAX_BYTES: usize = 96 * 1024;
    const _: () = assert!(
        STORAGE_MAX_BYTES == 98304,
        "CAPTURE_SCRIPT spells the cap out"
    );

    /// Body of the `callAsyncJavaScript` that `web_storage_capture` runs in [`AGENT_WORLD`], in the main
    /// frame: the top-level origin and its localStorage as JSON. The size is counted while the list is built
    /// (exactly the UTF-8 length of `JSON.stringify(local_storage)`); past [`STORAGE_MAX_BYTES`] the snapshot
    /// is dropped whole in the page (209 §4.1) and an empty `local_storage` crosses the bridge instead.
    pub(crate) const CAPTURE_SCRIPT: &str = concat!(
        "const max=",
        98304,
        ",e=new TextEncoder(),s=[];let n=1;",
        "for(let i=0;i<localStorage.length;i++){const k=localStorage.key(i);if(k===null)continue;",
        "const p=[k,localStorage.getItem(k)??\"\"];n+=e.encode(JSON.stringify(p)).length+1;",
        "if(n>max)return JSON.stringify({origin:location.origin,local_storage:[]});s.push(p);}",
        "return JSON.stringify({origin:location.origin,local_storage:s});"
    );

    /// `web_storage_capture` result (209 §4.5). `Debug` never prints the stored keys or values.
    #[derive(PartialEq, Eq, Serialize, Deserialize)]
    pub(crate) struct Storage {
        pub(crate) origin: String,
        pub(crate) local_storage: Vec<(String, String)>,
    }

    impl fmt::Debug for Storage {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("Storage")
                .field("origin", &self.origin)
                .field(
                    "local_storage",
                    &format_args!("[redacted; {} entries]", self.local_storage.len()),
                )
                .finish()
        }
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
                name: "sid".into(),
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
            assert_eq!(
                diff(std::slice::from_ref(&a), std::slice::from_ref(&a_strict)).set,
                vec![a_strict]
            );
            // The same name under another path is another cookie.
            let mut a_sub = a.clone();
            a_sub.path = "/sub".into();
            let d = diff(std::slice::from_ref(&a), &[a.clone(), a_sub.clone()]);
            assert_eq!(d.set, vec![a_sub]);
            assert!(d.removed.is_empty());
            assert_eq!(diff(std::slice::from_ref(&a), &[]).removed, vec![a.key()]);
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

        #[test]
        fn profile_cookie_scope() {
            let props = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
                pairs
                    .iter()
                    .map(|(k, v)| ((*k).into(), (*v).into()))
                    .collect()
            };
            let host = |domain: &str, p: &[(&str, &str)]| {
                let (d, host_only, rule) = scope("sid", domain, &props(p));
                (d, host_only, rule)
            };
            // `Domain=example.com`: WebKit's leading dot, on the cookie or in its `Domain` property.
            assert_eq!(
                host(".Example.com", &[]),
                ("example.com".into(), false, ScopeRule::Dot)
            );
            assert_eq!(
                host("example.com", &[("Domain", ".example.com")]),
                ("example.com".into(), false, ScopeRule::Dot)
            );
            // No `Domain`: a bare host, whose origin is that host.
            assert_eq!(
                host("app.example.com", &[]),
                ("app.example.com".into(), true, ScopeRule::Bare)
            );
            assert_eq!(
                host(
                    "app.example.com",
                    &[("OriginURL", "https://app.example.com/login")]
                ),
                ("app.example.com".into(), true, ScopeRule::Bare)
            );
            // No dot, but set from a subdomain: it had `Domain=`.
            assert_eq!(
                host("example.com", &[("originurl", "https://app.example.com/")]),
                ("example.com".into(), false, ScopeRule::OriginSubdomain)
            );
            // A suffix that is not a label boundary is not a subdomain.
            assert_eq!(
                host("example.com", &[("OriginURL", "https://badexample.com/")]),
                ("example.com".into(), true, ScopeRule::Bare)
            );
            // `__Host-` is host-only whatever the domain says.
            let (d, host_only, rule) = scope(
                "__Host-sid",
                ".example.com",
                &props(&[("OriginURL", "https://a.example.com/")]),
            );
            assert_eq!(
                (d.as_str(), host_only, rule),
                ("example.com", true, ScopeRule::HostPrefix)
            );
            // IP hosts: host-only, dot or not.
            for ip in ["192.168.1.10", ".10.0.0.1", "[::1]", "::1"] {
                let (_, host_only, rule) = scope("sid", ip, &[]);
                assert_eq!((host_only, rule), (true, ScopeRule::IpHost), "{ip}");
            }
        }

        #[test]
        fn profile_debug_redacts_values() {
            let c = cookie("sid", "s3cr3t-cookie");
            let s = Storage {
                origin: "https://app.example.com".into(),
                local_storage: vec![("t0k3n-key".into(), "s3cr3t-storage".into())],
            };
            let printed = format!("{c:?} {s:?} {:?}", diff(&[], std::slice::from_ref(&c)));
            for secret in ["s3cr3t-cookie", "s3cr3t-storage", "t0k3n-key"] {
                assert!(!printed.contains(secret), "{secret} in {printed}");
            }
            assert!(printed.contains("[redacted]") && printed.contains("app.example.com"));
        }

        #[test]
        fn profile_caller_rule() {
            assert!(check_caller("main", false).is_ok());
            assert!(check_caller("popout-web-0123456789abcdef", false).is_ok());
            assert!(check_caller("island", false).is_ok());
            assert!(check_caller("main", true).is_ok());
            for (caller, main_only) in [
                ("web-0123456789abcdef", false),
                ("web-0123456789abcdef", true),
                ("web-", false),
                ("island", true),
                ("popout-chat-x", true),
            ] {
                let e = check_caller(caller, main_only).unwrap_err();
                assert!(
                    e.starts_with("no access to the browser profile from"),
                    "{caller}"
                );
            }
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
