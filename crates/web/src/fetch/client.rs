//! The HTTP client seam under [`Fetcher`](super::Fetcher) (design doc 122
//! revision 1): one trait, two kinds. [`ClientKind::Reqwest`] is today's
//! client — redirects off, the SSRF-guarded resolver, the same timeouts, a
//! streamed body. [`ClientKind::WreqChrome`] (a Chrome TLS/HTTP fingerprint
//! through `wreq`) is W5's; `build` says [`ClientError::NotBuilt`] until then.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::stream::{BoxStream, StreamExt, TryStreamExt};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

use super::FetchError;
use super::ssrf::GuardedResolver;

/// Which client answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    /// Plain `reqwest` (rustls), theuth's own identity.
    Reqwest,
    /// `wreq` with a Chrome emulation profile (TLS + HTTP/2 fingerprint).
    WreqChrome,
}

impl Default for ClientKind {
    /// Today's client.
    fn default() -> Self {
        Self::Reqwest
    }
}

/// When page fetches use the stealth client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Impersonate {
    /// Never: `reqwest` only.
    Never,
    /// `reqwest` first; the stealth client after a diagnosed challenge.
    OnChallenge,
    /// Stealth client on every page fetch (default).
    #[default]
    Always,
}

/// What every client is built from.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// `User-Agent` of the `Reqwest` kind (the `WreqChrome` kind sends its profile's).
    pub user_agent: String,
    /// `Accept-Language` value; carried for W1/W5 (the `Reqwest` kind does not send it yet).
    pub accept_language: String,
    /// TCP connect timeout.
    pub connect_timeout: Duration,
    /// Whole-request timeout (what `Fetcher::new_with_timeouts` called `timeout`).
    pub read_timeout: Duration,
    /// Body cap the caller enforces while draining `body`; carried for clients that can pre-cap.
    pub max_body: usize,
}

/// Why a client could not be built or a request did not complete.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The kind has no implementation yet.
    #[error("stealth client not built")]
    NotBuilt,
    /// Connect, TLS, read or guard failure; a guard refusal uses [`ClientError::rejected`].
    #[error("{0}")]
    Transport(String),
    /// The client could not be constructed.
    #[error("{0}")]
    Build(String),
}

/// Prefix of a [`ClientError::Transport`] that carries an SSRF-guard refusal
/// (the `Display` of [`FetchError::Rejected`]).
const REJECTED: &str = "rejected: ";

impl ClientError {
    /// A guard refusal from inside the client (the resolver said no): a
    /// `Transport` the `Fetcher` maps back to [`FetchError::Rejected`].
    /// Every `HttpClient` MUST use this for guard refusals.
    #[must_use]
    pub fn rejected(detail: &str) -> Self {
        Self::Transport(format!("{REJECTED}{detail}"))
    }

    /// The guard refusal detail when this is one.
    #[must_use]
    pub fn as_rejected(&self) -> Option<&str> {
        match self {
            Self::Transport(s) => s.strip_prefix(REJECTED),
            Self::NotBuilt | Self::Build(_) => None,
        }
    }

    /// Map a `reqwest` error, surfacing a guard refusal from the resolver.
    pub(super) fn from_reqwest(e: &reqwest::Error) -> Self {
        let mut source = std::error::Error::source(e);
        while let Some(s) = source {
            if let Some(FetchError::Rejected(m)) = s.downcast_ref::<FetchError>() {
                return Self::rejected(m);
            }
            source = s.source();
        }
        Self::Transport(format!("HTTP request failed: {e}"))
    }
}

/// One hop's reply; `body` streams until the caller stops draining it.
pub struct ClientResponse {
    /// HTTP status.
    pub status: u16,
    /// Response headers.
    pub headers: HeaderMap,
    /// The URL that answered (no redirects were followed, so the request URL).
    pub final_url: Url,
    /// Body chunks.
    pub body: BoxStream<'static, Result<Bytes, ClientError>>,
}

/// One GET, no redirects, under the SSRF guard.
#[async_trait::async_trait]
pub trait HttpClient: Send + Sync {
    /// Which kind this is.
    fn kind(&self) -> ClientKind;
    /// One request, redirects NOT followed (the Fetcher's loop re-checks SSRF per hop), extra headers appended after the client's defaults.
    async fn get(
        &self,
        url: &Url,
        headers: &[(HeaderName, HeaderValue)],
    ) -> Result<ClientResponse, ClientError>;
}

/// Build a client of `kind`.
///
/// # Errors
/// [`ClientError::Build`] when `reqwest` refuses the configuration;
/// [`ClientError::NotBuilt`] for [`ClientKind::WreqChrome`] until W5 lands it.
pub fn build(kind: ClientKind, cfg: &ClientConfig) -> Result<Arc<dyn HttpClient>, ClientError> {
    match kind {
        ClientKind::Reqwest => Ok(Arc::new(ReqwestClient::new(cfg)?)),
        ClientKind::WreqChrome => Err(ClientError::NotBuilt),
    }
}

/// Today's `reqwest` client: `redirect::Policy::none()`, [`GuardedResolver`],
/// `connect_timeout` / whole-request `read_timeout`, `user_agent`.
pub(super) fn reqwest_client(cfg: &ClientConfig) -> Result<reqwest::Client, ClientError> {
    reqwest::Client::builder()
        .user_agent(&cfg.user_agent)
        .timeout(cfg.read_timeout)
        .connect_timeout(cfg.connect_timeout)
        .redirect(reqwest::redirect::Policy::none())
        .dns_resolver(GuardedResolver)
        .build()
        .map_err(|e| ClientError::Build(e.to_string()))
}

/// [`ClientKind::Reqwest`].
pub struct ReqwestClient {
    client: reqwest::Client,
}

impl ReqwestClient {
    /// Build from `cfg` (see [`reqwest_client`]).
    ///
    /// # Errors
    /// [`ClientError::Build`] when `reqwest` refuses the configuration.
    pub fn new(cfg: &ClientConfig) -> Result<Self, ClientError> {
        Ok(Self {
            client: reqwest_client(cfg)?,
        })
    }
}

#[async_trait::async_trait]
impl HttpClient for ReqwestClient {
    fn kind(&self) -> ClientKind {
        ClientKind::Reqwest
    }

    async fn get(
        &self,
        url: &Url,
        headers: &[(HeaderName, HeaderValue)],
    ) -> Result<ClientResponse, ClientError> {
        let mut req = self.client.get(url.as_str());
        for (name, value) in headers {
            req = req.header(name.clone(), value.clone());
        }
        let response = req
            .send()
            .await
            .map_err(|e| ClientError::from_reqwest(&e))?;
        Ok(ClientResponse {
            status: response.status().as_u16(),
            headers: response.headers().clone(),
            final_url: url.clone(),
            body: response
                .bytes_stream()
                .map_err(|e| ClientError::from_reqwest(&e))
                .boxed(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_round_trips_and_wreq_is_not_built() {
        let e = ClientError::rejected("private address 10.0.0.1");
        assert_eq!(e.as_rejected(), Some("private address 10.0.0.1"));
        assert_eq!(e.to_string(), "rejected: private address 10.0.0.1");
        assert_eq!(ClientError::Transport("reset".into()).as_rejected(), None);
        let cfg = ClientConfig {
            user_agent: "t".into(),
            accept_language: "en".into(),
            connect_timeout: Duration::from_secs(1),
            read_timeout: Duration::from_secs(1),
            max_body: 1,
        };
        assert!(matches!(
            build(ClientKind::WreqChrome, &cfg),
            Err(ClientError::NotBuilt)
        ));
        assert_eq!(
            build(ClientKind::Reqwest, &cfg).unwrap().kind(),
            ClientKind::Reqwest
        );
        assert_eq!(
            serde_json::to_value(ClientKind::WreqChrome).unwrap(),
            serde_json::json!("wreq_chrome")
        );
        assert_eq!(Impersonate::default(), Impersonate::Always);
    }
}
