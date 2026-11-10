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
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
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
        let live = c.shared.read().clone();
        live.staged
            .contacts
            .values()
            .find(|f| f.eid == remote)
            .map(|f| f.addrs.clone())
            .unwrap_or_default()
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
        anyhow::bail!("a stranger sent a request outside a ceremony");
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
    let svc = hyper::service::service_fn(move |mut req: hyper::Request<hyper::body::Incoming>| {
        let (shared, remote, ctx) = (shared.clone(), remote.clone(), ctx.clone());
        async move {
            if let Some(c) = &ctx
                && let Some(res) = contact_route(c, &remote, &req).await
            {
                return Ok::<_, std::convert::Infallible>(res);
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

/// 183 F1: the contact-only routes of [`serve_http`]; `None` for everything else.
async fn contact_route<B>(
    c: &Arc<Ctx>,
    remote: &str,
    req: &hyper::Request<B>,
) -> Option<axum::response::Response> {
    use axum::response::IntoResponse as _;
    match (req.method(), req.uri().path()) {
        (&hyper::Method::GET, super::offers::CARD_PATH) => {
            Some(match c.contact_card(remote).await {
                Ok(card) => (
                    [(hyper::header::CACHE_CONTROL, "private, no-store")],
                    axum::Json(card),
                )
                    .into_response(),
                Err(e) => {
                    tracing::warn!(%remote, "a2a: contact card: {e}");
                    hyper::StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            })
        }
        (&hyper::Method::POST, super::offers::NOTICE_PATH) => {
            c.offers_noticed(remote);
            Some(hyper::StatusCode::NO_CONTENT.into_response())
        }
        _ => None,
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
    /// The contact's endpoint id.
    pub eid: String,
    /// Dial hints.
    pub hints: Hints,
    /// 183 §7.7: the export to reach (`/agents/<export>/rpc`); `None` posts to `/rpc`,
    /// which the contact's gate maps to the first export it granted.
    pub export: Option<String>,
}

fn refused(e: impl std::fmt::Display) -> A2AError {
    A2AError::new(error_code::UNSUPPORTED_OPERATION, e.to_string())
}

impl IrohTransport {
    /// One JSON-RPC call on a fresh stream. A refusal at the gate (not a contact, no export
    /// granted) is `UNSUPPORTED_OPERATION`, so the sender records it as refused; anything
    /// after the request left is `INTERNAL_ERROR` (the outcome is unknown).
    async fn call(
        &self,
        params: &ServiceParams,
        method: &str,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, A2AError> {
        tokio::time::timeout(
            CALL_TIMEOUT,
            self.call_on(&self.net, params, method, payload),
        )
        .await
        .map_err(|_| A2AError::internal("no answer from the contact in time"))?
    }

    async fn call_on(
        &self,
        net: &mesh::net::Net,
        params: &ServiceParams,
        method: &str,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, A2AError> {
        let stream = net
            .a2a_stream(&self.eid, &self.hints.addrs, self.hints.relay.as_deref())
            .await
            .map_err(|e| refused(format!("contact unreachable: {e:#}")))?;
        let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(refused)?;
        tokio::spawn(async move {
            let _ = conn.await;
        });
        let id = JsonRpcId::String(uuid::Uuid::now_v7().to_string());
        let body = serde_json::to_vec(&JsonRpcRequest::new(id, method, Some(payload)))
            .map_err(|e| A2AError::internal(e.to_string()))?;
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
            .map_err(|e| A2AError::internal(e.to_string()))?;
        let resp = sender
            .send_request(req)
            .await
            .map_err(|e| A2AError::internal(format!("contact request: {e}")))?;
        match resp.status() {
            s if s.is_success() => {}
            hyper::StatusCode::UNAUTHORIZED => return Err(refused("not a contact there")),
            hyper::StatusCode::NOT_FOUND => {
                return Err(refused("the contact grants no export to this engine"));
            }
            s => return Err(A2AError::internal(format!("contact answered HTTP {s}"))),
        }
        let body = http_body_util::Limited::new(resp.into_body(), a2a_client::MAX_RESPONSE_BYTES)
            .collect()
            .await
            .map_err(|e| A2AError::internal(format!("contact response: {e}")))?
            .to_bytes();
        let rpc: JsonRpcResponse = serde_json::from_slice(&body)
            .map_err(|e| A2AError::internal(format!("contact response: {e}")))?;
        if let Some(err) = rpc.error {
            return Err(A2AError::new(err.code, err.message));
        }
        rpc.result
            .ok_or_else(|| A2AError::internal("JSON-RPC response missing result"))
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
