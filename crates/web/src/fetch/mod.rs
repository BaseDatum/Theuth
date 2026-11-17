//! Page fetch (design doc 16 §5): HTTP GET behind the SSRF guard, manual
//! redirects re-checked per hop, body caps, and HTML → markdown / text; plus
//! the raw [`Fetcher::request`] used for route replay (44 §0.6).
//!
//! - `ssrf`: the mandatory guard; [`check_url`] for callers outside the fetcher.
//! - `text`: one HTML parser for both [`FetchFormat`]s, plus truncation and the pageable [`cut`].
//! - `client`: the HTTP client seam (`reqwest` today, `wreq` Chrome emulation by W5).

pub mod charset;
pub mod client;
mod download;
pub mod issue;
pub mod meta;
mod ssrf;
mod text;

use std::io::Read;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::stream::{BoxStream, StreamExt, TryStreamExt};
use reqwest::header::{HeaderName, HeaderValue};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

use client::{ClientConfig, ClientError, ClientKind, HttpClient, Impersonate};

pub use download::{DownloadError, DownloadOpts, Downloaded, download};
pub use ssrf::{check_url, is_blocked_hostname, is_blocked_ip};
pub use text::{Cut, MAX_OMITTED_HEADINGS, collapse_ws, cut, hard_truncate, take_title};

/// Whole-request timeout for one page fetch.
pub const TIMEOUT: Duration = Duration::from_secs(8);
/// Connect timeout.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Output shape of fetched content.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum FetchFormat {
    /// Plain prose: markdown with headings, links and emphasis flattened.
    Text,
    /// Markdown keeping headings and links so the model can follow up.
    #[default]
    Markdown,
}

/// A fetched page (the `structured` half of `read_url`). Every field added by
/// design doc 122 (`issue` onward) has a serde default, so older callers and
/// stored payloads keep working.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FetchResponse {
    /// Final URL after redirects.
    pub url: String,
    /// `<title>` or first heading, when the body was HTML.
    pub title: Option<String>,
    /// Content in the requested [`FetchFormat`].
    pub content: String,
    /// Whether the content or the raw body was cut at a cap.
    pub truncated: bool,
    /// Wall time of the fetch in milliseconds.
    pub took_ms: u64,
    /// Why the page did not read as content, with the next rung.
    #[serde(default)]
    pub issue: Option<issue::PageIssue>,
    /// title, description, author, published (RFC 3339), lang, canonical, site_name.
    #[serde(default)]
    pub meta: meta::PageMeta,
    /// HTTP attempts made (every request of every rung).
    #[serde(default = "default_attempts")]
    pub attempts: u8,
    /// A rung after the first attempt produced this response (122 R1-D1).
    #[serde(default)]
    pub ladder: bool,
    /// Encoding label used to decode the body.
    #[serde(default = "default_charset")]
    pub charset: String,
    /// Characters removed by compaction before the cap.
    #[serde(default)]
    pub compacted_chars: usize,
    /// Pass as `offset` to continue; null when nothing was omitted.
    #[serde(default)]
    pub next_offset: Option<usize>,
    /// Headings of the omitted tail (≤ 12).
    #[serde(default)]
    pub omitted_headings: Vec<String>,
    /// `fetch`, `render` or `api`.
    #[serde(default = "default_tier")]
    pub tier: String,
    /// The API source used (`hn-item`, `npm`, …).
    #[serde(default)]
    pub source_kind: Option<String>,
    /// Non-fatal remarks (an API source that failed, a ladder attempt that did not help).
    #[serde(default)]
    pub notes: Vec<String>,
    /// Which HTTP client answered (`reqwest` or `wreq_chrome`).
    #[serde(default)]
    pub client: ClientKind,
}

fn default_attempts() -> u8 {
    1
}

fn default_charset() -> String {
    "utf-8".to_owned()
}

fn default_tier() -> String {
    "fetch".to_owned()
}

impl Default for FetchResponse {
    fn default() -> Self {
        Self {
            url: String::new(),
            title: None,
            content: String::new(),
            truncated: false,
            took_ms: 0,
            issue: None,
            meta: meta::PageMeta::default(),
            attempts: default_attempts(),
            ladder: false,
            charset: default_charset(),
            compacted_chars: 0,
            next_offset: None,
            omitted_headings: Vec::new(),
            tier: default_tier(),
            source_kind: None,
            notes: Vec::new(),
            client: ClientKind::default(),
        }
    }
}

/// Raw reply of [`Fetcher::request`]; non-2xx is `Ok`, read `status`.
#[derive(Clone, Debug)]
pub struct RawResponse {
    /// Final URL after redirects.
    pub url: String,
    /// HTTP status code.
    pub status: u16,
    /// Response headers as `(lowercase name, value)`; non-UTF-8 values skipped.
    pub headers: Vec<(String, String)>,
    /// Raw body bytes, cut at the fetcher's byte cap.
    pub body: Vec<u8>,
    /// Whether `body` was cut at the cap.
    pub truncated: bool,
    /// Wall time of the request in milliseconds.
    pub took_ms: u64,
}

/// Why a fetch produced nothing.
#[derive(Clone, Debug, thiserror::Error)]
pub enum FetchError {
    /// The URL failed the SSRF guard or a hard policy (redirect count).
    #[error("rejected: {0}")]
    Rejected(String),
    /// The request ran but did not yield usable content.
    #[error("{0}")]
    Failed(String),
}

/// Start of the [`FetchError::Rejected`] text for a redirect chain past `max_redirects`.
const TOO_MANY_REDIRECTS: &str = "too many redirects";

impl FetchError {
    /// Map a transport error, surfacing a guard refusal from the DNS
    /// resolver as [`FetchError::Rejected`].
    fn from_reqwest(e: &reqwest::Error) -> Self {
        ClientError::from_reqwest(e).into()
    }

    /// The refusal is the redirect limit, not the guard (scheme, credentials, an address): a
    /// browser may still try such a page (16 §5, 2026-11-11); a guard refusal never.
    #[must_use]
    pub fn is_redirect_limit(&self) -> bool {
        matches!(self, Self::Rejected(m) if m.starts_with(TOO_MANY_REDIRECTS))
    }
}

impl From<ClientError> for FetchError {
    fn from(e: ClientError) -> Self {
        match e.as_rejected() {
            Some(m) => Self::Rejected(m.to_owned()),
            None => Self::Failed(e.to_string()),
        }
    }
}

/// Most HTTP attempts per client rung (design doc 122 D2).
pub const MAX_ATTEMPTS: u8 = 2;
/// Sleep before attempt `n` (D2); a `Retry-After` replaces the second entry.
const RETRY_DELAYS: [Duration; 2] = [Duration::ZERO, Duration::from_secs(1)];
/// Statuses worth a second attempt.
const RETRY_STATUSES: [u16; 9] = [429, 502, 503, 504, 520, 521, 522, 523, 524];
/// `Accept` on every `Reqwest` page fetch (D4).
pub const ACCEPT: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";
/// `notes` entry when the stealth client is configured but could not be built
/// (122 R1-D1); an installation that disables `wreq` still reads pages.
pub const STEALTH_NOT_BUILT: &str = "stealth client not built; fetched with reqwest";

/// How a page window is produced (122 D6/D7): `cut` to `max_chars` from
/// `offset`, after `compact::compact` when `compact` is set.
#[derive(Clone, Copy, Debug)]
pub struct PageOptions {
    /// Content cap in chars.
    pub max_chars: usize,
    /// Char offset of the window.
    pub offset: usize,
    /// Run `compact` before the cut.
    pub compact: bool,
}

impl PageOptions {
    /// `max_chars` from the start, compaction on.
    #[must_use]
    pub fn new(max_chars: usize) -> Self {
        Self {
            max_chars,
            offset: 0,
            compact: true,
        }
    }
}

/// Markdown after compaction, projection and the cut (122 D6/D7).
pub struct Finished {
    /// The window in the requested format, marker included.
    pub content: String,
    /// `compact`'s `removed_chars`.
    pub compacted_chars: usize,
    /// The cut's fields.
    pub cut: Cut,
}

/// `compact` (when asked) → `markdown_to_text` (for [`FetchFormat::Text`]) → [`cut`].
#[must_use]
pub fn finish(markdown: &str, format: FetchFormat, opts: &PageOptions) -> Finished {
    let (md, compacted_chars) = if opts.compact {
        let c = crate::compact::compact(
            markdown,
            &crate::compact::CompactOptions {
                fold_links: format == FetchFormat::Text,
                keep_images: false,
            },
        );
        (c.text, c.removed_chars)
    } else {
        (markdown.to_owned(), 0)
    };
    let body = match format {
        FetchFormat::Markdown => md,
        FetchFormat::Text => text::markdown_to_text(&md),
    };
    let cut = cut(&body, opts.offset, opts.max_chars);
    Finished {
        content: cut.text.clone(),
        compacted_chars,
        cut,
    }
}

/// One settled (non-redirect) response of an attempt, body already read.
struct Hop {
    status: u16,
    headers: reqwest::header::HeaderMap,
    url: Url,
    raw: Vec<u8>,
    truncated: bool,
}

/// What one attempt produced.
enum Attempt {
    Hop(Hop),
    /// Guard refusal, binary refusal, oversize `Content-Length`: never retried.
    Fatal(FetchError),
    /// Transport, decode or budget error: retried while the budget allows.
    Transient(FetchError),
}

/// HTTP page fetcher behind the SSRF guard.
///
/// Redirects are disabled on the client and followed by hand so every
/// `Location` re-runs the guard; DNS goes through the guarded resolver in
/// `ssrf`, so the client connects only to addresses that passed
/// [`is_blocked_ip`] (closes the resolve/connect TOCTOU of 16 §10).
///
/// Page fetches (`fetch`/`fetch_html`) run the D2 retry loop under one total
/// budget of `2 × timeout`, dispatch between the `Reqwest` and `WreqChrome`
/// clients per [`Impersonate`] (122 R1-D1), and diagnose walls with
/// [`issue::detect`]. [`Fetcher::request`] keeps one attempt and plain
/// `reqwest`.
///
/// The guard is public-only. [`Self::with_private_network`] adds a twin whose
/// guard and clients admit the local network; a call uses it only inside a
/// [`tools::egress::scope`] granting it (an owner's tool call), so contacts'
/// turns and unscoped callers stay public-only, and the twin's connection
/// pool is never shared with the public lane.
pub struct Fetcher {
    /// The honest client (122 §5): the only one `Impersonate::Never` uses.
    reqwest: Arc<dyn HttpClient>,
    /// The Chrome-emulating client; `None` when `impersonate` is `never` or
    /// the build failed (then the `Reqwest` fallback carries [`STEALTH_NOT_BUILT`]).
    stealth: Option<Arc<dyn HttpClient>>,
    impersonate: Impersonate,
    /// Under `OnChallenge`: escalate to `stealth` on a diagnosed 403/503 challenge.
    header_ladder: bool,
    accept_language: String,
    /// `request` (route replay, arbitrary method and body) keeps a plain
    /// `reqwest` client built from the same config: never stealth (122 §5).
    client: reqwest::Client,
    cfg: ClientConfig,
    max_bytes: usize,
    max_redirects: usize,
    allow: Vec<SocketAddr>,
    /// This fetcher's guard admits the local network (only ever the twin).
    private: bool,
    /// The local-network twin, when `web.allow_private_network` is on.
    lan: Option<Box<Fetcher>>,
}

impl Fetcher {
    /// Build a fetcher: `redirect::Policy::none()`, 8 s total / 2 s connect,
    /// `user_agent`, raw body capped at `max_bytes`, at most `max_redirects` hops;
    /// `Impersonate::default()` and the default `Accept-Language` until
    /// [`Self::with_policy`] applies the config.
    ///
    /// # Errors
    /// When the `reqwest` client cannot be built.
    pub fn new(user_agent: &str, max_bytes: usize, max_redirects: usize) -> anyhow::Result<Self> {
        Self::new_with_timeouts(
            user_agent,
            max_bytes,
            max_redirects,
            TIMEOUT,
            CONNECT_TIMEOUT,
        )
    }

    /// Like [`Self::new`] with explicit whole-request and connect timeouts;
    /// the total page budget is `2 × timeout`.
    ///
    /// # Errors
    /// When the `reqwest` client cannot be built.
    pub fn new_with_timeouts(
        user_agent: &str,
        max_bytes: usize,
        max_redirects: usize,
        timeout: Duration,
        connect_timeout: Duration,
    ) -> anyhow::Result<Self> {
        let defaults = crate::WebConfig::default();
        let cfg = ClientConfig {
            user_agent: user_agent.to_owned(),
            accept_language: defaults.accept_language.clone(),
            connect_timeout,
            read_timeout: timeout,
            max_body: max_bytes,
            private: false,
        };
        Ok(Self::build(cfg, max_redirects)?.with_policy(
            defaults.impersonate,
            defaults.header_ladder,
            None,
        ))
    }

    /// The `Reqwest` clients for `cfg` (its `private` sets the guard), no
    /// stealth client, no twin; `Impersonate::Never` until [`Self::with_policy`].
    fn build(cfg: ClientConfig, max_redirects: usize) -> anyhow::Result<Self> {
        let defaults = crate::WebConfig::default();
        Ok(Self {
            reqwest: client::build(ClientKind::Reqwest, &cfg)?,
            stealth: None,
            impersonate: Impersonate::Never,
            header_ladder: defaults.header_ladder,
            accept_language: cfg.accept_language.clone(),
            client: client::reqwest_client(&cfg)?,
            max_bytes: cfg.max_body,
            private: cfg.private,
            cfg,
            max_redirects,
            allow: Vec::new(),
            lan: None,
        })
    }

    /// `web.allow_private_network` (16 §5, amended 2026-11-11): `true` builds
    /// the local-network twin (same limits, policy and test allowlist, kept in
    /// step by [`Self::with_policy`] and [`Self::allowing`]); `false` drops it.
    ///
    /// # Errors
    /// When the twin's `reqwest` client cannot be built.
    pub fn with_private_network(mut self, on: bool) -> anyhow::Result<Self> {
        self.lan = None;
        if on {
            let twin = Self::build(
                ClientConfig {
                    private: true,
                    ..self.cfg.clone()
                },
                self.max_redirects,
            )?
            .with_policy(
                self.impersonate,
                self.header_ladder,
                Some(&self.accept_language),
            )
            .allowing(self.allow.clone());
            self.lan = Some(Box::new(twin));
        }
        Ok(self)
    }

    /// The lane this call runs on: the twin inside an `egress` scope granting
    /// the local network, else this (public-only) fetcher.
    fn lane(&self) -> &Self {
        match &self.lan {
            Some(twin) if tools::egress::lan() => twin,
            _ => self,
        }
    }

    /// Whether a call made here now may reach the local network; what the
    /// render path passes to [`check_url`].
    #[must_use]
    pub fn reaches_private(&self) -> bool {
        self.lane().private
    }

    /// Apply the page policy (122 R1-D1, D4): which client goes first, whether
    /// `on_challenge` escalates, and the `Accept-Language` sent. Builds the
    /// `WreqChrome` client when `impersonate` can use it; a build failure
    /// ([`ClientError::Build`]) leaves the `Reqwest` fallback with a note on
    /// every response that wanted stealth.
    #[must_use]
    pub fn with_policy(
        mut self,
        impersonate: Impersonate,
        header_ladder: bool,
        accept_language: Option<&str>,
    ) -> Self {
        if let Some(lang) = accept_language {
            self.accept_language = lang.to_owned();
            self.cfg.accept_language = lang.to_owned();
        }
        self.impersonate = impersonate;
        self.header_ladder = header_ladder;
        self.lan = self
            .lan
            .map(|twin| Box::new(twin.with_policy(impersonate, header_ladder, accept_language)));
        self.stealth = match impersonate {
            Impersonate::Never => None,
            Impersonate::OnChallenge | Impersonate::Always => {
                match client::build(ClientKind::WreqChrome, &self.cfg) {
                    Ok(c) => Some(c),
                    Err(e) => {
                        tracing::debug!(error = %e, "stealth client unavailable; reqwest fallback");
                        None
                    }
                }
            }
        };
        self
    }

    /// Test hook: let these exact blocked `ip:port` literals through the guard
    /// (loopback wiremock). Not reachable from config.
    #[doc(hidden)]
    #[must_use]
    pub fn allowing(mut self, addrs: Vec<SocketAddr>) -> Self {
        self.lan = self.lan.map(|twin| Box::new(twin.allowing(addrs.clone())));
        self.allow = addrs;
        self
    }

    /// Total budget of one page fetch: attempts, redirects and body (D2).
    #[must_use]
    pub fn total_budget(&self) -> Duration {
        self.cfg.read_timeout * 2
    }

    /// GET `url`, follow redirects under the guard, and return the body as
    /// `format`, cut to `max_chars` scalars from the start, compaction on.
    ///
    /// # Errors
    /// [`FetchError::Rejected`] from the guard or too many redirects;
    /// [`FetchError::Failed`] for transport errors, a non-2xx without a
    /// diagnosed issue, binary content or an oversize `Content-Length`.
    pub async fn fetch(
        &self,
        url: &str,
        format: FetchFormat,
        max_chars: usize,
    ) -> Result<FetchResponse, FetchError> {
        self.fetch_page(url, format, PageOptions::new(max_chars))
            .await
    }

    /// [`Self::fetch`] with an explicit window (`read_url`'s `offset`/`compact`).
    ///
    /// # Errors
    /// Same as [`Self::fetch`].
    pub async fn fetch_page(
        &self,
        url: &str,
        format: FetchFormat,
        opts: PageOptions,
    ) -> Result<FetchResponse, FetchError> {
        self.fetch_kind(url, Some(format), opts).await
    }

    /// GET `url` and return the decoded body without HTML→markdown conversion.
    /// Tool-sized truncation is the caller's `max_chars` (a hard cut, no
    /// marker: the body is raw HTML or JSON); pass a large value for
    /// crawl/scrape. `issue` and `meta` are still diagnosed on HTML bodies.
    ///
    /// # Errors
    /// Same as [`Self::fetch`].
    pub async fn fetch_html(
        &self,
        url: &str,
        max_chars: usize,
    ) -> Result<FetchResponse, FetchError> {
        self.fetch_kind(url, None, PageOptions::new(max_chars))
            .await
    }

    /// The client rungs for this fetch (122 R1-D1): `always` → stealth;
    /// `on_challenge` → reqwest, then stealth when `header_ladder`; `never` →
    /// reqwest. A wanted-but-unbuilt stealth rung is the reqwest client with
    /// [`STEALTH_NOT_BUILT`] noted once.
    fn rungs(&self, notes: &mut Vec<String>) -> Vec<Arc<dyn HttpClient>> {
        let mut stealth = || {
            self.stealth.clone().unwrap_or_else(|| {
                if !notes.iter().any(|n| n == STEALTH_NOT_BUILT) {
                    notes.push(STEALTH_NOT_BUILT.to_owned());
                }
                Arc::clone(&self.reqwest)
            })
        };
        match self.impersonate {
            Impersonate::Always => vec![stealth()],
            Impersonate::OnChallenge if self.header_ladder => {
                vec![Arc::clone(&self.reqwest), stealth()]
            }
            Impersonate::OnChallenge | Impersonate::Never => vec![Arc::clone(&self.reqwest)],
        }
    }

    /// Extra request headers per client kind: `Accept`/`Accept-Language` on
    /// `Reqwest` (D4); the Chrome profile brings its own on `WreqChrome`.
    fn headers(&self, http: &dyn HttpClient) -> Vec<(HeaderName, HeaderValue)> {
        match http.kind() {
            ClientKind::Reqwest => [
                (reqwest::header::ACCEPT, ACCEPT),
                (
                    reqwest::header::ACCEPT_LANGUAGE,
                    self.accept_language.as_str(),
                ),
            ]
            .into_iter()
            .filter_map(|(k, v)| Some((k, HeaderValue::from_str(v).ok()?)))
            .collect(),
            ClientKind::WreqChrome => Vec::new(),
        }
    }

    async fn fetch_kind(
        &self,
        url: &str,
        format: Option<FetchFormat>,
        opts: PageOptions,
    ) -> Result<FetchResponse, FetchError> {
        self.lane().fetch_on_lane(url, format, opts).await
    }

    async fn fetch_on_lane(
        &self,
        url: &str,
        format: Option<FetchFormat>,
        opts: PageOptions,
    ) -> Result<FetchResponse, FetchError> {
        let started = Instant::now();
        let deadline = started + self.total_budget();
        let start = ssrf::parse_absolute(url)?;
        let mut notes = Vec::new();
        let rungs = self.rungs(&mut notes);
        let mut attempts = 0u8;
        let mut first: Option<FetchResponse> = None;
        for (rung, http) in rungs.iter().enumerate() {
            // A failed escalation keeps the diagnosis of the first rung.
            let settle = |first: Option<FetchResponse>, e: FetchError, attempts: u8| {
                first.map_or(Err(e), |mut f| {
                    f.attempts = attempts;
                    Ok(f)
                })
            };
            let hop = match self
                .attempts(http.as_ref(), &start, deadline, &mut attempts)
                .await
            {
                Ok(hop) => hop,
                Err(e) => return settle(first, e, attempts),
            };
            let status = hop.status;
            let mut resp = match self.read_body(hop, format, &opts, started).await {
                Ok(r) => r,
                Err(e) => return settle(first, e, attempts),
            };
            resp.attempts = attempts;
            resp.client = http.kind();
            resp.ladder = rung > 0;
            resp.notes.extend(notes.iter().cloned());
            let challenge = resp
                .issue
                .as_ref()
                .is_some_and(|i| i.kind == issue::IssueKind::AccessChallenge)
                && matches!(status, 403 | 503);
            let can_escalate = rung + 1 < rungs.len()
                && deadline.saturating_duration_since(Instant::now()) >= self.cfg.connect_timeout;
            match first.take() {
                // Rung 1 hit a challenge and a stealth rung is available: escalate.
                None if challenge && can_escalate => first = Some(resp),
                // Rung 2 is still a wall: the first diagnosis stands (attempts updated).
                Some(mut f) if resp.issue.is_some() => {
                    f.attempts = attempts;
                    return Ok(f);
                }
                _ => return Ok(resp),
            }
        }
        first.ok_or_else(|| FetchError::Failed("no client rung".into()))
    }

    /// The D2 loop on one client: up to [`MAX_ATTEMPTS`] attempts, the second
    /// after [`RETRY_DELAYS`] or `Retry-After` when it fits, none when
    /// `remaining < connect_timeout`. `Rejected` and other fatal errors return
    /// at once; the last transient error or retryable status is returned when
    /// the loop runs out.
    async fn attempts(
        &self,
        http: &dyn HttpClient,
        start: &Url,
        deadline: Instant,
        attempts: &mut u8,
    ) -> Result<Hop, FetchError> {
        let mut delay = RETRY_DELAYS[0];
        let mut last: Option<Result<Hop, FetchError>> = None;
        for n in 0..MAX_ATTEMPTS {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if n > 0 && delay + self.cfg.connect_timeout > remaining {
                break;
            }
            tokio::time::sleep(delay).await;
            *attempts += 1;
            let remaining = deadline.saturating_duration_since(Instant::now());
            let attempt =
                match tokio::time::timeout(remaining, self.attempt(http, start.clone())).await {
                    Ok(a) => a,
                    Err(_) => Attempt::Transient(FetchError::Failed(format!(
                        "fetch budget of {:.1}s exceeded",
                        self.total_budget().as_secs_f32()
                    ))),
                };
            match attempt {
                Attempt::Fatal(e) => return Err(e),
                Attempt::Transient(e) => {
                    delay = RETRY_DELAYS[1];
                    last = Some(Err(e));
                }
                Attempt::Hop(hop) if RETRY_STATUSES.contains(&hop.status) => {
                    delay = issue::retry_after(&hop.headers).unwrap_or(RETRY_DELAYS[1]);
                    last = Some(Ok(hop));
                }
                Attempt::Hop(hop) => return Ok(hop),
            }
        }
        last.unwrap_or_else(|| Err(FetchError::Failed("fetch budget exhausted".into())))
    }

    /// One attempt: the redirect loop under the guard, then the capped body.
    async fn attempt(&self, http: &dyn HttpClient, mut current: Url) -> Attempt {
        let headers = self.headers(http);
        let mut redirects = 0usize;
        loop {
            if let Err(e) = ssrf::check_static(&current, &self.allow, self.private) {
                return Attempt::Fatal(e);
            }
            tracing::debug!(url = %current, redirects, client = ?http.kind(), "fetch hop");
            let mut response = match http.get(&current, &headers).await {
                Ok(r) => r,
                Err(e) => {
                    let e = FetchError::from(e);
                    return match e {
                        FetchError::Rejected(_) => Attempt::Fatal(e),
                        FetchError::Failed(_) => Attempt::Transient(e),
                    };
                }
            };
            let Ok(status) = reqwest::StatusCode::from_u16(response.status) else {
                return Attempt::Transient(FetchError::Failed(format!(
                    "invalid status: {}",
                    response.status
                )));
            };
            if Self::is_redirect(status) {
                if redirects >= self.max_redirects {
                    return Attempt::Fatal(FetchError::Rejected(format!(
                        "{TOO_MANY_REDIRECTS} (max {})",
                        self.max_redirects
                    )));
                }
                let Some(location) = response
                    .headers
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                else {
                    return Attempt::Fatal(FetchError::Failed(
                        "redirect missing Location header".into(),
                    ));
                };
                current = match current.join(location) {
                    Ok(u) => u,
                    Err(e) => {
                        return Attempt::Fatal(FetchError::Failed(format!(
                            "invalid redirect Location: {e}"
                        )));
                    }
                };
                redirects += 1;
                continue;
            }
            // A binary type is refused before the body is downloaded, except the
            // two container types the anydoc sniff may still recognise (§6.1 w2).
            let ct = primary_type(&response.headers);
            if status.is_success() && is_binary(ct.as_deref()) && !sniffable(ct.as_deref()) {
                return Attempt::Fatal(unsupported(ct.as_deref()));
            }
            let content_length = response
                .headers
                .get(reqwest::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok()?.trim().parse::<u64>().ok());
            let (raw, truncated) = if !status.is_success() && is_binary(ct.as_deref()) {
                (Vec::new(), false)
            } else {
                match self.read_capped(content_length, &mut response.body).await {
                    Ok(b) => b,
                    Err(e @ FetchError::Rejected(_)) => return Attempt::Fatal(e),
                    Err(FetchError::Failed(m)) if m.starts_with("body too large") => {
                        return Attempt::Fatal(FetchError::Failed(m));
                    }
                    Err(e) => return Attempt::Transient(e),
                }
            };
            return Attempt::Hop(Hop {
                status: status.as_u16(),
                headers: response.headers,
                url: current,
                raw,
                truncated,
            });
        }
    }

    /// `method` `url` with `headers` and optional `body`, redirects re-checked
    /// per hop like [`Self::fetch`] (44 §0.6). 301/302/303 turn into a bodyless
    /// GET; 307/308 keep method and body. Caller `host` / `content-length`
    /// headers are skipped. Caller headers go only to the origin `url` names:
    /// a redirect elsewhere is refused when any were given, because an API
    /// key can sit under any header name (`X-Api-Key`, an expanded secret
    /// placeholder) and the fetcher cannot classify them reliably.
    /// The body is raw bytes: no gzip inflate, no document or HTML conversion.
    /// One attempt, no retries (`http_request` may be non-idempotent, 122 D2).
    ///
    /// # Errors
    /// [`FetchError::Rejected`] for an invalid method, the guard, too many
    /// redirects or a cross-origin redirect with caller headers;
    /// [`FetchError::Failed`] for transport errors or an oversize
    /// `Content-Length`. Non-2xx is `Ok`.
    pub async fn request(
        &self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: Option<Vec<u8>>,
    ) -> Result<RawResponse, FetchError> {
        self.lane()
            .request_on_lane(method, url, headers, body)
            .await
    }

    async fn request_on_lane(
        &self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        mut body: Option<Vec<u8>>,
    ) -> Result<RawResponse, FetchError> {
        let started = Instant::now();
        let mut method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|_| FetchError::Rejected(format!("invalid method: {method}")))?;
        let mut current = ssrf::parse_absolute(url)?;
        let origin = current.origin();
        let mut redirects = 0usize;
        loop {
            ssrf::check_static(&current, &self.allow, self.private)?;
            tracing::debug!(url = %current, %method, redirects, "request hop");
            let mut req = self.client.request(method.clone(), current.as_str());
            for (name, value) in headers {
                if name.eq_ignore_ascii_case("host") || name.eq_ignore_ascii_case("content-length")
                {
                    continue;
                }
                req = req.header(name.as_str(), value.as_str());
            }
            if let Some(b) = &body {
                req = req.body(b.clone());
            }
            let response = req.send().await.map_err(|e| FetchError::from_reqwest(&e))?;
            let status = response.status();
            if Self::is_redirect(status) {
                if redirects >= self.max_redirects {
                    return Err(FetchError::Rejected(format!(
                        "{TOO_MANY_REDIRECTS} (max {})",
                        self.max_redirects
                    )));
                }
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| FetchError::Failed("redirect missing Location header".into()))?;
                current = current
                    .join(location)
                    .map_err(|e| FetchError::Failed(format!("invalid redirect Location: {e}")))?;
                if !headers.is_empty() && current.origin() != origin {
                    return Err(FetchError::Rejected(format!(
                        "redirect to {current} leaves the request's origin; caller headers are not forwarded across origins"
                    )));
                }
                if !matches!(
                    status,
                    reqwest::StatusCode::TEMPORARY_REDIRECT
                        | reqwest::StatusCode::PERMANENT_REDIRECT
                ) {
                    method = reqwest::Method::GET;
                    body = None;
                }
                redirects += 1;
                continue;
            }
            let headers = response
                .headers()
                .iter()
                .filter_map(|(k, v)| Some((k.as_str().to_owned(), v.to_str().ok()?.to_owned())))
                .collect();
            let len = response.content_length();
            let mut body_stream = response
                .bytes_stream()
                .map_err(|e| ClientError::from_reqwest(&e))
                .boxed();
            let (body, truncated) = self.read_capped(len, &mut body_stream).await?;
            return Ok(RawResponse {
                url: current.to_string(),
                status: status.as_u16(),
                headers,
                body,
                truncated,
                took_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            });
        }
    }

    /// The 3xx statuses that carry a `Location` to follow (RFC 9110 §15.4).
    /// `304 Not Modified` (a conditional GET's answer) and the other 3xx are
    /// ordinary responses returned with their status (108 D1).
    fn is_redirect(status: reqwest::StatusCode) -> bool {
        matches!(
            status,
            reqwest::StatusCode::MOVED_PERMANENTLY
                | reqwest::StatusCode::FOUND
                | reqwest::StatusCode::SEE_OTHER
                | reqwest::StatusCode::TEMPORARY_REDIRECT
                | reqwest::StatusCode::PERMANENT_REDIRECT
        )
    }

    /// Refuse an oversize `Content-Length`, then read the body cut at `max_bytes`.
    async fn read_capped(
        &self,
        content_length: Option<u64>,
        body: &mut BoxStream<'static, Result<bytes::Bytes, ClientError>>,
    ) -> Result<(Vec<u8>, bool), FetchError> {
        if let Some(len) = content_length
            && usize::try_from(len).is_ok_and(|len| len > self.max_bytes)
        {
            return Err(FetchError::Failed(format!(
                "body too large (Content-Length {len} > max {})",
                self.max_bytes
            )));
        }
        let mut raw = Vec::new();
        let mut truncated = false;
        while let Some(chunk) = body.next().await {
            let chunk = chunk?;
            raw.extend_from_slice(&chunk);
            if raw.len() > self.max_bytes {
                raw.truncate(self.max_bytes);
                truncated = true;
                tracing::warn!(
                    max = self.max_bytes,
                    "fetch body exceeded max_fetch_bytes; truncating"
                );
                break;
            }
        }
        Ok((raw, truncated))
    }

    /// Inflate a gzip container (`sitemap.xml.gz`) capped post-inflate at
    /// `max_bytes` (D12: the cap sets `truncated`); a failed decode keeps the
    /// raw bytes.
    fn inflate(&self, raw: Vec<u8>, truncated: &mut bool) -> Vec<u8> {
        if !raw.starts_with(&[0x1f, 0x8b]) {
            return raw;
        }
        let mut inflated = Vec::new();
        let cap = u64::try_from(self.max_bytes).unwrap_or(u64::MAX);
        if flate2::read::GzDecoder::new(raw.as_slice())
            .take(cap.saturating_add(1))
            .read_to_end(&mut inflated)
            .is_err()
        {
            return raw;
        }
        if inflated.len() > self.max_bytes {
            inflated.truncate(self.max_bytes);
            *truncated = true;
            tracing::warn!(
                max = self.max_bytes,
                "inflated body exceeded max_fetch_bytes; truncating"
            );
        }
        inflated
    }

    /// Decode, convert and diagnose a settled hop. A 2xx document body goes
    /// through anydoc; a 2xx `is_binary` body is refused (`Unsupported` is
    /// the `Err`, 122 D1 row 17 as today's shape). A non-2xx body is read and
    /// diagnosed: with an issue it is `Ok` (the model gets the `ISSUE:` line
    /// and whatever prose the wall carried), without one it stays
    /// `Err(Failed("HTTP n"))`.
    async fn read_body(
        &self,
        hop: Hop,
        format: Option<FetchFormat>,
        opts: &PageOptions,
        started: Instant,
    ) -> Result<FetchResponse, FetchError> {
        let ok = (200..300).contains(&hop.status);
        let ct_full = hop
            .headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let content_type = primary_type(&hop.headers);
        let ct = content_type.as_deref();
        let mut body_truncated = hop.truncated;
        let raw = self.inflate(hop.raw, &mut body_truncated);
        let took = || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        if ok {
            // Image bytes under a non-image label (octet-stream, text/plain):
            // refused as the image they are, never decoded as text.
            if let Some(f) = tools::images::sniff(&raw) {
                return Err(unsupported(Some(f.to_mime_type())));
            }
            // A document body (PDF, Office, …) is rendered with anydoc (20 §2.3);
            // a truncated container is unparseable, so it is refused, not half-read.
            // The sniff runs before `is_binary` for octet-stream/zip (§6.1 w2).
            if let Some(fmt) = tools::doc::format(&raw) {
                if body_truncated {
                    return Err(FetchError::Failed(format!(
                        "{fmt:?} larger than max_fetch_bytes ({}); not converted",
                        self.max_bytes
                    )));
                }
                let md = tools::doc::to_markdown(raw, fmt)
                    .await
                    .map_err(|e| FetchError::Failed(format!("{fmt:?}: {e}")))?;
                let title = text::first_heading(&md);
                let w = window(&md, &md, format, opts);
                return Ok(FetchResponse {
                    url: hop.url.to_string(),
                    title,
                    content: w.content,
                    truncated: w.truncated,
                    took_ms: took(),
                    compacted_chars: w.compacted_chars,
                    next_offset: w.next_offset,
                    omitted_headings: w.omitted_headings,
                    ..FetchResponse::default()
                });
            }
            if is_binary(ct) {
                return Err(unsupported(ct));
            }
        }
        let (body, charset) = charset::decode(&raw, ct_full.as_deref());
        let is_html = is_html_type(ct) || (!is_plain_type(ct) && looks_like_html(&body));
        // A raw fetch of an XML body (sitemaps up to 8 MiB) is not converted
        // for diagnosis: it is not a page.
        let diagnose = format.is_some() || !matches!(ct, Some("application/xml" | "text/xml"));
        let (title, markdown, meta) = if is_html && diagnose {
            let md = text::html_to_markdown(&body);
            let title = text::html_title(&body).or_else(|| text::first_heading(&md));
            (title, md, meta::extract(&body, &hop.url))
        } else if is_html {
            (
                text::html_title(&body),
                String::new(),
                meta::PageMeta::default(),
            )
        } else {
            (None, body.trim().to_owned(), meta::PageMeta::default())
        };
        let issue = diagnose
            .then(|| {
                issue::detect(
                    hop.status,
                    &hop.headers,
                    &hop.url,
                    &body,
                    crate::excerpt::prose_chars(&markdown),
                )
            })
            .flatten();
        if !ok && issue.is_none() {
            return Err(FetchError::Failed(format!("HTTP {}", hop.status)));
        }
        let raw_text: &str = if is_html { &body } else { &markdown };
        let w = window(raw_text, &markdown, format, opts);
        Ok(FetchResponse {
            url: hop.url.to_string(),
            title,
            content: w.content,
            truncated: w.truncated || body_truncated,
            took_ms: took(),
            issue,
            meta,
            charset: charset.to_owned(),
            compacted_chars: w.compacted_chars,
            next_offset: w.next_offset,
            omitted_headings: w.omitted_headings,
            ..FetchResponse::default()
        })
    }
}

/// The content window of a response.
struct Window {
    content: String,
    truncated: bool,
    compacted_chars: usize,
    next_offset: Option<usize>,
    omitted_headings: Vec<String>,
}

/// `format: None` (raw body for scrape/crawl and API sources) is a hard cut
/// of `raw` with no marker; a format runs [`finish`] on `markdown`.
fn window(raw: &str, markdown: &str, format: Option<FetchFormat>, opts: &PageOptions) -> Window {
    match format {
        None => {
            let (content, truncated) = hard_truncate(raw, opts.max_chars);
            Window {
                content,
                truncated,
                compacted_chars: 0,
                next_offset: None,
                omitted_headings: Vec::new(),
            }
        }
        Some(f) => {
            let fin = finish(markdown, f, opts);
            Window {
                content: fin.content,
                truncated: fin.cut.truncated,
                compacted_chars: fin.compacted_chars,
                next_offset: fin.cut.next_offset,
                omitted_headings: fin.cut.omitted_headings,
            }
        }
    }
}

/// Lowercased primary media type (no parameters) of `Content-Type`, or `None` when absent.
fn primary_type(headers: &reqwest::header::HeaderMap) -> Option<String> {
    headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| {
            ct.split(';')
                .next()
                .unwrap_or(ct)
                .trim()
                .to_ascii_lowercase()
        })
}

/// Binary types whose bytes may still be a document container (§6.1 w2).
fn sniffable(ct: Option<&str>) -> bool {
    matches!(ct, Some("application/octet-stream" | "application/zip"))
}

fn unsupported(ct: Option<&str>) -> FetchError {
    FetchError::Failed(format!(
        "unsupported content type: {}",
        ct.unwrap_or("unknown")
    ))
}

/// `ct` is the lowercased primary media type (no parameters), or `None` when absent.
fn is_html_type(ct: Option<&str>) -> bool {
    // Missing Content-Type: assume HTML (common for simple servers).
    ct.is_none_or(|ct| {
        matches!(
            ct,
            "text/html" | "application/xhtml+xml" | "application/xml" | "text/xml"
        )
    })
}

fn is_plain_type(ct: Option<&str>) -> bool {
    ct.is_some_and(|ct| {
        matches!(
            ct,
            "text/plain" | "text/markdown" | "text/x-markdown" | "text/csv"
        )
    })
}

pub(crate) fn is_binary(ct: Option<&str>) -> bool {
    let Some(ct) = ct else { return false };
    if ct.starts_with("text/")
        || ct.contains("json")
        || ct.contains("xml")
        || ct.contains("javascript")
    {
        return false;
    }
    ["image/", "audio/", "video/", "font/"]
        .iter()
        .any(|p| ct.starts_with(p))
        || matches!(ct, "application/octet-stream" | "application/zip")
}

fn looks_like_html(body: &str) -> bool {
    let head: String = body
        .chars()
        .take(256)
        .collect::<String>()
        .to_ascii_lowercase();
    head.contains("<html") || head.contains("<!doctype html") || head.contains("<body")
}
