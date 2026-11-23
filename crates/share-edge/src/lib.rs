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
pub mod limits;
pub mod meter;
pub mod ops;
pub mod queue;
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
/// An invite is added by `admin invite new`.
pub fn ticket(cfg: &Config, id: share_proto::PublicKey) -> anyhow::Result<ShareEdgeTicket> {
    let mut addr = EndpointAddr::new(id);
    if !cfg.iroh.relay.is_empty() {
        addr = addr.with_relay_url(RelayUrl::from_str(&cfg.iroh.relay).context("iroh.relay")?);
    }
    for a in &cfg.iroh.public_addrs {
        addr = addr.with_ip_addr(*a);
    }
    Ok(ShareEdgeTicket { addr, invite: None })
}

/// The standby ticket (`ticket --standby`): the standby key with the live ticket's relay and addresses, what a
/// release pins beside the live one (§5.5, D15). Creates the standby key when there is none.
pub fn standby_ticket(cfg: &Config) -> anyhow::Result<ShareEdgeTicket> {
    let standby = load_or_create_key(&cfg.standby_key_path())?;
    ticket(cfg, standby.public())
}

/// `retire` (§5.5): sign a [`share_proto::Retire`] of the live key with the standby key, write it to
/// `retire.json`, and promote the standby key to `iroh.key` (the old key is kept as `iroh.key.retired`). The edge
/// must be stopped; the next `run` serves on the former standby key and sends the notice in every `welcome`.
pub fn retire(cfg: &Config, now_ms: u64) -> anyhow::Result<share_proto::Signed> {
    let live_path = cfg.key_path();
    let standby_path = cfg.standby_key_path();
    anyhow::ensure!(standby_path.exists(), "no standby key at {}", standby_path.display());
    let live = load_or_create_key(&live_path)?;
    let standby = load_or_create_key(&standby_path)?;
    let notice = share_proto::Retire {
        v: share_proto::Retire::VERSION,
        edge: standby.public(),
        retired: live.public(),
        issued_ms: now_ms,
    };
    let signed = notice.sign(&standby).map_err(|e| anyhow::anyhow!("{e}"))?;
    platform::files::create_private(&cfg.retire_path(), &serde_json::to_vec(&signed)?)
        .with_context(|| format!("write {}", cfg.retire_path().display()))?;
    let retired = live_path.with_extension("key.retired");
    std::fs::rename(&live_path, &retired).with_context(|| format!("move {}", live_path.display()))?;
    std::fs::rename(&standby_path, &live_path).with_context(|| format!("move {}", standby_path.display()))?;
    Ok(signed)
}

/// The retire notice in `retire.json`, when it verifies as signed by `id` (the key this edge runs on).
fn load_retire(cfg: &Config, id: &share_proto::PublicKey) -> anyhow::Result<Option<share_proto::Signed>> {
    let path = cfg.retire_path();
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    let signed: share_proto::Signed = serde_json::from_slice(&bytes).context("retire.json")?;
    match share_proto::Retire::verify(&signed, id) {
        Ok(_) => Ok(Some(signed)),
        Err(e) => {
            tracing::warn!("retire.json ignored: {e}");
            Ok(None)
        }
    }
}

/// Drain (§5.3, §5.9): refuse `hello`, send `drain {secs}` to every tunnel and wait until they have left or
/// `secs` passed. Returns the tunnels still attached at the deadline.
pub async fn drain(edge: &Edge, secs: u64) -> usize {
    edge.draining.store(true, std::sync::atomic::Ordering::Relaxed);
    for t in edge.tunnels.iter() {
        t.send(share_proto::Event::Drain(share_proto::events::DrainEvent { secs }));
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(secs);
    while !edge.tunnels.is_empty() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    edge.tunnels.len()
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

    /// Stop accepting, close every tunnel and the store (after [`drain`] on SIGTERM, see [`run`]).
    pub async fn shutdown(mut self) {
        self.tasks.abort_all();
        let _ = self.router.shutdown().await;
        // Today's bytes and the month's egress survive the restart (§9.6).
        let tunnels: Vec<_> = self.edge.tunnels.iter().map(|t| t.value().clone()).collect();
        for t in &tunnels {
            self.edge.meter.fold_tunnel(&self.edge, t);
        }
        if let Err(e) = meter::flush(&self.edge, state::now_ms()).await {
            tracing::warn!("meter flush at shutdown: {e:#}");
        }
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
    // §5.5: the standby key, created once beside the live one; `ticket --standby` prints it, `retire` promotes it.
    let standby = load_or_create_key(&cfg.standby_key_path())?.public();
    let retire = load_retire(&cfg, &key.public())?;
    let store = Store::open(&cfg.db_path(), cfg.db_journal).await?;
    let relay_mode = if cfg.iroh.relay.is_empty() {
        RelayMode::Disabled
    } else {
        RelayMode::Custom(RelayMap::from(
            RelayUrl::from_str(&cfg.iroh.relay).context("iroh.relay")?,
        ))
    };
    // §5.5: no pkarr publication; the tickets carry the relay URL and public addresses.
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
    meter::Meter::load(&edge, state::now_ms()).await?;
    let _ = edge.certs.set(certs::Certs::new(&backends, store, &edge.cfg));
    let _ = edge.standby.set(standby);
    if let Some(r) = retire {
        let _ = edge.retire.set(r);
    }
    let mut rb = Router::builder(endpoint);
    for alpn in share_proto::ALPNS {
        rb = rb.accept(*alpn, control::ShareProtocol { edge: edge.clone() });
    }
    let router = rb.spawn();
    let mut tasks = JoinSet::new();
    tasks.spawn(sni::serve(edge.clone(), listener));
    tasks.spawn(certs::run(edge.clone()));
    tasks.spawn(meter::run(edge.clone()));
    tasks.spawn(ops::run_brake(edge.clone()));
    if edge.cfg.require_caa {
        tasks.spawn(caa_daily(edge.clone()));
    }
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

/// §8.2: the CAA set re-checked every 24 h; a failure flips admission to `closed` (logged as an error).
async fn caa_daily(edge: Arc<Edge>) {
    let mut every = tokio::time::interval(std::time::Duration::from_secs(86_400));
    every.tick().await;
    loop {
        every.tick().await;
        match admin::caa_check(&edge, state::now_ms()).await {
            Ok(r) if !r.ok => tracing::error!(missing = ?r.missing, "daily CAA check failed"),
            Ok(_) => {}
            Err(e) => tracing::warn!("daily CAA check: {e:#}"),
        }
    }
}

/// `/healthz` and `/metrics` (appendix §8.4, §9.5 monitoring; [`ops::render`]).
async fn serve_health(edge: Arc<Edge>, listener: tokio::net::TcpListener) {
    use axum::routing::get;
    let metrics = move || {
        let e = edge.clone();
        async move { ops::render(&e, state::now_ms()).await }
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

/// `run`: start, wait for the stop signal (SIGTERM/SIGINT), drain, shut down. `config_path` is what
/// `admin reload` re-reads.
pub async fn run(cfg: Config, secrets: Secrets, config_path: std::path::PathBuf) -> anyhow::Result<()> {
    let running = start(cfg, secrets).await?;
    let _ = running.edge.config_path.set(config_path);
    platform::process::stop_signal().await?;
    let left = drain(&running.edge, running.edge.cfg.drain_secs).await;
    tracing::info!(left, "drained");
    running.shutdown().await;
    Ok(())
}
