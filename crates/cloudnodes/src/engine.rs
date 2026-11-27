//! The cloud-node service behind the `cloud_*` RPCs (design doc 224 §13): records, credentials,
//! the detached create job (ticket → server → storage → install → join → ready), list/status
//! merged from records, provider listings and the mesh, and restart/resize/update/destroy.
//! Every seam (provider clients, mesh, vault, events) is a trait or closure so the job and the
//! merge run in tests without a cloud, an iroh endpoint or a vault.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::task::AbortHandle;
use zeroize::Zeroizing;

use crate::creds::{self, Creds, OauthAttempt, StoredCred};
use crate::mesh_view::{MeshPeer, MeshView};
use crate::providers::{
    self, CloudError, CreateReq, Listed, Provider, ProviderStatus, Result, Transport, catalog,
    check_name,
};
use crate::records::{Record, Records};
use crate::types::{
    CloudEvent, CloudList, CloudNode, CredentialSource, DEFAULT_VOLUME_GB, DesktopOpen, Discovered,
    K8sInfo, K8sOptions, NodeLogs, NodeSpec, NodeState, OauthStart, ProviderError, ProviderInfo,
    ProviderKind, Quote, SIZES_GB, SizeList, SizeOption, Stage, TerminalOpen,
};

/// A cloud node's pairing ticket lives 15 minutes (a Droplet boots, installs packages and
/// downloads the release before it dials; the Settings pairing string keeps its 5).
pub const TICKET_TTL_MS: u64 = 15 * 60_000;

/// Where every event goes (the runtime bus in production, a vector in tests).
pub type Sink = Arc<dyn Fn(CloudEvent) + Send + Sync>;

/// Opens a provider client for a credential.
#[async_trait]
pub trait Connect: Send + Sync {
    /// A client for `kind` (`cred` for token providers, `k8s` for Kubernetes).
    fn open(
        &self,
        kind: ProviderKind,
        cred: Option<&StoredCred>,
        k8s: Option<&K8sOptions>,
    ) -> Result<Arc<dyn Provider>>;
    /// Is the token good (DigitalOcean: the account answers; Fly: listing works)?
    async fn check(&self, kind: ProviderKind, cred: &StoredCred) -> Result<()>;
    /// The provider's 2/4/8 GB sizes with live prices (DigitalOcean `GET /v2/sizes`).
    async fn sizes(
        &self,
        kind: ProviderKind,
        cred: &StoredCred,
        region: Option<&str>,
    ) -> Result<Vec<SizeOption>>;
}

/// The real provider clients over one HTTP transport.
pub struct LiveConnect {
    /// Shared by every client.
    pub http: Arc<dyn Transport>,
}

#[async_trait]
impl Connect for LiveConnect {
    fn open(
        &self,
        kind: ProviderKind,
        cred: Option<&StoredCred>,
        k8s: Option<&K8sOptions>,
    ) -> Result<Arc<dyn Provider>> {
        let token = || {
            cred.map(StoredCred::token)
                .ok_or_else(|| not_connected(kind))
        };
        match kind {
            ProviderKind::Digitalocean => Ok(Arc::new(providers::digitalocean::DigitalOcean::new(
                token()?,
                self.http.clone(),
            ))),
            ProviderKind::Fly => Ok(Arc::new(providers::fly::Fly::new(
                token()?,
                cred.and_then(|c| c.org.clone()),
                self.http.clone(),
            ))),
            #[cfg(feature = "kubernetes")]
            ProviderKind::Kubernetes => Ok(Arc::new(providers::kubernetes::Kubernetes::new(
                k8s.cloned().unwrap_or_default(),
            ))),
            #[cfg(not(feature = "kubernetes"))]
            ProviderKind::Kubernetes => {
                let _ = k8s;
                Err(CloudError("Kubernetes needs the desktop app".into()))
            }
            ProviderKind::Lightsail => Err(CloudError("AWS Lightsail is coming soon".into())),
        }
    }

    async fn check(&self, kind: ProviderKind, cred: &StoredCred) -> Result<()> {
        match kind {
            ProviderKind::Digitalocean => {
                providers::digitalocean::DigitalOcean::new(cred.token(), self.http.clone())
                    .account()
                    .await
                    .map(|_| ())
            }
            ProviderKind::Fly => {
                providers::fly::Fly::new(cred.token(), cred.org.clone(), self.http.clone())
                    .list()
                    .await
                    .map(|_| ())
            }
            _ => Ok(()),
        }
    }

    async fn sizes(
        &self,
        kind: ProviderKind,
        cred: &StoredCred,
        region: Option<&str>,
    ) -> Result<Vec<SizeOption>> {
        match kind {
            ProviderKind::Digitalocean => {
                providers::digitalocean::DigitalOcean::new(cred.token(), self.http.clone())
                    .sizes(region)
                    .await
            }
            _ => Ok(providers::sizes(kind)),
        }
    }
}

/// Timings of the jobs (tests shrink them).
#[derive(Clone, Copy, Debug)]
pub struct Pacing {
    /// How often the join wait reads the mesh.
    pub poll: Duration,
    /// After the node joined, how long to wait for it to be reachable before calling it ready.
    pub reachable_wait: Duration,
    /// Added to the ticket's expiry before the join wait gives up (mesh status lags a moment).
    pub grace_ms: u64,
    /// OAuth redeem polling.
    pub oauth_every: Duration,
    /// Redeem polls before giving up (`oauth_every` × this = 10 min).
    pub oauth_tries: u32,
    /// How long a provider's price list is reused (owner: 1 h).
    pub sizes_ttl: Duration,
}

impl Default for Pacing {
    fn default() -> Self {
        Self {
            poll: Duration::from_secs(5),
            reachable_wait: Duration::from_secs(120),
            grace_ms: 30_000,
            oauth_every: Duration::from_secs(2),
            oauth_tries: 300,
            sizes_ttl: Duration::from_secs(3600),
        }
    }
}

/// Seconds a stage usually takes, for the progress bar: `(server+storage, install)`.
/// DigitalOcean's install was measured at ≈230 s (224 §14); the others are estimates.
fn estimate_secs(kind: ProviderKind) -> (u32, u32) {
    match kind {
        ProviderKind::Digitalocean => (60, 230),
        ProviderKind::Fly => (40, 120),
        ProviderKind::Kubernetes | ProviderKind::Lightsail => (20, 90),
    }
}

/// Unix ms.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn not_connected(kind: ProviderKind) -> CloudError {
    CloudError(format!("{} is not connected", label(kind)))
}

/// What the UI calls the provider.
pub fn label(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Digitalocean => "DigitalOcean",
        ProviderKind::Fly => "Fly.io",
        ProviderKind::Kubernetes => "Kubernetes",
        ProviderKind::Lightsail => "AWS Lightsail",
    }
}

const ALL: [ProviderKind; 4] = [
    ProviderKind::Digitalocean,
    ProviderKind::Fly,
    ProviderKind::Kubernetes,
    ProviderKind::Lightsail,
];

/// Restart / resize / update.
#[derive(Clone, Copy, Debug)]
enum Op {
    Restart,
    Resize(u8),
    Update,
}

/// A price list's cache key: provider and region (`None`: every region).
type PriceKey = (ProviderKind, Option<String>);

/// The service: one per engine, shared by the RPC handlers and the jobs.
pub struct Cloud {
    records: Records,
    mesh: Arc<dyn MeshView>,
    creds: Creds,
    connect: Arc<dyn Connect>,
    sink: Sink,
    pace: Pacing,
    /// Where discovery looks (`None`: no home directory, e.g. iOS).
    home: Option<PathBuf>,
    /// Running jobs by node name.
    jobs: Mutex<HashMap<String, AbortHandle>>,
    /// Running OAuth polls.
    oauth: Mutex<HashMap<ProviderKind, AbortHandle>>,
    /// Live price lists by (provider, region), with when they were fetched. Failures are not
    /// kept.
    prices: Mutex<HashMap<PriceKey, (Instant, Vec<SizeOption>)>>,
    /// Serialises record read-modify-write.
    write: Mutex<()>,
}

impl Cloud {
    /// Wire the seams together; call [`Cloud::resume`] once afterwards.
    pub fn new(
        records: Records,
        mesh: Arc<dyn MeshView>,
        creds: Creds,
        connect: Arc<dyn Connect>,
        sink: Sink,
        pace: Pacing,
        home: Option<PathBuf>,
    ) -> Self {
        Self {
            records,
            mesh,
            creds,
            connect,
            sink,
            pace,
            home,
            jobs: Mutex::new(HashMap::new()),
            oauth: Mutex::new(HashMap::new()),
            prices: Mutex::new(HashMap::new()),
            write: Mutex::new(()),
        }
    }

    fn emit(&self, ev: CloudEvent) {
        (self.sink)(ev);
    }

    fn progress(
        &self,
        name: &str,
        stage: Option<Stage>,
        line: &str,
        pct: Option<u8>,
        eta: Option<u32>,
    ) {
        self.emit(CloudEvent::CloudNodeProgress {
            name: name.to_owned(),
            stage,
            line: line.to_owned(),
            pct,
            eta_secs: eta,
        });
    }

    /// Change one record on disk and publish the row.
    fn update(&self, name: &str, f: impl FnOnce(&mut Record)) -> Result<Record> {
        let rec = {
            let _w = self.write.lock();
            let mut rec = self
                .records
                .get(name)?
                .ok_or_else(|| CloudError(format!("no cloud node named {name}")))?;
            f(&mut rec);
            self.records.put(&rec)?;
            rec
        };
        self.emit(CloudEvent::CloudNodeChanged {
            node: view(&rec, None, None),
        });
        Ok(rec)
    }

    fn fail(&self, name: &str, error: &str) {
        tracing::warn!("cloudnodes: {name}: {error}");
        let r = self.update(name, |r| {
            r.state = NodeState::Failed;
            r.error = Some(error.to_owned());
        });
        if let Err(e) = r {
            tracing::warn!("cloudnodes: {name}: recording the failure: {e}");
        }
    }

    async fn provider(
        &self,
        kind: ProviderKind,
        k8s: Option<&K8sOptions>,
    ) -> Result<Arc<dyn Provider>> {
        let cred = match creds::row(kind) {
            Some(_) => Some(
                self.creds
                    .current(kind, now_ms())
                    .await?
                    .ok_or_else(|| not_connected(kind))?,
            ),
            None => None,
        };
        self.connect.open(kind, cred.as_ref(), k8s)
    }

    fn connected(&self, kind: ProviderKind) -> bool {
        matches!(self.creds.load(kind), Ok(Some(_)))
    }

    // --- credentials ----------------------------------------------------------------------

    fn discovered(&self) -> Vec<(ProviderKind, Discovered)> {
        self.home
            .as_deref()
            .map(providers::discovery::discover)
            .unwrap_or_default()
    }

    fn info_of(&self, kind: ProviderKind, found: &[(ProviderKind, Discovered)]) -> ProviderInfo {
        let discovered: Vec<Discovered> = found
            .iter()
            .filter(|(k, _)| *k == kind)
            .map(|(_, d)| d.clone())
            .collect();
        let stored = self
            .creds
            .load(kind)
            .ok()
            .flatten()
            .map(|(_, c)| c.source.clone());
        let kube = kind == ProviderKind::Kubernetes && !discovered.is_empty();
        ProviderInfo {
            provider: kind,
            label: label(kind).into(),
            available: match kind {
                ProviderKind::Lightsail => false,
                ProviderKind::Kubernetes => cfg!(feature = "kubernetes"),
                _ => true,
            },
            beta: kind == ProviderKind::Fly,
            desktop_only: kind == ProviderKind::Kubernetes,
            connected: stored.is_some() || kube,
            source: stored.or(kube.then_some(CredentialSource::Kubeconfig)),
            discovered,
            oauth: kind == ProviderKind::Digitalocean,
            sizes: providers::sizes(kind),
            regions: providers::regions(kind),
            volume_usd_per_gb: catalog::volume_usd_per_gb(kind),
        }
    }

    /// Every provider as the wizard shows it.
    pub fn providers(&self) -> Vec<ProviderInfo> {
        let found = self.discovered();
        ALL.iter().map(|k| self.info_of(*k, &found)).collect()
    }

    fn provider_info(&self, kind: ProviderKind) -> ProviderInfo {
        self.info_of(kind, &self.discovered())
    }

    async fn store_checked(&self, kind: ProviderKind, cred: StoredCred) -> Result<ProviderInfo> {
        if creds::row(kind).is_none() {
            return Err(CloudError(format!("{} stores no token", label(kind))));
        }
        self.connect.check(kind, &cred).await?;
        self.creds.save(kind, &cred).await?;
        self.emit(CloudEvent::CloudCredentialChanged {
            provider: kind,
            connected: true,
            error: None,
        });
        Ok(self.provider_info(kind))
    }

    /// Paste-a-token: checked against the provider before it is stored.
    pub async fn set_token(&self, kind: ProviderKind, token: String) -> Result<ProviderInfo> {
        let token = Zeroizing::new(token);
        let t = token.trim();
        if t.is_empty() {
            return Err(CloudError("the token is empty".into()));
        }
        self.store_checked(
            kind,
            StoredCred {
                access_token: t.to_owned(),
                refresh_token: None,
                expires_at_ms: None,
                source: CredentialSource::Pasted,
                org: None,
            },
        )
        .await
    }

    /// Copy a discovered CLI login into the vault (the user pressed "Use it").
    pub async fn import(&self, kind: ProviderKind, id: String) -> Result<ProviderInfo> {
        if kind == ProviderKind::Kubernetes {
            // kubectl reads the kubeconfig itself; nothing is copied.
            return Ok(self.provider_info(kind));
        }
        let home = self
            .home
            .as_deref()
            .ok_or_else(|| CloudError("no home directory to import from".into()))?;
        let found = providers::discovery::discover(home);
        if !found.iter().any(|(k, d)| *k == kind && d.id == id) {
            return Err(CloudError(format!(
                "no {} login {id:?} on this machine",
                label(kind)
            )));
        }
        let token = providers::discovery::read_secret(home, &id)
            .ok_or_else(|| CloudError(format!("{id}: no token to import")))?;
        self.store_checked(
            kind,
            StoredCred {
                access_token: token.to_string(),
                refresh_token: None,
                expires_at_ms: None,
                source: CredentialSource::Imported,
                org: None,
            },
        )
        .await
    }

    /// Remove the stored credential.
    pub async fn forget(&self, kind: ProviderKind) -> Result<()> {
        self.creds.forget(kind).await?;
        self.emit(CloudEvent::CloudCredentialChanged {
            provider: kind,
            connected: false,
            error: None,
        });
        Ok(())
    }

    /// "Sign in with DigitalOcean": the URL for the system browser; the engine polls the relay
    /// and announces the outcome as `CloudCredentialChanged`.
    pub fn oauth_start(self: &Arc<Self>, kind: ProviderKind) -> Result<OauthStart> {
        if kind != ProviderKind::Digitalocean {
            return Err(CloudError(format!(
                "{} has no sign-in; paste a token",
                label(kind)
            )));
        }
        let attempt = OauthAttempt::fresh();
        let url = attempt.start_url(&self.creds.relay);
        let me = self.clone();
        let task = tokio::spawn(async move {
            let r = creds::redeem(
                &*me.creds.http,
                &me.creds.relay,
                &attempt,
                me.pace.oauth_every,
                me.pace.oauth_tries,
                now_ms,
            )
            .await;
            let r = match r {
                Ok(cred) => me.creds.save(kind, &cred).await,
                Err(e) => Err(e),
            };
            me.oauth.lock().remove(&kind);
            me.emit(CloudEvent::CloudCredentialChanged {
                provider: kind,
                connected: r.is_ok() || me.connected(kind),
                error: r.err().map(|e| e.0),
            });
        });
        if let Some(old) = self.oauth.lock().insert(kind, task.abort_handle()) {
            old.abort();
        }
        Ok(OauthStart { url })
    }

    /// Stop waiting for a sign-in.
    pub fn oauth_cancel(&self, kind: ProviderKind) {
        if let Some(h) = self.oauth.lock().remove(&kind) {
            h.abort();
        }
    }

    // --- quote / k8s ---------------------------------------------------------------------

    /// The provider's sizes with live prices: fetched from its API (DigitalOcean `GET
    /// /v2/sizes`), reused for [`Pacing::sizes_ttl`]. Errors when there is no credential or the
    /// API fails (not cached).
    async fn priced_sizes(
        &self,
        kind: ProviderKind,
        region: Option<&str>,
    ) -> Result<Vec<SizeOption>> {
        if kind != ProviderKind::Digitalocean {
            return Err(CloudError(format!("{} has no price API here", label(kind))));
        }
        let key = (kind, region.map(str::to_owned));
        let cached = self
            .prices
            .lock()
            .get(&key)
            .filter(|(at, _)| at.elapsed() < self.pace.sizes_ttl)
            .map(|(_, s)| s.clone());
        if let Some(sizes) = cached {
            return Ok(sizes);
        }
        let cred = self
            .creds
            .current(kind, now_ms())
            .await?
            .ok_or_else(|| not_connected(kind))?;
        let sizes = self.connect.sizes(kind, &cred, region).await?;
        self.prices
            .lock()
            .insert(key, (Instant::now(), sizes.clone()));
        Ok(sizes)
    }

    /// `cloud_sizes`: live prices for DigitalOcean; the unpriced skeleton otherwise or when the
    /// fetch fails (`error` says why).
    pub async fn sizes(&self, kind: ProviderKind, region: Option<String>) -> SizeList {
        if kind != ProviderKind::Digitalocean {
            return SizeList {
                sizes: providers::sizes(kind),
                priced: false,
                error: None,
            };
        }
        match self.priced_sizes(kind, region.as_deref()).await {
            Ok(sizes) => SizeList {
                sizes,
                priced: true,
                error: None,
            },
            Err(e) => SizeList {
                sizes: providers::sizes(kind),
                priced: false,
                error: Some(e.0),
            },
        }
    }

    /// Monthly cost of `spec` from live prices (`total_usd: None` when they are unavailable).
    pub async fn quote(&self, spec: &NodeSpec) -> Result<Quote> {
        let kind = spec.provider;
        let sizes = self
            .priced_sizes(kind, Some(&spec.region))
            .await
            .unwrap_or_else(|_| providers::sizes(kind));
        providers::quote(
            kind,
            &sizes,
            spec.memory_gb,
            spec.volume_gb.unwrap_or(DEFAULT_VOLUME_GB),
        )
    }

    /// What the Kubernetes step needs about `context` (default: the current one).
    pub async fn k8s_info(&self, context: Option<String>) -> Result<K8sInfo> {
        #[cfg(feature = "kubernetes")]
        {
            providers::kubernetes::info(context.as_deref())
                .await
                .map_err(CloudError::from)
        }
        #[cfg(not(feature = "kubernetes"))]
        {
            let _ = context;
            Err(CloudError("Kubernetes needs the desktop app".into()))
        }
    }

    // --- create --------------------------------------------------------------------------

    /// Start the create job; returns at once with the record in `creating`.
    pub async fn create(self: &Arc<Self>, spec: NodeSpec) -> Result<CloudNode> {
        check_name(&spec.name)?;
        if !SIZES_GB.contains(&spec.memory_gb) {
            return Err(CloudError(format!(
                "size {} GB: one of {SIZES_GB:?} GB",
                spec.memory_gb
            )));
        }
        // Validates size and disk; the price is the wizard's business (`cloud_quote`).
        providers::quote(
            spec.provider,
            &providers::sizes(spec.provider),
            spec.memory_gb,
            spec.volume_gb.unwrap_or(DEFAULT_VOLUME_GB),
        )?;
        let prior = self.records.get(&spec.name)?;
        if let Some(r) = &prior
            && !matches!(r.state, NodeState::Failed | NodeState::VolumeKept)
        {
            return Err(CloudError(format!(
                "a cloud node named {} exists",
                spec.name
            )));
        }
        if let Some(r) = &prior
            && r.spec.provider != spec.provider
        {
            return Err(CloudError(format!(
                "{} is a {} node; destroy it first",
                spec.name,
                label(r.spec.provider)
            )));
        }
        let provider = self.provider(spec.provider, spec.k8s.as_ref()).await?;
        let peers = self.mesh.peers().await?;
        if peers.iter().any(|p| {
            p.name == spec.name && prior.as_ref().and_then(|r| r.peer_id.as_ref()) != Some(&p.id)
        }) {
            return Err(CloudError(format!(
                "a machine named {} is already in the mesh; pick another name",
                spec.name
            )));
        }
        let rec = Record {
            spec,
            created: None,
            state: NodeState::Creating,
            stage: Some(Stage::Server),
            error: None,
            // A node re-created on its kept volume comes back as the same engine.
            peer_id: prior.and_then(|r| r.peer_id),
            peers_before: peers.into_iter().map(|p| p.id).collect(),
            ticket_expires_ms: None,
            created_ms: now_ms(),
        };
        let name = rec.spec.name.clone();
        {
            let _w = self.write.lock();
            self.records.put(&rec)?;
        }
        let node = view(&rec, None, None);
        self.emit(CloudEvent::CloudNodeChanged { node: node.clone() });
        let me = self.clone();
        let job_name = name.clone();
        let task = tokio::spawn(async move {
            if let Err(e) = me.create_job(&job_name, provider).await {
                me.fail(&job_name, &e.0);
            }
            me.jobs.lock().remove(&job_name);
        });
        self.jobs.lock().insert(name, task.abort_handle());
        Ok(node)
    }

    async fn create_job(&self, name: &str, provider: Arc<dyn Provider>) -> Result<()> {
        let rec = self
            .records
            .get(name)?
            .ok_or_else(|| CloudError(format!("no cloud node named {name}")))?;
        let kind = rec.spec.provider;
        let (server_secs, install_secs) = estimate_secs(kind);
        self.progress(
            name,
            Some(Stage::Server),
            "Minting a one-time pairing ticket",
            Some(2),
            Some(server_secs + install_secs),
        );
        let ticket = Zeroizing::new(self.mesh.remote_ticket(TICKET_TTL_MS).await?);
        let expires = now_ms() + TICKET_TTL_MS;
        self.update(name, |r| r.ticket_expires_ms = Some(expires))?;
        let req = CreateReq {
            name: name.to_owned(),
            region: rec.spec.region.clone(),
            memory_gb: rec.spec.memory_gb,
            volume_gb: rec.spec.volume_gb.unwrap_or(DEFAULT_VOLUME_GB),
            ticket: ticket.to_string(),
            manifest: catalog::MANIFEST.into(),
            image: catalog::ENGINE_IMAGE.into(),
            k8s: rec.spec.k8s.clone(),
            desktop: rec.spec.desktop.unwrap_or(true),
        };
        let stage = Mutex::new(Stage::Server);
        let say = |s: Stage, line: &str| {
            let advanced = {
                let mut cur = stage.lock();
                let adv = s > *cur;
                if adv {
                    *cur = s;
                }
                adv
            };
            if advanced && let Err(e) = self.update(name, |r| r.stage = Some(s)) {
                tracing::warn!("cloudnodes: {name}: {e}");
            }
            let (pct, eta) = match s {
                Stage::Server => (10, server_secs + install_secs),
                _ => (25, server_secs / 3 + install_secs),
            };
            self.progress(name, Some(s), line, Some(pct), Some(eta));
        };
        let created = provider.create(&req, &say).await?;
        drop(req);
        self.update(name, |r| {
            r.created = Some(created);
            r.stage = Some(Stage::Install);
        })?;
        self.progress(
            name,
            Some(Stage::Install),
            "Installing the newest signed theuth release",
            Some(30),
            Some(install_secs),
        );
        self.wait_join(name).await
    }

    /// Install → Join → Ready: wait for the node to appear in the mesh (until the ticket's
    /// deadline), then to be reachable.
    async fn wait_join(&self, name: &str) -> Result<()> {
        let rec = self
            .records
            .get(name)?
            .ok_or_else(|| CloudError(format!("no cloud node named {name}")))?;
        let (_, install_secs) = estimate_secs(rec.spec.provider);
        let started = Instant::now();
        let joined = loop {
            match self.mesh.peers().await {
                Ok(peers) => {
                    if let Some(p) = joined_peer(&peers, &rec) {
                        break p;
                    }
                }
                Err(e) => tracing::debug!("cloudnodes: {name}: {e}"),
            }
            // Re-read: a destroy removes the record (and ends this wait).
            let deadline = self
                .records
                .get(name)?
                .ok_or_else(|| CloudError(format!("{name} was removed")))?
                .ticket_expires_ms
                .unwrap_or(rec.created_ms + TICKET_TTL_MS)
                .saturating_add(self.pace.grace_ms);
            if now_ms() >= deadline {
                return Err(CloudError(
                    "the node did not join the mesh before its 15-minute pairing ticket expired \
                     (its install failed or was too slow); destroy it and create it again"
                        .into(),
                ));
            }
            let elapsed = u32::try_from(started.elapsed().as_secs()).unwrap_or(u32::MAX);
            let frac = (f64::from(elapsed) / f64::from(install_secs.max(1))).min(0.95);
            let pct = 30 + (frac * 55.0) as u8;
            let eta = install_secs.saturating_sub(elapsed).max(10);
            self.progress(
                name,
                Some(Stage::Install),
                "Waiting for the node to install theuth and dial the mesh",
                Some(pct),
                Some(eta),
            );
            tokio::time::sleep(self.pace.poll).await;
        };
        self.update(name, |r| {
            r.peer_id = Some(joined.id.clone());
            r.stage = Some(Stage::Join);
        })?;
        self.progress(
            name,
            Some(Stage::Join),
            "Joined the mesh; connecting",
            Some(90),
            Some(10),
        );
        let until = Instant::now() + self.pace.reachable_wait;
        let mut reachable = joined.reachable;
        while !reachable && Instant::now() < until {
            tokio::time::sleep(self.pace.poll).await;
            reachable = self
                .mesh
                .peers()
                .await
                .ok()
                .and_then(|ps| ps.into_iter().find(|p| p.id == joined.id))
                .is_some_and(|p| p.reachable);
        }
        self.update(name, |r| {
            r.state = NodeState::Ready;
            r.stage = Some(Stage::Ready);
            r.error = None;
        })?;
        self.progress(
            name,
            Some(Stage::Ready),
            if reachable {
                "Ready"
            } else {
                "Joined; not reachable yet"
            },
            Some(100),
            Some(0),
        );
        Ok(())
    }

    /// After an engine restart: pick up interrupted jobs. A create that got past the server
    /// waits for the join again (the ticket may still be live); one interrupted earlier fails
    /// (destroy cleans up whatever was made); an interrupted restart/resize settles.
    pub fn resume(self: &Arc<Self>) {
        for rec in self.records.all() {
            let name = rec.spec.name.clone();
            match rec.state {
                NodeState::Creating if rec.created.is_some() => {
                    let me = self.clone();
                    let job_name = name.clone();
                    let task = tokio::spawn(async move {
                        if let Err(e) = me.wait_join(&job_name).await {
                            me.fail(&job_name, &e.0);
                        }
                        me.jobs.lock().remove(&job_name);
                    });
                    self.jobs.lock().insert(name, task.abort_handle());
                }
                NodeState::Creating => self.fail(
                    &name,
                    "the engine restarted while the server was being created; destroy it and create it again",
                ),
                NodeState::Busy => {
                    let _ = self.update(&name, |r| {
                        r.state = if r.peer_id.is_some() { NodeState::Ready } else { NodeState::Failed };
                    });
                }
                NodeState::Destroying => {
                    self.fail(&name, "the engine restarted during destroy; destroy it again")
                }
                _ => {}
            }
        }
    }

    // --- list / status -------------------------------------------------------------------

    /// Records, plus every connected provider's tagged nodes (orphans), plus the mesh.
    pub async fn list(&self) -> CloudList {
        let records = self.records.all();
        let peers = self.mesh.peers().await.unwrap_or_default();
        let mut errors = Vec::new();
        let mut listed: Vec<(ProviderKind, Listed)> = Vec::new();
        let mut targets: Vec<(ProviderKind, Option<K8sOptions>)> =
            [ProviderKind::Digitalocean, ProviderKind::Fly]
                .into_iter()
                .filter(|k| self.connected(*k))
                .map(|k| (k, None))
                .collect();
        // Kubernetes: the contexts this engine's nodes run in (each across all namespaces).
        let contexts: BTreeSet<Option<String>> = records
            .iter()
            .filter(|r| r.spec.provider == ProviderKind::Kubernetes)
            .map(|r| r.spec.k8s.as_ref().and_then(|k| k.context.clone()))
            .collect();
        for context in contexts {
            targets.push((
                ProviderKind::Kubernetes,
                Some(K8sOptions {
                    context,
                    ..K8sOptions::default()
                }),
            ));
        }
        for (kind, k8s) in targets {
            let r = match self.provider(kind, k8s.as_ref()).await {
                Ok(p) => p.list().await,
                Err(e) => Err(e),
            };
            match r {
                Ok(v) => listed.extend(v.into_iter().map(|l| (kind, l))),
                Err(e) => errors.push(ProviderError {
                    provider: kind,
                    message: e.0,
                }),
            }
        }
        let do_sizes = if self.connected(ProviderKind::Digitalocean) {
            self.priced_sizes(ProviderKind::Digitalocean, None)
                .await
                .ok()
        } else {
            None
        };
        let mut nodes = merge(&records, &listed, &peers, do_sizes.as_deref());
        self.fill_versions(&mut nodes).await;
        CloudList { nodes, errors }
    }

    /// One node: its record (or the orphan the providers list), the provider's view, the mesh.
    pub async fn status(&self, name: &str) -> Result<CloudNode> {
        check_name(name)?;
        let peers = self.mesh.peers().await.unwrap_or_default();
        let Some(rec) = self.records.get(name)? else {
            return self
                .list()
                .await
                .nodes
                .into_iter()
                .find(|n| n.name == name)
                .ok_or_else(|| CloudError(format!("no cloud node named {name}")));
        };
        let status = match self
            .provider(rec.spec.provider, rec.spec.k8s.as_ref())
            .await
        {
            Ok(p) => p.status(name).await.ok().flatten(),
            Err(_) => None,
        };
        let peer = peer_of(&rec, &peers);
        let mut node = view(&rec, peer, status.as_ref());
        let sizes = self.priced_sizes(rec.spec.provider, None).await.ok();
        node.monthly_usd = monthly(
            rec.spec.provider,
            sizes.as_deref(),
            node.size_slug.as_deref(),
            node.volume_gb.unwrap_or(DEFAULT_VOLUME_GB),
            rec.state == NodeState::VolumeKept,
        );
        self.fill_versions(std::slice::from_mut(&mut node)).await;
        Ok(node)
    }

    /// `t --version` of every reachable node (`info` over the mesh), 3 s each, in parallel.
    async fn fill_versions(&self, nodes: &mut [CloudNode]) {
        let calls = nodes.iter().map(|n| async {
            let id = n.peer_id.as_deref().filter(|_| n.reachable == Some(true))?;
            let v = self
                .mesh
                .call(id, "info", json!([]), Duration::from_secs(3))
                .await
                .ok()
                .flatten()?;
            v.get("version").and_then(Value::as_str).map(str::to_owned)
        });
        let versions = futures::future::join_all(calls).await;
        for (n, v) in nodes.iter_mut().zip(versions) {
            n.version = v;
        }
    }

    // --- restart / resize / update ---------------------------------------------------------

    /// Reboot the node's server (detached; returns `busy`).
    pub async fn restart(self: &Arc<Self>, name: &str) -> Result<CloudNode> {
        self.op(name, Op::Restart).await
    }

    /// Change the size (detached; returns `busy`).
    pub async fn resize(self: &Arc<Self>, name: &str, memory_gb: u8) -> Result<CloudNode> {
        if !SIZES_GB.contains(&memory_gb) {
            return Err(CloudError(format!(
                "size {memory_gb} GB: one of {SIZES_GB:?} GB"
            )));
        }
        self.op(name, Op::Resize(memory_gb)).await
    }

    /// No "install the update now" RPC exists on an engine (crates/update only reports), so an
    /// update is a restart: a VM's `ExecStartPre` and the image's entrypoint install the newest
    /// signed release before the engine starts (224 §5).
    pub async fn update_now(self: &Arc<Self>, name: &str) -> Result<CloudNode> {
        self.op(name, Op::Update).await
    }

    async fn op(self: &Arc<Self>, name: &str, op: Op) -> Result<CloudNode> {
        let rec = self
            .records
            .get(name)?
            .ok_or_else(|| CloudError(format!("no cloud node named {name}")))?;
        if !matches!(rec.state, NodeState::Ready | NodeState::Failed) {
            return Err(CloudError(format!("{name} is busy")));
        }
        let provider = self
            .provider(rec.spec.provider, rec.spec.k8s.as_ref())
            .await?;
        let before = rec.state.clone();
        let rec = self.update(name, |r| r.state = NodeState::Busy)?;
        let me = self.clone();
        let job_name = name.to_owned();
        let task = tokio::spawn(async move {
            let name = job_name.as_str();
            let line = match op {
                Op::Restart => "Restarting".to_owned(),
                Op::Resize(gb) => format!("Resizing to {gb} GB (the node restarts)"),
                Op::Update => "Restarting to install the newest release".to_owned(),
            };
            me.progress(name, None, &line, None, None);
            let outcome = match op {
                Op::Restart | Op::Update => provider.restart(name).await.map(|()| None),
                Op::Resize(gb) => provider.resize(name, gb).await.map(|slug| Some((gb, slug))),
            };
            let res = me.update(name, |r| {
                r.state = before;
                match &outcome {
                    Ok(Some((gb, slug))) => {
                        r.spec.memory_gb = *gb;
                        if let Some(c) = r.created.as_mut() {
                            c.size_slug = slug.clone();
                        }
                        r.error = None;
                    }
                    Ok(None) => r.error = None,
                    Err(_) => {}
                }
            });
            match outcome {
                Ok(_) => me.progress(name, None, "Done", None, None),
                Err(e) => me.progress(name, None, &format!("Failed: {e}"), None, None),
            }
            if let Err(e) = res {
                tracing::warn!("cloudnodes: {name}: {e}");
            }
            me.jobs.lock().remove(name);
        });
        self.jobs
            .lock()
            .insert(name.to_owned(), task.abort_handle());
        Ok(view(&rec, None, None))
    }

    // --- terminal / logs -----------------------------------------------------------------

    async fn peer_id(&self, name: &str) -> Result<String> {
        check_name(name)?;
        if let Some(id) = self.records.get(name)?.and_then(|r| r.peer_id) {
            return Ok(id);
        }
        let peers = self.mesh.peers().await?;
        let mut named = peers.into_iter().filter(|p| p.name == name);
        match (named.next(), named.next()) {
            (Some(p), None) => Ok(p.id),
            (None, _) => Err(CloudError(format!("{name} has not joined the mesh"))),
            (Some(_), Some(_)) => Err(CloudError(format!("more than one machine is named {name}"))),
        }
    }

    /// A shell on the node's engine (`engine_terminal` there), over the mesh: no SSH.
    pub async fn terminal(&self, name: &str) -> Result<TerminalOpen> {
        let id = self.peer_id(name).await?;
        let v = self
            .mesh
            .call(
                &id,
                "engine_terminal",
                json!([null]),
                Duration::from_secs(30),
            )
            .await?
            .ok_or_else(|| CloudError(format!("{name} is not connected")))?;
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
        Ok(TerminalOpen {
            session: s("session").ok_or_else(|| CloudError("the node opened no session".into()))?,
            job: s("job").ok_or_else(|| CloudError("the node opened no job".into()))?,
            engine: s("engine").unwrap_or(id),
        })
    }

    /// The tail of the node engine's log (`engine_logs` there).
    pub async fn logs(&self, name: &str, tail: Option<u32>) -> Result<NodeLogs> {
        let id = self.peer_id(name).await?;
        let v = self
            .mesh
            .call(
                &id,
                "engine_logs",
                json!([null, tail]),
                Duration::from_secs(30),
            )
            .await?
            .ok_or_else(|| CloudError(format!("{name} is not connected")))?;
        let lines = v
            .get("lines")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|l| l.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        Ok(NodeLogs {
            name: name.to_owned(),
            lines,
        })
    }

    /// The node's desktop, watched through a loopback tunnel over the mesh (`engine_desktop`).
    pub async fn desktop(&self, name: &str) -> Result<DesktopOpen> {
        if let Some(rec) = self.records.get(name)?
            && rec.spec.desktop == Some(false)
        {
            return Err(CloudError(format!("desktop is off for {name}")));
        }
        let id = self.peer_id(name).await?;
        self.mesh.desktop(&id).await
    }

    // --- destroy -------------------------------------------------------------------------

    /// Delete the node at its provider (the disk too unless `keep_volume`). Deleting the disk
    /// also unpairs the node; a kept disk keeps the pairing, because the node re-created on it
    /// comes back as the same engine and does not pair again (its volume says it is paired).
    pub async fn destroy(&self, name: &str, keep_volume: bool) -> Result<Vec<String>> {
        check_name(name)?;
        if let Some(h) = self.jobs.lock().remove(name) {
            h.abort();
        }
        let rec = self.records.get(name)?;
        let (kind, k8s) = match &rec {
            Some(r) => (r.spec.provider, r.spec.k8s.clone()),
            None => {
                let orphan = self
                    .list()
                    .await
                    .nodes
                    .into_iter()
                    .find(|n| n.name == name)
                    .ok_or_else(|| CloudError(format!("no cloud node named {name}")))?;
                (orphan.provider, orphan.k8s)
            }
        };
        let provider = self.provider(kind, k8s.as_ref()).await?;
        if rec.is_some() {
            self.update(name, |r| r.state = NodeState::Destroying)?;
        }
        self.progress(name, None, "Destroying", None, None);
        let mut lines = match provider.destroy(name, keep_volume).await {
            Ok(lines) => lines,
            Err(e) => {
                if rec.is_some() {
                    self.fail(name, &format!("destroy: {e}"));
                }
                return Err(e);
            }
        };
        if !keep_volume {
            let peer = match rec.as_ref().and_then(|r| r.peer_id.clone()) {
                Some(id) => Some(id),
                None => self.peer_id(name).await.ok(),
            };
            if let Some(id) = peer {
                match self.mesh.remove(&id).await {
                    Ok(()) => lines.push(format!("Removed {name} from the mesh")),
                    Err(e) => lines.push(format!("{name} is still a mesh peer: {e}")),
                }
            }
        }
        if keep_volume && rec.is_some() {
            self.update(name, |r| {
                r.state = NodeState::VolumeKept;
                r.stage = None;
                r.error = None;
                if let Some(c) = r.created.as_mut() {
                    c.server_id = None;
                    c.ip = None;
                }
            })?;
        } else {
            {
                let _w = self.write.lock();
                self.records.remove(name)?;
            }
            self.emit(CloudEvent::CloudNodeRemoved {
                name: name.to_owned(),
            });
        }
        for l in &lines {
            self.progress(name, None, l, None, None);
        }
        Ok(lines)
    }
}

/// The peer the create job waits for: a new engine with the node's name, or (re-created on a
/// kept volume) its old identity back and reachable.
fn joined_peer(peers: &[MeshPeer], rec: &Record) -> Option<MeshPeer> {
    peers
        .iter()
        .find(|p| {
            (p.name == rec.spec.name && !rec.peers_before.contains(&p.id))
                || (rec.peer_id.as_deref() == Some(p.id.as_str()) && p.reachable)
        })
        .cloned()
}

fn peer_of<'a>(rec: &Record, peers: &'a [MeshPeer]) -> Option<&'a MeshPeer> {
    let id = rec.peer_id.as_deref()?;
    peers.iter().find(|p| p.id == id)
}

/// What a node costs a month: its server's live price (by its size slug) plus its disk; a kept
/// disk alone. `None` when the price is unknown (no live list, not a DigitalOcean node).
fn monthly(
    kind: ProviderKind,
    sizes: Option<&[SizeOption]>,
    slug: Option<&str>,
    volume_gb: u32,
    disk_only: bool,
) -> Option<f64> {
    let disk = catalog::volume_usd_per_gb(kind)? * f64::from(volume_gb);
    let server = if disk_only {
        0.0
    } else {
        sizes?
            .iter()
            .find(|s| Some(s.slug.as_str()) == slug)?
            .monthly_usd?
    };
    Some(((server + disk) * 100.0).round() / 100.0)
}

/// One row from a record, with the mesh and provider facts when known. No price: that needs
/// the live list ([`merge`], `Cloud::status`).
pub fn view(rec: &Record, peer: Option<&MeshPeer>, status: Option<&ProviderStatus>) -> CloudNode {
    let kind = rec.spec.provider;
    let volume_gb = rec.spec.volume_gb.unwrap_or(DEFAULT_VOLUME_GB);
    CloudNode {
        name: rec.spec.name.clone(),
        provider: kind,
        region: (kind != ProviderKind::Kubernetes).then(|| rec.spec.region.clone()),
        region_label: region_label(kind, &rec.spec.region),
        memory_gb: Some(rec.spec.memory_gb),
        size_slug: rec
            .created
            .as_ref()
            .map(|c| c.size_slug.clone())
            .filter(|s| !s.is_empty())
            .or_else(|| catalog::slug(kind, rec.spec.memory_gb).ok()),
        volume_gb: Some(volume_gb),
        monthly_usd: None,
        state: rec.state.clone(),
        stage: rec.stage,
        error: rec.error.clone(),
        provider_state: status.and_then(|s| s.server_state.clone()),
        ip: status
            .and_then(|s| s.ip.clone())
            .or_else(|| rec.created.as_ref().and_then(|c| c.ip.clone())),
        peer_id: rec.peer_id.clone(),
        reachable: peer
            .map(|p| p.reachable)
            .or(rec.peer_id.as_ref().map(|_| false)),
        path: peer.map(|p| p.path),
        version: None,
        auto_update: true,
        desktop: rec.spec.desktop.unwrap_or(true),
        created_ms: Some(rec.created_ms),
        k8s: rec.spec.k8s.clone(),
    }
}

fn region_label(kind: ProviderKind, region: &str) -> Option<String> {
    providers::regions(kind)
        .into_iter()
        .find(|r| r.id == region)
        .map(|r| r.label)
}

/// Records first (with what their provider listed and the mesh says), then what the providers
/// list that no record names: orphans, or kept volumes. `do_sizes`: DigitalOcean's live prices.
pub fn merge(
    records: &[Record],
    listed: &[(ProviderKind, Listed)],
    peers: &[MeshPeer],
    do_sizes: Option<&[SizeOption]>,
) -> Vec<CloudNode> {
    let sizes = |kind: ProviderKind| do_sizes.filter(|_| kind == ProviderKind::Digitalocean);
    let mut nodes: Vec<CloudNode> = records
        .iter()
        .map(|rec| {
            let mut n = view(rec, peer_of(rec, peers), None);
            if let Some((_, l)) = listed
                .iter()
                .find(|(k, l)| *k == rec.spec.provider && l.name == rec.spec.name)
            {
                n.provider_state = l.server_state.clone();
                n.ip = l.ip.clone().or(n.ip);
            }
            n.monthly_usd = monthly(
                n.provider,
                sizes(n.provider),
                n.size_slug.as_deref(),
                n.volume_gb.unwrap_or(DEFAULT_VOLUME_GB),
                rec.state == NodeState::VolumeKept,
            );
            n
        })
        .collect();
    for (kind, l) in listed {
        if records
            .iter()
            .any(|r| r.spec.provider == *kind && r.spec.name == l.name)
        {
            continue;
        }
        let mut named = peers.iter().filter(|p| p.name == l.name);
        let peer = match (named.next(), named.next()) {
            (Some(p), None) => Some(p),
            _ => None,
        };
        let memory_gb = l.memory_gb;
        let size_slug = memory_gb.and_then(|gb| catalog::slug(*kind, gb).ok());
        nodes.push(CloudNode {
            name: l.name.clone(),
            provider: *kind,
            region: l.region.clone(),
            region_label: l.region.as_deref().and_then(|r| region_label(*kind, r)),
            memory_gb,
            monthly_usd: monthly(
                *kind,
                sizes(*kind),
                size_slug.as_deref(),
                l.volume_gb.unwrap_or(DEFAULT_VOLUME_GB),
                l.volume_only,
            ),
            size_slug,
            volume_gb: l.volume_gb,
            state: if l.volume_only {
                NodeState::VolumeKept
            } else {
                NodeState::Orphan
            },
            stage: None,
            error: None,
            provider_state: l.server_state.clone(),
            ip: l.ip.clone(),
            peer_id: peer.map(|p| p.id.clone()),
            reachable: peer.map(|p| p.reachable),
            path: peer.map(|p| p.path),
            version: None,
            auto_update: true,
            // Unknown for a node this engine did not make.
            desktop: false,
            created_ms: None,
            k8s: None,
        });
    }
    nodes
}

#[cfg(test)]
mod tests;
