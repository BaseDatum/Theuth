//! RPC plugin. Methods are `ask`s on named actors; `subscribe_events` streams
//! every wire-visible bus event as `{"type": ..., ...}`. See design doc §7.
//!
//! [`api::Api`] is the single list of harness commands. In-process callers
//! (the TUI) hold an [`api::Server`] from [`api::Server::new`] and call its
//! [`ApiServer`] methods directly; `serve` exposes the same trait over the
//! socket for remote clients.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures::future::{self, Either, Ready, ready};
use jsonrpsee::Methods;
use jsonrpsee::core::BoxError;
use jsonrpsee::server::{
    HttpBody, HttpRequest, HttpResponse, ServerBuilder, ServerConfig, serve_with_graceful_shutdown,
    stop_channel,
};
use runtime::Runtime;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tower::Service;

pub mod acp;
pub mod api;
pub mod bridge;
pub mod client;
pub mod engine;
pub mod forward;
pub mod landing;
pub mod pathac;
pub mod search;

#[cfg(test)]
mod export;

pub use self::search::SearchToolsPlugin;
pub use acp::AcpPlugin;
pub use api::{ApiClient, ApiServer};

/// `[rpc]` config section.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RpcConfig {
    /// Bind address; loopback only by design.
    pub listen: String,
}

impl Default for RpcConfig {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:7777".into(),
        }
    }
}

/// 130 E4: the client a connection declared with `X-Theuth-Client` on the
/// WebSocket upgrade — `desktop | tui | ios | companion`, `desktop` when the
/// header is absent or unknown. [`Auth`] puts it in every admitted request's
/// extensions beside the peer, so each call on the socket can stamp
/// `ActivityEntry.actor = user@<client>` ([`Client::actor`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Client(pub &'static str);

impl Client {
    /// The header name.
    pub const HEADER: &'static str = "x-theuth-client";
    /// What an absent or unknown header means.
    pub const DEFAULT: Client = Client("desktop");
    const KNOWN: [&'static str; 4] = ["desktop", "tui", "ios", "companion"];

    /// The header's value when it names a known client, else [`Self::DEFAULT`].
    pub fn parse(value: Option<&str>) -> Self {
        value
            .map(str::trim)
            .and_then(|v| Self::KNOWN.iter().find(|k| k.eq_ignore_ascii_case(v)))
            .map_or(Self::DEFAULT, |k| Client(k))
    }

    /// `user@<client>` for the connection behind `ext` (`user@desktop` when
    /// the call did not come over the socket, e.g. the in-process TUI).
    pub fn actor(ext: &jsonrpsee::Extensions) -> String {
        format!("user@{}", ext.get::<Client>().unwrap_or(&Self::DEFAULT).0)
    }
}

/// The plugin. Reads `<data_dir>/token` (creating it) and requires it as
/// `Authorization: Bearer <token>` on the WebSocket upgrade.
pub struct RpcPlugin {
    /// 63 §6.4: the server nobody asked for — the engine behind a `t`, `-p`
    /// or `--acp` without `--serve` — so every engine is attachable. Binds
    /// `rpc.listen` when the config sets one, else an ephemeral loopback
    /// port; a busy configured port falls back to ephemeral with a warning
    /// instead of failing the start. `false` = `--serve`/`--headless`: the
    /// configured address, bind failure fatal, as before.
    pub implicit: bool,
}

/// Any loopback port.
const EPHEMERAL: &str = "127.0.0.1:0";

#[async_trait::async_trait]
impl runtime::Plugin for RpcPlugin {
    fn name(&self) -> &'static str {
        "rpc"
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        let mut cfg: RpcConfig = rt.section("rpc").unwrap_or_default();
        if self.implicit && configured_listen(rt).is_none() {
            cfg.listen = EPHEMERAL.to_owned();
        }
        let addr = match serve(rt, cfg.clone()).await {
            Err(e) if self.implicit && cfg.listen != EPHEMERAL => {
                tracing::warn!(listen = %cfg.listen, error = %e, "rpc.listen busy; binding an ephemeral port");
                cfg.listen = EPHEMERAL.to_owned();
                serve(rt, cfg).await?
            }
            r => r?,
        };
        engine::set_listen(addr.to_string());
        // The link plugin dispatches decrypted device requests into our
        // methods and serves ACP over mode-1 conns (32 §4.3); it registers
        // in its own start, so hand it the dispatch once it is there.
        let link_rt = rt.clone();
        tokio::spawn(async move {
            let rt = link_rt;
            let methods = methods(rt.clone()).await;
            if let Some(link) = rt.lookup_when::<link::LinkActor>(link::LINK_ACTOR).await {
                let acp: link::AcpServe = Arc::new(|rt, ch: link::RecordChannel| {
                    Box::pin(acp::serve_records(rt, ch.inbound, ch.outbound))
                });
                let _ = link.tell(link::SetDispatch { methods, acp }).await;
            }
        });
        // 67 §4.2: the same ACP over the mesh's `theuth/acp/1` streams, and
        // mesh engines registered with `agent` as nodes.
        tokio::spawn(acp::serve_mesh(rt.clone()));
        // 39 §3.4: how many frontends hold a receiver; the updater only
        // restarts a service daemon when this is 0.
        let rt = rt.clone();
        tokio::spawn(async move {
            let tx = bridge::start(&rt).await;
            if let Some(u) = rt
                .lookup_when::<update::UpdateActor>(update::UPDATE_ACTOR)
                .await
            {
                let _ = u
                    .tell(update::SetAttached(Arc::new(move || tx.receiver_count())))
                    .await;
            }
        });
        Ok(())
    }
}

/// `rpc.listen` as the config (file or `THEUTH_RPC__LISTEN`) sets it, `None`
/// when it only comes from [`RpcConfig::default`].
pub fn configured_listen(rt: &Runtime) -> Option<String> {
    rt.config.find_value("rpc.listen").ok()?.into_string()
}

/// The full [`api::Api`] as a jsonrpsee `Methods` object, fed by the shared
/// [`bridge`]: what `serve` binds to the socket and what the link plugin
/// dispatches decrypted requests into with `raw_json_request` (31 §6.2).
pub async fn methods(rt: Runtime) -> jsonrpsee::Methods {
    let events = bridge::start(&rt).await;
    let mut server = api::Server::new(rt);
    server.events = events;
    server.into_rpc().into()
}

/// Start the bridge and the server; returns the bound address. The server
/// stops when `rt.shutdown` is cancelled.
pub async fn serve(rt: &Runtime, cfg: RpcConfig) -> anyhow::Result<SocketAddr> {
    let token_path = rt.data_dir.join("token");
    let token = load_token(&token_path)?;
    let methods = methods(rt.clone()).await;
    let listener = TcpListener::bind(cfg.listen.as_str()).await?;
    let addr = listener.local_addr()?;
    tracing::info!(%addr, token = %token_path.display(), "rpc listening");
    let shutdown = rt.shutdown.clone();
    tokio::spawn(accept(listener, methods, token, async move {
        shutdown.cancelled().await;
    }));
    Ok(addr)
}

/// Accept loop (jsonrpsee's low-level tower API). Each TCP connection is
/// one [`link::RpcPeer`]: [`Auth`] puts it in every admitted request's
/// extensions, and jsonrpsee copies the upgrade request's extensions into
/// every call on the resulting WebSocket, so all calls on one socket see
/// the same peer and two sockets never do. The peer's guard is owned by the
/// connection task here — never by request state — and dropped once the
/// hyper connection is over *and* jsonrpsee's session-closed signal fired,
/// which it does on disconnect without waiting for in-flight calls (or at
/// once when the connection never became a WebSocket). A call awaiting
/// [`link::RpcPeer::closed`] therefore cannot hold its own peer open. On
/// server stop jsonrpsee drains in-flight calls first, so the peer is
/// closed at stop, before that drain.
async fn accept(
    listener: TcpListener,
    methods: Methods,
    token: Arc<str>,
    shutdown: impl Future<Output = ()>,
) {
    let (stop_handle, server) = stop_channel();
    // One WebSocket per ACP child's MCP shim plus every UI and `t` CLI call:
    // jsonrpsee's default 100 refuses the ~95th child with 429, silently.
    let config = ServerConfig::builder().max_connections(4096).build();
    let services = ServerBuilder::new().set_config(config).to_service_builder();
    tokio::pin!(shutdown);
    loop {
        let sock = tokio::select! {
            r = listener.accept() => match r {
                Ok((sock, _)) => sock,
                Err(e) => { tracing::debug!("rpc accept: {e}"); continue }
            },
            _ = &mut shutdown => break,
        };
        let mut svc = services.clone().build(methods.clone(), stop_handle.clone());
        let session_closed = svc.on_session_closed();
        let (peer, guard) = link::RpcPeer::open();
        let auth = Auth {
            inner: svc,
            token: token.clone(),
            peer,
        };
        let stopped = stop_handle.clone().shutdown();
        let stop = stop_handle.clone().shutdown();
        tokio::spawn(async move {
            let closed = async {
                if let Err(e) = serve_with_graceful_shutdown(sock, auth, stopped).await {
                    tracing::debug!("rpc conn: {e}");
                }
                session_closed.await;
            };
            tokio::pin!(closed, stop);
            match future::select(closed, stop).await {
                Either::Left(_) => drop(guard),
                // Server stop: jsonrpsee drains in-flight calls before it
                // closes the session, so close the peer now, then let it drain.
                Either::Right((_, closed)) => {
                    drop(guard);
                    closed.await;
                }
            }
        });
    }
    let _ = server.stop();
}

/// Read the token, or create it (64 hex chars, owner-only).
fn load_token(path: &Path) -> anyhow::Result<Arc<str>> {
    if let Ok(t) = std::fs::read_to_string(path) {
        let t = t.trim();
        if !t.is_empty() {
            return Ok(t.into());
        }
    }
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    platform::files::write_private(path, token.as_bytes())?;
    Ok(token.into())
}

/// Per-connection HTTP middleware: 401 unless `Authorization: Bearer <token>`
/// or `?token=<token>`; an admitted request carries the connection's
/// [`link::RpcPeer`] in its extensions.
#[derive(Clone)]
struct Auth<S> {
    inner: S,
    token: Arc<str>,
    peer: link::RpcPeer,
}

impl<S> Auth<S> {
    fn authorized<B>(&self, req: &HttpRequest<B>) -> bool {
        let token = &*self.token;
        let header = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some_and(|t| t == token);
        // ponytail: plain `==`; loopback-only bind, timing side channel not in the threat model.
        header
            || req.uri().query().is_some_and(|q| {
                q.split('&')
                    .any(|kv| kv.strip_prefix("token=") == Some(token))
            })
    }
}

impl<S, B> Service<HttpRequest<B>> for Auth<S>
where
    S: Service<HttpRequest<B>, Response = HttpResponse, Error = BoxError>,
{
    type Response = HttpResponse;
    type Error = BoxError;
    type Future = Either<Ready<Result<HttpResponse, BoxError>>, S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: HttpRequest<B>) -> Self::Future {
        if self.authorized(&req) {
            let client = Client::parse(
                req.headers()
                    .get(Client::HEADER)
                    .and_then(|v| v.to_str().ok()),
            );
            req.extensions_mut().insert(self.peer.clone());
            req.extensions_mut().insert(client);
            Either::Right(self.inner.call(req))
        } else {
            let resp = HttpResponse::builder()
                .status(401)
                .body(HttpBody::from("unauthorized"))
                .unwrap_or_default();
            Either::Left(ready(Ok(resp)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonrpsee::core::client::ClientT;
    use jsonrpsee::rpc_params;
    use jsonrpsee::ws_client::{HeaderMap, HeaderValue, WsClient, WsClientBuilder};
    use link::RpcPeer;
    use tokio::sync::mpsc;

    async fn client(addr: SocketAddr) -> WsClient {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer t"));
        WsClientBuilder::default()
            .set_headers(headers)
            .build(format!("ws://{addr}"))
            .await
            .unwrap()
    }

    /// `(id, closed)` of the calling peer.
    async fn peer(c: &WsClient) -> (u64, bool) {
        c.request("peer", rpc_params![]).await.unwrap()
    }

    /// A server on an ephemeral port with `peer` → `(id, closed)` and
    /// `wait_closed`, which reports `("waiting", id)`, then `("closed", id)`
    /// once its peer closed.
    async fn probe(
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> (SocketAddr, mpsc::UnboundedReceiver<(&'static str, u64)>) {
        let (tx, rx) = mpsc::unbounded_channel::<(&'static str, u64)>();
        let mut module = jsonrpsee::RpcModule::new(tx);
        module
            .register_method("peer", |_, _, ext| {
                ext.get::<RpcPeer>().map(|p| (p.id(), p.is_closed()))
            })
            .unwrap();
        module
            .register_method("actor", |_, _, ext| Client::actor(ext))
            .unwrap();
        module
            .register_async_method("wait_closed", |_, tx, ext| async move {
                let peer = ext.get::<RpcPeer>().cloned().expect("authenticated");
                let _ = tx.send(("waiting", peer.id()));
                peer.closed().await;
                let _ = tx.send(("closed", peer.id()));
                peer.id()
            })
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(accept(listener, module.into(), "t".into(), shutdown));
        (addr, rx)
    }

    /// Start `wait_closed` on `c` and return once the server reports it waiting.
    async fn start_waiting(
        c: &WsClient,
        rx: &mut mpsc::UnboundedReceiver<(&'static str, u64)>,
        id: u64,
    ) {
        tokio::select! {
            r = c.request::<u64, _>("wait_closed", rpc_params![]) => panic!("returned early: {r:?}"),
            s = rx.recv() => assert_eq!(s, Some(("waiting", id))),
        }
    }

    async fn next_closed(
        rx: &mut mpsc::UnboundedReceiver<(&'static str, u64)>,
    ) -> Option<(&'static str, u64)> {
        tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("peer closed in time")
    }

    #[tokio::test]
    async fn peer_per_socket_closes_on_disconnect() {
        let (addr, mut rx) = probe(std::future::pending()).await;
        let a = client(addr).await;
        let b = client(addr).await;
        let (ida, _) = peer(&a).await;
        let (idb, _) = peer(&b).await;
        assert_ne!(ida, idb);
        assert_eq!(peer(&a).await, (ida, false));

        // A call on `a` waits for its own peer to close; that must not keep
        // the peer open once the socket is gone.
        start_waiting(&a, &mut rx, ida).await;
        drop(a);
        assert_eq!(next_closed(&mut rx).await, Some(("closed", ida)));
        // The other socket is untouched.
        assert_eq!(peer(&b).await, (idb, false));
    }

    #[tokio::test]
    async fn server_stop_closes_peers() {
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let (addr, mut rx) = probe(async move {
            let _ = stop_rx.await;
        })
        .await;
        let a = client(addr).await;
        let (ida, _) = peer(&a).await;
        start_waiting(&a, &mut rx, ida).await;
        // Client still connected: stop alone must close the peer, or the
        // waiting call would block the server's graceful shutdown.
        stop_tx.send(()).unwrap();
        assert_eq!(next_closed(&mut rx).await, Some(("closed", ida)));
    }

    /// 130 E4: `X-Theuth-Client: companion` on the upgrade stamps every call
    /// on that socket `user@companion`; no header (or an unknown one) is `user@desktop`.
    #[tokio::test]
    async fn client_header_stamps_the_connection() {
        let (addr, _rx) = probe(std::future::pending()).await;
        let with = |value: &'static str| async move {
            let mut headers = HeaderMap::new();
            headers.insert("authorization", HeaderValue::from_static("Bearer t"));
            headers.insert("x-theuth-client", HeaderValue::from_static(value));
            WsClientBuilder::default()
                .set_headers(headers)
                .build(format!("ws://{addr}"))
                .await
                .unwrap()
        };
        let actor = |c: WsClient| async move {
            let a: String = c.request("actor", rpc_params![]).await.unwrap();
            let again: String = c.request("actor", rpc_params![]).await.unwrap();
            assert_eq!(a, again, "per connection, not per call");
            a
        };
        assert_eq!(actor(with("companion").await).await, "user@companion");
        assert_eq!(actor(with("TUI").await).await, "user@tui");
        assert_eq!(actor(with("toaster").await).await, "user@desktop");
        assert_eq!(actor(client(addr).await).await, "user@desktop");
        assert_eq!(
            Client::actor(&jsonrpsee::Extensions::new()),
            "user@desktop",
            "in-process callers"
        );
    }

    #[tokio::test]
    async fn unauthorized_upgrade_is_rejected() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(accept(
            listener,
            jsonrpsee::RpcModule::new(()).into(),
            "t".into(),
            std::future::pending(),
        ));
        assert!(
            WsClientBuilder::default()
                .build(format!("ws://{addr}"))
                .await
                .is_err()
        );
    }
}
