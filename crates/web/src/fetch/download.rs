//! [`download`]: one public HTTPS file streamed to disk (design doc 199 D16, P3).
//!
//! Unlike [`Fetcher`](super::Fetcher) this lane is always public-only (never
//! the LAN twin, whatever `tools::egress` grants the turn), HTTPS-only on every
//! hop, has no whole-request timeout and never buffers the body. One timer
//! runs per download: `min(deadline, last progress + idle_timeout)`, armed
//! while awaiting each hop's headers and between body frames. No resume: a
//! failed download leaves nothing behind and restarts from zero.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::{Duration, Instant};

use futures::StreamExt;
use tokio::io::AsyncWriteExt;
use tokio::time::Sleep;
use tools::CancellationToken;
use url::Url;

use super::ssrf::{self, GuardedResolver};
use super::{Fetcher, primary_type};

/// Redirect hops followed, as `WebConfig::max_redirects`' default.
const MAX_REDIRECTS: usize = 5;
/// Disk write batch; the only per-download buffer besides one body frame.
const WRITE_BUF: usize = 1 << 20;

/// Limits of one [`download`].
#[derive(Clone, Debug)]
pub struct DownloadOpts {
    /// Largest body accepted, declared or streamed.
    pub max_bytes: u64,
    /// TCP + TLS connect timeout per hop.
    pub connect_timeout: Duration,
    /// No progress (headers or a non-empty body frame) for this long ends the download.
    pub idle_timeout: Duration,
    /// Ends the download even while bytes flow.
    pub deadline: Option<Instant>,
    /// Cancelling ends the download and removes the partial file.
    pub cancel: CancellationToken,
    /// Test hook: these exact blocked `ip:port` literals pass the guard (loopback). Not reachable from config.
    #[doc(hidden)]
    pub test_allow: Vec<SocketAddr>,
    /// Test hook: trust only this PEM root instead of the platform's. Not reachable from config.
    #[doc(hidden)]
    pub test_root_pem: Option<Vec<u8>>,
}

impl Default for DownloadOpts {
    fn default() -> Self {
        Self {
            max_bytes: 5_000_000_000,
            connect_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(60),
            deadline: None,
            cancel: CancellationToken::new(),
            test_allow: Vec::new(),
            test_root_pem: None,
        }
    }
}

/// A completed download; `dest` holds exactly `bytes` bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Downloaded {
    /// Bytes written.
    pub bytes: u64,
    /// BLAKE3 of the bytes, lowercase hex.
    pub blake3: String,
    /// Lowercased primary `Content-Type`, unvalidated: for display only.
    pub media_type: Option<String>,
    /// URL that served the body, after redirects.
    pub final_url: String,
}

/// Why a [`download`] failed. `Display` is short enough for a file card.
#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    /// Not an absolute URL (or an unusable redirect `Location`).
    #[error("refused: not a valid URL")]
    InvalidUrl,
    /// The URL or a redirect target is not `https`.
    #[error("refused: not HTTPS")]
    NotHttps,
    /// The URL or a redirect target carries `user:pass@`.
    #[error("refused: credentials in the URL")]
    Userinfo,
    /// The SSRF guard refused the host or an address it resolves to.
    #[error("refused: not a public address")]
    NotPublic,
    /// Over `max`: by `Content-Length` (`declared`, nothing read) or by the streamed count.
    #[error("too large: over {}", human(*max))]
    TooLarge {
        /// The limit.
        max: u64,
        /// The declared `Content-Length`; `None` when the stream ran past `max`.
        declared: Option<u64>,
    },
    /// No progress for the idle timeout.
    #[error("stalled: no data for {} s", .0.as_secs())]
    IdleTimeout(Duration),
    /// The caller's deadline passed.
    #[error("expired: deadline reached")]
    Deadline,
    /// The caller's token was cancelled.
    #[error("cancelled")]
    Cancelled,
    /// The final response was not 2xx (or a redirect without `Location`).
    #[error("failed: HTTP {0}")]
    HttpStatus(u16),
    /// More than the redirect limit.
    #[error("refused: too many redirects")]
    Redirects,
    /// Creating or writing `dest`.
    #[error("failed to save: {0}")]
    Io(#[from] std::io::Error),
    /// Connect, TLS or read failure.
    #[error("failed: {0}")]
    Transport(String),
}

/// `5 GB`, `1.5 MB`, `10 bytes` (decimal units).
fn human(n: u64) -> String {
    #[allow(clippy::cast_precision_loss)] // display only
    let f = n as f64;
    match n {
        1_000_000_000.. => format!("{} GB", f / 1e9),
        1_000_000.. => format!("{} MB", f / 1e6),
        _ => format!("{n} bytes"),
    }
}

/// GET `url` into a new file `dest` (created exclusively, mode 0600), hashing as it streams.
///
/// # Errors
/// See [`DownloadError`]. On any error, or when the future is dropped, a file
/// this call created is removed; an existing `dest` is never touched.
pub async fn download(
    url: &str,
    dest: &Path,
    opts: &DownloadOpts,
) -> Result<Downloaded, DownloadError> {
    let mut current = Url::parse(url.trim()).map_err(|_| DownloadError::InvalidUrl)?;
    let client = client(opts)?;
    let mut timer = Timer::new(opts);
    let mut redirects = 0usize;
    let response = loop {
        check(&current, &opts.test_allow)?;
        tracing::debug!(url = %current, redirects, "download hop");
        let sent = timer.race(&opts.cancel, client.get(current.as_str()).send());
        let response = sent.await?.map_err(|e| transport(&e))?;
        timer.progress();
        let status = response.status();
        if !Fetcher::is_redirect(status) {
            break response;
        }
        if redirects >= MAX_REDIRECTS {
            return Err(DownloadError::Redirects);
        }
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .ok_or(DownloadError::HttpStatus(status.as_u16()))?;
        current = current
            .join(location)
            .map_err(|_| DownloadError::InvalidUrl)?;
        redirects += 1;
    };
    if !response.status().is_success() {
        return Err(DownloadError::HttpStatus(response.status().as_u16()));
    }
    if let Some(len) = response.content_length()
        && len > opts.max_bytes
    {
        return Err(DownloadError::TooLarge {
            max: opts.max_bytes,
            declared: Some(len),
        });
    }
    let media_type = primary_type(response.headers());
    // `file` is declared after `partial`, so it is closed before `partial` removes the path.
    let (mut partial, file) = Partial::create(dest).await?;
    let mut file = tokio::io::BufWriter::with_capacity(WRITE_BUF, file);
    let mut body = response.bytes_stream();
    let mut hasher = blake3::Hasher::new();
    let mut bytes = 0u64;
    while let Some(frame) = timer.race(&opts.cancel, body.next()).await? {
        let frame = frame.map_err(|e| transport(&e))?;
        if frame.is_empty() {
            continue;
        }
        bytes += frame.len() as u64;
        if bytes > opts.max_bytes {
            return Err(DownloadError::TooLarge {
                max: opts.max_bytes,
                declared: None,
            });
        }
        hasher.update(&frame);
        file.write_all(&frame).await?;
        timer.progress();
    }
    file.flush().await?;
    file.get_ref().sync_all().await?;
    partial.keep();
    Ok(Downloaded {
        bytes,
        blake3: hasher.finalize().to_hex().to_string(),
        media_type,
        final_url: current.to_string(),
    })
}

/// The per-download client: guarded public-only resolver, no redirects, no
/// proxy (a proxy would resolve the target itself), HTTPS only, connect
/// timeout only — no whole-request timeout. No cookie store, no credentials.
fn client(opts: &DownloadOpts) -> Result<reqwest::Client, DownloadError> {
    let mut builder = reqwest::Client::builder()
        .user_agent(crate::engines::user_agent())
        .connect_timeout(opts.connect_timeout)
        .redirect(reqwest::redirect::Policy::none())
        .dns_resolver(GuardedResolver { private: false })
        .no_proxy()
        .https_only(true);
    if let Some(pem) = &opts.test_root_pem {
        let root = reqwest::Certificate::from_pem(pem)
            .map_err(|e| DownloadError::Transport(e.to_string()))?;
        builder = builder.tls_certs_only([root]);
    }
    builder
        .build()
        .map_err(|e| DownloadError::Transport(e.to_string()))
}

/// Every hop: `https`, no userinfo, then the public-only static guard
/// (denylisted hostnames, blocked IP literals). Hostnames are vetted again at
/// connect time by [`GuardedResolver`].
fn check(url: &Url, allow: &[SocketAddr]) -> Result<(), DownloadError> {
    if url.scheme() != "https" {
        return Err(DownloadError::NotHttps);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(DownloadError::Userinfo);
    }
    if url.host().is_none() {
        return Err(DownloadError::InvalidUrl);
    }
    ssrf::check_static(url, allow, false).map_err(|e| {
        tracing::debug!(%url, error = %e, "download refused");
        DownloadError::NotPublic
    })
}

/// A transport error; a resolver refusal of a blocked address is [`DownloadError::NotPublic`].
fn transport(e: &reqwest::Error) -> DownloadError {
    match super::client::ClientError::from_reqwest(e).as_rejected() {
        // `ssrf::resolve_checked`/`vet`: lookup failures start with "DNS ", refusals do not.
        Some(m) if !m.starts_with("DNS ") => DownloadError::NotPublic,
        Some(m) => DownloadError::Transport(m.to_owned()),
        None => DownloadError::Transport(e.to_string()),
    }
}

/// The one timer: fires at `min(deadline, last progress + idle)`.
struct Timer {
    sleep: Pin<Box<Sleep>>,
    idle: Duration,
    deadline: Option<tokio::time::Instant>,
}

impl Timer {
    fn new(opts: &DownloadOpts) -> Self {
        let deadline = opts.deadline.map(tokio::time::Instant::from_std);
        let mut timer = Self {
            sleep: Box::pin(tokio::time::sleep(Duration::ZERO)),
            idle: opts.idle_timeout,
            deadline,
        };
        timer.progress();
        timer
    }

    /// Restart the idle part.
    fn progress(&mut self) {
        let idle = tokio::time::Instant::now() + self.idle;
        let at = self.deadline.map_or(idle, |d| d.min(idle));
        self.sleep.as_mut().reset(at);
    }

    /// `fut`, unless the timer or `cancel` fires first.
    async fn race<F: Future>(
        &mut self,
        cancel: &CancellationToken,
        fut: F,
    ) -> Result<F::Output, DownloadError> {
        tokio::select! {
            biased;
            () = cancel.cancelled() => Err(DownloadError::Cancelled),
            () = self.sleep.as_mut() => Err(match self.deadline {
                Some(d) if tokio::time::Instant::now() >= d => DownloadError::Deadline,
                _ => DownloadError::IdleTimeout(self.idle),
            }),
            out = fut => Ok(out),
        }
    }
}

/// A file this download created; removed on drop unless [`Self::keep`] ran.
struct Partial {
    path: PathBuf,
    keep: bool,
}

impl Partial {
    /// Create `path` exclusively (mode 0600); an existing file is an error and stays untouched.
    async fn create(path: &Path) -> std::io::Result<(Self, tokio::fs::File)> {
        let mut open = tokio::fs::OpenOptions::new();
        open.write(true).create_new(true);
        #[cfg(unix)]
        open.mode(0o600);
        let file = open.open(path).await?;
        let partial = Self {
            path: path.to_owned(),
            keep: false,
        };
        Ok((partial, file))
    }

    fn keep(&mut self) {
        self.keep = true;
    }
}

impl Drop for Partial {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
