//! The visitor path on the hosting engine (210 §4.2 steps 4–5): a bi-stream the edge opened
//! carries a PROXY v2 preamble (`ppp`), then raw TLS that this engine terminates with the name's
//! `CertifiedKey` (`tokio-rustls`, ALPN `h2`/`http/1.1`), served by `hyper-util`'s auto builder
//! into [`crate::proxy::Proxy`].

use std::collections::HashMap;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use share_proto::proxy::{TLV_AUTHORITY, TLV_CONN_ID, conn_id_from};
use tokio::io::{AsyncRead, AsyncReadExt as _};

use crate::proxy::Proxy;

/// Largest PROXY v2 payload accepted (addresses + our two TLVs fit easily).
pub const PREAMBLE_MAX: usize = 1024;
/// Time to read the preamble.
pub const PREAMBLE_TIMEOUT: Duration = Duration::from_secs(5);
/// Time a visitor has to send a complete request head (slowloris), on HTTP/1.1 and HTTP/2: counted
/// from the moment its connection has no request in flight (accept, or the last response done).
pub const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest request head accepted (HTTP/1.1 read buffer, HTTP/2 header list).
pub const MAX_HEADER_BYTES: usize = 64 * 1024;
/// Most HTTP/2 streams one visitor connection may have open at once.
pub const MAX_STREAMS: u32 = 100;
/// After the head deadline's graceful close (GOAWAY, or no keep-alive on HTTP/1.1), time a
/// connection that still has no request in flight gets before it is dropped.
pub const CLOSE_GRACE: Duration = Duration::from_secs(2);

/// What the edge told us of the visitor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preamble {
    /// Visitor address.
    pub visitor: SocketAddr,
    /// SNI (`PP2_TYPE_AUTHORITY`).
    pub sni: Option<String>,
    /// The edge's connection id (TLV `0xE0`).
    pub conn_id: Option<u64>,
}

/// Read exactly one PROXY v2 header from `s`, leaving the TLS bytes unread.
pub async fn read_preamble<S: AsyncRead + Unpin>(s: &mut S) -> anyhow::Result<Preamble> {
    let mut head = [0u8; 16];
    s.read_exact(&mut head).await?;
    if head[..12] != ppp::v2::PROTOCOL_PREFIX[..] {
        anyhow::bail!("not a PROXY v2 preamble");
    }
    let len = u16::from_be_bytes([head[14], head[15]]) as usize;
    if len > PREAMBLE_MAX {
        anyhow::bail!("PROXY v2 preamble too long ({len})");
    }
    let mut buf = head.to_vec();
    buf.resize(16 + len, 0);
    s.read_exact(&mut buf[16..]).await?;
    let h = ppp::v2::Header::try_from(&buf[..]).map_err(|e| anyhow::anyhow!("PROXY v2: {e:?}"))?;
    let visitor = match h.addresses {
        ppp::v2::Addresses::IPv4(a) => SocketAddr::from((a.source_address, a.source_port)),
        // HAProxy on a dual-stack socket reports IPv4 visitors v4-mapped: keep them IPv4.
        ppp::v2::Addresses::IPv6(a) => {
            SocketAddr::new(IpAddr::V6(a.source_address).to_canonical(), a.source_port)
        }
        _ => SocketAddr::from((IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)),
    };
    let (mut sni, mut conn_id) = (None, None);
    for tlv in h.tlvs() {
        let tlv = tlv.map_err(|e| anyhow::anyhow!("PROXY v2 TLV: {e:?}"))?;
        match tlv.kind {
            TLV_AUTHORITY => sni = std::str::from_utf8(&tlv.value).ok().map(str::to_owned),
            TLV_CONN_ID => conn_id = conn_id_from(&tlv.value),
            _ => {}
        }
    }
    Ok(Preamble {
        visitor,
        sni,
        conn_id,
    })
}

/// Name certificates by `<name>.D`, swapped whole on issuance or renewal.
#[derive(Default)]
pub struct Certs {
    by_name: ArcSwap<HashMap<String, Arc<CertifiedKey>>>,
}

impl fmt::Debug for Certs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Certs")
            .field("names", &self.by_name.load().keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Certs {
    /// Install `<name>.D`'s pair after checking the chain's key is the vault row's (§6.2 step 5).
    pub fn install(&self, name_host: &str, chain_pem: &str, key_pem: &str) -> anyhow::Result<()> {
        let ck = certified_key(chain_pem, key_pem)?;
        let host = name_host.to_ascii_lowercase();
        self.by_name.rcu(|cur| {
            let mut next = (**cur).clone();
            next.insert(host.clone(), Arc::new(ck.clone()));
            next
        });
        Ok(())
    }

    /// Drop a name.
    pub fn remove(&self, name_host: &str) {
        let host = name_host.to_ascii_lowercase();
        self.by_name.rcu(|cur| {
            let mut next = (**cur).clone();
            next.remove(&host);
            next
        });
    }

    /// Exact `<name>.D`, else one label stripped (`app.<name>.D` → `<name>.D`).
    pub fn lookup(&self, sni: &str) -> Option<Arc<CertifiedKey>> {
        let sni = sni.trim_end_matches('.').to_ascii_lowercase();
        let map = self.by_name.load();
        map.get(&sni)
            .or_else(|| sni.split_once('.').and_then(|(_, parent)| map.get(parent)))
            .cloned()
    }
}

impl ResolvesServerCert for Certs {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        self.lookup(hello.server_name()?)
    }
}

/// A `CertifiedKey` from a PEM chain and a PEM private key; refuses a chain for another key.
pub fn certified_key(chain_pem: &str, key_pem: &str) -> anyhow::Result<CertifiedKey> {
    use rustls::pki_types::pem::PemObject as _;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    let chain =
        CertificateDer::pem_slice_iter(chain_pem.as_bytes()).collect::<Result<Vec<_>, _>>()?;
    anyhow::ensure!(!chain.is_empty(), "empty certificate chain");
    let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes())?;
    let signing = rustls::crypto::aws_lc_rs::sign::any_supported_type(&key)?;
    let ck = CertifiedKey::new(chain, signing);
    ck.keys_match()
        .map_err(|e| anyhow::anyhow!("the chain is not for this name key: {e}"))?;
    Ok(ck)
}

/// The TLS acceptor over `certs` (ALPN `h2`, `http/1.1`).
pub fn acceptor(certs: Arc<Certs>) -> anyhow::Result<tokio_rustls::TlsAcceptor> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut cfg = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_cert_resolver(certs);
    cfg.alpn_protocols = share_proto::proxy::VISITOR_ALPNS
        .iter()
        .map(|a| a.to_vec())
        .collect();
    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(cfg)))
}

/// The visitor HTTP server: an HTTP/1.1 request head must arrive within [`HEADER_READ_TIMEOUT`];
/// heads fit in [`MAX_HEADER_BYTES`]; an HTTP/2 connection has at most [`MAX_STREAMS`] open
/// streams. The HTTP/2 head deadline is enforced by [`serve_http`] (hyper has none).
pub fn http_server() -> hyper_util::server::conn::auto::Builder<TokioExecutor> {
    let mut b = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
    b.http1()
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_READ_TIMEOUT)
        .max_buf_size(MAX_HEADER_BYTES);
    b.http2()
        .timer(TokioTimer::new())
        .max_header_list_size(MAX_HEADER_BYTES as u32)
        .max_concurrent_streams(MAX_STREAMS);
    b
}

/// Requests in flight on one connection; a request counts from its head's arrival until its
/// response body is dropped (finished, failed or reset).
#[derive(Default)]
struct InFlight {
    n: std::sync::atomic::AtomicUsize,
    changed: tokio::sync::Notify,
}

struct InFlightGuard(Arc<InFlight>);

impl InFlight {
    fn enter(self: &Arc<Self>) -> InFlightGuard {
        self.n.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        self.changed.notify_one();
        InFlightGuard(self.clone())
    }

    fn idle(&self) -> bool {
        self.n.load(std::sync::atomic::Ordering::Acquire) == 0
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.0.n.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
        self.0.changed.notify_one();
    }
}

/// Serve HTTP on one visitor connection with [`http_server`] and the head deadline on both
/// protocols: whenever the connection has no request in flight for [`HEADER_READ_TIMEOUT`] (no
/// stream opened, a dribbled `HEADERS`/`CONTINUATION`, an idle keep-alive) it is closed gracefully,
/// and dropped if still idle [`CLOSE_GRACE`] later (a peer that ignores `GOAWAY` and its `PING`).
/// A long response or upload is never cut: it is in flight.
pub async fn serve_http<I, H, F>(io: I, handler: H) -> anyhow::Result<()>
where
    I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    H: Fn(http::Request<hyper::body::Incoming>) -> F + Send + Sync + 'static,
    F: Future<Output = http::Response<crate::proxy::Body>> + Send + 'static,
{
    use http_body_util::BodyExt as _;
    let busy = Arc::new(InFlight::default());
    let svc = {
        let busy = busy.clone();
        hyper::service::service_fn(move |req| {
            let guard = busy.enter();
            let res = handler(req);
            async move {
                let res = res.await.map(move |b| {
                    b.map_frame(move |f| {
                        let _ = &guard;
                        f
                    })
                });
                Ok::<_, std::convert::Infallible>(res)
            }
        })
    };
    let server = http_server();
    let conn = server.serve_connection_with_upgrades(TokioIo::new(io), svc);
    tokio::pin!(conn);
    let mut closing = false;
    loop {
        let wait = if !busy.idle() {
            None
        } else if closing {
            Some(CLOSE_GRACE)
        } else {
            Some(HEADER_READ_TIMEOUT)
        };
        let deadline = async {
            match wait {
                Some(d) => tokio::time::sleep(d).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            r = conn.as_mut() => return r.map_err(|e| anyhow::anyhow!("visitor connection: {e}")),
            () = busy.changed.notified() => {}
            () = deadline => {
                if closing {
                    anyhow::bail!("visitor connection: no request head within the deadline");
                }
                conn.as_mut().graceful_shutdown();
                closing = true;
            }
        }
    }
}

/// Serve one visitor stream to completion. Requests are checked against the TLS SNI (the
/// PROXY preamble's authority when the session has none), see [`Proxy::handle`].
pub async fn serve<S>(
    mut stream: S,
    tls: tokio_rustls::TlsAcceptor,
    proxy: Arc<Proxy>,
) -> anyhow::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let pre = tokio::time::timeout(PREAMBLE_TIMEOUT, read_preamble(&mut stream)).await??;
    let tls = tokio::time::timeout(PREAMBLE_TIMEOUT, tls.accept(stream)).await??;
    let visitor = pre.visitor;
    let sni: Arc<str> = tls
        .get_ref()
        .1
        .server_name()
        .map(str::to_owned)
        .or(pre.sni)
        .unwrap_or_default()
        .into();
    serve_http(tls, move |req| {
        let (proxy, sni) = (proxy.clone(), sni.clone());
        async move { proxy.handle(req, visitor, &sni).await }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn preamble_roundtrip_leaves_tls_bytes() {
        use ppp::v2::{Builder, Command, IPv4, Protocol, Version};
        let addrs: ppp::v2::Addresses =
            IPv4::new([198, 51, 100, 7], [203, 0, 113, 1], 40000, 443).into();
        let mut hdr =
            Builder::with_addresses(Version::Two | Command::Proxy, Protocol::Stream, addrs)
                .write_tlv(TLV_AUTHORITY, b"app.sky.tun.zip")
                .unwrap()
                .write_tlv(TLV_CONN_ID, &share_proto::proxy::conn_id_bytes(42))
                .unwrap()
                .build()
                .unwrap();
        hdr.extend_from_slice(b"\x16\x03\x01rest");
        let mut r = &hdr[..];
        let p = read_preamble(&mut r).await.unwrap();
        assert_eq!(p.visitor, "198.51.100.7:40000".parse().unwrap());
        assert_eq!(p.sni.as_deref(), Some("app.sky.tun.zip"));
        assert_eq!(p.conn_id, Some(42));
        assert_eq!(r, b"\x16\x03\x01rest");
    }

    #[tokio::test]
    async fn preamble_v4_mapped_visitor_is_ipv4() {
        use ppp::v2::{Builder, Command, IPv6, Protocol, Version};
        let mapped = std::net::Ipv4Addr::new(188, 251, 216, 205).to_ipv6_mapped();
        let addrs: ppp::v2::Addresses =
            IPv6::new(mapped, std::net::Ipv6Addr::LOCALHOST, 40000, 443).into();
        let hdr = Builder::with_addresses(Version::Two | Command::Proxy, Protocol::Stream, addrs)
            .build()
            .unwrap();
        let p = read_preamble(&mut &hdr[..]).await.unwrap();
        assert_eq!(p.visitor, "188.251.216.205:40000".parse().unwrap());
    }

    #[test]
    fn resolver_strips_one_label_and_checks_key() {
        let (key_pem, _csr) = crate::keys::name_key("sky", "tun.zip").unwrap();
        let key = rcgen::KeyPair::from_pem(&key_pem).unwrap();
        let params = rcgen::CertificateParams::new(crate::keys::sans("sky", "tun.zip")).unwrap();
        let cert = params.self_signed(&key).unwrap();
        let certs = Certs::default();
        certs.install("sky.tun.zip", &cert.pem(), &key_pem).unwrap();
        assert!(certs.lookup("sky.tun.zip").is_some());
        assert!(certs.lookup("APP.sky.tun.zip.").is_some());
        assert!(certs.lookup("a.b.sky.tun.zip").is_none());
        let (other, _) = crate::keys::name_key("sky", "tun.zip").unwrap();
        assert!(certs.install("sky.tun.zip", &cert.pem(), &other).is_err());
    }

    async fn ok(
        _req: http::Request<hyper::body::Incoming>,
    ) -> Result<http::Response<http_body_util::Full<bytes::Bytes>>, std::convert::Infallible> {
        Ok(http::Response::new(http_body_util::Full::new(
            bytes::Bytes::from_static(b"ok"),
        )))
    }

    /// m1: a request head that never completes is dropped after the header-read timeout.
    #[tokio::test(start_paused = true)]
    async fn slow_head_is_dropped() {
        use tokio::io::AsyncWriteExt as _;
        let (mut client, server) = tokio::io::duplex(1 << 20);
        let conn = tokio::spawn(async move {
            http_server()
                .serve_connection_with_upgrades(TokioIo::new(server), hyper::service::service_fn(ok))
                .await
                .is_err()
        });
        client
            .write_all(b"GET / HTTP/1.1\r\nHost: a\r\n")
            .await
            .unwrap();
        let done = tokio::time::timeout(HEADER_READ_TIMEOUT * 2, conn).await;
        assert!(done.is_ok(), "the connection outlived the header-read timeout");
        drop(client);
    }

    /// m1: a request head above the limit is refused, not buffered.
    #[tokio::test]
    async fn oversized_head_is_refused() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let (mut client, server) = tokio::io::duplex(1 << 20);
        tokio::spawn(async move {
            let _ = http_server()
                .serve_connection_with_upgrades(TokioIo::new(server), hyper::service::service_fn(ok))
                .await;
        });
        let big = format!(
            "GET / HTTP/1.1\r\nHost: a\r\nX-Big: {}\r\n\r\n",
            "a".repeat(MAX_HEADER_BYTES + 1024)
        );
        let _ = client.write_all(big.as_bytes()).await;
        let mut out = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut out)).await;
        let head = String::from_utf8_lossy(&out);
        assert!(head.starts_with("HTTP/1.1 431"), "{head}");
    }

    async fn answer(_req: http::Request<hyper::body::Incoming>) -> http::Response<crate::proxy::Body> {
        crate::proxy::text(http::StatusCode::OK, "ok")
    }

    /// Serve `server` with `serve_http`; resolves to the time the connection ended.
    fn served<H, F>(server: tokio::io::DuplexStream, h: H) -> tokio::task::JoinHandle<Duration>
    where
        H: Fn(http::Request<hyper::body::Incoming>) -> F + Send + Sync + 'static,
        F: Future<Output = http::Response<crate::proxy::Body>> + Send + 'static,
    {
        let t0 = tokio::time::Instant::now();
        tokio::spawn(async move {
            let _ = serve_http(server, h).await;
            t0.elapsed()
        })
    }

    const H2_PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

    /// An HTTP/2 connection that opens no stream is closed after the head deadline.
    #[tokio::test(start_paused = true)]
    async fn h2_connection_without_a_stream_is_dropped() {
        use tokio::io::AsyncWriteExt as _;
        let (mut client, server) = tokio::io::duplex(1 << 20);
        let conn = served(server, answer);
        client.write_all(H2_PREFACE).await.unwrap();
        client.write_all(&[0, 0, 0, 4, 0, 0, 0, 0, 0]).await.unwrap(); // empty SETTINGS
        let took = tokio::time::timeout(HEADER_READ_TIMEOUT * 3, conn)
            .await
            .expect("the h2 connection outlived the head deadline")
            .unwrap();
        assert!(took >= HEADER_READ_TIMEOUT, "closed early: {took:?}");
        assert!(took <= HEADER_READ_TIMEOUT + CLOSE_GRACE + Duration::from_secs(1), "{took:?}");
        drop(client);
    }

    /// A slow writer that starts a request head (`HEADERS` without `END_HEADERS`) and never
    /// finishes it is dropped after the head deadline, though it never answers `GOAWAY`/`PING`.
    #[tokio::test(start_paused = true)]
    async fn h2_dribbled_head_is_dropped() {
        use tokio::io::AsyncWriteExt as _;
        let (mut client, server) = tokio::io::duplex(1 << 20);
        let conn = served(server, answer);
        client.write_all(H2_PREFACE).await.unwrap();
        client.write_all(&[0, 0, 0, 4, 0, 0, 0, 0, 0]).await.unwrap();
        // HEADERS, stream 1, no END_HEADERS: `:method GET`, then the block stays open.
        client.write_all(&[0, 0, 1, 1, 0, 0, 0, 0, 1, 0x82]).await.unwrap();
        for _ in 0..3 {
            tokio::time::sleep(Duration::from_secs(3)).await;
            // CONTINUATION, stream 1, still no END_HEADERS: `accept-encoding: gzip, deflate`.
            let _ = client.write_all(&[0, 0, 1, 9, 0, 0, 0, 0, 1, 0x90]).await;
        }
        let took = tokio::time::timeout(HEADER_READ_TIMEOUT * 3, conn)
            .await
            .expect("the dribbling h2 connection outlived the head deadline")
            .unwrap();
        assert!(took >= HEADER_READ_TIMEOUT, "closed early: {took:?}");
        assert!(took <= HEADER_READ_TIMEOUT + CLOSE_GRACE + Duration::from_secs(1), "{took:?}");
        drop(client);
    }

    /// A response slower than the head deadline is not cut; once it is done and nothing else is
    /// in flight, the deadline runs again. Open streams per connection are capped.
    #[tokio::test(start_paused = true)]
    async fn h2_in_flight_requests_are_kept_and_streams_are_capped() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let (client, server) = tokio::io::duplex(1 << 20);
        let (now, most) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let (n, m) = (now.clone(), most.clone());
        let conn = served(server, move |req| {
            let (n, m) = (n.clone(), m.clone());
            async move {
                let cur = n.fetch_add(1, Ordering::SeqCst) + 1;
                m.fetch_max(cur, Ordering::SeqCst);
                let wait = if req.uri().path() == "/slow" { 30 } else { 1 };
                tokio::time::sleep(Duration::from_secs(wait)).await;
                n.fetch_sub(1, Ordering::SeqCst);
                crate::proxy::text(http::StatusCode::OK, "ok")
            }
        });
        let (send, driver) = hyper::client::conn::http2::Builder::new(TokioExecutor::new())
            .timer(TokioTimer::new())
            .handshake::<_, http_body_util::Empty<bytes::Bytes>>(TokioIo::new(client))
            .await
            .unwrap();
        tokio::spawn(driver);
        let get = |path: &str| {
            let mut send = send.clone();
            let req = http::Request::get(format!("https://a{path}"))
                .body(http_body_util::Empty::new())
                .unwrap();
            async move {
                let res = send.send_request(req).await.unwrap();
                use http_body_util::BodyExt as _;
                let status = res.status();
                res.into_body().collect().await.unwrap();
                status
            }
        };
        assert_eq!(get("/slow").await, http::StatusCode::OK);
        let burst = (0..MAX_STREAMS as usize + 50).map(|_| get("/"));
        let all = futures::future::join_all(burst).await;
        assert!(all.iter().all(|s| *s == http::StatusCode::OK));
        assert_eq!(most.load(Ordering::SeqCst), MAX_STREAMS as usize);
        let took = tokio::time::timeout(Duration::from_secs(60), conn)
            .await
            .expect("the idle connection was kept")
            .unwrap();
        assert!(took >= Duration::from_secs(30) + HEADER_READ_TIMEOUT, "{took:?}");
    }
}
