//! OAuth login plumbing shared by the agent (provider logins) and the tools
//! plugin (MCP server logins); design doc 05.
//!
//! - [`callback`]: one-shot loopback redirect listener.
//! - [`pkce`]: S256 verifier/challenge + CSRF state.
//! - [`Provider`] / [`PROVIDERS`]: authorization-code providers as data.
//! - [`Store`]: `<data_dir>/oauth/<ns>.json` token files, login and refresh.
//!
//! No UI, no genai, no rmcp: callers map [`Tokens`] onto their own request
//! types.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// How long a started login waits for the browser.
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
/// Refresh when this close to expiry.
const REFRESH_MARGIN_SECS: u64 = 60;

/// Bind `127.0.0.1:port` (0 = ephemeral), serve exactly one redirect, reply
/// with a "you can close this tab" page. Returns the bound port and a future
/// resolving to the full redirect URL (`http://127.0.0.1:<port>/path?code=…`).
/// Requests without a query string (favicon probes) are answered 404 and
/// ignored. The caller owns the deadline.
pub async fn callback(
    port: u16,
) -> std::io::Result<(
    u16,
    impl Future<Output = Result<String, String>> + Send + 'static,
)> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    let port = listener.local_addr()?.port();
    Ok((port, async move {
        loop {
            let (mut sock, _) = listener.accept().await.map_err(|e| e.to_string())?;
            let mut head = Vec::new();
            let mut buf = [0u8; 1024];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 16 * 1024 {
                match sock.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => head.extend_from_slice(&buf[..n]),
                }
            }
            let head = String::from_utf8_lossy(&head);
            let target = head.split_whitespace().nth(1).unwrap_or("/").to_owned();
            let (status, page) = if !target.contains('?') {
                ("404 Not Found", Cow::Borrowed(""))
            } else if target.contains("error=") {
                ("400 Bad Request", Cow::Owned(PAGE.replace("{state}", "err")))
            } else {
                ("200 OK", Cow::Owned(PAGE.replace("{state}", "ok")))
            };
            let resp = format!(
                "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
                page.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.shutdown().await;
            if target.contains('?') {
                return Ok(format!("http://127.0.0.1:{port}{target}"));
            }
        }
    }))
}

/// Completion page served on the loopback redirect; `{state}` is `ok` or `err`.
const PAGE: &str = include_str!("callback.html");

/// Open `url` in the user's browser; best effort, never fails the login.
pub fn open_browser(url: &str) {
    let _ = platform::open::open_url(url);
}

/// PKCE S256 pair and CSRF state, 32 random bytes each, base64url no pad.
pub struct Pkce {
    /// `code_verifier`.
    pub verifier: String,
    /// `code_challenge`.
    pub challenge: String,
    /// `state`.
    pub state: String,
}

/// Fresh [`Pkce`].
pub fn pkce() -> Pkce {
    let verifier = random();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    Pkce {
        verifier,
        challenge,
        state: random(),
    }
}

/// 32 random bytes (two v4 UUIDs) as base64url.
fn random() -> String {
    let mut b = [0u8; 32];
    b[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    b[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    URL_SAFE_NO_PAD.encode(b)
}

/// How the token endpoint wants its body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Body {
    /// `application/x-www-form-urlencoded` (RFC 6749 default).
    Form,
    /// JSON object with the same fields (Anthropic).
    Json,
}

/// How a provider hands out tokens.
#[derive(Clone, Debug)]
pub enum Flow {
    /// Authorization code + PKCE through a loopback redirect; refresh with
    /// `grant_type=refresh_token` at `token_url`.
    Code {
        /// Exchange and refresh body encoding.
        token_body: Body,
        /// Extra headers on the refresh request only (Anthropic wants the
        /// Claude Code ones there but not on the exchange).
        refresh_headers: &'static [(&'static str, &'static str)],
    },
    /// Cursor: `auth_url` is a deep link carrying the PKCE challenge and a
    /// uuid; `token_url` is polled with `uuid` + `verifier` until the browser
    /// finishes (404 = pending). Refresh: `POST refresh_url` with the refresh
    /// token as bearer and `{}` as body.
    Poll {
        /// Token exchange endpoint for the refresh token.
        refresh_url: &'static str,
    },
    /// RFC 8628 device authorization: `POST auth_url` (device endpoint) gives a
    /// user code and a verification URL; `token_url` is polled with the device
    /// code until the browser approves. Refresh: `grant_type=refresh_token`
    /// form at `token_url`.
    Device,
}

/// One login provider. Data, not code; `Cow` so the table is `const` and
/// tests can point at a loopback endpoint.
#[derive(Clone, Debug)]
pub struct Provider {
    /// genai namespace; also the token file stem.
    pub ns: Cow<'static, str>,
    /// Authorization endpoint (deep link for [`Flow::Poll`], device endpoint for [`Flow::Device`]).
    pub auth_url: Cow<'static, str>,
    /// Token endpoint (code exchange + refresh), or the poll endpoint.
    pub token_url: Cow<'static, str>,
    /// Public client id (empty for [`Flow::Poll`]).
    pub client_id: Cow<'static, str>,
    /// Space-separated scopes.
    pub scopes: Cow<'static, str>,
    /// Loopback port the client id registered; 0 for any.
    pub port: u16,
    /// Host in the registered redirect URI (`localhost` or `127.0.0.1`).
    pub redirect_host: Cow<'static, str>,
    /// Path in the registered redirect URI.
    pub redirect_path: Cow<'static, str>,
    /// Provider-specific authorize query parameters.
    pub extra_query: &'static [(&'static str, &'static str)],
    /// Grant shape.
    pub flow: Flow,
}

/// ChatGPT subscription via the Codex CLI public client. Responses are served
/// by `https://chatgpt.com/backend-api/codex/responses` (see design doc 05 §2.3).
pub const OPENAI: Provider = Provider {
    ns: Cow::Borrowed("openai"),
    auth_url: Cow::Borrowed("https://auth.openai.com/oauth/authorize"),
    token_url: Cow::Borrowed("https://auth.openai.com/oauth/token"),
    client_id: Cow::Borrowed("app_EMoamEEZ73f0CkXaXp7hrann"),
    scopes: Cow::Borrowed("openid profile email offline_access"),
    port: 1455,
    redirect_host: Cow::Borrowed("localhost"),
    redirect_path: Cow::Borrowed("/auth/callback"),
    extra_query: &[
        ("id_token_add_organizations", "true"),
        ("codex_cli_simplified_flow", "true"),
        ("originator", "codex_cli_rs"),
    ],
    flow: Flow::Code {
        token_body: Body::Form,
        refresh_headers: &[],
    },
};

/// Claude Pro/Max subscription via the Claude Code public client. Tokens are
/// `sk-ant-oat…` bearers for the plain Messages API with the Claude Code
/// betas (design doc 05 §2.3). The grant family dies ~30 days after the
/// interactive login regardless of refreshes (`invalid_grant`): re-login.
pub const ANTHROPIC: Provider = Provider {
    ns: Cow::Borrowed("anthropic"),
    // platform.claude.com issues console tokens without `user:inference`;
    // only the claude.ai endpoint grants a subscription token.
    auth_url: Cow::Borrowed("https://claude.ai/oauth/authorize"),
    token_url: Cow::Borrowed("https://api.anthropic.com/v1/oauth/token"),
    client_id: Cow::Borrowed("9d1c250a-e61b-44d9-88ed-5944d1962f5e"),
    scopes: Cow::Borrowed(
        "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload",
    ),
    port: 54545,
    redirect_host: Cow::Borrowed("localhost"),
    redirect_path: Cow::Borrowed("/callback"),
    extra_query: &[("code", "true")],
    flow: Flow::Code {
        token_body: Body::Json,
        refresh_headers: &[
            ("anthropic-beta", "oauth-2025-04-20"),
            (
                "User-Agent",
                "anthropic-sdk-typescript/0.112.1 userOAuthProvider",
            ),
        ],
    },
};

/// Cursor subscription. Not OAuth proper: the browser deep link completes
/// against Cursor's own backend and we poll for the JWT pair. Inference goes
/// to `api2.cursor.sh` through the `cursor` crate.
pub const CURSOR: Provider = Provider {
    ns: Cow::Borrowed("cursor"),
    auth_url: Cow::Borrowed("https://cursor.com/loginDeepControl"),
    token_url: Cow::Borrowed("https://api2.cursor.sh/auth/poll"),
    client_id: Cow::Borrowed(""),
    scopes: Cow::Borrowed(""),
    port: 0,
    redirect_host: Cow::Borrowed(""),
    redirect_path: Cow::Borrowed(""),
    extra_query: &[("mode", "login"), ("redirectTarget", "cli")],
    flow: Flow::Poll {
        refresh_url: "https://api2.cursor.sh/auth/exchange_user_api_key",
    },
};

/// SuperGrok / X Premium+ via the Grok CLI public client (device grant; xAI
/// publishes no loopback client). The access token is a plain bearer on
/// `https://api.x.ai/v1`, so genai's `xai` adapter is used unchanged.
pub const XAI: Provider = Provider {
    ns: Cow::Borrowed("xai"),
    auth_url: Cow::Borrowed("https://auth.x.ai/oauth2/device/code"),
    token_url: Cow::Borrowed("https://auth.x.ai/oauth2/token"),
    client_id: Cow::Borrowed("b1a00492-073a-47ea-816f-4c329264a828"),
    scopes: Cow::Borrowed("openid profile email offline_access grok-cli:access api:access"),
    port: 0,
    redirect_host: Cow::Borrowed(""),
    redirect_path: Cow::Borrowed(""),
    extra_query: &[],
    flow: Flow::Device,
};

/// Every provider a user can `/login`.
pub const PROVIDERS: &[Provider] = &[OPENAI, ANTHROPIC, CURSOR, XAI];

/// Provider by genai namespace.
pub fn provider(ns: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.ns == ns)
}

/// What a login leaves on disk.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Tokens {
    /// Bearer token.
    pub access_token: String,
    /// For [`Store::access`]; `None` means re-login on expiry.
    pub refresh_token: Option<String>,
    /// Unix seconds; JWT `exp` when the access token is one, else `now + expires_in`.
    pub expires_at: u64,
    /// Account the token belongs to (OpenAI: `chatgpt_account_id` claim;
    /// Anthropic: `account.uuid`; Cursor: JWT `sub` after `auth0|`).
    pub account_id: Option<String>,
}

impl fmt::Debug for Tokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tokens")
            .field("expires_at", &self.expires_at)
            .field("account_id", &self.account_id)
            .finish_non_exhaustive()
    }
}

/// RFC 8628 §3.2 device authorization response.
#[derive(Deserialize)]
struct DeviceCode {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    #[serde(default = "five")]
    interval: u64,
}

fn five() -> u64 {
    5
}

/// Token endpoint response: RFC 6749 §5.1 + `id_token`, Anthropic's
/// `account`, Cursor's camelCase pair.
#[derive(Deserialize)]
struct TokenResponse {
    #[serde(alias = "accessToken")]
    access_token: String,
    #[serde(alias = "refreshToken")]
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<u64>,
    account: Option<Account>,
}

#[derive(Deserialize)]
struct Account {
    uuid: Option<String>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// JWT payload as JSON; no signature check (we only read our own token).
fn claims(jwt: &str) -> Option<serde_json::Value> {
    let payload = jwt.split('.').nth(1)?;
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()
}

/// OpenAI `chatgpt_account_id` (top level or under `https://api.openai.com/auth`),
/// else Cursor's `auth0|<user>` subject.
fn account_id(jwt: &str) -> Option<String> {
    let c = claims(jwt)?;
    c.get("https://api.openai.com/auth")
        .and_then(|a| a.get("chatgpt_account_id"))
        .or_else(|| c.get("chatgpt_account_id"))
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .or_else(|| {
            let sub = c.get("sub")?.as_str()?;
            Some(sub.rsplit_once('|').map_or(sub, |(_, u)| u).to_owned())
        })
}

impl Tokens {
    /// From a token response; `prev` supplies what a refresh may omit.
    fn from_response(r: TokenResponse, prev: Option<&Tokens>) -> Tokens {
        let exp = claims(&r.access_token)
            .and_then(|c| c.get("exp")?.as_u64())
            .unwrap_or_else(|| now() + r.expires_in.unwrap_or(3600));
        Tokens {
            account_id: r
                .account
                .and_then(|a| a.uuid)
                .or_else(|| r.id_token.as_deref().and_then(account_id))
                .or_else(|| account_id(&r.access_token))
                .or_else(|| prev.and_then(|p| p.account_id.clone())),
            refresh_token: r
                .refresh_token
                .or_else(|| prev.and_then(|p| p.refresh_token.clone())),
            access_token: r.access_token,
            expires_at: exp,
        }
    }
}

/// `<data_dir>/oauth/`: one token file per provider namespace.
#[derive(Clone)]
pub struct Store {
    dir: PathBuf,
    http: reqwest::Client,
    /// One refresh at a time; concurrent model calls wait for it.
    refresh: Arc<tokio::sync::Mutex<()>>,
}

impl Store {
    /// Store under `<data_dir>/oauth`.
    pub fn new(data_dir: &Path) -> Store {
        Store {
            dir: data_dir.join("oauth"),
            http: reqwest::Client::new(),
            refresh: Arc::default(),
        }
    }

    /// Directory of MCP credential files (`<data_dir>/oauth/mcp`).
    pub fn mcp_dir(&self) -> PathBuf {
        self.dir.join("mcp")
    }

    fn path(&self, ns: &str) -> PathBuf {
        self.dir.join(format!("{ns}.json"))
    }

    /// Tokens on disk for `ns`; unreadable or invalid → warn + `None`.
    pub fn load(&self, ns: &str) -> Option<Tokens> {
        let path = self.path(ns);
        let text = std::fs::read_to_string(&path).ok()?;
        match serde_json::from_str(&text) {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::warn!("{}: {e}; ignoring", path.display());
                None
            }
        }
    }

    /// Write-then-rename, owner-only.
    pub fn save(&self, ns: &str, t: &Tokens) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        platform::files::write_private(&self.path(ns), serde_json::to_string_pretty(t)?.as_bytes())
    }

    /// Delete the token file; fine if absent.
    pub fn remove(&self, ns: &str) -> std::io::Result<()> {
        match std::fs::remove_file(self.path(ns)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// Start a login: the URL to open and a future that waits for the browser
    /// ([`LOGIN_TIMEOUT`]), obtains the tokens, saves and returns them.
    /// [`Flow::Code`] binds the callback port first (fails fast when taken).
    pub async fn login(
        &self,
        p: &Provider,
    ) -> Result<
        (
            String,
            Pin<Box<dyn Future<Output = Result<Tokens, String>> + Send + 'static>>,
        ),
        String,
    > {
        let k = pkce();
        let mut url = Url::parse(&p.auth_url).map_err(|e| e.to_string())?;
        let store = self.clone();
        let p = p.clone();
        match p.flow {
            Flow::Code { .. } => {
                let (port, back) = callback(p.port)
                    .await
                    .map_err(|e| format!("{}: cannot listen on 127.0.0.1:{}: {e}", p.ns, p.port))?;
                let redirect = format!("http://{}:{port}{}", p.redirect_host, p.redirect_path);
                url.query_pairs_mut()
                    .append_pair("response_type", "code")
                    .append_pair("client_id", &p.client_id)
                    .append_pair("redirect_uri", &redirect)
                    .append_pair("scope", &p.scopes)
                    .append_pair("code_challenge", &k.challenge)
                    .append_pair("code_challenge_method", "S256")
                    .append_pair("state", &k.state)
                    .extend_pairs(p.extra_query.iter().copied());
                let finish = async move {
                    let back = tokio::time::timeout(LOGIN_TIMEOUT, back)
                        .await
                        .map_err(|_| format!("{}: no browser callback within 5 minutes", p.ns))??;
                    let back = Url::parse(&back).map_err(|e| e.to_string())?;
                    let q: HashMap<String, String> = back.query_pairs().into_owned().collect();
                    if let Some(e) = q.get("error") {
                        let why = q.get("error_description").map(String::as_str).unwrap_or("");
                        return Err(format!("{}: {e} {why}", p.ns).trim_end().to_owned());
                    }
                    if q.get("state") != Some(&k.state) {
                        return Err(format!("{}: callback state mismatch", p.ns));
                    }
                    let code = q
                        .get("code")
                        .ok_or_else(|| format!("{}: callback without code", p.ns))?;
                    let form = [
                        ("grant_type", "authorization_code"),
                        ("code", code),
                        ("redirect_uri", &redirect),
                        ("client_id", &p.client_id),
                        ("code_verifier", &k.verifier),
                        // Anthropic requires it in the body; others ignore it.
                        ("state", &k.state),
                    ];
                    let t = store.token_request(&p, &form, &[], None).await?;
                    store
                        .save(&p.ns, &t)
                        .map_err(|e| format!("{}: saving tokens: {e}", p.ns))?;
                    Ok(t)
                };
                Ok((url.into(), Box::pin(finish)))
            }
            Flow::Poll { .. } => {
                let uuid = uuid::Uuid::new_v4().to_string();
                url.query_pairs_mut()
                    .append_pair("challenge", &k.challenge)
                    .append_pair("uuid", &uuid)
                    .extend_pairs(p.extra_query.iter().copied());
                let finish = async move {
                    let t = tokio::time::timeout(LOGIN_TIMEOUT, store.poll(&p, &uuid, &k.verifier))
                        .await
                        .map_err(|_| {
                            format!("{}: browser login not completed within 5 minutes", p.ns)
                        })??;
                    store
                        .save(&p.ns, &t)
                        .map_err(|e| format!("{}: saving tokens: {e}", p.ns))?;
                    Ok(t)
                };
                Ok((url.into(), Box::pin(finish)))
            }
            Flow::Device => {
                let d: DeviceCode = self
                    .http
                    .post(&*p.auth_url)
                    .header(reqwest::header::ACCEPT, "application/json")
                    .form(&[("client_id", &*p.client_id), ("scope", &*p.scopes)])
                    .send()
                    .await
                    .and_then(reqwest::Response::error_for_status)
                    .map_err(|e| format!("{}: device code: {e}", p.ns))?
                    .json()
                    .await
                    .map_err(|e| format!("{}: device code response: {e}", p.ns))?;
                // ponytail: without `verification_uri_complete` the user would
                // have to type `user_code`; `?user_code=` is what most servers
                // build `_complete` from. Surface the code separately if one
                // ever rejects it.
                let url = d
                    .verification_uri_complete
                    .unwrap_or_else(|| format!("{}?user_code={}", d.verification_uri, d.user_code));
                let interval = Duration::from_secs(d.interval.max(1));
                let finish = async move {
                    let t = tokio::time::timeout(
                        LOGIN_TIMEOUT,
                        store.poll_device(&p, &d.device_code, interval),
                    )
                    .await
                    .map_err(|_| {
                        format!("{}: browser login not completed within 5 minutes", p.ns)
                    })??;
                    store
                        .save(&p.ns, &t)
                        .map_err(|e| format!("{}: saving tokens: {e}", p.ns))?;
                    Ok(t)
                };
                Ok((url, Box::pin(finish)))
            }
        }
    }

    /// [`Flow::Poll`]: ask `token_url` until the browser has finished. 404 is
    /// "not yet"; backoff 1 s × 1.2 up to 10 s.
    async fn poll(&self, p: &Provider, uuid: &str, verifier: &str) -> Result<Tokens, String> {
        let mut wait = Duration::from_secs(1);
        let mut failures = 0;
        loop {
            tokio::time::sleep(wait).await;
            wait = (wait.mul_f32(1.2)).min(Duration::from_secs(10));
            let mut url = Url::parse(&p.token_url).map_err(|e| e.to_string())?;
            url.query_pairs_mut()
                .append_pair("uuid", uuid)
                .append_pair("verifier", verifier);
            let res = match self.http.get(url).send().await {
                Ok(r) if r.status() == reqwest::StatusCode::NOT_FOUND => continue,
                Ok(r) if r.status().is_success() => r,
                Ok(r) if failures < 3 => {
                    failures += 1;
                    tracing::debug!("{}: poll returned {}", p.ns, r.status());
                    continue;
                }
                Ok(r) => return Err(format!("{}: login poll returned {}", p.ns, r.status())),
                Err(e) => return Err(format!("{}: login poll: {e}", p.ns)),
            };
            let body = res
                .text()
                .await
                .map_err(|e| format!("{}: login poll: {e}", p.ns))?;
            let r: TokenResponse = serde_json::from_str(&body)
                .map_err(|e| format!("{}: login poll response: {e}", p.ns))?;
            return Ok(Tokens::from_response(r, None));
        }
    }

    /// [`Flow::Device`]: ask `token_url` for the device grant every `interval`.
    /// `authorization_pending` / `slow_down` (+5 s) mean not yet; any other
    /// error ends the login.
    async fn poll_device(
        &self,
        p: &Provider,
        device_code: &str,
        mut interval: Duration,
    ) -> Result<Tokens, String> {
        let form = [
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("device_code", device_code),
            ("client_id", &p.client_id),
        ];
        loop {
            tokio::time::sleep(interval).await;
            let res = self
                .http
                .post(&*p.token_url)
                .header(reqwest::header::ACCEPT, "application/json")
                .form(&form)
                .send()
                .await
                .map_err(|e| format!("{}: login poll: {e}", p.ns))?;
            let status = res.status();
            let body = res
                .text()
                .await
                .map_err(|e| format!("{}: login poll: {e}", p.ns))?;
            if status.is_success() {
                let r: TokenResponse = serde_json::from_str(&body)
                    .map_err(|e| format!("{}: login poll response: {e}", p.ns))?;
                return Ok(Tokens::from_response(r, None));
            }
            let err: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            match err["error"].as_str().unwrap_or("") {
                "authorization_pending" => {}
                "slow_down" => interval += Duration::from_secs(5),
                e => {
                    let why = err["error_description"].as_str().unwrap_or("");
                    return Err(format!("{}: login {status} {e} {why}", p.ns)
                        .trim_end()
                        .to_owned());
                }
            }
        }
    }

    /// Tokens for `p`, refreshed and re-saved when within a minute of expiry.
    /// Errors name `/login` so a UI can show them verbatim.
    pub async fn access(&self, p: &Provider) -> Result<Tokens, String> {
        let _one_at_a_time = self.refresh.lock().await;
        let t = self
            .load(&p.ns)
            .ok_or_else(|| format!("{}: not logged in; run /login {}", p.ns, p.ns))?;
        if t.expires_at > now() + REFRESH_MARGIN_SECS {
            return Ok(t);
        }
        let Some(refresh) = t.refresh_token.as_deref() else {
            return Err(format!("{}: login expired; run /login {}", p.ns, p.ns));
        };
        let fresh = match &p.flow {
            Flow::Code { .. } | Flow::Device => {
                let headers: &[(&str, &str)] = match &p.flow {
                    Flow::Code {
                        refresh_headers, ..
                    } => refresh_headers,
                    _ => &[],
                };
                let form = [
                    ("grant_type", "refresh_token"),
                    ("refresh_token", refresh),
                    ("client_id", &p.client_id),
                ];
                self.token_request(p, &form, headers, Some(&t)).await
            }
            Flow::Poll { refresh_url } => {
                let res = self
                    .http
                    .post(*refresh_url)
                    .bearer_auth(refresh)
                    .json(&serde_json::json!({}))
                    .send()
                    .await
                    .map_err(|e| format!("{}: refresh: {e}", p.ns));
                match res {
                    Ok(r) if r.status().is_success() => r
                        .text()
                        .await
                        .map_err(|e| format!("{}: refresh: {e}", p.ns))
                        .and_then(|b| {
                            serde_json::from_str::<TokenResponse>(&b)
                                .map_err(|e| format!("{}: refresh response: {e}", p.ns))
                        })
                        .map(|r| Tokens::from_response(r, Some(&t))),
                    Ok(r) => Err(format!("{}: refresh returned {}", p.ns, r.status())),
                    Err(e) => Err(e),
                }
            }
        }
        .map_err(|e| format!("{e}; run /login {}", p.ns))?;
        self.save(&p.ns, &fresh)
            .map_err(|e| format!("{}: saving tokens: {e}", p.ns))?;
        Ok(fresh)
    }

    /// POST the token endpoint, form or JSON per the flow. The body is never
    /// logged or returned in errors: it carries tokens.
    async fn token_request(
        &self,
        p: &Provider,
        fields: &[(&str, &str)],
        headers: &[(&str, &str)],
        prev: Option<&Tokens>,
    ) -> Result<Tokens, String> {
        let mut req = self
            .http
            .post(&*p.token_url)
            .header(reqwest::header::ACCEPT, "application/json");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        req = match p.flow {
            Flow::Code {
                token_body: Body::Json,
                ..
            } => req.json(&fields.iter().copied().collect::<HashMap<_, _>>()),
            _ => req.form(fields),
        };
        let res = req
            .send()
            .await
            .map_err(|e| format!("{}: token endpoint: {e}", p.ns))?;
        let status = res.status();
        let body = res
            .text()
            .await
            .map_err(|e| format!("{}: token endpoint: {e}", p.ns))?;
        if !status.is_success() {
            return Err(format!("{}: token endpoint returned {status}", p.ns));
        }
        let r: TokenResponse =
            serde_json::from_str(&body).map_err(|e| format!("{}: token response: {e}", p.ns))?;
        Ok(Tokens::from_response(r, prev))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jwt(claims: serde_json::Value) -> String {
        format!(
            "h.{}.s",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap())
        )
    }

    /// Answer every request on a loopback listener with `body` (JSON, 200).
    async fn fake_token_endpoint(body: String) -> String {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/token", l.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf).await;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(resp.as_bytes()).await;
            }
        });
        url
    }

    #[tokio::test]
    async fn callback_serves_one_redirect_and_ignores_favicon() {
        let (port, got) = callback(0).await.unwrap();
        let got = tokio::spawn(got);
        let client = reqwest::Client::new();
        let r = client
            .get(format!("http://127.0.0.1:{port}/favicon.ico"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 404);
        let r = client
            .get(format!("http://127.0.0.1:{port}/cb?code=abc&state=xyz"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        assert!(r.text().await.unwrap().contains("Logged in"));
        assert_eq!(
            got.await.unwrap().unwrap(),
            format!("http://127.0.0.1:{port}/cb?code=abc&state=xyz")
        );
    }

    #[tokio::test]
    async fn login_exchanges_code_saves_and_refresh_keeps_old_refresh_token() {
        let dir = std::env::temp_dir().join(format!("oauth-test-{}", uuid::Uuid::new_v4()));
        let access = jwt(serde_json::json!({
            "exp": now() + 30,  // inside the refresh margin: `access` must refresh
            "https://api.openai.com/auth": { "chatgpt_account_id": "acct_1" }
        }));
        let token_url = fake_token_endpoint(
            serde_json::json!({ "access_token": access, "refresh_token": "r1", "expires_in": 3600 })
                .to_string(),
        )
        .await;
        let p = Provider {
            ns: Cow::Borrowed("test"),
            auth_url: Cow::Borrowed("https://example.com/authorize"),
            token_url: Cow::Owned(token_url),
            client_id: Cow::Borrowed("cid"),
            scopes: Cow::Borrowed("openid"),
            port: 0,
            redirect_host: Cow::Borrowed("127.0.0.1"),
            redirect_path: Cow::Borrowed("/cb"),
            extra_query: &[("x", "1")],
            flow: Flow::Code {
                token_body: Body::Form,
                refresh_headers: &[],
            },
        };
        let store = Store::new(&dir);
        let (url, finish) = store.login(&p).await.unwrap();
        // Callers spawn `finish` before the browser lands; the test does the same.
        let finish = tokio::spawn(finish);
        let url = Url::parse(&url).unwrap();
        let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["x"], "1");
        let redirect = &q["redirect_uri"];
        // The browser lands on the redirect with the code and our state.
        reqwest::get(format!("{redirect}?code=c0de&state={}", q["state"]))
            .await
            .unwrap();
        let t = finish.await.unwrap().unwrap();
        assert_eq!(t.account_id.as_deref(), Some("acct_1"));
        assert_eq!(t.refresh_token.as_deref(), Some("r1"));
        assert_eq!(store.load("test").unwrap(), t);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("oauth/test.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // Refresh path: the endpoint omits refresh_token → the old one survives.
        let fresh_access = jwt(serde_json::json!({ "exp": now() + 7200 }));
        let refresh_url = fake_token_endpoint(
            serde_json::json!({ "access_token": fresh_access, "expires_in": 7200 }).to_string(),
        )
        .await;
        let p2 = Provider {
            token_url: Cow::Owned(refresh_url),
            ..p
        };
        let t2 = store.access(&p2).await.unwrap();
        assert_eq!(t2.access_token, fresh_access);
        assert_eq!(t2.refresh_token.as_deref(), Some("r1"));
        assert_eq!(t2.account_id.as_deref(), Some("acct_1"));
        assert!(t2.expires_at > now() + 7000);
        // Fresh tokens are returned as-is.
        assert_eq!(store.access(&p2).await.unwrap(), t2);
        store.remove("test").unwrap();
        assert!(store.load("test").is_none());
        store.remove("test").unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Cursor-style: deep link, then poll until the browser is done; refresh
    /// with the refresh token as bearer.
    #[tokio::test]
    async fn poll_login_waits_for_404s_then_takes_the_jwt_pair() {
        let dir = std::env::temp_dir().join(format!("oauth-test-{}", uuid::Uuid::new_v4()));
        let access = jwt(serde_json::json!({ "exp": now() + 7200, "sub": "auth0|user_42" }));
        // Two 404s (pending), then the pair.
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let poll_url = format!("http://{}/auth/poll", l.local_addr().unwrap());
        let body = serde_json::json!({ "accessToken": access, "refreshToken": "rt" }).to_string();
        tokio::spawn(async move {
            let mut n = 0;
            while let Ok((mut s, _)) = l.accept().await {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf).await;
                n += 1;
                let resp = if n <= 2 {
                    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_owned()
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                };
                let _ = s.write_all(resp.as_bytes()).await;
            }
        });
        let p = Provider {
            token_url: Cow::Owned(poll_url),
            ..CURSOR
        };
        let store = Store::new(&dir);
        let (url, finish) = store.login(&p).await.unwrap();
        let url = Url::parse(&url).unwrap();
        let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
        assert_eq!(q["mode"], "login");
        assert!(q.contains_key("challenge") && q.contains_key("uuid"));
        let t = finish.await.unwrap();
        assert_eq!(t.access_token, access);
        assert_eq!(t.refresh_token.as_deref(), Some("rt"));
        assert_eq!(t.account_id.as_deref(), Some("user_42"));
        assert_eq!(store.load("cursor").unwrap(), t);
        let _ = std::fs::remove_dir_all(dir);
    }
}
