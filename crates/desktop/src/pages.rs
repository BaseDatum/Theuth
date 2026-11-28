//! Design 120 §4.2: the `theuth-page` scheme. The UI fetches an `.html` artifact over RPC, injects
//! its bootstrap and hands the bytes to [`page_put`]; the webview then loads them as their *own*
//! document from `theuth-page://localhost/<token>` inside `<iframe sandbox="allow-scripts
//! allow-forms">`. Custom-protocol documents never receive `security.csp`, so the headers below are
//! the page's whole policy: a served `Content-Type` (wry sets none), `CSP: sandbox …` for an opaque
//! origin even top-level, `nosniff`, `no-store`, no `Access-Control-Allow-Origin` ever. The token is
//! fresh 128-bit randomness per put — never the `call_id` — because a sandboxed page may navigate
//! itself and page URLs must stay unguessable.

use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tauri::http::{Method, Request, Response, StatusCode, header};
use tauri::{Manager, Runtime, State, UriSchemeContext};

/// Registered on the shared builder; URL form per platform in [`page_url`].
pub const SCHEME: &str = "theuth-page";
const MAX_ENTRIES: usize = 64;
const MAX_BYTES: usize = 128 * 1024 * 1024;

/// Security policy profile for a `theuth-page` document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PageProfile {
    /// Existing HTML artifact policy (unchanged: `sandbox allow-scripts allow-forms`).
    #[default]
    Standard,
    /// Strict Deck HTML section policy (design doc 229): zero network, zero forms,
    /// no same-origin, no embedded objects/frames — only inline styles/scripts + data URIs.
    DeckStrict,
}

impl PageProfile {
    /// Parse an optional profile selector, failing closed on unknown or empty explicit values.
    /// Omitting `profile` (`None`) preserves the legacy `Standard` artifact profile.
    pub fn parse(raw: Option<&str>) -> Result<Self, String> {
        match raw.map(str::trim) {
            None | Some("standard") => Ok(Self::Standard),
            Some("deck") | Some("deck_strict") => Ok(Self::DeckStrict),
            Some(other) => Err(format!("unknown page profile: {other}")),
        }
    }

    pub const fn csp(self) -> &'static str {
        match self {
            Self::Standard => "sandbox allow-scripts allow-forms",
            Self::DeckStrict => {
                "sandbox allow-scripts; default-src 'none'; connect-src 'none'; form-action 'none'; base-uri 'none'; object-src 'none'; frame-src 'none'; child-src 'none'; worker-src 'none'; media-src 'none'; img-src data:; font-src data:; style-src 'unsafe-inline'; script-src 'unsafe-inline'"
            }
        }
    }
}

#[derive(Clone)]
struct PageEntry {
    body: Arc<[u8]>,
    profile: PageProfile,
}

/// Managed state: token → page bytes + policy profile, LRU-bounded by count and total bytes.
#[derive(Default)]
pub struct Pages(Mutex<Inner>);

#[derive(Default)]
struct Inner {
    by_token: HashMap<String, PageEntry>,
    /// One page per call; a second `page_put` for the same call replaces the first.
    by_call: HashMap<String, String>,
    /// Least recently used at the front. ponytail: O(n) refresh, n ≤ 64.
    order: VecDeque<String>,
    bytes: usize,
}

impl Inner {
    fn remove(&mut self, token: &str) {
        if let Some(entry) = self.by_token.remove(token) {
            self.bytes -= entry.body.len();
            self.order.retain(|t| t != token);
            self.by_call.retain(|_, t| t != token);
        }
    }
}

impl Pages {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A plain cache: nothing half-done survives a panic, so a poisoned lock is still usable.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Store `html` under a fresh token with the standard artifact profile; returns the token.
    #[cfg(test)]
    pub fn put(&self, call_id: String, html: Vec<u8>) -> String {
        self.put_with_profile(call_id, html, PageProfile::Standard)
    }

    /// Store `html` under a fresh token with an explicit [`PageProfile`]; returns the token.
    pub fn put_with_profile(&self, call_id: String, html: Vec<u8>, profile: PageProfile) -> String {
        let token = format!("{:032x}", rand::random::<u128>());
        let mut inner = self.lock();
        if let Some(old) = inner.by_call.remove(&call_id) {
            inner.remove(&old);
        }
        inner.bytes += html.len();
        inner.by_token.insert(
            token.clone(),
            PageEntry {
                body: Arc::from(html),
                profile,
            },
        );
        inner.by_call.insert(call_id, token.clone());
        inner.order.push_back(token.clone());
        // The page just put always stays, even when it alone exceeds the byte budget.
        while inner.order.len() > 1 && (inner.order.len() > MAX_ENTRIES || inner.bytes > MAX_BYTES)
        {
            let oldest = inner.order[0].clone();
            inner.remove(&oldest);
        }
        token
    }

    pub fn drop_call(&self, call_id: &str) {
        let mut inner = self.lock();
        if let Some(token) = inner.by_call.remove(call_id) {
            inner.remove(&token);
        }
    }

    /// The entry for `token`, refreshing its recency.
    fn get_entry(&self, token: &str) -> Option<PageEntry> {
        let mut inner = self.lock();
        let entry = inner.by_token.get(token)?.clone();
        if let Some(i) = inner.order.iter().position(|t| t == token) {
            inner.order.remove(i);
            inner.order.push_back(token.to_owned());
        }
        Some(entry)
    }

    #[cfg(test)]
    fn get(&self, token: &str) -> Option<Arc<[u8]>> {
        self.get_entry(token).map(|e| e.body)
    }
}

/// tauri's own host rule for custom schemes (`protocol/isolation.rs`, `custom_protocol_workaround.rs`):
/// `<scheme>://localhost` on macOS/iOS/Linux, `http://<scheme>.localhost` on Windows/Android.
fn page_url(token: &str) -> String {
    if cfg!(any(windows, target_os = "android")) {
        format!("http://{SCHEME}.localhost/{token}")
    } else {
        format!("{SCHEME}://localhost/{token}")
    }
}

#[tauri::command(rename_all = "snake_case")]
pub fn page_put(
    pages: State<'_, Pages>,
    call_id: String,
    html: String,
    profile: Option<String>,
) -> Result<String, String> {
    let policy = PageProfile::parse(profile.as_deref())?;
    Ok(page_url(&pages.put_with_profile(
        call_id,
        html.into_bytes(),
        policy,
    )))
}

#[tauri::command(rename_all = "snake_case")]
pub fn page_drop(pages: State<'_, Pages>, call_id: String) -> Result<(), String> {
    pages.drop_call(&call_id);
    Ok(())
}

/// The `register_uri_scheme_protocol` callback.
pub fn handler<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    req: Request<Vec<u8>>,
) -> Response<Cow<'static, [u8]>> {
    respond(&ctx.app_handle().state::<Pages>(), &req)
}

/// §4.2 table. Keyed on the path alone: the query is never read and a fragment never arrives.
fn respond(pages: &Pages, req: &Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    if req.method() != Method::GET {
        return status(StatusCode::METHOD_NOT_ALLOWED);
    }
    let token = req.uri().path().strip_prefix('/').unwrap_or_default();
    let Some(entry) = pages.get_entry(token) else {
        return status(StatusCode::NOT_FOUND);
    };
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CONTENT_SECURITY_POLICY, entry.profile.csp())
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::REFERRER_POLICY, "no-referrer");
    if entry.profile == PageProfile::DeckStrict {
        builder = builder.header("x-dns-prefetch-control", "off");
    }
    builder
        .body(Cow::Owned(entry.body.to_vec()))
        .expect("static headers")
}

fn status(code: StatusCode) -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(code)
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Cow::Borrowed(&[][..]))
        .expect("static headers")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(url: &str) -> Request<Vec<u8>> {
        Request::builder().uri(url).body(Vec::new()).unwrap()
    }

    fn put(pages: &Pages, call: &str, html: &str) -> String {
        pages.put(call.into(), html.as_bytes().to_vec())
    }

    fn header<'a>(res: &'a Response<Cow<'static, [u8]>>, name: header::HeaderName) -> &'a str {
        res.headers().get(name).map_or("", |v| v.to_str().unwrap())
    }

    #[test]
    fn known_token_gets_the_frozen_headers_and_body() {
        let pages = Pages::default();
        let token = put(&pages, "c1", "<p>hi</p>");
        let res = respond(&pages, &get(&format!("theuth-page://localhost/{token}")));
        assert_eq!(res.status(), StatusCode::OK);
        let headers: Vec<(String, String)> = res
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap().to_string()))
            .collect();
        assert_eq!(
            headers,
            [
                ("content-type", "text/html; charset=utf-8"),
                (
                    "content-security-policy",
                    "sandbox allow-scripts allow-forms"
                ),
                ("x-content-type-options", "nosniff"),
                ("cache-control", "no-store"),
                ("referrer-policy", "no-referrer"),
            ]
            .map(|(k, v)| (k.to_string(), v.to_string()))
        );
        assert!(
            res.headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none()
        );
        assert_eq!(&res.body()[..], b"<p>hi</p>");
        // Windows/Android host form resolves to the same path.
        let res = respond(
            &pages,
            &get(&format!("http://theuth-page.localhost/{token}")),
        );
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[test]
    fn unknown_token_is_404_text_plain_without_body() {
        let pages = Pages::default();
        put(&pages, "c1", "x");
        let res = respond(&pages, &get("theuth-page://localhost/nope"));
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        assert_eq!(header(&res, header::CONTENT_TYPE), "text/plain");
        assert!(res.body().is_empty());
        assert!(
            res.headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none()
        );
        assert_eq!(
            respond(&pages, &get("theuth-page://localhost/")).status(),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn other_methods_are_405() {
        let pages = Pages::default();
        let token = put(&pages, "c1", "x");
        for m in [Method::POST, Method::HEAD, Method::PUT, Method::OPTIONS] {
            let req = Request::builder()
                .method(m.clone())
                .uri(format!("theuth-page://localhost/{token}"))
                .body(Vec::new())
                .unwrap();
            assert_eq!(
                respond(&pages, &req).status(),
                StatusCode::METHOD_NOT_ALLOWED,
                "{m}"
            );
        }
    }

    #[test]
    fn query_and_fragment_are_ignored() {
        let pages = Pages::default();
        let token = put(&pages, "c1", "x");
        for suffix in ["?x=1&y=2", "#theuth-theme=%7B%7D", "?q#frag"] {
            let res = respond(
                &pages,
                &get(&format!("theuth-page://localhost/{token}{suffix}")),
            );
            assert_eq!(res.status(), StatusCode::OK, "{suffix}");
        }
        // A token is the whole path: a sub-path is not a match.
        let res = respond(
            &pages,
            &get(&format!("theuth-page://localhost/{token}/extra")),
        );
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn evicts_by_count_least_recently_used_first() {
        let pages = Pages::default();
        let first = put(&pages, "c0", "x");
        let second = put(&pages, "c1", "x");
        for i in 2..MAX_ENTRIES {
            put(&pages, &format!("c{i}"), "x");
        }
        assert!(pages.get(&first).is_some(), "64 entries fit");
        // `first` was just read, so `second` is the oldest now.
        put(&pages, "c64", "x");
        assert!(pages.get(&first).is_some());
        assert!(pages.get(&second).is_none(), "65th put evicts the oldest");
        assert_eq!(pages.lock().order.len(), MAX_ENTRIES);
    }

    #[test]
    fn evicts_by_bytes_least_recently_used_first() {
        let pages = Pages::default();
        let big = vec![b'x'; MAX_BYTES / 4];
        let a = pages.put("a".into(), big.clone());
        let b = pages.put("b".into(), big.clone());
        let c = pages.put("c".into(), big.clone());
        let d = pages.put("d".into(), big.clone());
        assert!(pages.get(&a).is_some(), "exactly 128 MiB fits");
        // `a` was just read, so `b` is now the least recently used.
        pages.put("e".into(), vec![b'x'; 1]);
        assert!(pages.get(&a).is_some());
        assert!(pages.get(&b).is_none());
        assert!(pages.get(&c).is_some() && pages.get(&d).is_some());
        assert!(pages.lock().bytes <= MAX_BYTES);
        // A single page over budget still lands; everything else goes.
        let huge = pages.put("h".into(), vec![b'x'; MAX_BYTES + 1]);
        assert!(pages.get(&huge).is_some());
        assert_eq!(pages.lock().order.len(), 1);
    }

    #[test]
    fn page_drop_evicts_that_call_only() {
        let pages = Pages::default();
        let t1 = put(&pages, "c1", "one");
        let t2 = put(&pages, "c2", "two");
        pages.drop_call("c1");
        assert!(pages.get(&t1).is_none());
        assert!(pages.get(&t2).is_some());
        assert_eq!(pages.lock().bytes, 3);
        pages.drop_call("never-put");
        assert!(pages.get(&t2).is_some());
    }

    #[test]
    fn re_put_for_a_call_replaces_its_page() {
        let pages = Pages::default();
        let t1 = put(&pages, "c1", "one");
        let t2 = put(&pages, "c1", "two");
        assert!(pages.get(&t1).is_none());
        assert_eq!(&pages.get(&t2).unwrap()[..], b"two");
        assert_eq!(pages.lock().order.len(), 1);
    }

    #[test]
    fn tokens_are_unique_random_and_not_the_call_id() {
        let pages = Pages::default();
        let a = put(&pages, "call-1", "x");
        let b = put(&pages, "call-1", "x");
        assert_ne!(a, b);
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert!(!a.contains("call-1"));
        assert!(page_url(&a).ends_with(&format!("localhost/{a}")));
    }

    #[test]
    fn deck_strict_profile_enforces_no_network_no_forms_no_same_origin() {
        let pages = Pages::default();
        let deck_tok = pages.put_with_profile(
            "deck-sec-1".into(),
            b"<p>deck</p>".to_vec(),
            PageProfile::DeckStrict,
        );
        let deck_res = respond(&pages, &get(&format!("theuth-page://localhost/{deck_tok}")));
        assert_eq!(deck_res.status(), StatusCode::OK);
        let csp = header(&deck_res, header::CONTENT_SECURITY_POLICY);
        assert!(csp.contains("sandbox allow-scripts"));
        assert!(!csp.contains("allow-same-origin"));
        assert!(!csp.contains("allow-forms"));
        assert!(csp.contains("default-src 'none'"));
        assert!(csp.contains("connect-src 'none'"));
        assert!(csp.contains("form-action 'none'"));
        assert!(csp.contains("object-src 'none'"));
        assert!(csp.contains("frame-src 'none'"));
        assert!(csp.contains("child-src 'none'"));
        assert!(csp.contains("worker-src 'none'"));
        assert!(csp.contains("media-src 'none'"));
        assert_eq!(
            deck_res
                .headers()
                .get("x-dns-prefetch-control")
                .and_then(|v| v.to_str().ok()),
            Some("off")
        );
        assert!(
            deck_res
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none()
        );

        // Standard artifact pages keep their existing policy untouched.
        let std_tok = put(&pages, "art-1", "<p>artifact</p>");
        let std_res = respond(&pages, &get(&format!("theuth-page://localhost/{std_tok}")));
        assert_eq!(
            header(&std_res, header::CONTENT_SECURITY_POLICY),
            "sandbox allow-scripts allow-forms"
        );
        assert!(std_res.headers().get("x-dns-prefetch-control").is_none());
    }

    #[test]
    fn page_profile_parse_fails_closed_on_unknown_values() {
        assert_eq!(PageProfile::parse(None), Ok(PageProfile::Standard));
        assert_eq!(
            PageProfile::parse(Some("standard")),
            Ok(PageProfile::Standard)
        );
        assert_eq!(
            PageProfile::parse(Some(" deck ")),
            Ok(PageProfile::DeckStrict)
        );
        assert_eq!(
            PageProfile::parse(Some("deck_strict")),
            Ok(PageProfile::DeckStrict)
        );
        for bad in ["", "   ", "deck-strict", "DECK", "Standard", "relaxed", "none"] {
            assert!(
                PageProfile::parse(Some(bad)).is_err(),
                "expected Err for unknown profile {bad:?}"
            );
        }
    }
}
