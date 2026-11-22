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
    use std::collections::{BTreeMap, HashSet};
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

    /// `web_profile_write`'s result (209 §4.5): counts only, never a value.
    /// - `set`, `removed`: cookies WebKit reported set and store cookies it reported deleted.
    /// - `skipped`: cookies of `set` not set ([`Unsettable`], or `NSHTTPCookie` refused the properties).
    /// - `storage_applied`: per snapshot origin (as [`web_origin`] spells it), the number of open tiles of
    ///   that origin whose localStorage took it; `0` when no tile of the origin was open or it left it.
    /// - `timed_out`: WebKit had not reported everything by the deadline; the counts are what it had
    ///   reported until then, and the store may hold a part of the write: read it again.
    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
    pub(crate) struct Written {
        pub(crate) set: usize,
        pub(crate) removed: usize,
        pub(crate) skipped: usize,
        pub(crate) storage_applied: BTreeMap<String, usize>,
        pub(crate) timed_out: bool,
    }

    /// Which store cookies a write deletes (209 §4.5).
    /// - A cookie of `set` replaces the store cookie of the same key **and the same scope** only: a
    ///   host-only `sid` on `example.com` and a domain-wide `sid` on `.example.com` are two cookies, and
    ///   setting one leaves the other. Deleting it first also removes a same-scope copy WebKit spells
    ///   differently (a domain-wide cookie it holds without the dot).
    /// - A key of `remove` has no scope: it deletes the key in every scope. A key that `set` also carries
    ///   is a replacement, so only that scope goes.
    #[derive(Debug, Default)]
    pub(crate) struct Doom {
        replaced_host_only: HashSet<CookieKey>,
        replaced_wide: HashSet<CookieKey>,
        removed: HashSet<CookieKey>,
    }

    impl Doom {
        /// `set` already mapped by [`native_set`] (its dot is the scope); `remove` as the UI sent it.
        pub(crate) fn new(set: &[NativeSet], remove: &[CookieKey]) -> Self {
            let mut doom = Doom::default();
            for n in set {
                let key = jar_key(&n.name, &n.domain, &n.path);
                if n.domain.starts_with('.') {
                    doom.replaced_wide.insert(key);
                } else {
                    doom.replaced_host_only.insert(key);
                }
            }
            doom.removed = remove
                .iter()
                .map(|k| jar_key(&k.name, &k.domain, &k.path))
                .filter(|k| !doom.replaced_host_only.contains(k) && !doom.replaced_wide.contains(k))
                .collect();
            doom
        }

        /// Whether the store cookie of `NSHTTPCookie` `name`, `domain`, `path` and the [`PROPERTIES`] of
        /// its `properties` goes: its key by [`jar_key`], its scope by [`scope`], as `web_profile_read`
        /// reports them.
        pub(crate) fn dooms(
            &self,
            name: &str,
            domain: &str,
            path: &str,
            properties: &[(String, String)],
        ) -> bool {
            let key = jar_key(name, domain, path);
            if self.removed.contains(&key) {
                return true;
            }
            let (_, host_only, _) = scope(name, domain, properties);
            if host_only {
                self.replaced_host_only.contains(&key)
            } else {
                self.replaced_wide.contains(&key)
            }
        }
    }

    /// The cookie half of a [`WritePlan`], applied on the main thread by `apple_cookies::write`.
    pub(crate) struct CookieWrite {
        pub(crate) set: Vec<NativeSet>,
        pub(crate) doom: Doom,
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

    impl StorageApply {
        /// The named arguments of [`APPLY_STORAGE_SCRIPT`], in the order the script names them.
        pub(crate) fn arguments(&self) -> [(&'static str, &str); 2] {
            [("origin", &self.origin), ("entries", &self.entries)]
        }
    }

    /// The snapshots of `storage` for a tile whose top-level page is at `url`: those of its origin
    /// ([`web_origin`]); none for a page that is not http(s).
    pub(crate) fn storage_for<'a>(storage: &'a [StorageApply], url: &str) -> Vec<&'a StorageApply> {
        let Some(origin) = web_origin(url) else {
            return Vec::new();
        };
        storage.iter().filter(|s| s.origin == origin).collect()
    }

    /// What `web_profile_write` does, worked out off the main thread (209 §4.5).
    pub(crate) struct WritePlan {
        pub(crate) cookies: CookieWrite,
        pub(crate) storage: Vec<StorageApply>,
    }

    /// Map `set` with [`native_set`] (a refusal is counted, logged by reason, never by value), work out
    /// what goes ([`Doom`]), and keep the snapshots whose origin is http(s), keyed by [`web_origin`]; a
    /// second snapshot of one origin replaces the first.
    pub(crate) fn plan_write(
        set: Vec<Cookie>,
        remove: Vec<CookieKey>,
        storage: Vec<Storage>,
    ) -> WritePlan {
        let mut skipped = 0;
        let set: Vec<NativeSet> = set
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
        let doom = Doom::new(&set, &remove);
        let mut by_origin = BTreeMap::new();
        for s in storage {
            let Some(origin) = web_origin(&s.origin) else {
                tracing::debug!(origin = %s.origin, "web profile write: storage origin skipped");
                continue;
            };
            if let Ok(entries) = serde_json::to_string(&s.local_storage) {
                by_origin.insert(origin, entries);
            }
        }
        WritePlan {
            cookies: CookieWrite { set, doom, skipped },
            storage: by_origin
                .into_iter()
                .map(|(origin, entries)| StorageApply { origin, entries })
                .collect(),
        }
    }

    /// The completion count of one cookie write (209 §4.5): WebKit answers each `deleteCookie:` and
    /// `setCookie:` by a completion handler, and the write is done once every request was made
    /// ([`issued`](Self::issued)) and every handler ran. Each event returns `true` exactly once, when
    /// it completes the write; a handler beyond the expected count is ignored. At the deadline the
    /// counts reached so far are the answer ([`written`](Self::written) with `timed_out`).
    #[derive(Debug, Default)]
    pub(crate) struct Completion {
        removes: usize,
        sets: usize,
        removed: usize,
        set: usize,
        skipped: usize,
        issued: bool,
        answered: bool,
    }

    impl Completion {
        /// A write whose plan already refused `skipped` cookies.
        pub(crate) fn new(skipped: usize) -> Self {
            Completion {
                skipped,
                ..Completion::default()
            }
        }

        /// The store was listed: `removes` deletions and `sets` sets will be requested, `refused`
        /// cookies of the plan were refused by `NSHTTPCookie`.
        pub(crate) fn start(&mut self, removes: usize, sets: usize, refused: usize) {
            self.removes = removes;
            self.sets = sets;
            self.skipped += refused;
        }

        /// A deletion's handler ran.
        pub(crate) fn removed(&mut self) -> bool {
            self.removed = (self.removed + 1).min(self.removes);
            self.due()
        }

        /// A set's handler ran.
        pub(crate) fn set(&mut self) -> bool {
            self.set = (self.set + 1).min(self.sets);
            self.due()
        }

        /// Every request was made.
        pub(crate) fn issued(&mut self) -> bool {
            self.issued = true;
            self.due()
        }

        fn due(&mut self) -> bool {
            let due = self.issued
                && !self.answered
                && self.removed == self.removes
                && self.set == self.sets;
            self.answered |= due;
            due
        }

        /// The counts so far; no storage.
        pub(crate) fn written(&self, timed_out: bool) -> Written {
            Written {
                set: self.set,
                removed: self.removed,
                skipped: self.skipped,
                storage_applied: BTreeMap::new(),
                timed_out,
            }
        }
    }

    /// The error of a main-thread call that did not answer within `budget`, the time it was given.
    pub(crate) fn timeout_message(budget: Duration) -> String {
        let budget = if budget.subsec_millis() == 0 {
            format!("{} s", budget.as_secs())
        } else {
            format!("{} ms", budget.as_millis())
        };
        format!("the main thread did not run the web view call within {budget}")
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
                vec![],
                vec![
                    Storage {
                        origin: "https://App.Example.com:443/ignored".into(),
                        local_storage: vec![("k".into(), "old".into())],
                    },
                    Storage {
                        origin: "file:///x".into(),
                        local_storage: vec![],
                    },
                    Storage {
                        origin: "http://mac.lan:3000".into(),
                        local_storage: vec![],
                    },
                    // A second snapshot of one origin replaces the first.
                    Storage {
                        origin: "https://app.example.com".into(),
                        local_storage: vec![("k".into(), "v\"".into())],
                    },
                ],
            );
            assert_eq!(plan.cookies.set.len(), 1);
            assert_eq!(plan.cookies.skipped, 2);
            let origins: Vec<&str> = plan.storage.iter().map(|s| s.origin.as_str()).collect();
            assert_eq!(origins, ["http://mac.lan:3000", "https://app.example.com"]);
            assert_eq!(
                serde_json::from_str::<Vec<(String, String)>>(&plan.storage[1].entries).unwrap(),
                vec![("k".to_owned(), "v\"".to_owned())]
            );
            assert_eq!(plan.storage[0].entries, "[]");
            assert_eq!(
                serde_json::to_value(Written {
                    set: 1,
                    removed: 2,
                    skipped: 3,
                    storage_applied: [("https://a.test".to_owned(), 2)].into(),
                    timed_out: true,
                })
                .unwrap(),
                serde_json::json!({
                    "set": 1, "removed": 2, "skipped": 3,
                    "storage_applied": {"https://a.test": 2}, "timed_out": true
                })
            );
        }

        fn key(name: &str, domain: &str, path: &str) -> CookieKey {
            CookieKey {
                name: name.into(),
                domain: domain.into(),
                path: path.into(),
            }
        }

        #[test]
        fn profile_write_doom() {
            let origin = |host: &str| vec![("OriginURL".to_owned(), format!("https://{host}/"))];
            let at = |name: &str, path: &str| Cookie {
                path: path.into(),
                ..contract(name, "example.com", true)
            };
            let plan = plan_write(
                vec![
                    // Host-only `sid` on example.com, path `/`.
                    contract("sid", "Example.com", true),
                    // Domain-wide `wide` on example.com.
                    contract("wide", "example.com", false),
                    // `deep` on `/app` only.
                    at("deep", "/app"),
                    // An IP host is host-only whatever the flag says.
                    contract("ip", "10.0.0.1", false),
                    // Also named in `remove`: a replacement, its own scope only.
                    contract("both", "example.com", true),
                ],
                vec![
                    key("gone", ".Example.com", ""),
                    key("both", "example.com", "/"),
                ],
                vec![],
            );
            let doom = &plan.cookies.doom;
            // (name, NSHTTPCookie.domain, path, properties) of a store cookie → deleted?
            let none = Vec::new();
            for (name, domain, path, properties, want) in [
                ("sid", "example.com", "/", &none, true),
                ("sid", "EXAMPLE.com", "", &none, true),
                ("sid", ".example.com", "/", &none, false),
                ("sid", "example.com", "/", &origin("www.example.com"), false),
                ("sid", "www.example.com", "/", &none, false),
                ("sid", "example.com", "/x", &none, false),
                ("wide", ".example.com", "/", &none, true),
                ("wide", "example.com", "/", &origin("www.example.com"), true),
                ("wide", "example.com", "/", &none, false),
                ("deep", "example.com", "/app", &none, true),
                ("deep", "example.com", "/", &none, false),
                ("deep", "example.com", "/app/x", &none, false),
                ("ip", "10.0.0.1", "/", &none, true),
                ("ip", ".10.0.0.1", "/", &none, true),
                ("ip", "10.0.0.2", "/", &none, false),
                ("gone", "example.com", "/", &none, true),
                ("gone", ".example.com", "/", &none, true),
                ("gone", "example.com", "/app", &none, false),
                ("gone", "a.example.com", "/", &none, false),
                ("both", "example.com", "/", &none, true),
                ("both", ".example.com", "/", &none, false),
                ("other", "example.com", "/", &none, false),
            ] {
                assert_eq!(
                    doom.dooms(name, domain, path, properties),
                    want,
                    "{name} {domain} {path} {properties:?}"
                );
            }
        }

        #[test]
        fn profile_write_completion() {
            // Nothing to do: done once the (empty) requests are made, once.
            let mut c = Completion::new(2);
            c.start(0, 0, 0);
            assert!(c.issued());
            assert!(!c.issued());
            assert_eq!(
                c.written(false),
                Written {
                    skipped: 2,
                    ..Written::default()
                }
            );
            // One deletion: not done before the requests are all made, done at its handler.
            let mut c = Completion::new(0);
            c.start(1, 0, 0);
            assert!(!c.issued());
            assert!(c.removed());
            assert!(!c.removed());
            assert_eq!((c.written(false).removed, c.written(false).set), (1, 0));
            // A handler WebKit runs before the last request is made does not finish the write.
            let mut c = Completion::new(0);
            c.start(0, 1, 0);
            assert!(!c.set());
            assert!(c.issued());
            // n: only the last handler answers; one more is ignored.
            let mut c = Completion::new(1);
            c.start(2, 3, 1);
            assert!(!c.issued());
            let events = [c.set(), c.removed(), c.set(), c.removed(), c.set()];
            assert_eq!(events, [false, false, false, false, true]);
            assert!(!c.set());
            assert_eq!(
                c.written(false),
                Written {
                    set: 3,
                    removed: 2,
                    skipped: 2,
                    ..Written::default()
                }
            );
            // A handler never called: never done; the deadline answers what was reached.
            let mut c = Completion::new(0);
            c.start(1, 2, 0);
            assert!(!c.issued());
            assert!(!c.removed());
            assert!(!c.set());
            assert_eq!(
                c.written(true),
                Written {
                    set: 1,
                    removed: 1,
                    timed_out: true,
                    ..Written::default()
                }
            );
            // The main thread never listed the store: the plan's refusals only.
            assert_eq!(
                Completion::new(3).written(true),
                Written {
                    skipped: 3,
                    timed_out: true,
                    ..Written::default()
                }
            );
        }

        #[test]
        fn profile_write_storage_targets() {
            let plan = plan_write(
                vec![],
                vec![],
                vec![
                    Storage {
                        origin: "https://app.example.com".into(),
                        local_storage: vec![("k".into(), "v".into())],
                    },
                    Storage {
                        origin: "http://mac.lan:3000".into(),
                        local_storage: vec![],
                    },
                ],
            );
            let to = |url: &str| -> Vec<&str> {
                storage_for(&plan.storage, url)
                    .into_iter()
                    .map(|s| s.origin.as_str())
                    .collect()
            };
            assert_eq!(
                to("https://APP.example.com/a?b#c"),
                ["https://app.example.com"]
            );
            assert_eq!(
                to("https://app.example.com:443/"),
                ["https://app.example.com"]
            );
            assert_eq!(to("http://mac.lan:3000/x"), ["http://mac.lan:3000"]);
            for url in [
                "https://app.example.com:8443/",
                "http://app.example.com/",
                "https://example.com/",
                "http://mac.lan/",
                "about:blank",
                "",
            ] {
                assert!(to(url).is_empty(), "{url}");
            }
            // The script reads exactly the arguments passed, `entries` as JSON pairs.
            let args = plan.storage[1].arguments();
            assert_eq!(args.map(|(name, _)| name), ["origin", "entries"]);
            assert_eq!(args[0].1, "https://app.example.com");
            assert_eq!(
                serde_json::from_str::<Vec<(String, String)>>(args[1].1).unwrap(),
                vec![("k".to_owned(), "v".to_owned())]
            );
            assert!(APPLY_STORAGE_SCRIPT.contains("location.origin!==origin"));
            assert!(APPLY_STORAGE_SCRIPT.contains("JSON.parse(entries)"));
        }

        #[test]
        fn profile_timeout_names_the_budget() {
            assert_eq!(
                timeout_message(Duration::from_secs(5)),
                "the main thread did not run the web view call within 5 s"
            );
            assert_eq!(
                timeout_message(Duration::from_millis(200)),
                "the main thread did not run the web view call within 200 ms"
            );
        }
    }
}

/// The agent's tile operations (209 §4.7, D8): `web_view_call`'s argument check per op, the script wrapper
/// and its reply, the caller rule, and the snapshot's size and pixel arithmetic. The platform code is in
/// `apple_tile` (macOS and iOS); Windows and Linux check the same arguments and answer `CALL_UNSUPPORTED`.
#[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
pub(crate) mod call {
    use std::time::{Duration, Instant};

    use serde::{Deserialize, Serialize};
    use serde_json::{Map, Value};

    /// Error of `web_view_call` on Windows and Linux (209 §4.7: the CDP route is not built).
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    pub(crate) const CALL_UNSUPPORTED: &str = "unsupported: web_view_call needs macOS or iOS";

    /// A script's wait when `timeout_ms` is not given: the engine's own default wait (209 §4.7).
    pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
    /// The longest `timeout_ms` accepted.
    pub(crate) const MAX_TIMEOUT: Duration = Duration::from_secs(300);
    /// `source` at most this many bytes.
    pub(crate) const MAX_SOURCE: usize = 1024 * 1024;
    /// At most this many `arguments`.
    pub(crate) const MAX_ARGUMENTS: usize = 64;
    /// A script's result at most this many UTF-16 units of JSON; counted in the page, so a larger one never
    /// crosses the bridge.
    pub(crate) const MAX_RESULT: usize = 2 * 1024 * 1024;
    /// A thrown message is cut to this many code points (in the page, so a cut never splits a surrogate pair).
    const MAX_THROWN: usize = 2000;
    /// A snapshot's long side at most this many pixels.
    pub(crate) const SNAPSHOT_MAX_SIDE: usize = 1600;
    /// How long WebKit has to take a snapshot (the main-thread hop included; encoding is not counted).
    pub(crate) const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(10);
    /// How often `navigate`, `back`, `forward` and `reload` look whether the load finished when they cannot
    /// wait for the navigation itself (macOS; an iOS history move within the document).
    pub(crate) const SETTLE_POLL: Duration = Duration::from_millis(100);
    /// How often a wait looks whether `stop` ran for its tile.
    pub(crate) const STOP_POLL: Duration = Duration::from_millis(50);
    /// The error of a call `stop` abandoned (209 §9: the user's Stop).
    pub(crate) const STOPPED: &str = "stopped by the user";
    /// `error` of a load answer when WebKit's error is not known: macOS, where the shell sees that the load
    /// ended without finishing but not why (wry owns the navigation delegate), or an iOS failure that
    /// carried no `NSError`.
    pub(crate) const LOAD_FAILED: &str = "load failed";

    /// Name of the one argument the wrapped body gets: the JSON array of the caller's argument values.
    pub(crate) const ARGS_NAME: &str = "__theuth_args";
    /// Names the wrapper keeps for itself.
    const RESERVED_PREFIX: &str = "__theuth";

    /// Which content world a script runs in (209 §4.7).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
    #[serde(rename_all = "lowercase")]
    pub(crate) enum World {
        /// `WKContentWorld` named `theuth-agent` (`profile::AGENT_WORLD`): own globals, same DOM.
        Isolated,
        /// `WKContentWorld.page`: the page's globals, which the page can see and change.
        Page,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ScriptArgs {
        world: World,
        source: String,
        #[serde(default)]
        arguments: Map<String, Value>,
        timeout_ms: Option<u64>,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct NavigateArgs {
        url: String,
        timeout_ms: Option<u64>,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct LoadArgs {
        timeout_ms: Option<u64>,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct NoArgs {}

    /// A checked `script` op. `Debug` never prints the source or the argument values.
    #[derive(PartialEq)]
    pub(crate) struct Script {
        pub(crate) world: World,
        /// The body that runs: the caller's source in [`wrap`].
        pub(crate) body: String,
        /// The value of [`ARGS_NAME`]: the argument values as one JSON array, in the order of the
        /// wrapper's parameters.
        pub(crate) args_json: String,
        pub(crate) timeout: Duration,
    }

    impl std::fmt::Debug for Script {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Script")
                .field("world", &self.world)
                .field("body", &format_args!("[{} bytes]", self.body.len()))
                .field(
                    "args_json",
                    &format_args!("[{} bytes]", self.args_json.len()),
                )
                .field("timeout", &self.timeout)
                .finish()
        }
    }

    /// One checked tile op (209 §4.7). The load ops wait until the main frame finished loading, at most
    /// their `timeout`.
    #[derive(Debug, PartialEq)]
    pub(crate) enum Op {
        Script(Script),
        /// `url` is not yet checked against the allowlist: that needs the app (`check_url`).
        Navigate {
            url: String,
            timeout: Duration,
        },
        History {
            forward: bool,
            timeout: Duration,
        },
        Reload(Duration),
        Snapshot,
        /// The user's Stop (209 §9): `stopLoading`, and every wait of the tile's calls in flight ends with
        /// [`STOPPED`].
        Stop,
    }

    impl Op {
        /// `op` and its `args` (absent counts as `{}`), or the reason they are refused. Unknown fields are
        /// refused, so a caller's typo is an error and not a default.
        pub(crate) fn parse(op: &str, args: Option<Value>) -> Result<Self, String> {
            let args = args.unwrap_or_else(|| Value::Object(Map::new()));
            let invalid = |e: String| format!("invalid args for `{op}`: {e}");
            // serde's message names the field or the expected type; a value it quotes is the caller's own
            // argument, never page data.
            let de = |e: serde_json::Error| invalid(e.to_string());
            match op {
                "script" => {
                    let a: ScriptArgs = serde_json::from_value(args).map_err(de)?;
                    let timeout = timeout(a.timeout_ms).map_err(invalid)?;
                    if a.source.len() > MAX_SOURCE {
                        return Err(invalid(format!("source is over {} KiB", MAX_SOURCE / 1024)));
                    }
                    if a.arguments.len() > MAX_ARGUMENTS {
                        return Err(invalid(format!("more than {MAX_ARGUMENTS} arguments")));
                    }
                    for name in a.arguments.keys() {
                        if !identifier(name) || name.starts_with(RESERVED_PREFIX) {
                            return Err(invalid(format!(
                                "argument name `{name}` is not a JavaScript identifier of the caller's"
                            )));
                        }
                    }
                    let names: Vec<&str> = a.arguments.keys().map(String::as_str).collect();
                    let values: Vec<&Value> = a.arguments.values().collect();
                    Ok(Op::Script(Script {
                        world: a.world,
                        body: wrap(&a.source, &names),
                        args_json: serde_json::to_string(&values).map_err(de)?,
                        timeout,
                    }))
                }
                "navigate" => {
                    let a: NavigateArgs = serde_json::from_value(args).map_err(de)?;
                    Ok(Op::Navigate {
                        url: a.url,
                        timeout: timeout(a.timeout_ms).map_err(invalid)?,
                    })
                }
                "back" | "forward" | "reload" => {
                    let a: LoadArgs = serde_json::from_value(args).map_err(de)?;
                    let timeout = timeout(a.timeout_ms).map_err(invalid)?;
                    Ok(match op {
                        "reload" => Op::Reload(timeout),
                        _ => Op::History {
                            forward: op == "forward",
                            timeout,
                        },
                    })
                }
                "snapshot" | "stop" => {
                    let NoArgs {} = serde_json::from_value(args).map_err(de)?;
                    Ok(if op == "stop" { Op::Stop } else { Op::Snapshot })
                }
                _ => Err(format!("unknown web view op `{op}`")),
            }
        }
    }

    /// `timeout_ms` as a duration: absent is [`DEFAULT_TIMEOUT`]; 0 and above [`MAX_TIMEOUT`] are refused.
    pub(crate) fn timeout(ms: Option<u64>) -> Result<Duration, String> {
        let Some(ms) = ms else {
            return Ok(DEFAULT_TIMEOUT);
        };
        let max = MAX_TIMEOUT.as_millis() as u64;
        if ms == 0 || ms > max {
            return Err(format!("timeout_ms must be 1..={max}, got {ms}"));
        }
        Ok(Duration::from_millis(ms))
    }

    /// What `navigate`, `back`, `forward` and `reload` answer (209 §4.7): the main frame's URL and title once
    /// the load finished (or failed: `error` set, `loading: false`), or as they are at the timeout, with
    /// `loading: true`. `error` is absent unless the load failed.
    #[derive(Debug, Clone, PartialEq, Serialize)]
    pub(crate) struct Loaded {
        pub(crate) url: String,
        pub(crate) title: String,
        pub(crate) loading: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub(crate) error: Option<String>,
    }

    /// A failed load's `error`: WebKit's error domain and code, `NSURLErrorDomain/-1003` (iOS, where the
    /// shell's delegate sees the error).
    #[cfg(any(target_os = "ios", test))]
    pub(crate) fn load_error(domain: &str, code: isize) -> String {
        format!("{domain}/{code}")
    }

    /// One look at a tile while a load op waits without the navigation's own callbacks: the main frame's
    /// state and how many main-frame loads finished in the view so far (macOS: wry's page-load `Finished`
    /// count; 0 where none is kept).
    #[derive(Debug, Clone, PartialEq)]
    pub(crate) struct Look {
        pub(crate) state: Loaded,
        pub(crate) finished: u64,
    }

    /// When a load op without its navigation's callbacks may answer (209 §4.7). The state read right after
    /// the op may still be the page before it (WebKit need not flip `loading` inside the request), so
    /// `!loading` counts only after evidence of the op's load: a finished load counted since the op
    /// (`counts`), the main frame's URL changed, or `loading` seen. With `counts`, a load seen loading that
    /// stops without a finish counted for two looks in a row ended without finishing: [`LOAD_FAILED`].
    #[derive(Debug)]
    pub(crate) struct Watch {
        url: String,
        finished: u64,
        counts: bool,
        seen: bool,
        quiet: bool,
    }

    impl Watch {
        /// From the look taken just before the op, on the same main-thread turn.
        pub(crate) fn new(before: &Look, counts: bool) -> Self {
            Self {
                url: before.state.url.clone(),
                finished: before.finished,
                counts,
                seen: false,
                quiet: false,
            }
        }

        /// The answer for this look, or `None` to look again. `end`: the call's time is up, so a load not
        /// done answers `loading: true`.
        pub(crate) fn step(&mut self, look: Look, end: bool) -> Option<Loaded> {
            let Look { state, finished } = look;
            let timed_out = |state: Loaded| {
                end.then_some(Loaded {
                    loading: true,
                    ..state
                })
            };
            if state.loading {
                self.seen = true;
                self.quiet = false;
                return timed_out(state);
            }
            if (self.counts && finished != self.finished) || state.url != self.url {
                return Some(state);
            }
            if self.seen && !self.counts {
                return Some(state);
            }
            if self.seen && self.quiet {
                return Some(Loaded {
                    error: Some(LOAD_FAILED.to_owned()),
                    ..state
                });
            }
            self.quiet = self.seen;
            timed_out(state)
        }
    }

    /// `back` / `forward` with no history entry that way: nothing happens.
    pub(crate) fn no_history(forward: bool) -> String {
        format!(
            "no page to go {} to",
            if forward { "forward" } else { "back" }
        )
    }

    /// A snapshot's `width` and `height` (209 §4.7): the visible viewport in CSS px, the view's size in points
    /// over the page's scale (iOS pinch zoom; macOS magnification times page zoom), rounded. A scale that
    /// is not positive counts as 1.
    pub(crate) fn css_size(width: f64, height: f64, scale: f64) -> (u32, u32) {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let px = |points: f64| (points / scale).round().clamp(0.0, f64::from(u32::MAX)) as u32;
        (px(width), px(height))
    }

    /// An ASCII JavaScript identifier (no reserved-word check: one would fail the script with a syntax error).
    fn identifier(name: &str) -> bool {
        let mut chars = name.chars();
        chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
    }

    /// The body `callAsyncJavaScript` runs, given the one argument [`ARGS_NAME`]: the caller's `source` as an
    /// async function of `names`, applied to the parsed values. It answers one JSON string, which
    /// [`reply`] reads off the main thread: `{"value":…}`, `{"error":…}` (a throw or rejection, the message
    /// cut in the page) or `{"over":n}` (JSON past [`MAX_RESULT`], which never crosses the bridge). A syntax
    /// error in `source` fails the whole body; WebKit reports it.
    pub(crate) fn wrap(source: &str, names: &[&str]) -> String {
        format!(
            concat!(
                "const __theuth_f=async function({names}){{\n{source}\n}};\n",
                "let __theuth_v;",
                "try{{__theuth_v=await __theuth_f.apply(this,JSON.parse({args}));}}",
                "catch(e){{let m;try{{m=e instanceof Error?e.name+\": \"+e.message:String(e);}}",
                "catch(_){{m=\"a value that cannot be printed\";}}",
                "return JSON.stringify({{error:Array.from(String(m)).slice(0,{thrown}).join('')}});}}",
                "let __theuth_r;",
                "try{{__theuth_r=JSON.stringify(__theuth_v);}}",
                "catch(e){{return JSON.stringify({{not_json:Array.from(String(e&&e.message)).slice(0,{thrown}).join('')}});}}",
                "if(__theuth_r===undefined)return '{{\"value\":null}}';",
                "if(__theuth_r.length>{max})return JSON.stringify({{over:__theuth_r.length}});",
                "return '{{\"value\":'+__theuth_r+'}}';"
            ),
            names = names.join(","),
            source = source,
            args = ARGS_NAME,
            thrown = MAX_THROWN,
            max = MAX_RESULT,
        )
    }

    /// What [`wrap`]'s body answers.
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Reply {
        #[serde(default)]
        value: Option<Value>,
        error: Option<String>,
        not_json: Option<String>,
        over: Option<u64>,
    }

    /// The script's value from [`wrap`]'s JSON (`None`: WebKit answered nothing, which the wrapper never
    /// does), or the error the caller gets. Errors never quote the value.
    pub(crate) fn reply(json: Option<&str>) -> Result<Value, String> {
        let shape = || "script answered an unexpected shape".to_owned();
        let reply: Reply = serde_json::from_str(json.ok_or_else(shape)?).map_err(|_| shape())?;
        match reply {
            Reply { error: Some(e), .. } => Err(format!("script threw: {e}")),
            Reply {
                not_json: Some(e), ..
            } => Err(format!("script result is not JSON: {e}")),
            Reply { over: Some(n), .. } => Err(format!(
                "script result too large: {n} characters of JSON, at most {MAX_RESULT}"
            )),
            Reply { value: Some(v), .. } => Ok(v),
            // `{"value":null}` deserialises `value` as `None`.
            Reply { value: None, .. } => Ok(Value::Null),
        }
    }

    /// A WebKit failure of the whole body (a syntax error in `source`, the page gone): its message, cut.
    pub(crate) fn failed(message: &str) -> String {
        let cut: String = message.chars().take(MAX_THROWN).collect();
        format!("script failed: {cut}")
    }

    pub(crate) fn timed_out(timeout: Duration) -> String {
        format!("script timed out after {} ms", timeout.as_millis())
    }

    /// Prefix of `web_view_call`'s refusal of a caller.
    const NO_ACCESS: &str = "no access to web view";

    /// Only the main webview of the window that holds the tile may drive it (209 §4.7): never a tile
    /// (`web-*`), never another webview of that window, never another window. On iOS the tile's window is
    /// `main`, the only one.
    pub(crate) fn check_holder(
        label: &str,
        caller: &str,
        caller_window: &str,
        tile_window: &str,
    ) -> Result<(), String> {
        check_driver(label, caller, caller_window)?;
        if caller_window != tile_window {
            Err(format!("{NO_ACCESS} `{label}` from `{caller}`"))
        } else {
            Ok(())
        }
    }

    /// The half of [`check_holder`] that needs no tile: the caller is a window's main webview, not a tile.
    /// Runs before the tile is looked up or the arguments are read, so a stranger learns nothing of either.
    pub(crate) fn check_driver(
        label: &str,
        caller: &str,
        caller_window: &str,
    ) -> Result<(), String> {
        if caller.starts_with(super::PREFIX) || caller != caller_window {
            Err(format!("{NO_ACCESS} `{label}` from `{caller}`"))
        } else {
            Ok(())
        }
    }

    /// The PNG size for an image of `width` × `height` pixels: the same, or scaled down so the long side
    /// is [`SNAPSHOT_MAX_SIDE`], each side at least 1. `None` for an empty image.
    pub(crate) fn fit(width: usize, height: usize) -> Option<(usize, usize)> {
        if width == 0 || height == 0 {
            return None;
        }
        let long = width.max(height);
        if long <= SNAPSHOT_MAX_SIDE {
            return Some((width, height));
        }
        let scale = |side: usize| ((side * SNAPSHOT_MAX_SIDE + long / 2) / long).max(1);
        Some((scale(width), scale(height)))
    }

    /// Premultiplied RGBA (what a CoreGraphics bitmap holds) to straight RGBA (what PNG stores), in place,
    /// with every painted pixel opaque: a view's snapshot is opaque, and the only partial alpha a downscale
    /// makes is the filter's falloff at the image's edges, which would fade the border. Clear pixels stay
    /// clear.
    pub(crate) fn straighten(rgba: &mut [u8]) {
        for px in rgba.as_chunks_mut::<4>().0 {
            let a = u32::from(px[3]);
            if a == 0 || a == 255 {
                continue;
            }
            for c in &mut px[..3] {
                *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
            px[3] = 255;
        }
    }

    /// The one log line of a call (209 §4.7): op, label, duration and ok/error, never a value or an
    /// error's text (a script's can quote the page).
    pub(crate) fn log(op: &str, label: &str, started: Instant, outcome: &Result<Value, String>) {
        let ms = started.elapsed().as_millis() as u64;
        let outcome = if outcome.is_ok() { "ok" } else { "error" };
        tracing::debug!(op, label, ms, outcome, "web view call");
    }

    #[cfg(test)]
    mod tests {
        use serde_json::json;

        use super::*;

        fn script(args: Value) -> Result<Script, String> {
            match Op::parse("script", Some(args))? {
                Op::Script(s) => Ok(s),
                other => panic!("not a script: {other:?}"),
            }
        }

        #[test]
        fn call_load_and_snapshot_args() {
            let t = Duration::from_millis(2500);
            for (op, want, default) in [
                (
                    "back",
                    Op::History {
                        forward: false,
                        timeout: t,
                    },
                    Op::History {
                        forward: false,
                        timeout: DEFAULT_TIMEOUT,
                    },
                ),
                (
                    "forward",
                    Op::History {
                        forward: true,
                        timeout: t,
                    },
                    Op::History {
                        forward: true,
                        timeout: DEFAULT_TIMEOUT,
                    },
                ),
                ("reload", Op::Reload(t), Op::Reload(DEFAULT_TIMEOUT)),
            ] {
                assert_eq!(Op::parse(op, None).unwrap(), default);
                assert_eq!(Op::parse(op, Some(json!({}))).unwrap(), default);
                assert_eq!(
                    Op::parse(op, Some(json!({ "timeout_ms": 2500 }))).unwrap(),
                    want
                );
                let err = Op::parse(op, Some(json!({ "timeout_ms": 0 }))).unwrap_err();
                assert!(err.contains("timeout_ms must be"), "{err}");
            }
            assert_eq!(
                Op::parse(
                    "navigate",
                    Some(json!({ "url": "https://example.com/", "timeout_ms": 2500 }))
                )
                .unwrap(),
                Op::Navigate {
                    url: "https://example.com/".into(),
                    timeout: t
                }
            );
            assert!(Op::parse("navigate", None).unwrap_err().contains("url"));
            assert_eq!(Op::parse("snapshot", None).unwrap(), Op::Snapshot);
            assert_eq!(
                Op::parse("snapshot", Some(json!({}))).unwrap(),
                Op::Snapshot
            );
            assert_eq!(Op::parse("stop", None).unwrap(), Op::Stop);
            assert_eq!(Op::parse("stop", Some(json!({}))).unwrap(), Op::Stop);
            for op in ["back", "forward", "reload", "snapshot", "navigate", "stop"] {
                let err = Op::parse(
                    op,
                    Some(json!({ "url": "https://a.b/", "full_page": true })),
                )
                .unwrap_err();
                assert!(
                    err.starts_with(&format!("invalid args for `{op}`")),
                    "{err}"
                );
            }
            assert_eq!(
                Op::parse("click", None).unwrap_err(),
                "unknown web view op `click`"
            );
        }

        fn look(url: &str, loading: bool, finished: u64) -> Look {
            Look {
                state: Loaded {
                    url: url.into(),
                    title: "T".into(),
                    loading,
                    error: None,
                },
                finished,
            }
        }

        #[test]
        fn call_settle_never_answers_the_page_before_the_op() {
            const A: &str = "https://a.example/";
            const B: &str = "https://b.example/";
            // macOS (finished loads counted): the old page, not loading, is not an answer until the end.
            let mut w = Watch::new(&look(A, false, 3), true);
            assert_eq!(w.step(look(A, false, 3), false), None);
            let end = w.step(look(A, false, 3), true).unwrap();
            assert_eq!((end.url.as_str(), end.loading, end.error), (A, true, None));
            // Seen loading, then a finish counted: the new page.
            let mut w = Watch::new(&look(A, false, 3), true);
            assert_eq!(w.step(look(B, true, 3), false), None);
            assert_eq!(w.step(look(B, true, 3), true).unwrap().loading, true);
            assert_eq!(
                w.step(look(B, false, 4), false),
                Some(look(B, false, 4).state)
            );
            // A reload that started and finished between two looks: only the count shows it.
            let mut w = Watch::new(&look(A, false, 3), true);
            assert_eq!(
                w.step(look(A, false, 4), false),
                Some(look(A, false, 4).state)
            );
            // A history move within the document: the URL changed, nothing loaded.
            let mut w = Watch::new(&look(A, false, 3), true);
            assert_eq!(
                w.step(look("https://a.example/#2", false, 3), false),
                Some(look("https://a.example/#2", false, 3).state)
            );
        }

        #[test]
        fn call_settle_load_that_ends_without_finishing_fails() {
            const A: &str = "https://a.example/";
            let mut w = Watch::new(&look(A, false, 7), true);
            assert_eq!(w.step(look("https://dns.invalid/", true, 7), false), None);
            // Back on the old page, no finish: one quiet look is not yet a failure (a finish may follow).
            assert_eq!(w.step(look(A, false, 7), false), None);
            // Loading again resets it.
            assert_eq!(w.step(look(A, true, 7), false), None);
            assert_eq!(w.step(look(A, false, 7), false), None);
            let failed = w.step(look(A, false, 7), false).unwrap();
            assert_eq!(failed.error.as_deref(), Some(LOAD_FAILED));
            assert!(!failed.loading);
            assert_eq!(
                serde_json::to_value(&failed).unwrap(),
                json!({ "url": A, "title": "T", "loading": false, "error": "load failed" })
            );
            // Without counts (iOS, a history move WebKit gave no navigation for): loading seen and over is done.
            let mut w = Watch::new(&look(A, false, 0), false);
            assert_eq!(w.step(look(A, false, 0), false), None);
            assert_eq!(w.step(look(A, true, 0), false), None);
            assert_eq!(
                w.step(look(A, false, 0), false),
                Some(look(A, false, 0).state)
            );
        }

        #[test]
        fn call_load_answer_omits_error_unless_failed() {
            assert_eq!(
                serde_json::to_value(look("https://a.example/", false, 0).state).unwrap(),
                json!({ "url": "https://a.example/", "title": "T", "loading": false })
            );
            assert_eq!(
                load_error("NSURLErrorDomain", -1003),
                "NSURLErrorDomain/-1003"
            );
        }

        #[test]
        fn call_snapshot_css_size() {
            assert_eq!(css_size(1024.0, 768.0, 1.0), (1024, 768));
            assert_eq!(css_size(820.0, 1180.0, 2.0), (410, 590));
            assert_eq!(css_size(375.5, 667.4, 1.0), (376, 667));
            assert_eq!(css_size(800.0, 600.0, 0.0), (800, 600));
            assert_eq!(css_size(800.0, 600.0, f64::NAN), (800, 600));
            assert_eq!(css_size(800.0, 600.0, -2.0), (800, 600));
        }

        #[test]
        fn call_script_args() {
            let s = script(json!({ "world": "isolated", "source": "return 1" })).unwrap();
            assert_eq!(s.world, World::Isolated);
            assert_eq!(s.timeout, DEFAULT_TIMEOUT);
            assert_eq!(s.args_json, "[]");
            let s = script(json!({
                "world": "page",
                "source": "return a + b.x",
                "arguments": { "a": 1, "b": { "x": "two" } },
                "timeout_ms": 1500,
            }))
            .unwrap();
            assert_eq!(s.world, World::Page);
            assert_eq!(s.timeout, Duration::from_millis(1500));
            assert_eq!(s.args_json, r#"[1,{"x":"two"}]"#);
            assert!(
                s.body
                    .starts_with("const __theuth_f=async function(a,b){\nreturn a + b.x\n};")
            );

            for (args, needle) in [
                (json!({ "source": "1" }), "world"),
                (json!({ "world": "main", "source": "1" }), "`main`"),
                (json!({ "world": "page" }), "source"),
                (
                    json!({ "world": "page", "source": "1", "args": {} }),
                    "args",
                ),
                (
                    json!({ "world": "page", "source": "1", "arguments": [] }),
                    "map",
                ),
                (
                    json!({ "world": "page", "source": "1", "arguments": { "1x": 0 } }),
                    "1x",
                ),
                (
                    json!({ "world": "page", "source": "1", "arguments": { "a-b": 0 } }),
                    "a-b",
                ),
                (
                    json!({ "world": "page", "source": "1", "arguments": { "__theuth_args": 0 } }),
                    "__theuth_args",
                ),
                (
                    json!({ "world": "page", "source": "1", "timeout_ms": 0 }),
                    "timeout_ms",
                ),
                (
                    json!({ "world": "page", "source": "1", "timeout_ms": -5 }),
                    "-5",
                ),
            ] {
                let err = script(args.clone()).unwrap_err();
                assert!(
                    err.starts_with("invalid args for `script`"),
                    "{args}: {err}"
                );
                assert!(err.contains(needle), "{args}: {err}");
            }
            let big = "x".repeat(MAX_SOURCE + 1);
            assert!(
                script(json!({ "world": "page", "source": big }))
                    .unwrap_err()
                    .contains("source")
            );
            let many: Map<String, Value> = (0..=MAX_ARGUMENTS)
                .map(|i| (format!("a{i}"), json!(i)))
                .collect();
            assert!(
                script(json!({ "world": "page", "source": "1", "arguments": many }))
                    .unwrap_err()
                    .contains("arguments")
            );
        }

        #[test]
        fn call_timeout_bounds() {
            assert_eq!(timeout(None).unwrap(), DEFAULT_TIMEOUT);
            assert_eq!(timeout(Some(1)).unwrap(), Duration::from_millis(1));
            assert_eq!(timeout(Some(300_000)).unwrap(), MAX_TIMEOUT);
            assert!(timeout(Some(0)).is_err());
            assert_eq!(
                timeout(Some(300_001)).unwrap_err(),
                "timeout_ms must be 1..=300000, got 300001"
            );
            assert_eq!(
                timed_out(Duration::from_millis(1500)),
                "script timed out after 1500 ms"
            );
        }

        #[test]
        fn call_identifiers() {
            for ok in ["a", "_", "$", "ref", "e3", "$el", "snake_case", "camelCase"] {
                assert!(identifier(ok), "{ok}");
            }
            for bad in ["", "3e", "a b", "a-b", "é", "a.b", "a;b"] {
                assert!(!identifier(bad), "{bad}");
            }
        }

        #[test]
        fn call_script_reply_shape() {
            assert_eq!(
                reply(Some(r#"{"value":{"n":1,"s":"x"}}"#)).unwrap(),
                json!({"n":1,"s":"x"})
            );
            assert_eq!(reply(Some(r#"{"value":null}"#)).unwrap(), Value::Null);
            assert_eq!(
                reply(Some(r#"{"value":[1,"a",true]}"#)).unwrap(),
                json!([1, "a", true])
            );
            assert_eq!(
                reply(Some(r#"{"error":"TypeError: x is undefined"}"#)).unwrap_err(),
                "script threw: TypeError: x is undefined"
            );
            assert_eq!(
                reply(Some(r#"{"not_json":"cyclic object value"}"#)).unwrap_err(),
                "script result is not JSON: cyclic object value"
            );
            assert!(
                reply(Some(r#"{"over":3000000}"#))
                    .unwrap_err()
                    .starts_with("script result too large: 3000000")
            );
            for bad in [
                None,
                Some(""),
                Some("42"),
                Some(r#"{"value":1,"extra":2}"#),
                Some("{"),
            ] {
                assert_eq!(
                    reply(bad).unwrap_err(),
                    "script answered an unexpected shape"
                );
            }
            assert_eq!(
                failed(&"x".repeat(5000)).len(),
                "script failed: ".len() + MAX_THROWN
            );
        }

        #[test]
        fn call_holder_rule() {
            let tile = "web-0123456789abcdef";
            assert!(check_holder(tile, "main", "main", "main").is_ok());
            assert!(check_holder(tile, "popout-chat-1", "popout-chat-1", "popout-chat-1").is_ok());
            // A tile, another window, or a second webview of the holding window.
            for (caller, window, holder) in [
                ("web-fedcba9876543210", "main", "main"),
                (tile, "main", "main"),
                ("main", "main", "popout-web-0123456789abcdef"),
                ("island", "island", "main"),
                ("helper", "main", "main"),
            ] {
                assert_eq!(
                    check_holder(tile, caller, window, holder).unwrap_err(),
                    format!("no access to web view `{tile}` from `{caller}`")
                );
            }
        }

        #[test]
        fn call_snapshot_fit() {
            assert_eq!(fit(0, 10), None);
            assert_eq!(fit(10, 0), None);
            assert_eq!(fit(800, 600), Some((800, 600)));
            assert_eq!(fit(1600, 1600), Some((1600, 1600)));
            assert_eq!(fit(3200, 2000), Some((1600, 1000)));
            assert_eq!(fit(2048, 2732), Some((1199, 1600)));
            assert_eq!(fit(2732, 2048), Some((1600, 1199)));
            assert_eq!(fit(100_000, 10), Some((1600, 1)));
            assert_eq!(fit(1601, 1), Some((1600, 1)));
        }

        #[test]
        fn call_straighten() {
            let mut px = [
                10, 20, 30, 255, // opaque: unchanged
                0, 0, 0, 0, // clear: unchanged
                64, 32, 0, 128, // half: colour doubled, made opaque
                200, 0, 0, 100, // over-bright input clamps
                1, 2, 3, // a partial pixel is left alone
            ];
            straighten(&mut px);
            assert_eq!(
                px,
                [
                    10, 20, 30, 255, 0, 0, 0, 0, 128, 64, 0, 255, 255, 0, 0, 255, 1, 2, 3
                ]
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
