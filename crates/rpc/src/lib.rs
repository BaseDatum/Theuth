//! Generic JSON-RPC transport, connection authentication and engine discovery.
//! Owners supply method modules through [`rpc_core::ModuleFactory`].

#[cfg(test)]
use std::net::SocketAddr;
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


use rpc_core::{Client, ConnId};


use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tower::Service;

/// Transport-only methods and typed clients.
pub mod infra;
pub mod client;
pub mod engine;

mod diag;
mod wslog;

/// Assemble transport-only RPC methods.
pub fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    infra::rpc_module(server)
}

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





/// `rpc.listen` as the config (file or `THEUTH_RPC__LISTEN`) sets it, `None`
/// when it only comes from [`RpcConfig::default`].
pub fn configured_listen(rt: &Runtime) -> Option<String> {
    rt.config.find_value("rpc.listen").ok()?.into_string()
}



/// Accept loop (jsonrpsee's low-level tower API). Each TCP connection is
/// one [`rpc_core::RpcPeer`]: [`Auth`] puts it in every admitted request's
/// extensions, and jsonrpsee copies the upgrade request's extensions into
/// every call on the resulting WebSocket, so all calls on one socket see
/// the same peer and two sockets never do. The peer's guard is owned by the
/// connection task here — never by request state — and dropped once the
/// hyper connection is over *and* jsonrpsee's session-closed signal fired,
/// which it does on disconnect without waiting for in-flight calls (or at
/// once when the connection never became a WebSocket). A call awaiting
/// [`rpc_core::RpcPeer::closed`] therefore cannot hold its own peer open. On
/// server stop jsonrpsee drains in-flight calls first, so the peer is
/// closed at stop, before that drain.
pub async fn accept(
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
    let mut consecutive_errors: u32 = 0;
    loop {
        let (sock, addr) = tokio::select! {
            r = listener.accept() => match r {
                Ok(accepted) => {
                    consecutive_errors = 0;
                    accepted
                }
                Err(e) => {
                    if is_fatal_accept_error(&e) {
                        tracing::warn!("rpc accept fatal ({e}); exiting accept loop");
                        break;
                    }
                    consecutive_errors = consecutive_errors.saturating_add(1);
                    #[cfg(target_os = "ios")]
                    if consecutive_errors >= 5 {
                        tracing::warn!("rpc accept repeated errors ({e}); exiting defunct iOS accept loop");
                        break;
                    }
                    tracing::debug!(consecutive_errors, "rpc accept: {e}");
                    // Back off briefly so transient errors (e.g. EMFILE) or revoked mobile sockets never spin 100% CPU.
                    tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => continue,
                        _ = &mut shutdown => break,
                    }
                }
            },
            _ = &mut shutdown => break,
        };
        let conn = Arc::new(wslog::Conn::new(addr));
        let mut svc = services.clone().build(methods.clone(), stop_handle.clone());
        let session_closed = svc.on_session_closed();
        let (peer, guard) = rpc_core::RpcPeer::open();
        let auth = Auth {
            inner: svc,
            token: token.clone(),
            peer,
            conn: conn.clone(),
        };
        let sock = wslog::Watched::new(sock, conn.clone());
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
                Either::Left(_) => {
                    drop(guard);
                    conn.log_closed(false);
                }
                // Server stop: jsonrpsee drains in-flight calls before it
                // closes the session, so close the peer now, then let it drain.
                Either::Right((_, closed)) => {
                    drop(guard);
                    closed.await;
                    conn.log_closed(true);
                }
            }
        });
    }
    let _ = server.stop();
}

/// Errors that mean the listening socket descriptor itself is closed or revoked (for example when
/// iOS reclaims a listening socket on background suspension) and retrying `accept()` will never
/// succeed.
fn is_fatal_accept_error(err: &std::io::Error) -> bool {
    if matches!(
        err.kind(),
        std::io::ErrorKind::InvalidInput | std::io::ErrorKind::NotConnected
    ) {
        return true;
    }
    #[cfg(unix)]
    if let Some(code) = err.raw_os_error() {
        // EBADF (9), EINVAL (22), ENOTSOCK (38 macOS/iOS, 88 Linux), ENOTCONN (57 macOS/iOS, 107 Linux)
        if matches!(code, 9 | 22 | 38 | 57 | 88 | 107) {
            return true;
        }
    }
    false
}



/// Per-connection HTTP middleware: 401 unless `Authorization: Bearer <token>`
/// or `?token=<token>`; an admitted request carries the connection's
/// [`rpc_core::RpcPeer`] and [`ConnId`] in its extensions, and an admitted upgrade
/// logs the connection's `rpc: ws open` line.
#[derive(Clone)]
struct Auth<S> {
    inner: S,
    token: Arc<str>,
    peer: rpc_core::RpcPeer,
    conn: Arc<wslog::Conn>,
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
            if jsonrpsee::server::ws::is_upgrade_request(&req) {
                self.conn.log_open(req.uri().query());
            }
            req.extensions_mut().insert(self.peer.clone());
            req.extensions_mut().insert(client);
            req.extensions_mut().insert(ConnId(self.conn.id));
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
    use rpc_core::RpcPeer;
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
                // Detached like production cleanup: jsonrpsee cancels the call
                // itself once its socket is gone.
                let watch = peer.clone();
                tokio::spawn(async move {
                    watch.closed().await;
                    let _ = tx.send(("closed", watch.id()));
                });
                peer.closed().await;
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
