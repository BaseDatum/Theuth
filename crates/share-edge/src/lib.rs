//! `theuth-share-edge` (design doc 210): the standalone blind SNI edge. Engines dial it on `theuth/share/1`
//! with a mesh delegation, claim a name and register routes; visitors reach it on TCP 443 and are spliced, by
//! SNI and without decryption, onto a bi-stream of the engine's tunnel behind a PROXY v2 preamble.
//!
//! Depends only on `share-proto`, `platform` and external crates (D14).

pub mod admin;
pub mod config;
pub mod control;
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

/// Bind everything and start serving.
pub async fn start(cfg: Config, secrets: Secrets) -> anyhow::Result<Running> {
    platform::files::private_dir(&cfg.data_dir)
        .with_context(|| format!("data_dir {}", cfg.data_dir.display()))?;
    let key = load_or_create_key(&cfg.key_path())?;
    let store = Store::open(&cfg.db_path()).await?;
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
    let edge = Arc::new(Edge::new(cfg, secrets, store, id)?);
    let mut rb = Router::builder(endpoint);
    for alpn in share_proto::ALPNS {
        rb = rb.accept(*alpn, control::ShareProtocol { edge: edge.clone() });
    }
    let router = rb.spawn();
    let mut tasks = JoinSet::new();
    tasks.spawn(sni::serve(edge.clone(), listener));
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
            format!(
                "share_edge_tunnels {}\nshare_edge_routes {}\nshare_edge_conns {}\n",
                e.tunnels.len(),
                e.routes.len(),
                e.cfg.limits.global_conns as usize - e.visitors.available_permits()
            )
        }
    };
    let app = axum::Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/metrics", get(metrics));
    if let Err(e) = axum::serve(listener, app).await {
        tracing::warn!("admin.http: {e}");
    }
}

/// `run`: start, wait for the stop signal, shut down.
pub async fn run(cfg: Config, secrets: Secrets) -> anyhow::Result<()> {
    let running = start(cfg, secrets).await?;
    platform::process::stop_signal().await?;
    running.shutdown().await;
    Ok(())
}
