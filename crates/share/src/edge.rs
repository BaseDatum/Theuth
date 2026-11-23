//! The connection to a share edge (210 §5.3, §5.9): the dialer and delegation seams, the
//! control-plane client (one HTTP/1.1 request per bi-stream the engine opens) and the per-edge
//! state machine `idle → dialing → info → delegating → hello → attached → draining | blocked |
//! removed`.

use std::io;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt as _, Full, Limited};
use hyper_util::rt::TokioIo;
use serde::de::DeserializeOwned;
use share_proto::{
    EndpointAddr, ErrorBody, ErrorCode, Event, Hello, Info, KillReason, PublicKey, RoutesPut,
    ShareEdgeTicket, Signed, Welcome, limits, paths,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, watch};

use crate::model::EdgeState;

/// A bidirectional byte stream: one QUIC bi-stream.
pub trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// A boxed [`Io`].
pub type Stream = Box<dyn Io>;

/// One connection to the edge on `theuth/share/1`.
#[async_trait::async_trait]
pub trait EdgeConn: Send + Sync {
    /// Open a bi-stream (control request).
    async fn open(&self) -> io::Result<Stream>;
    /// The next bi-stream the edge opened (a visitor); `None` once the connection is gone.
    async fn accept(&self) -> Option<Stream>;
    /// Close the connection.
    fn close(&self);
}

#[async_trait::async_trait]
impl EdgeConn for iroh::endpoint::Connection {
    /// A control stream; `open_bi` waits at most [`limits::OPEN_BI_TIMEOUT_MS`] for stream
    /// credit (§5.3), so a stalled edge fails the request instead of hanging it.
    async fn open(&self) -> io::Result<Stream> {
        let open = tokio::time::timeout(
            Duration::from_millis(limits::OPEN_BI_TIMEOUT_MS),
            self.open_bi(),
        );
        let (send, recv) = open
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "open_bi to the edge timed out"))?
            .map_err(io::Error::other)?;
        Ok(Box::new(tokio::io::join(recv, send)))
    }

    async fn accept(&self) -> Option<Stream> {
        let (send, recv) = self.accept_bi().await.ok()?;
        Some(Box::new(tokio::io::join(recv, send)))
    }

    fn close(&self) {
        iroh::endpoint::Connection::close(self, 0u32.into(), b"share: closed");
    }
}

/// Dials an edge from the engine's mesh endpoint (D18). The engine's implementation is
/// [`crate::wiring::MeshDialer`] over `mesh`'s `Net::dial_alpn` with
/// [`crate::wiring::tunnel_transport`] and no `join_network` (§5.3).
#[async_trait::async_trait]
pub trait EdgeDialer: Send + Sync {
    /// Dial `addr` offering `alpns` (newest first).
    async fn dial(&self, addr: EndpointAddr, alpns: &[&[u8]]) -> anyhow::Result<Arc<dyn EdgeConn>>;
}

/// Issues delegations (§5.7). In M1 the engine is its own holder ([`SelfHolder`]).
// TODO(210-M2, crates agent/mesh): `share.delegate` on the lease holder over `theuth/rpc/1`.
#[async_trait::async_trait]
pub trait DelegationSource: Send + Sync {
    /// A delegation for this engine at `edge`/`domain`, `issued_ms = min(holder clock, edge_now_ms)`.
    async fn delegate(
        &self,
        edge: PublicKey,
        domain: &str,
        edge_now_ms: u64,
    ) -> anyhow::Result<Signed>;
}

/// A fixed key signing for this engine (tests, tools). The engine uses
/// [`crate::wiring::MeshDelegation`], which reads or mints `mesh/share_key` per call.
pub struct SelfHolder {
    /// The mesh share key.
    pub mesh_key: share_proto::SecretKey,
    /// This engine's endpoint id.
    pub engine: PublicKey,
}

#[async_trait::async_trait]
impl DelegationSource for SelfHolder {
    async fn delegate(
        &self,
        edge: PublicKey,
        domain: &str,
        edge_now_ms: u64,
    ) -> anyhow::Result<Signed> {
        let d = share_proto::Delegation::new(
            domain.to_owned(),
            edge,
            self.mesh_key.public(),
            self.engine,
            platform::time::now_ms(),
            edge_now_ms,
        );
        Ok(d.sign(&self.mesh_key)?)
    }
}

/// A control-plane failure.
#[derive(Debug, thiserror::Error)]
pub enum EdgeError {
    /// The edge answered `4xx`/`5xx` with `{code, …}`.
    #[error("edge answered {status}: {}", body.code)]
    Http {
        /// HTTP status.
        status: u16,
        /// The body.
        body: Box<ErrorBody>,
    },
    /// Stream, HTTP or JSON failure.
    #[error("edge transport: {0}")]
    Transport(#[from] anyhow::Error),
}

impl EdgeError {
    /// The edge's code, if it answered one.
    pub fn code(&self) -> Option<ErrorCode> {
        match self {
            EdgeError::Http { body, .. } => Some(body.code),
            EdgeError::Transport(_) => None,
        }
    }
}

/// `POST /v1/names/{name}/csr` answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CsrAnswer {
    /// `200`.
    Issued(share_proto::CertChain),
    /// `202`.
    Pending(share_proto::CertPending),
}

/// The control-plane client on one connection.
#[derive(Clone)]
pub struct Control {
    conn: Arc<dyn EdgeConn>,
}

type Resp = hyper::Response<hyper::body::Incoming>;

impl Control {
    /// Over `conn`.
    pub fn new(conn: Arc<dyn EdgeConn>) -> Self {
        Self { conn }
    }

    async fn send(
        &self,
        method: http::Method,
        path: &str,
        body: Bytes,
        ctype: &str,
    ) -> Result<Resp, EdgeError> {
        let stream = self.conn.open().await.map_err(anyhow::Error::from)?;
        let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(anyhow::Error::from)?;
        tokio::spawn(async move {
            let _ = conn.await;
        });
        let req = http::Request::builder()
            .method(method)
            .uri(path)
            .header(http::header::HOST, "edge")
            .header(http::header::CONTENT_TYPE, ctype)
            .body(Full::new(body))
            .map_err(anyhow::Error::from)?;
        let resp = sender
            .send_request(req)
            .await
            .map_err(anyhow::Error::from)?;
        if resp.status().is_client_error() || resp.status().is_server_error() {
            let status = resp.status().as_u16();
            let bytes = read_limited(resp).await?;
            let body = serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| ErrorBody::new(ErrorCode::Unknown));
            return Err(EdgeError::Http {
                status,
                body: Box::new(body),
            });
        }
        Ok(resp)
    }

    async fn json<T: DeserializeOwned>(
        &self,
        method: http::Method,
        path: &str,
        body: Option<&impl serde::Serialize>,
    ) -> Result<(u16, T), EdgeError> {
        let bytes = match body {
            Some(b) => Bytes::from(serde_json::to_vec(b).map_err(anyhow::Error::from)?),
            None => Bytes::new(),
        };
        let resp = self.send(method, path, bytes, "application/json").await?;
        let status = resp.status().as_u16();
        let bytes = read_limited(resp).await?;
        let v = if bytes.is_empty() {
            serde_json::from_slice(b"null")
        } else {
            serde_json::from_slice(&bytes)
        }
        .map_err(anyhow::Error::from)?;
        Ok((status, v))
    }

    /// `GET /v1/info`.
    pub async fn info(&self) -> Result<Info, EdgeError> {
        Ok(self
            .json(http::Method::GET, paths::INFO, None::<&()>)
            .await?
            .1)
    }

    /// `POST /v1/hello`.
    pub async fn hello(&self, h: &Hello) -> Result<Welcome, EdgeError> {
        Ok(self
            .json(http::Method::POST, paths::HELLO, Some(h))
            .await?
            .1)
    }

    /// `PUT /v1/routes` with this engine's full set.
    pub async fn put_routes(&self, hosts: Vec<String>) -> Result<(), EdgeError> {
        let _: (u16, serde_json::Value) = self
            .json(http::Method::PUT, paths::ROUTES, Some(&RoutesPut { hosts }))
            .await?;
        Ok(())
    }

    /// `GET /v1/names/{name}`.
    pub async fn check_name(&self, name: &str) -> Result<share_proto::NameAvailability, EdgeError> {
        Ok(self
            .json(http::Method::GET, &paths::name(name), None::<&()>)
            .await?
            .1)
    }

    /// `POST /v1/names {name}`: allocate a client-drawn random name.
    pub async fn allocate(&self, name: &str) -> Result<share_proto::NameGranted, EdgeError> {
        let req = share_proto::NameRequest {
            name: name.to_owned(),
        };
        Ok(self
            .json(http::Method::POST, paths::NAMES, Some(&req))
            .await?
            .1)
    }

    /// `PUT /v1/names/{name}`: claim a reserved name.
    pub async fn claim(&self, name: &str) -> Result<share_proto::NameGranted, EdgeError> {
        Ok(self
            .json(http::Method::PUT, &paths::name(name), None::<&()>)
            .await?
            .1)
    }

    /// `DELETE /v1/names/{name}`.
    pub async fn release(&self, name: &str) -> Result<share_proto::NameReleased, EdgeError> {
        Ok(self
            .json(http::Method::DELETE, &paths::name(name), None::<&()>)
            .await?
            .1)
    }

    /// `POST /v1/names/{name}/csr` with the CSR DER.
    pub async fn csr(
        &self,
        name: &str,
        der: Vec<u8>,
        rotate: bool,
    ) -> Result<CsrAnswer, EdgeError> {
        let resp = self
            .send(
                http::Method::POST,
                &paths::name_csr(name, rotate),
                der.into(),
                "application/pkcs10",
            )
            .await?;
        let status = resp.status().as_u16();
        let bytes = read_limited(resp).await?;
        Ok(if status == 202 {
            CsrAnswer::Pending(serde_json::from_slice(&bytes).map_err(anyhow::Error::from)?)
        } else {
            CsrAnswer::Issued(serde_json::from_slice(&bytes).map_err(anyhow::Error::from)?)
        })
    }

    /// `GET /v1/names/{name}/cert`.
    pub async fn cert(&self, name: &str) -> Result<share_proto::CertChain, EdgeError> {
        Ok(self
            .json(http::Method::GET, &paths::name_cert(name), None::<&()>)
            .await?
            .1)
    }

    /// `POST /v1/mesh/stop`: Stop all (§7.4).
    pub async fn stop_all(&self) -> Result<share_proto::StopAll, EdgeError> {
        Ok(self
            .json(http::Method::POST, paths::MESH_STOP, None::<&()>)
            .await?
            .1)
    }

    /// `GET /v1/events`: SSE parsed into [`Event`]s on a channel; closes when the stream ends.
    pub async fn events(&self) -> Result<mpsc::Receiver<Event>, EdgeError> {
        let resp = self
            .send(
                http::Method::GET,
                paths::EVENTS,
                Bytes::new(),
                "text/event-stream",
            )
            .await?;
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            let mut body = resp.into_body();
            let mut parser = SseParser::default();
            while let Some(Ok(frame)) = body.frame().await {
                let Ok(data) = frame.into_data() else {
                    continue;
                };
                for ev in parser.push(&data) {
                    if tx.send(ev).await.is_err() {
                        return;
                    }
                }
            }
        });
        Ok(rx)
    }
}

async fn read_limited(resp: Resp) -> Result<Bytes, EdgeError> {
    Ok(Limited::new(resp.into_body(), limits::MAX_JSON_BODY)
        .collect()
        .await
        .map_err(|e| anyhow::anyhow!("edge body: {e}"))?
        .to_bytes())
}

/// Longest SSE line kept; a longer one is dropped whole (m1).
pub const SSE_MAX_LINE: usize = 64 * 1024;
/// Largest `data` of one event; a larger event is dropped whole (m1).
pub const SSE_MAX_DATA: usize = limits::MAX_JSON_BODY;

/// Incremental `text/event-stream` parser for `event:` + `data:` records. Bytes are split on
/// `\n` before decoding, so a multibyte character split across chunks survives; line and data
/// sizes are capped.
#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
    /// Inside an over-long line: drop bytes up to the next `\n`.
    skipping: bool,
    event: Option<String>,
    data: String,
    /// The current event outgrew [`SSE_MAX_DATA`]: drop it at the blank line.
    oversize: bool,
}

impl SseParser {
    /// Feed bytes; returns the complete, known events. Unknown names are skipped (additive rule).
    pub fn push(&mut self, mut bytes: &[u8]) -> Vec<Event> {
        let mut out = Vec::new();
        while let Some(i) = bytes.iter().position(|&b| b == b'\n') {
            let (head, tail) = (&bytes[..i], &bytes[i + 1..]);
            bytes = tail;
            if std::mem::take(&mut self.skipping) {
                self.buf.clear();
                continue;
            }
            if self.buf.len() + head.len() > SSE_MAX_LINE {
                self.buf.clear();
                continue;
            }
            self.buf.extend_from_slice(head);
            let raw = std::mem::take(&mut self.buf);
            let line = String::from_utf8_lossy(&raw);
            self.line(line.trim_end_matches('\r'), &mut out);
        }
        if !self.skipping {
            if self.buf.len() + bytes.len() > SSE_MAX_LINE {
                self.buf.clear();
                self.skipping = true;
            } else {
                self.buf.extend_from_slice(bytes);
            }
        }
        out
    }

    fn line(&mut self, line: &str, out: &mut Vec<Event>) {
        if line.is_empty() {
            if let Some(name) = self.event.take()
                && !std::mem::take(&mut self.oversize)
                && let Ok(Some(ev)) = Event::parse(&name, &self.data)
            {
                out.push(ev);
            }
            self.oversize = false;
            self.data.clear();
        } else if let Some(v) = line.strip_prefix("event:") {
            self.event = Some(v.trim().to_owned());
        } else if let Some(v) = line.strip_prefix("data:") {
            let v = v.trim_start();
            if self.oversize || self.data.len() + v.len() + 1 > SSE_MAX_DATA {
                self.oversize = true;
                self.data.clear();
                return;
            }
            if !self.data.is_empty() {
                self.data.push('\n');
            }
            self.data.push_str(v);
        }
    }
}

/// The edge speaks a protocol this build cannot (M11); shown as `blocked` in `share.status`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Incompatible {
    /// Another protocol major.
    #[error("protocol_mismatch: the edge speaks v{edge}, this theuth v{ours}")]
    Protocol {
        /// The edge's `v`.
        edge: u32,
        /// [`share_proto::PROTOCOL_MAJOR`].
        ours: u32,
    },
    /// This build is older than the edge's `min_client`.
    #[error("client_too_old: the edge needs theuth {min} or newer")]
    ClientTooOld {
        /// The edge's `min_client`.
        min: String,
    },
}

/// Check an `info` or `welcome` (`v`, `min_client`) against this build's agent `me`.
pub fn compat(v: u32, min_client: &str, me: &str) -> Result<(), Incompatible> {
    if v != share_proto::PROTOCOL_MAJOR {
        return Err(Incompatible::Protocol {
            edge: v,
            ours: share_proto::PROTOCOL_MAJOR,
        });
    }
    if !share_proto::version::client_ok(me, min_client) {
        return Err(Incompatible::ClientTooOld {
            min: min_client.to_owned(),
        });
    }
    Ok(())
}

/// What the state machine asks of the rest of the plugin while attached.
#[async_trait::async_trait]
pub trait Attachment: Send + Sync {
    /// This engine's full route set.
    fn hosts(&self) -> Vec<String>;
    /// A `welcome` arrived (apply `stop_all_ms`, record names, fetch missing chains).
    async fn welcome(&self, control: &Control, info: &Info, welcome: &Welcome);
    /// An SSE event other than `drain`.
    async fn event(&self, control: &Control, event: Event);
    /// A visitor stream the edge opened.
    fn visitor(&self, stream: Stream);
    /// The connection is gone (shares go `offline` after 10 s without a reconnect).
    fn lost(&self);
    /// The engine was revoked or removed (§5.9 `removed`).
    async fn removed(&self);
    /// The state changed.
    fn state(&self, state: &EdgeState, clock_off_ms: Option<i64>);
}

/// Backoff of `dialing` (§5.9): 1 s doubling to 5 min, ±20 % jitter.
pub fn backoff(attempt: u32) -> Duration {
    let base = 1000u64.saturating_mul(1u64 << attempt.min(9)).min(300_000);
    let jitter = (uuid::Uuid::new_v4().as_u128() % 401) as i64 - 200; // ‰
    Duration::from_millis((base as i64 + base as i64 * jitter / 1000).max(0) as u64)
}

/// Re-`hello` interval with a renewed delegation (§5.7).
pub const RENEW: Duration = Duration::from_millis(limits::DELEGATION_RENEW_MS);
/// Wait after a clock code (§5.9).
pub const CLOCK_RETRY: Duration = Duration::from_secs(300);

/// Timings, overridable in tests.
#[derive(Clone, Debug)]
pub struct Timing {
    /// Backoff per failed attempt.
    pub backoff: fn(u32) -> Duration,
    /// Re-hello interval.
    pub renew: Duration,
    /// Wait after a clock code.
    pub clock_retry: Duration,
    /// Jitter before redialing after a drain.
    pub drain_redial: fn() -> Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            backoff,
            renew: RENEW,
            clock_retry: CLOCK_RETRY,
            drain_redial: || {
                Duration::from_millis(1000 + (uuid::Uuid::new_v4().as_u128() % 4000) as u64)
            },
        }
    }
}

/// One edge's state machine (§5.9).
pub struct Client {
    /// Dials.
    pub dialer: Arc<dyn EdgeDialer>,
    /// Signs delegations.
    pub delegation: Arc<dyn DelegationSource>,
    /// Live ticket first, standby second (§5.5).
    // TODO(210-M4): fail over to the standby after 10 min unreachable and honour `welcome.retire`.
    pub tickets: Vec<ShareEdgeTicket>,
    /// The terms revision to send in `hello`, once accepted.
    pub terms_rev: watch::Receiver<Option<String>>,
    /// The plugin side.
    pub attachment: Arc<dyn Attachment>,
    /// Timings.
    pub timing: Timing,
}

enum Next {
    Dial,
    Idle,
    Blocked(String),
    Removed,
}

impl Client {
    fn set(&self, s: EdgeState, clock: Option<i64>) {
        self.attachment.state(&s, clock);
    }

    /// Run until `need` closes. `need` true = something needs the edge (§5.2); a change while
    /// `blocked` is the user action that retries.
    pub async fn run(self, mut need: watch::Receiver<bool>) {
        let mut attempt = 0u32;
        loop {
            self.set(EdgeState::Idle, None);
            if need.wait_for(|n| *n).await.is_err() {
                return;
            }
            loop {
                if !*need.borrow() {
                    break;
                }
                self.set(EdgeState::Dialing { attempt }, None);
                let next = tokio::select! {
                    n = self.session(&mut attempt) => n,
                    _ = need.wait_for(|n| !*n) => Next::Idle,
                };
                match next {
                    Next::Idle => break,
                    Next::Dial => {
                        let wait = (self.timing.backoff)(attempt);
                        attempt = attempt.saturating_add(1);
                        tokio::select! {
                            _ = tokio::time::sleep(wait) => {}
                            _ = need.wait_for(|n| !*n) => break,
                        }
                    }
                    Next::Blocked(reason) => {
                        self.set(EdgeState::Blocked { reason }, None);
                        if need.changed().await.is_err() {
                            return;
                        }
                        attempt = 0;
                    }
                    Next::Removed => {
                        self.set(EdgeState::Removed, None);
                        self.attachment.removed().await;
                        break;
                    }
                }
            }
        }
    }

    /// One connection, `dialing` through `attached`/`draining`.
    async fn session(&self, attempt: &mut u32) -> Next {
        let Some(ticket) = self.tickets.first() else {
            return Next::Blocked(crate::config::NO_SHARING_EDGE.into());
        };
        let conn = match self
            .dialer
            .dial(ticket.addr.clone(), share_proto::ALPNS)
            .await
        {
            Ok(c) => c,
            Err(e) => {
                tracing::debug!("share: dial {}: {e:#}", ticket.edge_id());
                return Next::Dial;
            }
        };
        let next = self.attached(ticket, conn.clone(), attempt).await;
        conn.close();
        self.attachment.lost();
        next
    }

    async fn attached(
        &self,
        ticket: &ShareEdgeTicket,
        conn: Arc<dyn EdgeConn>,
        attempt: &mut u32,
    ) -> Next {
        let control = Control::new(conn.clone());
        self.set(EdgeState::Info, None);
        let info = match control.info().await {
            Ok(i) => i,
            Err(e) if e.code() == Some(ErrorCode::ClientTooOld) => {
                return Next::Blocked("client_too_old".into());
            }
            Err(_) => return Next::Dial,
        };
        if info.edge_id != ticket.edge_id() {
            return Next::Blocked("the edge's key differs from the ticket".into());
        }
        let me = share_proto::version::client_agent(platform::version::get());
        if let Err(e) = compat(info.v, &info.min_client, &me) {
            return Next::Blocked(e.to_string());
        }
        let skew = share_proto::Delegation::clock_skew_ms(platform::time::now_ms(), info.now_ms);
        let clock = share_proto::Delegation::clock_off(platform::time::now_ms(), info.now_ms)
            .then_some(skew);

        // delegating → hello, repeated at renewal and after `delegation_expired`.
        let welcome = match self.hello(&control, &info, clock).await {
            Ok(w) => w,
            Err(n) => return n,
        };
        if let Err(e) = compat(welcome.v, &welcome.min_client, &me) {
            return Next::Blocked(e.to_string());
        }
        *attempt = 0;
        self.attachment.welcome(&control, &info, &welcome).await;
        let mut events = match control.events().await {
            Ok(rx) => rx,
            Err(_) => return Next::Dial,
        };
        if let Err(e) = control.put_routes(self.attachment.hosts()).await {
            tracing::warn!("share: PUT /v1/routes: {e}");
        }
        self.set(EdgeState::Attached, clock);

        let accept = {
            let (conn, att) = (conn.clone(), self.attachment.clone());
            tokio::spawn(async move {
                while let Some(s) = conn.accept().await {
                    att.visitor(s);
                }
            })
        };
        let _abort = AbortOnDrop(accept);
        let mut renew = tokio::time::interval_at(
            tokio::time::Instant::now() + self.timing.renew,
            self.timing.renew,
        );
        loop {
            tokio::select! {
                ev = events.recv() => match ev {
                    None => return Next::Dial,
                    Some(Event::Drain(d)) => {
                        self.set(EdgeState::Draining, clock);
                        tokio::time::sleep(Duration::from_secs(d.secs)).await;
                        tokio::time::sleep((self.timing.drain_redial)()).await;
                        return Next::Dial;
                    }
                    Some(Event::Kill(k)) if k.host.is_none() => match k.reason {
                        KillReason::DelegationExpired => {
                            if let Err(n) = self.hello(&control, &info, clock).await { return n; }
                            self.set(EdgeState::Attached, clock);
                        }
                        KillReason::Revoked => return Next::Removed,
                        // TODO(210-M2): mesh_rekeyed waits for the vault's new `mesh/share_key`.
                        _ => { self.attachment.event(&control, Event::Kill(k)).await; return Next::Dial; }
                    },
                    Some(ev) => self.attachment.event(&control, ev).await,
                },
                _ = renew.tick() => {
                    if let Err(n) = self.hello(&control, &info, clock).await { return n; }
                }
            }
        }
    }

    /// `delegating` + `hello` with the §5.9 error mapping.
    async fn hello(
        &self,
        control: &Control,
        info: &Info,
        clock: Option<i64>,
    ) -> Result<Welcome, Next> {
        self.set(EdgeState::Delegating, clock);
        let delegation = match self
            .delegation
            .delegate(info.edge_id, &info.domain, info.now_ms)
            .await
        {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("share: delegation: {e:#}");
                return Err(Next::Dial);
            }
        };
        self.set(EdgeState::Hello, clock);
        let hello = Hello {
            v: share_proto::PROTOCOL_MAJOR,
            delegation,
            invite: self.tickets.first().and_then(|t| t.invite),
            client: share_proto::version::client_agent(platform::version::get()),
            terms_rev: self.terms_rev.borrow().clone(),
        };
        match control.hello(&hello).await {
            Ok(w) => Ok(w),
            Err(e) => Err(match e.code() {
                Some(ErrorCode::DelegationNotYetValid | ErrorCode::DelegationExpired) => {
                    let now = match &e {
                        EdgeError::Http { body, .. } => body.now_ms.unwrap_or(info.now_ms),
                        EdgeError::Transport(_) => info.now_ms,
                    };
                    let off = share_proto::Delegation::clock_skew_ms(platform::time::now_ms(), now);
                    self.set(EdgeState::Hello, Some(off));
                    tokio::time::sleep(self.timing.clock_retry).await;
                    Next::Dial
                }
                Some(ErrorCode::Revoked) => Next::Removed,
                // TODO(210-M2): wait for the vault's new `mesh/share_key`, then delegate again.
                Some(ErrorCode::MeshRekeyed) => Next::Dial,
                Some(
                    ErrorCode::Internal
                    | ErrorCode::Unknown
                    | ErrorCode::RateLimited
                    | ErrorCode::Paced
                    | ErrorCode::EdgeFull,
                ) => Next::Dial,
                Some(code) => Next::Blocked(code.as_str().to_owned()),
                None => Next::Dial,
            }),
        }
    }
}

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
pub(crate) mod tests;
