//! Page fetch (design doc 16 §5): HTTP GET behind the SSRF guard, manual
//! redirects re-checked per hop, body caps, and HTML → markdown / text; plus
//! the raw [`Fetcher::request`] used for route replay (44 §0.6).
//!
//! - `ssrf`: the mandatory guard; [`check_url`] for callers outside the fetcher.
//! - `text`: one HTML parser for both [`FetchFormat`]s, plus truncation.

mod ssrf;
mod text;

use std::io::Read;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

pub use ssrf::{check_url, is_blocked_hostname, is_blocked_ip};
pub use text::{collapse_ws, hard_truncate};

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

/// A fetched page (the `structured` half of `read_url`).
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

impl FetchError {
    /// Map a transport error, surfacing a guard refusal from the DNS
    /// resolver as [`FetchError::Rejected`].
    fn from_reqwest(e: &reqwest::Error) -> Self {
        let mut source = std::error::Error::source(e);
        while let Some(s) = source {
            if let Some(f) = s.downcast_ref::<Self>() {
                return f.clone();
            }
            source = s.source();
        }
        Self::Failed(format!("HTTP request failed: {e}"))
    }
}

/// HTTP page fetcher behind the SSRF guard.
///
/// Redirects are disabled on the client and followed by hand so every
/// `Location` re-runs the guard; DNS goes through the guarded resolver in
/// `ssrf`, so the client connects only to addresses that passed
/// [`is_blocked_ip`] (closes the resolve/connect TOCTOU of 16 §10).
pub struct Fetcher {
    client: reqwest::Client,
    max_bytes: usize,
    max_redirects: usize,
    allow: Vec<SocketAddr>,
}

impl Fetcher {
    /// Build a fetcher: `redirect::Policy::none()`, 8 s total / 2 s connect,
    /// `user_agent`, raw body capped at `max_bytes`, at most `max_redirects` hops.
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

    /// Like [`Self::new`] with explicit whole-request and connect timeouts.
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
        let client = reqwest::Client::builder()
            .user_agent(user_agent)
            .timeout(timeout)
            .connect_timeout(connect_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .dns_resolver(ssrf::GuardedResolver)
            .build()?;
        Ok(Self {
            client,
            max_bytes,
            max_redirects,
            allow: Vec::new(),
        })
    }

    /// Test hook: let these exact blocked `ip:port` literals through the guard
    /// (loopback wiremock). Not reachable from config.
    #[doc(hidden)]
    #[must_use]
    pub fn allowing(mut self, addrs: Vec<SocketAddr>) -> Self {
        self.allow = addrs;
        self
    }

    /// GET `url`, follow redirects under the guard, and return the body as
    /// `format`, cut to `max_chars` scalars.
    ///
    /// # Errors
    /// [`FetchError::Rejected`] from the guard or too many redirects;
    /// [`FetchError::Failed`] for transport errors, non-2xx, binary content
    /// or an oversize `Content-Length`.
    pub async fn fetch(
        &self,
        url: &str,
        format: FetchFormat,
        max_chars: usize,
    ) -> Result<FetchResponse, FetchError> {
        self.fetch_kind(url, Some(format), max_chars).await
    }

    /// GET `url` and return the decoded body without HTML→markdown conversion.
    /// Tool-sized truncation is the caller's `max_chars`; pass a large value
    /// for crawl/scrape.
    ///
    /// # Errors
    /// Same as [`Self::fetch`].
    pub async fn fetch_html(
        &self,
        url: &str,
        max_chars: usize,
    ) -> Result<FetchResponse, FetchError> {
        self.fetch_kind(url, None, max_chars).await
    }

    async fn fetch_kind(
        &self,
        url: &str,
        format: Option<FetchFormat>,
        max_chars: usize,
    ) -> Result<FetchResponse, FetchError> {
        let started = Instant::now();
        let mut current = ssrf::parse_absolute(url)?;
        let mut redirects = 0usize;
        loop {
            ssrf::check_static(&current, &self.allow)?;
            tracing::debug!(url = %current, redirects, "fetch hop");
            let response = self
                .client
                .get(current.as_str())
                .send()
                .await
                .map_err(|e| FetchError::from_reqwest(&e))?;
            let status = response.status();
            if status.is_redirection() {
                if redirects >= self.max_redirects {
                    return Err(FetchError::Rejected(format!(
                        "too many redirects (max {})",
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
                redirects += 1;
                continue;
            }
            if !status.is_success() {
                return Err(FetchError::Failed(format!("HTTP {}", status.as_u16())));
            }
            return self
                .read_body(response, current, format, max_chars, started)
                .await;
        }
    }

    /// `method` `url` with `headers` and optional `body`, redirects re-checked
    /// per hop like [`Self::fetch`] (44 §0.6). 301/302/303 turn into a bodyless
    /// GET; 307/308 keep method and body. Caller `host` / `content-length`
    /// headers are skipped. The body is raw bytes: no gzip inflate, no
    /// document or HTML conversion.
    ///
    /// # Errors
    /// [`FetchError::Rejected`] for an invalid method, the guard or too many
    /// redirects; [`FetchError::Failed`] for transport errors or an oversize
    /// `Content-Length`. Non-2xx is `Ok`.
    pub async fn request(
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
        let mut redirects = 0usize;
        loop {
            ssrf::check_static(&current, &self.allow)?;
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
            if status.is_redirection() {
                if redirects >= self.max_redirects {
                    return Err(FetchError::Rejected(format!(
                        "too many redirects (max {})",
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
            let (body, truncated) = self.read_capped(response).await?;
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

    /// Refuse an oversize `Content-Length`, then read the body cut at `max_bytes`.
    async fn read_capped(
        &self,
        mut response: reqwest::Response,
    ) -> Result<(Vec<u8>, bool), FetchError> {
        if let Some(len) = response.content_length()
            && usize::try_from(len).is_ok_and(|len| len > self.max_bytes)
        {
            return Err(FetchError::Failed(format!(
                "body too large (Content-Length {len} > max {})",
                self.max_bytes
            )));
        }
        let mut raw = Vec::new();
        let mut truncated = false;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| FetchError::from_reqwest(&e))?
        {
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

    async fn read_body(
        &self,
        response: reqwest::Response,
        url: Url,
        format: Option<FetchFormat>,
        max_chars: usize,
        started: Instant,
    ) -> Result<FetchResponse, FetchError> {
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|ct| {
                ct.split(';')
                    .next()
                    .unwrap_or(ct)
                    .trim()
                    .to_ascii_lowercase()
            });
        let ct = content_type.as_deref();
        if is_binary(ct) {
            return Err(FetchError::Failed(format!(
                "unsupported content type: {}",
                ct.unwrap_or("unknown")
            )));
        }
        let (mut raw, body_truncated) = self.read_capped(response).await?;
        // Gzip container (`sitemap.xml.gz`); a failed decode keeps the raw bytes.
        if raw.starts_with(&[0x1f, 0x8b]) {
            let mut inflated = Vec::new();
            if flate2::read::GzDecoder::new(raw.as_slice())
                .take(u64::try_from(self.max_bytes).unwrap_or(u64::MAX))
                .read_to_end(&mut inflated)
                .is_ok()
            {
                raw = inflated;
            }
        }
        // A document body (PDF, Office, …) is rendered with anydoc (20 §2.3);
        // a truncated container is unparseable, so it is refused, not half-read.
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
            let (content, cut) = hard_truncate(&md, max_chars);
            return Ok(FetchResponse {
                url: url.to_string(),
                title: text::first_heading(&md),
                content,
                truncated: cut,
                took_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            });
        }
        let body = String::from_utf8_lossy(&raw);
        let is_html = is_html_type(ct) || (!is_plain_type(ct) && looks_like_html(&body));
        let (title, content) = if is_html {
            match format {
                None => (text::html_title(&body), body.to_string()),
                Some(FetchFormat::Markdown) => {
                    let md = text::html_to_markdown(&body);
                    let title = text::html_title(&body).or_else(|| text::first_heading(&md));
                    (title, md)
                }
                Some(FetchFormat::Text) => {
                    let md = text::html_to_markdown(&body);
                    let title = text::html_title(&body).or_else(|| text::first_heading(&md));
                    (title, text::markdown_to_text(&md))
                }
            }
        } else {
            (None, body.trim().to_owned())
        };
        let (content, cut) = hard_truncate(&content, max_chars);
        Ok(FetchResponse {
            url: url.to_string(),
            title,
            content,
            truncated: cut || body_truncated,
            took_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        })
    }
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

fn is_binary(ct: Option<&str>) -> bool {
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
