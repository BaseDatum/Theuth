//! Page fetch (design doc 16 §5): HTTP GET behind the SSRF guard, manual
//! redirects re-checked per hop, body caps, and HTML → markdown / text.
//!
//! - `ssrf`: the mandatory guard; [`check_url`] for callers outside the fetcher.
//! - `text`: one HTML parser for both [`FetchFormat`]s, plus truncation.

mod ssrf;
mod text;

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

/// A fetched page (the `structured` half of `web_fetch`).
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
        let client = reqwest::Client::builder()
            .user_agent(user_agent)
            .timeout(TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
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

    async fn read_body(
        &self,
        mut response: reqwest::Response,
        url: Url,
        format: FetchFormat,
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
        if let Some(len) = response.content_length()
            && usize::try_from(len).is_ok_and(|len| len > self.max_bytes)
        {
            return Err(FetchError::Failed(format!(
                "body too large (Content-Length {len} > max {})",
                self.max_bytes
            )));
        }
        let mut raw = Vec::new();
        let mut body_truncated = false;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| FetchError::from_reqwest(&e))?
        {
            raw.extend_from_slice(&chunk);
            if raw.len() > self.max_bytes {
                raw.truncate(self.max_bytes);
                body_truncated = true;
                tracing::warn!(
                    max = self.max_bytes,
                    "fetch body exceeded max_fetch_bytes; truncating"
                );
                break;
            }
        }
        let body = String::from_utf8_lossy(&raw);
        let is_html = is_html_type(ct) || (!is_plain_type(ct) && looks_like_html(&body));
        let (title, content) = if is_html {
            let md = text::html_to_markdown(&body);
            let title = text::html_title(&body).or_else(|| text::first_heading(&md));
            let content = match format {
                FetchFormat::Markdown => md,
                FetchFormat::Text => text::markdown_to_text(&md),
            };
            (title, content)
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
        || matches!(
            ct,
            "application/octet-stream" | "application/pdf" | "application/zip" | "application/gzip"
        )
}

fn looks_like_html(body: &str) -> bool {
    let head: String = body
        .chars()
        .take(256)
        .collect::<String>()
        .to_ascii_lowercase();
    head.contains("<html") || head.contains("<!doctype html") || head.contains("<body")
}
