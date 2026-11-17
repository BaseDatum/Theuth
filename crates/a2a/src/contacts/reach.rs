//! 183 §7.2 (W-21): `theuth/a2a/1`, A2A between contacts over the iroh endpoint. Each
//! bidirectional stream carries one HTTP/1.1 request into the same axum router the
//! loopback listener serves — the caller is the stream's pinned endpoint id
//! ([`ContactCaller`]), never a bearer — or, when its first line is `{"pair": …}`, one
//! ceremony ([`super::ceremony`]). The mesh refuses family peers before anything here runs;
//! [`Gate::admit`] refuses everyone but live contacts and an open window's strangers.
//!
//! Outbound, [`IrohTransport`] is the SDK [`a2a_client::Transport`] of a contact: the
//! JSON-RPC binding (ProtoJSON payloads, as the SDK's own transport sends them) over a fresh
//! stream per call. Streaming calls are refused, so a follower polls `GetTask`.

use std::pin::Pin;
use std::sync::{Arc, Weak};
use std::task::{Context, Poll};
use std::time::Duration;

use a2a_client::{ServiceParams, Transport};
use a2a_types::*;
use async_trait::async_trait;
use futures::stream::BoxStream;
use http_body_util::BodyExt as _;
use hyper_util::rt::TokioIo;
use replica::ByteStream;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tower::ServiceExt as _;

use crate::Ctx;
use crate::server::{ContactCaller, IrohGate};

/// The first line of a stream: an HTTP request line, or a ceremony's `{"pair": …}` (which
/// carries a signed card, hence the room).
pub const LINE_MAX: usize = 64 * 1024;
/// A stream that sends nothing for this long is dropped.
pub const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// One inbound request's whole deadline (headers, body and response). ponytail: streaming
/// (SSE) is not carried to contacts yet (`no_streams`), so one deadline fits every call.
pub const HTTP_TIMEOUT: Duration = Duration::from_secs(60);
/// One outbound call's deadline (dial, request and response).
pub const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// How the contact pinned to `eid` is reached right now.
pub fn path(net: Option<&mesh::net::Net>, eid: &str) -> mesh::PeerPath {
    net.map_or(mesh::PeerPath::Offline, |n| n.a2a_path(eid))
}

/// Bytes already read from a stream, served before the rest of it.
pub(crate) struct Prefixed {
    head: Vec<u8>,
    pos: usize,
    inner: Box<dyn ByteStream>,
}

impl Prefixed {
    pub(crate) fn new(head: Vec<u8>, inner: Box<dyn ByteStream>) -> Self {
        Self {
            head,
            pos: 0,
            inner,
        }
    }
}

impl AsyncRead for Prefixed {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.pos < self.head.len() {
            let n = buf.remaining().min(self.head.len() - self.pos);
            let start = self.pos;
            buf.put_slice(&self.head[start..start + n]);
            self.pos += n;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut *self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for Prefixed {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut *self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.inner).poll_shutdown(cx)
    }
}

/// Read up to and including the first `\n`, at most [`LINE_MAX`] bytes.
pub(crate) async fn read_line(s: &mut (impl AsyncRead + Unpin)) -> std::io::Result<Vec<u8>> {
    let mut line = Vec::new();
    loop {
        let b = s.read_u8().await?;
        line.push(b);
        if b == b'\n' {
            return Ok(line);
        }
        if line.len() >= LINE_MAX {
            return Err(std::io::Error::other("first line too long"));
        }
    }
}

/// The a2a plugin's `theuth/a2a/1` handler on the mesh endpoint.
pub(crate) struct Gate(pub Weak<Ctx>);

impl mesh::net::ContactAlpn for Gate {
    fn admit(&self, remote: &str) -> bool {
        self.0
            .upgrade()
            .is_some_and(|c| c.gate(remote) != IrohGate::Refuse)
    }

    fn stream(
        &self,
        remote: String,
        stream: Box<dyn ByteStream>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let ctx = self.0.clone();
        Box::pin(async move {
            let Some(c) = ctx.upgrade() else { return };
            if let Err(e) = serve(c, remote.clone(), stream).await {
                tracing::debug!(%remote, "theuth/a2a/1 stream: {e:#}");
            }
        })
    }

    fn hints(&self, remote: &str) -> Vec<std::net::SocketAddr> {
        let Some(c) = self.0.upgrade() else {
            return Vec::new();
        };
        // 191 (D3): any machine of any contact this machine has addresses for (a roster's
        // other members, a record still bootstrapping), not only a staged contact's first.
        let live = c.shared.read().clone();
        live.staged.addrs.get(remote).cloned().unwrap_or_default()
    }
}

/// One inbound stream: a ceremony, or one HTTP request of a live contact.
pub(crate) async fn serve(
    c: Arc<Ctx>,
    remote: String,
    mut stream: Box<dyn ByteStream>,
) -> anyhow::Result<()> {
    let line = tokio::time::timeout(READ_TIMEOUT, read_line(&mut stream)).await??;
    if line.first() == Some(&b'{') {
        return super::ceremony::serve(c, remote, line, stream).await;
    }
    if !matches!(c.gate(&remote), IrohGate::Contact { .. }) {
        return refuse(&remote, stream).await;
    }
    // 199 (D2): `GET`/`POST /a2a/files/<id>` is served here, before `serve_http`: outside its
    // 60 s `HTTP_TIMEOUT`, `Route::of`, `forward::route` and the A2A rate limits, through the
    // agent's offers under the caller's transfers token (`files::route`).
    if super::files::is_route(&line) {
        return super::files::route(&c, &remote, &line, stream).await;
    }
    let shared = c.shared.clone();
    serve_http(
        shared,
        remote,
        Box::new(Prefixed::new(line, stream)),
        Some(c),
    )
    .await
}

/// What a caller that is no contact here reads (a stranger inside an open Add contact window,
/// or one refused since its stream opened): nothing about this machine's contacts or exports
/// beyond what the admitted stream already said.
pub const NOT_A_CONTACT: &str = "not a contact of this machine";

/// [`serve`]: an HTTP request from a caller the gate admits only for a ceremony is answered
/// `401` with [`NOT_A_CONTACT`], not dropped (the caller read a bare "connection error"),
/// and logged once per caller (the set forgets everyone past 1024). The rest of its head is
/// read first (at most 64 lines, within [`READ_TIMEOUT`]), so the answer follows its request.
async fn refuse(remote: &str, mut stream: Box<dyn ByteStream>) -> anyhow::Result<()> {
    let head = async {
        for _ in 0..64 {
            if matches!(&read_line(&mut stream).await?[..], b"\r\n" | b"\n") {
                break;
            }
        }
        std::io::Result::Ok(())
    };
    tokio::time::timeout(READ_TIMEOUT, head).await??;
    static SEEN: std::sync::LazyLock<parking_lot::Mutex<std::collections::HashSet<String>>> =
        std::sync::LazyLock::new(Default::default);
    let first = {
        let mut seen = SEEN.lock();
        if seen.len() >= 1024 {
            seen.clear();
        }
        seen.insert(remote.to_owned())
    };
    if first {
        tracing::info!(%remote, "theuth/a2a/1: refused a request: {NOT_A_CONTACT}");
    }
    let resp = format!(
        "HTTP/1.1 401 Unauthorized\r\ncontent-type: text/plain\r\ncontent-length: {}\r\n\
         connection: close\r\n\r\n{NOT_A_CONTACT}",
        NOT_A_CONTACT.len()
    );
    stream.write_all(resp.as_bytes()).await?;
    stream.shutdown().await?;
    Ok(())
}

/// One HTTP/1.1 request on `stream` into the router, as `remote`. 183 F1: with the engine's
/// context, a contact's `GET` [`super::offers::CARD_PATH`] is its own card (the exports
/// granted to it as skills) and `POST` [`super::offers::NOTICE_PATH`] makes this engine fetch
/// the contact's card again.
pub(crate) async fn serve_http(
    shared: crate::server::Shared,
    remote: String,
    stream: Box<dyn ByteStream>,
    ctx: Option<Arc<Ctx>>,
) -> anyhow::Result<()> {
    let svc = hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
        let (shared, remote, ctx) = (shared.clone(), remote.clone(), ctx.clone());
        async move {
            let mut req = req;
            if let Some(c) = &ctx {
                return Ok::<_, std::convert::Infallible>(match Route::of(&req) {
                    Some(route) => contact_route(c, &remote, route, req).await,
                    // 191 (D3): served here, or carried to the machine that owns it.
                    None => super::forward::route(c, &remote, req).await,
                });
            }
            req.extensions_mut().insert(ContactCaller(remote));
            crate::server::app(shared)
                .oneshot(req.map(axum::body::Body::new))
                .await
        }
    });
    let conn = hyper::server::conn::http1::Builder::new()
        .timer(hyper_util::rt::TokioTimer::new())
        .header_read_timeout(READ_TIMEOUT)
        .keep_alive(false)
        .serve_connection(TokioIo::new(stream), svc);
    // One request per stream: the whole exchange gets a deadline (a slow body or a
    // stalled response cannot hold the stream).
    tokio::time::timeout(HTTP_TIMEOUT, conn)
        .await
        .map_err(|_| anyhow::anyhow!("theuth/a2a/1 request exceeded {HTTP_TIMEOUT:?}"))??;
    Ok(())
}

/// 183 F1: the contact-only routes of [`serve_http`].
#[derive(Clone, Copy, PartialEq)]
enum Route {
    Card,
    Notice,
    Path,
}

impl Route {
    fn of<B>(req: &hyper::Request<B>) -> Option<Self> {
        match (req.method(), req.uri().path()) {
            (&hyper::Method::GET, super::offers::CARD_PATH) => Some(Self::Card),
            (&hyper::Method::POST, super::offers::NOTICE_PATH) => Some(Self::Notice),
            (&hyper::Method::GET, super::rosters::ROSTER_PATH) => Some(Self::Path),
            _ => None,
        }
    }
}

/// One contact-only request. Each runs under the contact's rate and concurrency bounds
/// (review 1). 191 (D1): `NOTICE_PATH` reads a members/v1 body of up to
/// [`super::rosters::NOTICE_BYTES_MAX`] (413 above it), and `GET`
/// [`super::rosters::ROSTER_PATH`] serves a proof path of the caller's own records.
async fn contact_route(
    c: &Arc<Ctx>,
    remote: &str,
    route: Route,
    req: hyper::Request<hyper::body::Incoming>,
) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    use http_body_util::BodyExt as _;
    let admitted = c.shared.read().admit_contact(remote);
    let _permit = match admitted {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    match route {
        Route::Card => match c.contact_card(remote).await {
            Ok(card) => (
                [(hyper::header::CACHE_CONTROL, "private, no-store")],
                axum::Json(card),
            )
                .into_response(),
            Err(e) => {
                tracing::warn!(%remote, "a2a: contact card: {}", super::offers::clip(&e));
                hyper::StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        },
        Route::Notice => {
            let max = super::rosters::NOTICE_BYTES_MAX;
            let declared = req
                .headers()
                .get(hyper::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<usize>().ok());
            if declared.is_some_and(|n| n > max) {
                return hyper::StatusCode::PAYLOAD_TOO_LARGE.into_response();
            }
            let Ok(body) = http_body_util::Limited::new(req.into_body(), max)
                .collect()
                .await
                .map(|b| b.to_bytes())
            else {
                return hyper::StatusCode::PAYLOAD_TOO_LARGE.into_response();
            };
            // Each copy is verified on its own: a body that does not parse, or copies that do
            // not verify, change nothing. The ingest runs here, under the contact's permit
            // (its rate and concurrency bounds), before the answer: nothing outlives the
            // request or queues past those bounds. The card is fetched again as before.
            let pushed = if body.is_empty() {
                None
            } else {
                serde_json::from_slice::<super::rosters::Carried>(&body)
                    .inspect_err(|e| tracing::debug!(%remote, "191: roster push not read: {e}"))
                    .ok()
            };
            if let Some(m) = pushed {
                c.ingest(&m, remote).await;
            }
            c.offers_noticed(remote);
            hyper::StatusCode::NO_CONTENT.into_response()
        }
        Route::Path => match c
            .roster_path(remote, req.uri().query().unwrap_or_default())
            .await
        {
            Ok(answer) => axum::Json(answer).into_response(),
            Err((status, why)) => (status, why).into_response(),
        },
    }
}

/// Where to dial a contact: its pinned id and the latest hints for it.
#[derive(Clone, Debug, Default)]
pub struct Hints {
    /// Direct addresses.
    pub addrs: Vec<std::net::SocketAddr>,
    /// Home relay URL.
    pub relay: Option<String>,
}

/// The SDK transport of one contact (§7.7): JSON-RPC over `theuth/a2a/1`.
pub struct IrohTransport {
    /// This engine's endpoint.
    pub net: Arc<mesh::net::Net>,
    /// The contact's endpoint id (dialled first).
    pub eid: String,
    /// Dial hints.
    pub hints: Hints,
    /// 183 §7.7: the export to reach (`/agents/<export>/rpc`); `None` posts to `/rpc`,
    /// which the contact's gate maps to the first export it granted.
    pub export: Option<String>,
    /// 191 (D3): the contact's other admitted machines, tried in turn when one refuses
    /// before admitting anything (unreachable, not a contact there, `NOT_OWNER`).
    pub members: Vec<String>,
    /// 191: the member that last answered (tried first: a follower's `GetTask` reaches the
    /// machine holding its task); shared by the contact's sends.
    pub answered: Arc<parking_lot::Mutex<Option<String>>>,
}

fn refused(e: impl std::fmt::Display) -> A2AError {
    A2AError::new(error_code::UNSUPPORTED_OPERATION, e.to_string())
}

/// 191 (D3): a member's gate refused this machine ("not a contact" there): its roster of us
/// does not list this machine yet (or it blocked or removed us). The sender's receipt reads
/// "<contact> has not accepted this machine yet".
pub const NOT_ACCEPTED: &str = "the contact has not accepted this machine yet";

/// A call's failure, and whether nothing was admitted by that member (another may be tried).
type Failed = (bool, A2AError);

impl IrohTransport {
    /// Whom to try, in order: the member that last answered, then one with a live
    /// connection, then the rest as given (the dialled one first).
    fn order(&self) -> Vec<String> {
        let mut all: Vec<String> = Vec::new();
        for e in std::iter::once(&self.eid).chain(&self.members) {
            if !all.contains(e) {
                all.push(e.clone());
            }
        }
        let last = self.answered.lock().clone();
        all.sort_by_key(|e| {
            (
                last.as_deref() != Some(e.as_str()),
                self.net.a2a_path(e) == mesh::PeerPath::Offline,
            )
        });
        all
    }

    /// One JSON-RPC call on a fresh stream, to the first member that admits it, each member
    /// tried at most once. A refusal at the gate (not a contact, no export granted) is
    /// `UNSUPPORTED_OPERATION`, so the sender records it as refused; anything after the
    /// request left is `INTERNAL_ERROR` (the outcome is unknown) and is never sent again
    /// elsewhere. 191 (D3, S4): a member answering `not owner` moves the call on; when every
    /// member refused and one of them said `not owner`, that error is the answer (never
    /// "unreachable" nor an internal error, the deadline included).
    pub(crate) async fn call(
        &self,
        params: &ServiceParams,
        method: &str,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, A2AError> {
        let not_owner = parking_lot::Mutex::new(None::<A2AError>);
        let go = async {
            let (mut last, mut not_accepted) = (None, false);
            for eid in self.order() {
                match self.call_on(&eid, params, method, payload.clone()).await {
                    Ok(v) => {
                        *self.answered.lock() = Some(eid);
                        return Ok(v);
                    }
                    Err((true, e)) => {
                        tracing::debug!(%eid, "a2a: contact member refused ({}); trying the next", e.message);
                        if e.code == agent::communication::network::NOT_OWNER_CODE {
                            *not_owner.lock() = Some(e.clone());
                        }
                        not_accepted |= e.message == NOT_ACCEPTED;
                        last = Some(e);
                    }
                    Err((false, e)) => return Err(e),
                }
            }
            // A `not owner` first, then a member that has not accepted this machine yet.
            let seen = not_owner.lock().take();
            Err(seen
                .or(not_accepted.then(|| refused(NOT_ACCEPTED)))
                .or(last)
                .unwrap_or_else(|| refused("contact unreachable")))
        };
        match tokio::time::timeout(CALL_TIMEOUT, go).await {
            Ok(r) => r,
            Err(_) => Err(not_owner
                .lock()
                .take()
                .unwrap_or_else(|| A2AError::internal("no answer from the contact in time"))),
        }
    }

    async fn call_on(
        &self,
        eid: &str,
        params: &ServiceParams,
        method: &str,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, Failed> {
        let net = &self.net;
        let hints = if eid == self.eid {
            self.hints.addrs.clone()
        } else {
            Vec::new()
        };
        // 191 (D3): a gate that refused this machine closed the connection "not a contact"
        // ([`gate_refused`], read from this call's own error, never from another connection);
        // nothing was served on it, so another member may be tried.
        let gone = |at_gate: bool, e: String| {
            (
                true,
                refused(if at_gate { NOT_ACCEPTED.to_owned() } else { e }),
            )
        };
        let stream = net
            .a2a_stream(eid, &hints, self.hints.relay.as_deref())
            .await
            .map_err(|e| {
                gone(
                    e.chain().any(gate_refused),
                    format!("contact unreachable: {e:#}"),
                )
            })?;
        let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|e| gone(gate_refused(&e), e.to_string()))?;
        tokio::spawn(async move {
            let _ = conn.await;
        });
        let after = |e: A2AError| (false, e);
        let id = JsonRpcId::String(uuid::Uuid::now_v7().to_string());
        let body = serde_json::to_vec(&JsonRpcRequest::new(id, method, Some(payload)))
            .map_err(|e| after(A2AError::internal(e.to_string())))?;
        let path = self
            .export
            .as_ref()
            .map_or_else(|| "/rpc".to_owned(), |e| format!("/agents/{e}/rpc"));
        let mut req = hyper::Request::post(path)
            .header(hyper::header::HOST, "theuth")
            .header(hyper::header::CONTENT_TYPE, "application/json");
        for (k, values) in params {
            for v in values {
                req = req.header(k.as_str(), v.as_str());
            }
        }
        let req = req
            .body(http_body_util::Full::new(bytes::Bytes::from(body)))
            .map_err(|e| after(A2AError::internal(e.to_string())))?;
        // 191 (D3, S4): the member's gate closed the connection as `not a contact` (its roster
        // of us does not list this machine yet, or it blocked or removed us): the request never
        // reached its router (nothing admitted there), so the next is tried.
        let resp = sender.send_request(req).await.map_err(|e| {
            if gate_refused(&e) {
                (true, refused(NOT_ACCEPTED))
            } else {
                after(A2AError::internal(format!("contact request: {e}")))
            }
        })?;
        match resp.status() {
            s if s.is_success() => {}
            // Refused at that member's gate or router: nothing was admitted there (a member
            // that does not publish the export, a chat not on it yet, refuses with 404).
            hyper::StatusCode::UNAUTHORIZED => return Err((true, refused("not a contact there"))),
            hyper::StatusCode::NOT_FOUND => {
                return Err((true, refused("the contact grants no export to this engine")));
            }
            s => {
                return Err(after(A2AError::internal(format!(
                    "contact answered HTTP {s}"
                ))));
            }
        }
        let body = http_body_util::Limited::new(resp.into_body(), a2a_client::MAX_RESPONSE_BYTES)
            .collect()
            .await
            .map_err(|e| after(A2AError::internal(format!("contact response: {e}"))))?
            .to_bytes();
        let rpc: JsonRpcResponse = serde_json::from_slice(&body)
            .map_err(|e| after(A2AError::internal(format!("contact response: {e}"))))?;
        if let Some(err) = rpc.error {
            // 191 (D3, S4): `not owner`: another of the contact's machines owns the session;
            // the next member is tried (P3 forwards instead).
            let next = err.code == agent::communication::network::NOT_OWNER_CODE;
            return Err((next, A2AError::new(err.code, err.message)));
        }
        rpc.result
            .ok_or_else(|| after(A2AError::internal("JSON-RPC response missing result")))
    }

    async fn typed<Req, Resp>(
        &self,
        params: &ServiceParams,
        method: &str,
        req: &Req,
    ) -> Result<Resp, A2AError>
    where
        Req: a2a_pb::protojson_conv::ProtoJsonPayload,
        Resp: a2a_pb::protojson_conv::ProtoJsonPayload,
    {
        let payload = a2a_pb::protojson_conv::to_value(req)
            .map_err(|e| A2AError::internal(format!("request as ProtoJSON: {e}")))?;
        let v = self.call(params, method, payload).await?;
        a2a_pb::protojson_conv::from_value(v)
            .map_err(|e| A2AError::internal(format!("result as ProtoJSON: {e}")))
    }
}

/// The connection under `e` was closed by the remote's gate ([`mesh::net::A2A_NOT_A_CONTACT`]);
/// an error that only says the stream broke (a reset, a timeout, our own close) is not.
fn gate_refused(e: &(dyn std::error::Error + 'static)) -> bool {
    use iroh::endpoint::{ConnectionError, ReadError, WriteError};
    let lost = e
        .downcast_ref::<ConnectionError>()
        .or_else(|| match e.downcast_ref::<WriteError>() {
            Some(WriteError::ConnectionLost(c)) => Some(c),
            _ => None,
        })
        .or_else(|| match e.downcast_ref::<ReadError>() {
            Some(ReadError::ConnectionLost(c)) => Some(c),
            _ => None,
        });
    if let Some(ConnectionError::ApplicationClosed(c)) = lost {
        return c.reason[..] == *mesh::net::A2A_NOT_A_CONTACT;
    }
    // `io::Error::source` skips the error it wraps: look inside it first.
    let wrapped = e
        .downcast_ref::<std::io::Error>()
        .and_then(std::io::Error::get_ref)
        .map(|w| w as &(dyn std::error::Error + 'static));
    wrapped.or_else(|| e.source()).is_some_and(gate_refused)
}

fn no_streams() -> A2AError {
    A2AError::unsupported_operation("streaming is not carried to contacts yet; poll GetTask")
}

#[async_trait]
impl Transport for IrohTransport {
    async fn send_message(
        &self,
        params: &ServiceParams,
        req: &SendMessageRequest,
    ) -> Result<SendMessageResponse, A2AError> {
        self.typed(params, methods::SEND_MESSAGE, req).await
    }

    async fn send_streaming_message(
        &self,
        _params: &ServiceParams,
        _req: &SendMessageRequest,
    ) -> Result<BoxStream<'static, Result<StreamResponse, A2AError>>, A2AError> {
        Err(no_streams())
    }

    async fn get_task(
        &self,
        params: &ServiceParams,
        req: &GetTaskRequest,
    ) -> Result<Task, A2AError> {
        self.typed(params, methods::GET_TASK, req).await
    }

    async fn list_tasks(
        &self,
        params: &ServiceParams,
        req: &ListTasksRequest,
    ) -> Result<ListTasksResponse, A2AError> {
        self.typed(params, methods::LIST_TASKS, req).await
    }

    async fn cancel_task(
        &self,
        params: &ServiceParams,
        req: &CancelTaskRequest,
    ) -> Result<Task, A2AError> {
        self.typed(params, methods::CANCEL_TASK, req).await
    }

    async fn subscribe_to_task(
        &self,
        _params: &ServiceParams,
        _req: &SubscribeToTaskRequest,
    ) -> Result<BoxStream<'static, Result<StreamResponse, A2AError>>, A2AError> {
        Err(no_streams())
    }

    async fn create_push_config(
        &self,
        _params: &ServiceParams,
        _req: &TaskPushNotificationConfig,
    ) -> Result<TaskPushNotificationConfig, A2AError> {
        Err(A2AError::push_notification_not_supported())
    }

    async fn get_push_config(
        &self,
        _params: &ServiceParams,
        _req: &GetTaskPushNotificationConfigRequest,
    ) -> Result<TaskPushNotificationConfig, A2AError> {
        Err(A2AError::push_notification_not_supported())
    }

    async fn list_push_configs(
        &self,
        _params: &ServiceParams,
        _req: &ListTaskPushNotificationConfigsRequest,
    ) -> Result<ListTaskPushNotificationConfigsResponse, A2AError> {
        Err(A2AError::push_notification_not_supported())
    }

    async fn delete_push_config(
        &self,
        _params: &ServiceParams,
        _req: &DeleteTaskPushNotificationConfigRequest,
    ) -> Result<(), A2AError> {
        Err(A2AError::push_notification_not_supported())
    }

    async fn get_extended_agent_card(
        &self,
        _params: &ServiceParams,
        _req: &GetExtendedAgentCardRequest,
    ) -> Result<AgentCard, A2AError> {
        Err(A2AError::unsupported_operation(
            "extended agent card not supported",
        ))
    }

    async fn destroy(&self) -> Result<(), A2AError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A caller that is no contact here reads a real `401` and why, not a dropped stream.
    #[tokio::test]
    async fn a_non_contact_reads_401_and_why_not_a_dropped_stream() {
        let (client, server) = tokio::io::duplex(4096);
        tokio::spawn(refuse("stranger", Box::new(server)));
        let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(client))
            .await
            .unwrap();
        tokio::spawn(conn);
        let req = hyper::Request::get(super::super::offers::CARD_PATH)
            .header(hyper::header::HOST, "theuth")
            .body(http_body_util::Empty::<bytes::Bytes>::new())
            .unwrap();
        let resp = sender.send_request(req).await.unwrap();
        assert_eq!(resp.status(), hyper::StatusCode::UNAUTHORIZED);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], NOT_A_CONTACT.as_bytes());
    }
}
