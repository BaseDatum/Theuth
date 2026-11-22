//! The web view tile's pure rules (205 §4.3, §5, §9), shared by the desktop child webviews (`webview.rs`) and the
//! iOS shell's raw `WKWebView`s (`ios_webview.rs`): labels, events, error prefixes, the load / navigation
//! allowlist, the browser-open throttle and the data store identifier; and, in [`profile`], the browser
//! profile's cookie shape, jar diff, change debounce, storage-capture parsing and the write plan
//! (209 §4.1, §4.2, §4.5).

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
        let origin_sub = property(properties, "OriginURL")
            .and_then(|u| tauri::Url::parse(u).ok())
            .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
            .is_some_and(|host| host.ends_with(&format!(".{bare}")));
        let rule = if name.starts_with("__Host-") {
            ScopeRule::HostPrefix
        } else if ip(&bare) {
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

    /// A cookie as `NSHTTPCookie` properties take it, the inverse of [`scope`] (`web_profile_write`,
    /// 209 §4.5). No `Debug`: it holds the value.
    pub(crate) struct NativeSet {
        pub(crate) name: String,
        pub(crate) value: String,
        /// `NSHTTPCookieDomain`: the bare host of a host-only cookie (WebKit's host-only form), `.` and
        /// the domain for a domain-wide one (WebKit's mark of `Domain=`); lower case.
        pub(crate) domain: String,
        pub(crate) path: String,
        /// `NSHTTPCookieExpires`, Unix seconds; `None` for a session cookie (no expiry property).
        pub(crate) expires: Option<i64>,
        pub(crate) secure: bool,
        pub(crate) http_only: bool,
        /// `NSHTTPCookieSameSitePolicy`; `None` omits it.
        pub(crate) same_site: Option<SameSitePolicy>,
    }

    /// The two policies `NSHTTPCookie` names (`NSHTTPCookieSameSiteLax`, `NSHTTPCookieSameSiteStrict`).
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum SameSitePolicy {
        Lax,
        Strict,
    }

    /// Why a cookie of `web_profile_write` was not set.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Unsettable {
        /// A `__Host-` cookie that is not secure, host-only and on path `/`: a browser never holds one.
        HostPrefix,
        /// No name, no domain, or a path that does not start with `/`.
        Malformed,
    }

    /// The `NSHTTPCookie` properties of `cookie` (209 §4.5): host-only → the bare host, domain-wide →
    /// `.` + domain (a leading `.` on the incoming domain also means domain-wide, as §4.1 stores it), an
    /// IP address always bare (host-only, as [`scope`] reads it back). `expires` `None` is a session
    /// cookie. `same_site` `"Lax"` / `"Strict"` map to WebKit's policies; `"None"` and `""` omit the
    /// property: WebKit has no `None` policy distinct from unspecified (its cookie converts a missing
    /// policy to `None`), so both read back as `""`. `partitioned` cannot be carried (209 D1).
    pub(crate) fn native_set(cookie: Cookie) -> Result<NativeSet, Unsettable> {
        let dotted = cookie.domain.starts_with('.');
        let bare = cookie.domain.trim_start_matches('.').to_ascii_lowercase();
        if cookie.name.is_empty() || bare.is_empty() || !cookie.path.starts_with('/') {
            return Err(Unsettable::Malformed);
        }
        let host_only = (cookie.host_only && !dotted) || ip(&bare);
        if cookie.name.starts_with("__Host-") && !(cookie.secure && host_only && cookie.path == "/")
        {
            return Err(Unsettable::HostPrefix);
        }
        Ok(NativeSet {
            domain: if host_only { bare } else { format!(".{bare}") },
            name: cookie.name,
            value: cookie.value,
            path: cookie.path,
            expires: cookie.expires,
            secure: cookie.secure,
            http_only: cookie.http_only,
            same_site: match cookie.same_site.to_ascii_lowercase().as_str() {
                "lax" => Some(SameSitePolicy::Lax),
                "strict" => Some(SameSitePolicy::Strict),
                _ => None,
            },
        })
    }

    /// Whether `host` (lower case, no dot) is an IP address, bracketed or not.
    fn ip(host: &str) -> bool {
        host.trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .is_ok()
    }

    /// The contract key of a cookie in the tile store, from `NSHTTPCookie`'s `name`, `domain` and `path`
    /// (the domain without WebKit's leading dot, lower case; an empty path is `/`), as [`Cookie::from`]
    /// keys it: what `web_profile_write`'s `remove` keys are matched against.
    pub(crate) fn jar_key(name: &str, domain: &str, path: &str) -> CookieKey {
        CookieKey {
            name: name.to_owned(),
            domain: domain.trim_start_matches('.').to_ascii_lowercase(),
            path: if path.is_empty() { "/" } else { path }.to_owned(),
        }
    }

    /// The http(s) origin of `url` (`scheme://host[:port]`), `None` for any other URL.
    pub(crate) fn web_origin(url: &str) -> Option<String> {
        tauri::Url::parse(url)
            .ok()
            .filter(|u| matches!(u.scheme(), "http" | "https"))
            .map(|u| u.origin().ascii_serialization())
    }

    /// `web_profile_write`'s result (209 §4.5): counts only. `skipped` counts the cookies of `set` that
    /// were not set ([`Unsettable`], or `NSHTTPCookie` refused the properties).
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
    pub(crate) struct Written {
        pub(crate) set: usize,
        pub(crate) removed: usize,
        pub(crate) skipped: usize,
    }

    /// The cookie half of a [`WritePlan`], applied on the main thread by `apple_cookies::write`.
    pub(crate) struct CookieWrite {
        pub(crate) set: Vec<NativeSet>,
        /// Normalised by [`jar_key`]'s rule; every cookie of the store with one of these keys is deleted.
        pub(crate) remove: std::collections::HashSet<CookieKey>,
        /// Cookies of `set` already refused by [`native_set`].
        pub(crate) skipped: usize,
    }

    /// One origin's localStorage to apply to the open tiles of that origin. No `Debug`: `entries` holds
    /// the values.
    #[derive(Clone)]
    pub(crate) struct StorageApply {
        /// As [`web_origin`] spells it, compared with a tile's top-level origin.
        pub(crate) origin: String,
        /// `local_storage` as JSON `[[k, v]]`, passed to the script as an argument, never in its source.
        pub(crate) entries: String,
    }

    /// What `web_profile_write` does, worked out off the main thread (209 §4.5).
    pub(crate) struct WritePlan {
        pub(crate) cookies: CookieWrite,
        pub(crate) storage: Vec<StorageApply>,
    }

    /// Map `set` with [`native_set`] (a refusal is counted, logged by reason, never by value), normalise
    /// `remove`, and keep the snapshots whose origin is http(s), keyed by [`web_origin`].
    pub(crate) fn plan_write(
        set: Vec<Cookie>,
        remove: Vec<CookieKey>,
        storage: Vec<Storage>,
    ) -> WritePlan {
        let mut skipped = 0;
        let set = set
            .into_iter()
            .filter_map(|c| {
                native_set(c)
                    .inspect_err(|reason| {
                        skipped += 1;
                        tracing::debug!(?reason, "web profile write: cookie skipped");
                    })
                    .ok()
            })
            .collect();
        let remove = remove
            .iter()
            .map(|k| jar_key(&k.name, &k.domain, &k.path))
            .collect();
        let storage = storage
            .into_iter()
            .filter_map(|s| {
                let Some(origin) = web_origin(&s.origin) else {
                    tracing::debug!(origin = %s.origin, "web profile write: storage origin skipped");
                    return None;
                };
                Some(StorageApply {
                    origin,
                    entries: serde_json::to_string(&s.local_storage).ok()?,
                })
            })
            .collect();
        WritePlan {
            cookies: CookieWrite {
                set,
                remove,
                skipped,
            },
            storage,
        }
    }

    /// Body of the `callAsyncJavaScript` that applies a [`StorageApply`] in [`AGENT_WORLD`], in the main
    /// frame, with the arguments `origin` and `entries`: `localStorage.setItem` per pair, no other key
    /// removed; `false` (nothing written) when the page moved to another origin since it was picked.
    pub(crate) const APPLY_STORAGE_SCRIPT: &str = concat!(
        "if(location.origin!==origin)return false;",
        "for(const [k,v] of JSON.parse(entries))localStorage.setItem(k,v);",
        "return true;"
    );

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
    /// is dropped whole in the page (209 §4.1) and only `{origin, over: true}` crosses the bridge, which
    /// [`parse_capture`] turns into the [`TOO_LARGE`] error: never an empty list, which means "cleared".
    pub(crate) const CAPTURE_SCRIPT: &str = concat!(
        "const max=",
        98304,
        ",e=new TextEncoder(),s=[];let n=1;",
        "for(let i=0;i<localStorage.length;i++){const k=localStorage.key(i);if(k===null)continue;",
        "const p=[k,localStorage.getItem(k)??\"\"];n+=e.encode(JSON.stringify(p)).length+1;",
        "if(n>max)return JSON.stringify({origin:location.origin,over:true});s.push(p);}",
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

    /// What [`CAPTURE_SCRIPT`] returns: a snapshot, or the over-cap marker.
    #[derive(Deserialize)]
    struct Captured {
        origin: String,
        local_storage: Option<Vec<(String, String)>>,
        #[serde(default)]
        over: bool,
    }

    /// The capture script's JSON, refused unless the origin is http(s) (an `about:blank` or opaque page
    /// has no origin to store under), and refused with [`TOO_LARGE`] when the snapshot passed
    /// [`STORAGE_MAX_BYTES`].
    pub(crate) fn parse_capture(json: &str) -> Result<Storage, String> {
        // The error names the position only: serde's message can quote a storage value.
        let shape = |column: usize| {
            format!("storage capture returned an unexpected shape (at column {column})")
        };
        let captured: Captured = serde_json::from_str(json).map_err(|e| shape(e.column()))?;
        let web = tauri::Url::parse(&captured.origin)
            .is_ok_and(|u| matches!(u.scheme(), "http" | "https"));
        if !web {
            return Err(format!("{NO_ORIGIN} `{}`", captured.origin));
        }
        if captured.over {
            return Err(format!(
                "{TOO_LARGE}: the localStorage of `{}` is over {} KiB",
                captured.origin,
                STORAGE_MAX_BYTES / 1024
            ));
        }
        Ok(Storage {
            origin: captured.origin,
            local_storage: captured.local_storage.ok_or_else(|| shape(0))?,
        })
    }

    /// Prefix of `web_storage_capture`'s refusal for a page without an http(s) origin.
    const NO_ORIGIN: &str = "no web origin";

    /// Prefix of `web_storage_capture`'s refusal for a snapshot over [`STORAGE_MAX_BYTES`] (209 §4.1).
    pub(crate) const TOO_LARGE: &str = "storage too large";

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
            // Neither a snapshot nor the marker.
            assert!(
                parse_capture(r#"{"origin":"https://a.test"}"#)
                    .unwrap_err()
                    .starts_with("storage capture returned an unexpected shape")
            );
        }

        #[test]
        fn profile_storage_over_cap_is_an_error_not_empty() {
            // A genuinely empty localStorage is a snapshot: the site cleared its storage.
            let empty = parse_capture(r#"{"origin":"https://a.test","local_storage":[]}"#).unwrap();
            assert_eq!(empty.origin, "https://a.test");
            assert!(empty.local_storage.is_empty());
            // Past the cap the script sends only the marker: an error, never an empty list.
            let e = parse_capture(r#"{"origin":"https://a.test","over":true}"#).unwrap_err();
            assert!(e.starts_with("storage too large"), "{e}");
            assert!(e.contains("96 KiB"), "{e}");
            // The marker wins over any list that came with it.
            let e = parse_capture(
                r#"{"origin":"https://a.test","over":true,"local_storage":[["k","s3cr3t"]]}"#,
            )
            .unwrap_err();
            assert!(
                e.starts_with("storage too large") && !e.contains("s3cr3t"),
                "{e}"
            );
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

        fn contract(name: &str, domain: &str, host_only: bool) -> Cookie {
            Cookie {
                name: name.into(),
                value: "v".into(),
                domain: domain.into(),
                host_only,
                path: "/".into(),
                expires: Some(1_795_000_000),
                secure: true,
                http_only: true,
                same_site: "Lax".into(),
                partitioned: false,
            }
        }

        /// What `apple_cookies` reads back from an `NSHTTPCookie` made of `n` (no properties).
        fn read_back(n: &NativeSet) -> Cookie {
            Cookie::from(NativeCookie {
                name: n.name.clone(),
                value: n.value.clone(),
                domain: n.domain.clone(),
                path: n.path.clone(),
                expires: n.expires.map(|s| s as f64),
                session_only: n.expires.is_none(),
                secure: n.secure,
                http_only: n.http_only,
                same_site: n.same_site.map(|p| format!("{p:?}").to_ascii_lowercase()),
                properties: vec![],
            })
        }

        #[test]
        fn profile_write_cookie_mapping() {
            // Host-only: the bare host, no dot.
            let n = native_set(contract("sid", "App.Example.com", true)).unwrap();
            assert_eq!(n.domain, "app.example.com");
            assert_eq!(
                (n.name.as_str(), n.value.as_str(), n.path.as_str()),
                ("sid", "v", "/")
            );
            assert_eq!(n.expires, Some(1_795_000_000));
            assert!(n.secure && n.http_only);
            assert_eq!(n.same_site, Some(SameSitePolicy::Lax));
            // Domain-wide: a leading dot; a dotted incoming domain is domain-wide whatever `host_only` says.
            assert_eq!(
                native_set(contract("sid", "example.com", false))
                    .unwrap()
                    .domain,
                ".example.com"
            );
            assert_eq!(
                native_set(contract("sid", ".example.com", true))
                    .unwrap()
                    .domain,
                ".example.com"
            );
            // An IP address is always host-only.
            assert_eq!(
                native_set(contract("sid", "10.0.0.1", false))
                    .unwrap()
                    .domain,
                "10.0.0.1"
            );
            // A session cookie has no expiry; flags go through as they are.
            let n = native_set(Cookie {
                expires: None,
                secure: false,
                http_only: false,
                path: "/app".into(),
                ..contract("sid", "a.test", true)
            })
            .unwrap();
            assert_eq!(n.expires, None);
            assert!(!n.secure && !n.http_only);
            assert_eq!(n.path, "/app");
            // SameSite: Lax and Strict are WebKit's policies; None and "" omit it.
            for (same_site, want) in [
                ("Lax", Some(SameSitePolicy::Lax)),
                ("Strict", Some(SameSitePolicy::Strict)),
                ("strict", Some(SameSitePolicy::Strict)),
                ("None", None),
                ("", None),
            ] {
                let c = Cookie {
                    same_site: same_site.into(),
                    ..contract("sid", "a.test", true)
                };
                assert_eq!(native_set(c).unwrap().same_site, want, "{same_site}");
            }
            // Malformed cookies are refused.
            for c in [
                contract("", "a.test", true),
                contract("sid", "", true),
                contract("sid", ".", false),
                Cookie {
                    path: "app".into(),
                    ..contract("sid", "a.test", true)
                },
            ] {
                assert_eq!(native_set(c).err(), Some(Unsettable::Malformed));
            }
        }

        #[test]
        fn profile_write_host_prefix_rules() {
            assert!(native_set(contract("__Host-sid", "app.example.com", true)).is_ok());
            for c in [
                contract("__Host-sid", "example.com", false),
                contract("__Host-sid", ".example.com", true),
                Cookie {
                    secure: false,
                    ..contract("__Host-sid", "app.example.com", true)
                },
                Cookie {
                    path: "/app".into(),
                    ..contract("__Host-sid", "app.example.com", true)
                },
            ] {
                assert_eq!(native_set(c).err(), Some(Unsettable::HostPrefix));
            }
        }

        #[test]
        fn profile_write_reads_back_as_written() {
            // The inverse of `scope`: what `web_profile_write` sets, `web_profile_read` returns unchanged
            // (SameSite `None` is the one exception: WebKit reads it back as "").
            for c in [
                contract("__Host-sid", "app.example.com", true),
                contract("sid", "example.com", false),
                contract("sid", "192.168.1.10", true),
                Cookie {
                    expires: None,
                    same_site: "Strict".into(),
                    ..contract("pref", "a.test", true)
                },
                Cookie {
                    same_site: String::new(),
                    ..contract("pref", "a.test", false)
                },
            ] {
                let n = native_set(c.clone()).unwrap();
                assert_eq!(read_back(&n), c);
            }
            let n = native_set(Cookie {
                same_site: "None".into(),
                ..contract("x", "a.test", true)
            })
            .unwrap();
            assert_eq!(read_back(&n).same_site, "");
        }

        #[test]
        fn profile_write_plan() {
            let plan = plan_write(
                vec![
                    contract("sid", "a.test", true),
                    contract("__Host-x", "a.test", false),
                    contract("", "a.test", true),
                ],
                vec![CookieKey {
                    name: "old".into(),
                    domain: ".A.test".into(),
                    path: String::new(),
                }],
                vec![
                    Storage {
                        origin: "https://App.Example.com:443/ignored".into(),
                        local_storage: vec![("k".into(), "v\"".into())],
                    },
                    Storage {
                        origin: "file:///x".into(),
                        local_storage: vec![],
                    },
                ],
            );
            assert_eq!(plan.cookies.set.len(), 1);
            assert_eq!(plan.cookies.skipped, 2);
            // A removal key matches the store's cookie whatever its dot, case or empty path.
            assert!(plan.cookies.remove.contains(&jar_key("old", ".a.test", "")));
            assert!(plan.cookies.remove.contains(&jar_key("old", "a.test", "/")));
            assert!(
                !plan
                    .cookies
                    .remove
                    .contains(&jar_key("old", "a.test", "/x"))
            );
            assert_eq!(plan.storage.len(), 1);
            assert_eq!(plan.storage[0].origin, "https://app.example.com");
            assert_eq!(
                serde_json::from_str::<Vec<(String, String)>>(&plan.storage[0].entries).unwrap(),
                vec![("k".to_owned(), "v\"".to_owned())]
            );
            assert_eq!(
                web_origin("http://mac.lan:3000/a?b").as_deref(),
                Some("http://mac.lan:3000")
            );
            assert_eq!(web_origin("about:blank"), None);
            assert_eq!(
                serde_json::to_value(Written {
                    set: 1,
                    removed: 2,
                    skipped: 3
                })
                .unwrap(),
                serde_json::json!({"set": 1, "removed": 2, "skipped": 3})
            );
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
