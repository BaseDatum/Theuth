//! The visitor path (210 §4.2): TCP accept, ClientHello peek with `rustls::server::Acceptor` keeping the raw
//! bytes, routing on SNI, a PROXY v2 preamble on a fresh bi-stream of the engine's tunnel, then a byte splice.
//! The edge never presents a certificate: every refusal is a TCP reset.

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use rustls::server::Acceptor;
use share_proto::names;
use share_proto::proxy::{
    ACME_TLS_ALPN, CLIENT_HELLO_MAX, CLIENT_HELLO_TIMEOUT_MS, TLV_AUTHORITY, TLV_CONN_ID,
    conn_id_bytes,
};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};

use crate::state::{Edge, Tunnel, net_key, now_ms};

/// The parts of a ClientHello the edge routes on, with the exact bytes read so far.
#[derive(Debug)]
pub struct Peeked {
    /// Every byte read from the visitor, replayed to the engine.
    pub bytes: Vec<u8>,
    /// SNI as sent.
    pub sni: Option<String>,
    /// Offered ALPN protocols.
    pub alpn: Vec<Vec<u8>>,
}

/// Why a visitor is reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refuse {
    /// No complete ClientHello within 5 s.
    Timeout,
    /// More than 16 KiB without a complete ClientHello.
    TooLarge,
    /// Not TLS, or the peer closed early.
    NotTls,
    /// No SNI.
    NoSni,
    /// ALPN offers `acme-tls/1` (§6.7).
    AcmeTls,
    /// Not under `D`, the apex, deeper than one label, or a bad label.
    Host,
}

/// Read until rustls parses a whole ClientHello (it may span segments), within 16 KiB and 5 s.
pub async fn peek_client_hello<R: AsyncRead + Unpin>(r: &mut R) -> Result<Peeked, Refuse> {
    let read = async {
        let mut acceptor = Acceptor::default();
        let mut bytes = Vec::with_capacity(2048);
        let mut chunk = [0u8; 4096];
        loop {
            let n = r.read(&mut chunk).await.map_err(|_| Refuse::NotTls)?;
            if n == 0 {
                return Err(Refuse::NotTls);
            }
            if bytes.len() + n > CLIENT_HELLO_MAX {
                return Err(Refuse::TooLarge);
            }
            bytes.extend_from_slice(&chunk[..n]);
            let mut fresh = &chunk[..n];
            while !fresh.is_empty() {
                acceptor.read_tls(&mut fresh).map_err(|_| Refuse::NotTls)?;
            }
            match acceptor.accept() {
                Ok(None) => continue,
                Ok(Some(accepted)) => {
                    let hello = accepted.client_hello();
                    return Ok(Peeked {
                        sni: hello.server_name().map(str::to_owned),
                        alpn: hello
                            .alpn()
                            .map(|it| it.map(<[u8]>::to_vec).collect())
                            .unwrap_or_default(),
                        bytes,
                    });
                }
                Err(_) => return Err(Refuse::NotTls),
            }
        }
    };
    tokio::time::timeout(Duration::from_millis(CLIENT_HELLO_TIMEOUT_MS), read)
        .await
        .unwrap_or(Err(Refuse::Timeout))
}

/// The lowercase host to route, or why not: `acme-tls/1` first (no tunnel may answer a TLS-ALPN-01
/// challenge), then SNI under `D` at most one label below a name.
pub fn route_host(p: &Peeked, domain: &str) -> Result<String, Refuse> {
    if p.alpn.iter().any(|a| a == ACME_TLS_ALPN) {
        return Err(Refuse::AcmeTls);
    }
    let sni = p.sni.as_deref().ok_or(Refuse::NoSni)?;
    let parts = names::parse_host(sni, domain).map_err(|_| Refuse::Host)?;
    Ok(names::host(&parts.name, parts.label.as_deref(), domain))
}

/// The PROXY v2 preamble (§5.3): visitor and edge address, SNI in `PP2_TYPE_AUTHORITY`, the connection id in
/// TLV `0xE0`.
pub fn proxy_preamble(
    visitor: SocketAddr,
    local: SocketAddr,
    host: &str,
    conn_id: u64,
) -> io::Result<Vec<u8>> {
    use ppp::v2::{Builder, Command, Protocol, Version};
    // One address family for both ends: a v4 visitor on a v6 listener is sent v4-mapped (or the reverse).
    let (src, dst) = match (visitor, local) {
        (SocketAddr::V4(_), SocketAddr::V4(_)) | (SocketAddr::V6(_), SocketAddr::V6(_)) => {
            (visitor, local)
        }
        _ => (to_v6(visitor), to_v6(local)),
    };
    Builder::with_addresses(Version::Two | Command::Proxy, Protocol::Stream, (src, dst))
        .write_tlv(TLV_AUTHORITY, host.as_bytes())?
        .write_tlv(TLV_CONN_ID, &conn_id_bytes(conn_id))?
        .build()
}

fn to_v6(a: SocketAddr) -> SocketAddr {
    match a {
        SocketAddr::V4(v4) => SocketAddr::new(IpAddr::V6(v4.ip().to_ipv6_mapped()), v4.port()),
        v6 => v6,
    }
}

/// Accept visitors until the listener fails.
pub async fn serve(edge: Arc<Edge>, listener: TcpListener) {
    loop {
        match listener.accept().await {
            Ok((tcp, peer)) => {
                tokio::spawn(visitor(edge.clone(), tcp, peer));
            }
            Err(e) => {
                tracing::warn!("visitor accept: {e}");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

fn reset(tcp: TcpStream) {
    let _ = tcp.set_zero_linger();
    drop(tcp);
}

/// Decrements the per-IP count on drop.
struct IpSlot<'a>(&'a Edge, IpAddr);

impl Drop for IpSlot<'_> {
    fn drop(&mut self) {
        self.0.per_ip.remove_if_mut(&self.1, |_, n| {
            *n -= 1;
            *n == 0
        });
    }
}

/// Decrements the tunnel's stream count on drop.
struct StreamSlot(Arc<Tunnel>);

impl Drop for StreamSlot {
    fn drop(&mut self) {
        self.0.streams.fetch_sub(1, Ordering::Relaxed);
    }
}

async fn visitor(edge: Arc<Edge>, mut tcp: TcpStream, peer: SocketAddr) {
    let ip = net_key(peer.ip());
    // TODO(210-M3): per-tunnel new-connection rate and the byte shaping of §9.6.
    let Ok(_global) = edge.visitors.clone().try_acquire_owned() else {
        return reset(tcp);
    };
    if edge.ip_rate.check_key(&ip).is_err() {
        return reset(tcp);
    }
    {
        let mut n = edge.per_ip.entry(ip).or_insert(0);
        if *n >= edge.cfg.limits.conns_per_ip {
            drop(n);
            return reset(tcp);
        }
        *n += 1;
    }
    let _ip_slot = IpSlot(&edge, ip);
    let Ok(local) = tcp.local_addr() else {
        return reset(tcp);
    };
    let peeked = match peek_client_hello(&mut tcp).await {
        Ok(p) => p,
        Err(_) => return reset(tcp),
    };
    let Ok(host) = route_host(&peeked, &edge.cfg.domain) else {
        return reset(tcp);
    };
    let Some(route) = edge.routes.get(&host).map(|r| r.clone()) else {
        return reset(tcp);
    };
    let tunnel = route.tunnel.clone();
    if tunnel.streams.fetch_add(1, Ordering::Relaxed) >= edge.cfg.limits.conns_per_tunnel {
        tunnel.streams.fetch_sub(1, Ordering::Relaxed);
        return reset(tcp);
    }
    let _stream_slot = StreamSlot(tunnel.clone());
    let open = Duration::from_secs(edge.cfg.limits.open_bi_timeout_secs);
    let Ok(Ok((mut send, recv))) = tokio::time::timeout(open, tunnel.conn.open_bi()).await else {
        return reset(tcp);
    };
    let conn_id = edge.conn_id();
    let Ok(preamble) = proxy_preamble(peer, local, &host, conn_id) else {
        return reset(tcp);
    };
    if send.write_all(&preamble).await.is_err() || send.write_all(&peeked.bytes).await.is_err() {
        return reset(tcp);
    }
    tunnel.conns.fetch_add(1, Ordering::Relaxed);
    let activity = Arc::new(AtomicU64::new(now_ms()));
    let up = Arc::new(AtomicU64::new(peeked.bytes.len() as u64));
    let down = Arc::new(AtomicU64::new(0));
    let mut visitor = Metered {
        inner: tcp,
        read: up.clone(),
        written: down.clone(),
        activity: activity.clone(),
    };
    let mut engine = tokio::io::join(recv, send);
    let idle = Duration::from_secs(edge.cfg.limits.idle_secs);
    let clean = tokio::select! {
        r = tokio::io::copy_bidirectional(&mut visitor, &mut engine) => r.is_ok(),
        () = idle_watch(activity, idle) => false,
    };
    let (up, down) = (up.load(Ordering::Relaxed), down.load(Ordering::Relaxed));
    tunnel.bytes.fetch_add(up + down, Ordering::Relaxed);
    if !clean {
        // Stream reset ↔ TCP RST (§5.3).
        let (recv, mut send) = engine.into_inner();
        let _ = send.reset(0u32.into());
        drop(recv);
        reset(visitor.inner);
    }
    let _ = edge
        .store
        .conn_log(
            now_ms(),
            &host,
            &peer.ip().to_string(),
            &route.mesh.to_string(),
            &tunnel.engine.to_string(),
            up,
            down,
        )
        .await;
}

async fn idle_watch(activity: Arc<AtomicU64>, idle: Duration) {
    let idle_ms = u64::try_from(idle.as_millis()).unwrap_or(u64::MAX);
    loop {
        let last = activity.load(Ordering::Relaxed);
        let left = (last + idle_ms).saturating_sub(now_ms());
        if left == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(left.max(1000))).await;
    }
}

/// Counts bytes both ways and stamps activity. TODO(210-M3): fold into tunnel, mesh and network budgets and
/// await the per-tunnel `governor` byte limiter here (§9.6); a visitor that stops reading for 5 s is reset.
struct Metered<T> {
    inner: T,
    read: Arc<AtomicU64>,
    written: Arc<AtomicU64>,
    activity: Arc<AtomicU64>,
}

impl<T: AsyncRead + Unpin> AsyncRead for Metered<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let r = Pin::new(&mut self.inner).poll_read(cx, buf);
        let n = buf.filled().len() - before;
        if n > 0 {
            self.read.fetch_add(n as u64, Ordering::Relaxed);
            self.activity.store(now_ms(), Ordering::Relaxed);
        }
        r
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Metered<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let r = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = r
            && n > 0
        {
            self.written.fetch_add(n as u64, Ordering::Relaxed);
            self.activity.store(now_ms(), Ordering::Relaxed);
        }
        r
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt as _;

    /// The bytes a rustls client sends first: a whole ClientHello for `sni` offering `alpn`.
    pub(crate) fn client_hello(sni: &str, alpn: &[&[u8]]) -> Vec<u8> {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let mut cfg = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("versions")
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth();
        cfg.alpn_protocols = alpn.iter().map(|a| a.to_vec()).collect();
        let name = rustls::pki_types::ServerName::try_from(sni.to_string()).expect("server name");
        let mut conn = rustls::ClientConnection::new(Arc::new(cfg), name).expect("client");
        let mut out = Vec::new();
        while conn.wants_write() {
            conn.write_tls(&mut out).expect("write_tls");
        }
        out
    }

    async fn peek(bytes: &[u8]) -> Result<Peeked, Refuse> {
        let (mut a, mut b) = tokio::io::duplex(64 * 1024);
        a.write_all(bytes).await.unwrap();
        peek_client_hello(&mut b).await
    }

    #[tokio::test]
    async fn acme_tls_alpn_is_reset() {
        let p = peek(&client_hello("app.sky.tun.zip", &[b"h2", ACME_TLS_ALPN]))
            .await
            .unwrap();
        assert_eq!(route_host(&p, "tun.zip"), Err(Refuse::AcmeTls));
        // Even on a valid host, and alone.
        let p = peek(&client_hello("sky.tun.zip", &[ACME_TLS_ALPN]))
            .await
            .unwrap();
        assert_eq!(route_host(&p, "tun.zip"), Err(Refuse::AcmeTls));
    }

    #[tokio::test]
    async fn routes_on_lowercase_host_and_keeps_bytes() {
        let raw = client_hello("App.SKY.tun.zip", &[b"h2", b"http/1.1"]);
        let p = peek(&raw).await.unwrap();
        assert_eq!(p.bytes, raw);
        assert_eq!(route_host(&p, "tun.zip").unwrap(), "app.sky.tun.zip");
    }

    #[tokio::test]
    async fn deep_foreign_and_garbage_are_refused() {
        let p = peek(&client_hello("a.b.sky.tun.zip", &[])).await.unwrap();
        assert_eq!(route_host(&p, "tun.zip"), Err(Refuse::Host));
        let p = peek(&client_hello("sky.example.com", &[])).await.unwrap();
        assert_eq!(route_host(&p, "tun.zip"), Err(Refuse::Host));
        assert_eq!(
            peek(b"GET / HTTP/1.1\r\n\r\n").await.unwrap_err(),
            Refuse::NotTls
        );
    }

    #[test]
    fn preamble_carries_sni_and_conn_id() {
        let pre = proxy_preamble(
            "198.51.100.7:50000".parse().unwrap(),
            "[::]:443".parse().unwrap(),
            "app.sky.tun.zip",
            42,
        )
        .unwrap();
        let h = ppp::v2::Header::try_from(pre.as_slice()).unwrap();
        assert_eq!(h.len(), pre.len());
        let tlvs: Vec<_> = h.tlvs().map(Result::unwrap).collect();
        assert!(
            tlvs.iter()
                .any(|t| t.kind == TLV_AUTHORITY && &*t.value == b"app.sky.tun.zip")
        );
        assert!(tlvs.iter().any(
            |t| t.kind == TLV_CONN_ID && share_proto::proxy::conn_id_from(&t.value) == Some(42)
        ));
    }
}
