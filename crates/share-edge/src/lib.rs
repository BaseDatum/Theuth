//! `theuth-share-edge` (design doc 210): the standalone blind SNI edge. Engines dial it on `theuth/share/1`
//! with a mesh delegation, claim a name and register routes; visitors reach it on TCP 443 and are spliced, by
//! SNI and without decryption, onto a bi-stream of the engine's tunnel behind a PROXY v2 preamble.
//!
//! Depends only on `share-proto`, `platform` and external crates (D14).

pub mod acme;
pub mod admin;
pub mod certs;
pub mod config;
pub mod control;
pub mod dns;
pub mod k8s;
pub mod sni;
pub mod state;
pub mod store;

use std::net::SocketAddr;
use std::path::Path;
use std::str::FromStr as _;
use std::sync::Arc;

use anyhow::Context as _;
use iroh::endpoint::presets;
use iroh::protocol::Router;
use iroh::{Endpoint, RelayMap, RelayMode, RelayUrl, SecretKey};
use share_proto::{EndpointAddr, ShareEdgeTicket};
use tokio::task::JoinSet;

use crate::config::{Config, Secrets};
use crate::state::Edge;
use crate::store::Store;

/// Load `<data_dir>/iroh.key` or create it (32 random bytes, 0600), as the engine's mesh identity does.
pub fn load_or_create_key(path: &Path) -> anyhow::Result<SecretKey> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let bytes: [u8; 32] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| anyhow::anyhow!("{}: expected 32 bytes", path.display()))?;
            Ok(SecretKey::from_bytes(&bytes))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let secret = SecretKey::generate();
            if let Some(dir) = path.parent() {
                platform::files::private_dir(dir)?;
            }
            platform::files::create_private(path, &secret.to_bytes())
                .with_context(|| format!("create {}", path.display()))?;
            Ok(secret)
        }
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

/// The ticket clients paste into `share.edge` (§5.5): the endpoint id, the home relay and `iroh.public_addrs`.
pub fn ticket(cfg: &Config, id: share_proto::PublicKey) -> anyhow::Result<ShareEdgeTicket> {
    let mut addr = EndpointAddr::new(id);
    if !cfg.iroh.relay.is_empty() {
        addr = addr.with_relay_url(RelayUrl::from_str(&cfg.iroh.relay).context("iroh.relay")?);
    }
    for a in &cfg.iroh.public_addrs {
        addr = addr.with_ip_addr(*a);
    }
    // TODO(210-M4): `--standby <pubkey>` two-key form and invites.
    Ok(ShareEdgeTicket { addr, invite: None })
}

/// A running edge.
pub struct Running {
    /// Shared state.
    pub edge: Arc<Edge>,
    /// The bound visitor listener's address.
    pub tcp_addr: SocketAddr,
    /// The iroh router (owns the endpoint).
    pub router: Router,
    tasks: JoinSet<()>,
}

impl Running {
    /// The endpoint's dialable address (bound sockets plus relay).
    pub fn endpoint_addr(&self) -> EndpointAddr {
        self.router.endpoint().addr()
    }

    /// Stop accepting, close every tunnel and the store. TODO(210-M4): `drain {secs}` before closing.
    pub async fn shutdown(mut self) {
        self.tasks.abort_all();
        let _ = self.router.shutdown().await;
        self.edge.store.close().await;
    }
}

/// Bind everything and start serving, with the production backends: zone-update's Porkbun client over the
/// DNS credential, the registered ACME account, and the system resolver for `require_caa`.
pub async fn start(cfg: Config, secrets: Secrets) -> anyhow::Result<Running> {
    let Some(cred) = secrets.dns.clone() else {
        anyhow::bail!(
            "no DNS credential: set `dns.credential_file` or THEUTH_SHARE_EDGE_DNS__CREDENTIAL (§8.3)"
        );
    };
    if cfg.dns.provider != "porkbun" {
        anyhow::bail!("dns.provider {}: only `porkbun` is built (§6.4)", cfg.dns.provider);
    }
    let zone = Arc::new(dns::PorkbunZone::new(cfg.dns_zone(), cred)?);
    let ca = acme::load_cas(&cfg).await?.remove(0);
    let backends = certs::Backends {
        zone,
        ca,
        caa: Arc::new(acme::SystemCaa),
        probe: Arc::new(dns::AuthoritativeTxt),
    };
    start_with(cfg, secrets, backends).await
}

/// `start` with the given certificate backends (tests pass fakes). With `require_caa` the edge refuses to start
/// until `D`'s CAA set names every enabled CA's account and method (§8.2).
pub async fn start_with(
    cfg: Config,
    secrets: Secrets,
    backends: certs::Backends,
) -> anyhow::Result<Running> {
    if cfg.require_caa {
        // TODO(210-M4): re-check daily; at runtime a failure flips admission to `closed` and alerts.
        acme::require_caa(&*backends.caa, &cfg, std::slice::from_ref(&backends.ca)).await?;
    }
    // Before binding anything: a k8s section that cannot run (no feature, no in-cluster config) fails start.
    let k8s = if cfg.k8s.enabled {
        Some(k8s_manager(&cfg).await?)
    } else {
        None
    };
    platform::files::private_dir(&cfg.data_dir)
        .with_context(|| format!("data_dir {}", cfg.data_dir.display()))?;
    let key = load_or_create_key(&cfg.key_path())?;
    let store = Store::open(&cfg.db_path(), cfg.db_journal).await?;
    let relay_mode = if cfg.iroh.relay.is_empty() {
        RelayMode::Disabled
    } else {
        RelayMode::Custom(RelayMap::from(
            RelayUrl::from_str(&cfg.iroh.relay).context("iroh.relay")?,
        ))
    };
    // TODO(210-M4): pkarr publication when `iroh.pkarr`.
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(key)
        .alpns(share_proto::ALPNS.iter().map(|a| a.to_vec()).collect())
        .relay_mode(relay_mode)
        .bind_addr(cfg.iroh.listen)?
        .bind()
        .await
        .context("bind iroh endpoint")?;
    let id = endpoint.id();
    let listener = tokio::net::TcpListener::bind(cfg.listen)
        .await
        .with_context(|| format!("listen {}", cfg.listen))?;
    let tcp_addr = listener.local_addr()?;
    let admin_socket = cfg.admin_socket();
    let health = cfg.admin.http.clone();
    let edge = Arc::new(Edge::new(cfg, secrets, store.clone(), id)?);
    let _ = edge.certs.set(certs::Certs::new(&backends, store, &edge.cfg));
    let mut rb = Router::builder(endpoint);
    for alpn in share_proto::ALPNS {
        rb = rb.accept(*alpn, control::ShareProtocol { edge: edge.clone() });
    }
    let router = rb.spawn();
    let mut tasks = JoinSet::new();
    tasks.spawn(sni::serve(edge.clone(), listener));
    tasks.spawn(certs::run(edge.clone()));
    if let Some(k) = k8s {
        let _ = edge.k8s.set(k.clone());
        tasks.spawn(k8s::run(edge.clone(), k));
    }
    let admin = admin::bind(&admin_socket)?;
    tasks.spawn(admin::serve(edge.clone(), admin, admin_socket));
    if !health.is_empty() {
        let l = tokio::net::TcpListener::bind(&health)
            .await
            .with_context(|| format!("admin.http {health}"))?;
        tasks.spawn(serve_health(edge.clone(), l));
    }
    tracing::info!(%id, %tcp_addr, domain = %edge.cfg.domain, "theuth-share-edge running");
    Ok(Running {
        edge,
        tcp_addr,
        router,
        tasks,
    })
}

/// `/healthz` and `/metrics`. TODO(210-M4): the full metric set of appendix §8.4.
async fn serve_health(edge: Arc<Edge>, listener: tokio::net::TcpListener) {
    use axum::routing::get;
    let metrics = move || {
        let e = edge.clone();
        async move {
            let mut m = format!(
                "share_edge_tunnels {}\nshare_edge_routes {}\nshare_edge_conns {}\n",
                e.tunnels.len(),
                e.routes.len(),
                e.cfg.limits.global_conns as usize - e.visitors.available_permits()
            );
            if let Some(k) = e.k8s.get() {
                let n = k.errors.load(std::sync::atomic::Ordering::Relaxed);
                m.push_str(&format!("share_edge_k8s_errors_total {n}\n"));
            }
            m
        }
    };
    let app = axum::Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/metrics", get(metrics));
    if let Err(e) = axum::serve(listener, app).await {
        tracing::warn!("admin.http: {e}");
    }
}

/// The per-name object manager over the in-cluster client (`Config::infer`); the namespace is `k8s.namespace`
/// or the inferred one (in a pod: its service-account namespace file).
#[cfg(feature = "k8s")]
async fn k8s_manager(cfg: &Config) -> anyhow::Result<Arc<k8s::K8s>> {
    let (client, inferred) = k8s::client::KubeClient::infer(&cfg.k8s.field_manager).await?;
    let ns = if cfg.k8s.namespace.is_empty() {
        inferred
    } else {
        cfg.k8s.namespace.clone()
    };
    Ok(Arc::new(k8s::K8s::new(
        cfg.k8s.clone(),
        &cfg.domain,
        &ns,
        Arc::new(client),
    )))
}

#[cfg(not(feature = "k8s"))]
async fn k8s_manager(_: &Config) -> anyhow::Result<Arc<k8s::K8s>> {
    anyhow::bail!("`k8s.enabled: true` needs a build with the cargo feature `k8s`")
}

/// `run`: start, wait for the stop signal, shut down.
pub async fn run(cfg: Config, secrets: Secrets) -> anyhow::Result<()> {
    let running = start(cfg, secrets).await?;
    platform::process::stop_signal().await?;
    running.shutdown().await;
    Ok(())
}
