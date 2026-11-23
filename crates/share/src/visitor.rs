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
use hyper_util::rt::{TokioExecutor, TokioIo};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use share_proto::proxy::{TLV_AUTHORITY, TLV_CONN_ID, conn_id_from};
use tokio::io::{AsyncRead, AsyncReadExt as _};

use crate::proxy::Proxy;

/// Largest PROXY v2 payload accepted (addresses + our two TLVs fit easily).
pub const PREAMBLE_MAX: usize = 1024;
/// Time to read the preamble.
pub const PREAMBLE_TIMEOUT: Duration = Duration::from_secs(5);

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
        ppp::v2::Addresses::IPv6(a) => SocketAddr::from((a.source_address, a.source_port)),
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

/// Serve one visitor stream to completion.
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
    let svc = hyper::service::service_fn(move |req| {
        let proxy = proxy.clone();
        async move { Ok::<_, std::convert::Infallible>(proxy.handle(req, visitor).await) }
    });
    hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
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
}
