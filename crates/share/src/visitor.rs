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
/// Time a visitor has to send a complete HTTP/1.1 request head (slowloris).
pub const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest request head accepted (HTTP/1.1 read buffer, HTTP/2 header list).
pub const MAX_HEADER_BYTES: usize = 64 * 1024;

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

/// The visitor HTTP server: a request head must arrive within [`HEADER_READ_TIMEOUT`] and fit in
/// [`MAX_HEADER_BYTES`].
// TODO(210-M2): HTTP/2 has no head-read deadline in hyper; an h2 connection that opens no stream
// lives until the edge's idle timeout.
pub fn http_server() -> hyper_util::server::conn::auto::Builder<TokioExecutor> {
    let mut b = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
    b.http1()
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_READ_TIMEOUT)
        .max_buf_size(MAX_HEADER_BYTES);
    b.http2()
        .timer(TokioTimer::new())
        .max_header_list_size(MAX_HEADER_BYTES as u32);
    b
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
    let svc = hyper::service::service_fn(move |req| {
        let (proxy, sni) = (proxy.clone(), sni.clone());
        async move { Ok::<_, std::convert::Infallible>(proxy.handle(req, visitor, &sni).await) }
    });
    http_server()
        .serve_connection_with_upgrades(TokioIo::new(tls), svc)
        .await
        .map_err(|e| anyhow::anyhow!("visitor connection: {e}"))
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
}
