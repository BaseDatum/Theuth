//! The hosting engine's share service (210 §5.1, §5.9, §7.4, §7.6): publish, stop, Stop all,
//! the routing table and certificates, and the [`Attachment`] side of the edge state machine.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use anyhow::Context as _;
use parking_lot::Mutex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use share_proto::{
    ErrorCode, Event, Hello, Info, KillReason, NameInfo, NameKind, NameState, Welcome, names,
};
use tokio::sync::watch;

use crate::config::{EdgeChoice, NO_SHARING_EDGE, ShareConfig};
use crate::edge::{Attachment, Control, CsrAnswer, DelegationSource, EdgeDialer, EdgeError, Stream};
use crate::gate::{self, Gate, Operator};
use crate::keys::{self, NameKeyStore};
use crate::model::{
    Access, ApprovedBy, CreatedBy, EdgeState, EdgeTerms, Share, ShareChanged, ShareState,
    ShareStatus, Source, StoppedBy,
};
use crate::proxy::Proxy;
use crate::routes::{Lookup, Mount, Table};
use crate::sources::{self, DirPin, OwnPorts, PortGuard, ProcessCheck};
use crate::store::Store;
use crate::tools::Plan;
use crate::visitor::{self, Certs};

/// "Public sharing is off" (§7.1).
pub const SWITCH_OFF: &str = "Public sharing is off in Settings › Sharing";
/// A previewed publish whose shown name was claimed by someone else before the owner's
/// `Allow` landed (FIX6 N3).
pub const NAME_TAKEN_MEANWHILE: &str = "the name shown was taken meanwhile; publish again";
/// How long a publish waits for the edge.
pub const ATTACH_WAIT: Duration = Duration::from_secs(30);
/// How long a publish waits for the certificate step (vault key, CSR) before answering
/// `pending`; the step goes on in the background and the share goes live via `share.changed`
/// (E2E1 finding 9: a fresh name key settles for `REKEY_SETTLE_SECS`).
pub const PUBLISH_CERT_WAIT: Duration = Duration::from_millis(1500);
/// How long Stop all waits to attach before handing the edge stop to the lease holder.
pub const STOP_ALL_ATTACH_WAIT: Duration = Duration::from_secs(10);
/// How long a `POST /v1/names/status` answer serves `share_names_list` while detached.
pub const NAMES_STATUS_FRESH_MS: u64 = 30_000;
/// How long `share_names_list` waits for a `POST /v1/names/status` answer while detached.
pub const NAMES_STATUS_WAIT: Duration = Duration::from_secs(5);
/// The approval cards Stop all and the switch turned off withdraw.
const PUBLISH_TOOL: &str = "share_publish";
/// `approved_by.surface` of an act the owner pressed in their own client ([`Consent::of_press`]).
pub const PRESS: &str = "press";

/// Who asked for a publish.
#[derive(Clone, Debug)]
pub enum CallOrigin {
    /// A tool call.
    Call {
        /// Session.
        session: String,
        /// Call id.
        call_id: String,
    },
    /// Settings or the card.
    Settings,
}

/// Proof that the owner, in person, approved an act (§7.2): the only way to publish, switch
/// sharing on, change the edge, accept terms, reserve or transfer a name. Built from the tool
/// registry's scope ([`Consent::of_call`]), by raising the human-only row ([`Consent::ask`]), or
/// from the owner's own press in a UI ([`Consent::of_press`], TERMS-FIRST); an empty or
/// non-owner row is refused, so the ask cannot be skipped by an agent.
#[derive(Clone, Debug)]
pub struct Consent(ApprovedBy, u64);

impl Consent {
    fn from_approved(a: ::tools::Approved, asked_ms: Option<u64>) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !a.id.0.is_nil() && a.surface == "owner",
            "this needs the owner's own approval"
        );
        // The row's id is a v7 UUID minted when the card was raised: its timestamp is when the
        // owner was asked, for a row the registry raised before the tool body ran.
        let asked_ms = asked_ms.unwrap_or_else(|| {
            a.id.0
                .get_timestamp()
                .map_or(0, |t| {
                    let (s, n) = t.to_unix();
                    s * 1000 + u64::from(n) / 1_000_000
                })
        });
        Ok(Self(
            ApprovedBy {
                approval_id: a.id.0.to_string(),
                surface: a.surface.to_owned(),
            },
            asked_ms,
        ))
    }

    /// The human-only row the running tool call passed; `None` outside a gated body.
    pub fn of_call() -> Option<Self> {
        ::tools::approved().and_then(|a| Self::from_approved(a, None).ok())
    }

    /// Raise a human-only row for `what` (an RPC acting for the owner) and wait for `Allow`.
    pub async fn ask(what: &str, args: serde_json::Value) -> anyhow::Result<Self> {
        let asked_ms = platform::time::now_ms();
        let a = ::tools::ask_human_only(::tools::approval::RequestApproval {
            session: runtime::SessionId(uuid::Uuid::nil()),
            tool: what.to_owned(),
            args,
            risk: ::tools::Risk::Destructive,
            call_id: None,
        })
        .await?;
        Self::from_approved(a, Some(asked_ms))
    }

    /// The owner pressed the button in their own client (TERMS-FIRST): the call's
    /// connection is a human press in the sense of 171 §12.1 invariant 1 (as
    /// `cells_guard::press`) — an authenticated peer that is neither a share-link guest nor the
    /// internal peer. The internal peer is the MCP owner console, the ACP agent half, the TUI and
    /// every family-forwarded call, so an agent never gets here; those callers [`Consent::ask`].
    /// What remains is the owner's desktop/app socket (bearer token) or a paired device (Noise):
    /// the same set that can resolve a human-only row (`Approval`'s `Resolve`, the 210 §7.2
    /// token residual), so skipping the card for them grants nothing the card withheld.
    pub fn of_press(ext: &jsonrpsee::Extensions) -> Option<Self> {
        let p = ext.get::<rpc_core::RpcPeer>()?;
        if p.guest().is_some() || p.is_internal() || p.is_closed() {
            return None;
        }
        Some(Self(
            ApprovedBy {
                approval_id: uuid::Uuid::now_v7().to_string(),
                surface: PRESS.to_owned(),
            },
            platform::time::now_ms(),
        ))
    }

    /// `Self::of_press`, else raise the human-only row for `what`.
    pub async fn press_or_ask(
        ext: &jsonrpsee::Extensions,
        what: &str,
        args: serde_json::Value,
    ) -> anyhow::Result<Self> {
        match Self::of_press(ext) {
            Some(c) => Ok(c),
            None => Self::ask(what, args).await,
        }
    }

    /// The row, for `approved_by`.
    pub fn approved_by(&self) -> &ApprovedBy {
        &self.0
    }

    /// When the owner was asked, Unix ms: a Stop all at or after it voids the approval (§7.4).
    pub fn asked_ms(&self) -> u64 {
        self.1
    }

    #[cfg(test)]
    pub(crate) fn test() -> Self {
        Self::from_approved(
            ::tools::Approved {
                id: ::tools::ApprovalId(uuid::Uuid::now_v7()),
                surface: "owner",
            },
            None,
        )
        .unwrap()
    }

    #[cfg(test)]
    pub(crate) fn test_asked_at(asked_ms: u64) -> Self {
        let mut c = Self::test();
        c.1 = asked_ms;
        c
    }
}

/// The outcome of Stop all: local shares stopped, and whether the edge confirmed the mesh-wide
/// stop (`edge_pending` = not yet; it is sent again at the next attach).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StopAllOutcome {
    /// Local shares stopped.
    pub stopped: usize,
    /// Why the edge has not confirmed the stop yet.
    pub edge_pending: Option<String>,
}

/// The `<name>.D` a share's host is under (`app.sky.D` → `sky.D`).
pub fn name_host_of(host: &str, domain: &str) -> String {
    let rest = host.strip_suffix(domain).unwrap_or(host).trim_end_matches('.');
    let name = rest.rsplit('.').next().unwrap_or(rest);
    names::host(name, None, domain)
}

/// A publish result; `password` only for `access: password`, for the owner's surfaces.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Published {
    /// The record.
    pub share: Share,
    /// The generated password.
    pub password: Option<String>,
    /// While `share.state` is `pending`: where the certificate stands, as far as known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<PendingCert>,
}

/// A pending share's certificate: the edge's queue position and estimate when it answered in
/// time, both `None` while the certificate step still runs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PendingCert {
    /// Position in the edge's certificate queue.
    pub queue_pos: Option<u32>,
    /// Estimated seconds to a certificate.
    pub eta_s: Option<u64>,
}

/// A publish's name and exact URL, computed before the owner is asked (§7.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishPreview {
    /// The `<name>` of `<label>.<name>.D`.
    pub name: String,
    /// The exact URL the share will have.
    pub url: String,
    /// The name was drawn locally and is not claimed yet: [`Engine::publish_previewed`] claims
    /// it after the owner's `Allow` (and fails if it was taken meanwhile).
    pub drawn: bool,
}

/// A random name a publish claimed at the edge: released (on its own task) when dropped before
/// [`Claim::keep`], i.e. when the publish fails or its caller goes away before the row is stored.
struct Claim {
    engine: std::sync::Weak<Engine>,
    name: Option<String>,
}

impl Claim {
    /// The share row is stored: the name stays claimed.
    fn keep(mut self) {
        self.name = None;
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        let Some(name) = self.name.take() else { return };
        let (Some(me), Ok(rt)) = (self.engine.upgrade(), tokio::runtime::Handle::try_current())
        else {
            return;
        };
        rt.spawn(async move { me.release_unused(&name).await });
    }
}

/// Where [`Engine::ensure_cert`] left a name.
enum CertStep {
    Installed,
    Pending(PendingCert),
}

/// Seams other crates implement (see the as-built doc).
pub struct Hooks {
    /// `mesh`'s `Net::dial_alpn`.
    pub dialer: Option<Arc<dyn EdgeDialer>>,
    /// The delegation signer (M1: [`crate::edge::SelfHolder`]).
    pub delegation: Option<Arc<dyn DelegationSource>>,
    /// Name keys in the vault.
    pub name_keys: Option<Arc<dyn NameKeyStore>>,
    /// Own-listener registry.
    pub own_ports: Arc<dyn OwnPorts>,
    /// Process-tree check.
    pub process: Arc<dyn ProcessCheck>,
}

struct Link {
    control: Control,
    info: Info,
    welcome: Welcome,
}

/// The service.
pub struct Engine {
    me: std::sync::Weak<Self>,
    /// Config defaults.
    pub cfg: ShareConfig,
    /// `share.sqlite`.
    pub store: Store,
    data_dir: PathBuf,
    engine_id: String,
    table: Arc<Table>,
    certs: Arc<Certs>,
    gate: Arc<Gate>,
    ports: PortGuard,
    proxy: Arc<Proxy>,
    tls: tokio_rustls::TlsAcceptor,
    hooks: Hooks,
    rt: Option<runtime::Runtime>,
    need: watch::Sender<bool>,
    terms: watch::Sender<Option<String>>,
    link: Mutex<Option<Arc<Link>>>,
    attached: watch::Sender<bool>,
    /// Publishes admitted but not yet stored; read with the store under `admit` (m6).
    reserved: AtomicU32,
    admit: tokio::sync::Mutex<()>,
    /// `<name>.D` → the welcome's `(not_after, spki)` of the installed chain (B2).
    installed: Mutex<HashMap<String, NameInfo>>,
    /// Generated passwords of this run, for the owner's `share_password` (M9); never logged.
    passwords: Mutex<HashMap<String, String>>,
    /// Shares left unmounted for want of a local approval or with the switch off (FIX4 #1).
    unmounted: Mutex<std::collections::BTreeSet<String>>,
    /// Random names allocated at the edge in this run, before a welcome lists them (§7.3).
    fresh: Mutex<std::collections::BTreeSet<String>>,
    /// The edge state machine runs (set by `start`): Stop all may wait for it to attach.
    client: std::sync::atomic::AtomicBool,
    /// The last `POST /v1/names/status` answer: `(at_ms, edge id, domain, names)`.
    names_status: Mutex<Option<(u64, String, String, Vec<NameInfo>)>>,
}

/// Six random bytes (names, labels).
pub fn random_bytes() -> [u8; names::RANDOM_LEN] {
    let b = uuid::Uuid::new_v4().into_bytes();
    [b[0], b[1], b[2], b[3], b[4], b[5] ^ b[9]]
}

impl Engine {
    /// Build the service; nothing dials until a need (§5.2).
    pub fn new(
        cfg: ShareConfig,
        store: Store,
        data_dir: PathBuf,
        engine_id: String,
        hooks: Hooks,
        rt: Option<runtime::Runtime>,
    ) -> anyhow::Result<Arc<Self>> {
        let key = gate::load_key(&data_dir.join("share").join("cookie.key"))?;
        let table = Arc::new(Table::default());
        let certs = Arc::new(Certs::default());
        let gate = Arc::new(Gate::new(key));
        let ports = PortGuard {
            own: hooks.own_ports.clone(),
            process: hooks.process.clone(),
        };
        let proxy = Arc::new(Proxy {
            table: table.clone(),
            ports: ports.clone(),
            gate: gate.clone(),
        });
        let tls = visitor::acceptor(certs.clone())?;
        Ok(Arc::new_cyclic(|me| Self {
            me: me.clone(),
            cfg,
            store,
            data_dir,
            engine_id,
            table,
            certs,
            gate,
            ports,
            proxy,
            tls,
            hooks,
            rt,
            need: watch::channel(false).0,
            terms: watch::channel(None).0,
            link: Mutex::new(None),
            attached: watch::channel(false).0,
            reserved: AtomicU32::new(0),
            admit: tokio::sync::Mutex::new(()),
            installed: Mutex::new(HashMap::new()),
            passwords: Mutex::new(HashMap::new()),
            unmounted: Mutex::new(std::collections::BTreeSet::new()),
            fresh: Mutex::new(std::collections::BTreeSet::new()),
            client: std::sync::atomic::AtomicBool::new(false),
            names_status: Mutex::new(None),
        }))
    }

    /// Start the edge state machine for the configured edge (one per edge, §5.9) and the expiry
    /// ticker. Without a dialer, a delegation source or a ticket to dial (edge `none`, or the
    /// default while its tickets are placeholders) no client runs and publishing says so.
    pub async fn start(self: &Arc<Self>) -> anyhow::Result<()> {
        self.stop_at_start().await;
        self.restore().await;
        let choice = self.store.edge().await?;
        let tickets = choice.tickets();
        if let (Some(dialer), Some(delegation), Some(t)) =
            (&self.hooks.dialer, &self.hooks.delegation, tickets.first())
        {
            let rev = self.store.terms_rev(&t.edge_id().to_string()).await?;
            self.terms.send_replace(rev);
            let client = crate::edge::Client {
                dialer: dialer.clone(),
                delegation: delegation.clone(),
                tickets,
                terms_rev: self.terms.subscribe(),
                attachment: self.clone(),
                timing: Default::default(),
            };
            tokio::spawn(client.run(self.need.subscribe()));
            self.client.store(true, Ordering::SeqCst);
        } else if tickets.is_empty() {
            tracing::info!("share: {NO_SHARING_EDGE}");
        }
        let me = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(30));
            loop {
                tick.tick().await;
                let Some(me) = me.upgrade() else { return };
                me.expire().await;
            }
        });
        self.refresh_need().await;
        Ok(())
    }

    pub(crate) async fn emit(&self, share: &Share) {
        if let Some(rt) = &self.rt {
            rt.publish(ShareChanged {
                share: share.clone(),
            })
            .await;
        }
    }

    async fn mine(&self) -> anyhow::Result<Vec<Share>> {
        Ok(self
            .store
            .list(platform::time::now_ms())
            .await?
            .into_iter()
            .filter(|s| s.engine == self.engine_id && s.state != ShareState::Stopped)
            .collect())
    }

    async fn refresh_need(&self) {
        let busy = self.mine().await.map(|v| !v.is_empty()).unwrap_or(false);
        self.need
            .send_if_modified(|n| std::mem::replace(n, busy) != busy);
    }

    /// One `POST /v1/names/status` at the configured edge (§5.2): the mesh's names as the edge
    /// sees them, without attaching and without setting `need`; the answer is cached.
    pub async fn names_status(&self) -> anyhow::Result<(String, String, Vec<NameInfo>)> {
        let (Some(dialer), Some(delegation)) = (&self.hooks.dialer, &self.hooks.delegation) else {
            anyhow::bail!("{NO_SHARING_EDGE}");
        };
        let Some(ticket) = self.store.edge().await?.tickets().into_iter().next() else {
            anyhow::bail!("{NO_SHARING_EDGE}");
        };
        let (info, status) =
            crate::edge::names_status(dialer.as_ref(), delegation.as_ref(), &ticket).await?;
        let seen = (info.edge_id.to_string(), info.domain, status.names);
        *self.names_status.lock() = Some((
            platform::time::now_ms(),
            seen.0.clone(),
            seen.1.clone(),
            seen.2.clone(),
        ));
        Ok(seen)
    }

    /// The names' edge view while detached, for `share_names_list`: a status answer under
    /// [`NAMES_STATUS_FRESH_MS`] old, else [`Self::names_status`] within [`NAMES_STATUS_WAIT`].
    /// `None` with the switch off or on any failure (an older edge's `404` included): the states
    /// stay unknown.
    pub(crate) async fn detached_names(&self) -> Option<(String, String, Vec<NameInfo>)> {
        if !self.store.enabled().await.unwrap_or(false) {
            return None;
        }
        let cached = self.names_status.lock().clone();
        if let Some((at, edge, domain, names)) = cached
            && platform::time::now_ms().saturating_sub(at) < NAMES_STATUS_FRESH_MS
        {
            return Some((edge, domain, names));
        }
        match tokio::time::timeout(NAMES_STATUS_WAIT, self.names_status()).await {
            Ok(Ok(seen)) => Some(seen),
            Ok(Err(e)) => {
                tracing::debug!("share: names status: {e:#}");
                None
            }
            Err(_) => {
                tracing::debug!("share: names status: no answer within {NAMES_STATUS_WAIT:?}");
                None
            }
        }
    }

    /// The current link, dropping it first when its connection is gone (the edge closes the
    /// tunnel on `stop_all`, E2E1 finding 4): a control call never goes out on a dead connection.
    fn live_link(&self) -> Option<Arc<Link>> {
        let mut link = self.link.lock();
        if link.as_ref().is_some_and(|l| l.control.is_closed()) {
            *link = None;
            self.attached.send_replace(false);
        }
        link.clone()
    }

    /// Forget `stale` (if it is still the current link) and close its connection, so the state
    /// machine sees its events end and dials again.
    fn drop_link(&self, stale: &Arc<Link>) {
        let mut link = self.link.lock();
        if link.as_ref().is_some_and(|l| Arc::ptr_eq(l, stale)) {
            *link = None;
            self.attached.send_replace(false);
        }
        drop(link);
        stale.control.close();
    }

    async fn wait_attached(&self) -> anyhow::Result<Arc<Link>> {
        self.need.send_replace(true);
        if let Some(l) = self.live_link() {
            return Ok(l);
        }
        let mut rx = self.attached.subscribe();
        let _ = tokio::time::timeout(ATTACH_WAIT, rx.wait_for(|a| *a)).await;
        self.live_link()
            .ok_or_else(|| anyhow::anyhow!("Can't reach the sharing edge; see Settings › Sharing"))
    }

    /// One control call on the attached edge; a transport failure (the connection died under
    /// it) drops the link, waits for the redial and tries once more. For idempotent calls.
    pub(crate) async fn with_control<T, F, Fut>(&self, call: F) -> anyhow::Result<T>
    where
        F: Fn(Control, Info) -> Fut,
        Fut: Future<Output = Result<T, EdgeError>>,
    {
        let link = self.wait_attached().await?;
        match call(link.control.clone(), link.info.clone()).await {
            Err(EdgeError::Transport(e)) => {
                tracing::info!("share: edge connection failed ({e:#}); redialing once");
                self.drop_link(&link);
                let link = self.wait_attached().await?;
                Ok(call(link.control.clone(), link.info.clone()).await?)
            }
            r => Ok(r?),
        }
    }

    async fn push_routes(&self) {
        let control = self.link.lock().as_ref().map(|l| l.control.clone());
        let Some(c) = control else { return };
        if let Err(e) = c.put_routes_at(self.routes_put().await).await {
            tracing::warn!("share: PUT /v1/routes: {e}");
        }
    }

    /// This engine's routes with each host's `published_ms`: the earliest `live_ms` of a live
    /// share under it, so the edge's `stopped_all` check judges every host by its own publish.
    async fn routes_put(&self) -> share_proto::RoutesPut {
        let hosts = self.table.hosts();
        let mut published_ms = std::collections::BTreeMap::new();
        if let Ok(mine) = self.mine().await {
            for s in mine {
                if let Some(live) = s.live_ms
                    && hosts.contains(&s.host)
                {
                    let e = published_ms.entry(s.host.clone()).or_insert(live);
                    *e = (*e).min(live);
                }
            }
        }
        share_proto::RoutesPut { hosts, published_ms }
    }

    /// Restore this engine's shares from the store into the route table and the password gate
    /// (M9): at start and at every `welcome`. A password share whose hash is gone can never be
    /// opened and is stopped; an expired one stops as `expiry`; one without this engine's own
    /// approval record (FIX4 #1) or while the switch is off is left unmounted.
    async fn restore(&self) {
        let Ok(mine) = self.mine().await else { return };
        let now = platform::time::now_ms();
        for s in mine
            .into_iter()
            .filter(|s| matches!(s.state, ShareState::Live | ShareState::Offline))
        {
            if s.expires_ms.is_some_and(|e| e <= now) {
                let _ = self.stop(&s.id, StoppedBy::Expiry).await;
                continue;
            }
            if !self.may_mount(&s).await {
                continue;
            }
            if s.access == Access::Password {
                match self.store.secret(&s.id).await {
                    Ok(Some(phc)) => self.gate.set_password(&s.id, Some(phc)),
                    Ok(None) => {
                        tracing::warn!("share: {} lost its password hash; stopping it", s.id);
                        let _ = self.stop(&s.id, StoppedBy::User).await;
                        continue;
                    }
                    Err(e) => {
                        tracing::warn!("share: password of {}: {e:#}", s.id);
                        continue;
                    }
                }
            }
            if let Some(m) = self.mount(&s).await {
                self.table.insert(m);
            }
        }
    }

    /// §7.4: "Quitting the engine stops its shares; only reserved-name shares started from
    /// Settings without expiry resume on restart." Run once at start, before [`Self::restore`]:
    /// every share of this engine not stopped is stopped unless it was created from Settings
    /// under a reserved name the mesh still holds (those keep their own expiry: every share has
    /// one, `Expiry` has no "never").
    async fn stop_at_start(&self) {
        let Ok(mine) = self.mine().await else { return };
        let reserved: Vec<String> = match self.store.names().await {
            Ok(n) => n
                .into_iter()
                .filter(|n| n.kind == "reserved" && n.released_ms.is_none())
                .map(|n| names::host(&n.name, None, &n.domain))
                .collect(),
            Err(e) => {
                tracing::warn!("share: names at start: {e:#}");
                Vec::new()
            }
        };
        for s in mine {
            let resumes = matches!(s.created_by, CreatedBy::Settings(_))
                && reserved.contains(&name_host_of(&s.host, &s.domain));
            if !resumes {
                tracing::info!("share: {} stopped with the engine's last run (§7.4)", s.id);
                let _ = self.stop(&s.id, StoppedBy::User).await;
            }
        }
    }

    /// A share of this engine may be mounted (FIX4 #1): the switch is on and this engine's own
    /// consent path recorded exactly this share. Otherwise it is left unmounted and listed in
    /// [`Self::unmounted`]. A store error decides nothing (FIX6 N2): the share is left as it
    /// is, so a routed one stays routed and an unmounted one stays unmounted.
    async fn may_mount(&self, s: &Share) -> bool {
        let why = match (self.store.enabled().await, self.store.locally_approved(s).await) {
            (Ok(true), Ok(true)) => {
                self.unmounted.lock().remove(&s.id);
                return true;
            }
            (Ok(false), _) => "public sharing is off",
            (_, Ok(false)) => "no approval for it was given on this machine",
            (Err(e), _) | (_, Err(e)) => {
                let routed = self.table.routes(&s.id);
                tracing::warn!(
                    "share: mount check of {} failed, left as is (routed: {routed}): {e:#}",
                    s.id
                );
                return routed;
            }
        };
        tracing::warn!("share: {} left unmounted: {why}", s.id);
        self.table.remove(&s.id);
        self.unmounted.lock().insert(s.id.clone());
        false
    }

    /// Shares of this engine the store lists as live or pending that were left unmounted
    /// because no local approval matches them or the switch is off (FIX4 #1).
    pub fn unmounted(&self) -> Vec<String> {
        self.unmounted.lock().iter().cloned().collect()
    }

    /// The switch is on and no Stop all happened since the owner was asked (§7.4, FIX4 #6).
    async fn still_approved(&self, consent: &Consent) -> anyhow::Result<()> {
        anyhow::ensure!(self.store.enabled().await?, SWITCH_OFF);
        if let Some(b) = self.store.stop_all_ms().await?
            && b >= consent.asked_ms()
        {
            anyhow::bail!("Stop all was pressed after this publish was asked for; ask again");
        }
        Ok(())
    }

    /// The mount for one of this engine's shares (M1). A `dir` share is served from the pin
    /// stored at publish, never re-pinned: when its path no longer names that directory (device
    /// and inode), now leads elsewhere, or now falls under a protected place (this engine's data
    /// dir, `$HOME`, a dot dir), or no pin was stored, the share stops as `source_gone`.
    async fn mount(&self, s: &Share) -> Option<Mount> {
        let Source::Dir(d) = &s.source else {
            return Some(Mount::of(s, None));
        };
        let why = match self.store.pin(&s.id).await {
            Ok(Some(pin)) => match self.recheck(&pin) {
                Ok(()) => return Some(Mount::of(s, Some(pin))),
                Err(why) => why,
            },
            Ok(None) => "no pinned folder was stored at publish".to_owned(),
            Err(e) => {
                tracing::warn!("share: pin of {}: {e:#}", s.id);
                return None;
            }
        };
        tracing::warn!("share {}: {d}: {why}; stopping it", s.id);
        let _ = self.stop(&s.id, StoppedBy::SourceGone).await;
        None
    }

    /// A stored pin still names the approved, allowed directory.
    fn recheck(&self, pin: &DirPin) -> Result<(), String> {
        let canon = sources::check_root(
            &pin.canon.to_string_lossy(),
            &self.data_dir,
            std::env::home_dir().as_deref(),
        )
        .map_err(|e| e.to_string())?;
        if canon != pin.canon {
            return Err(format!("the path now leads to {}", canon.display()));
        }
        pin.check()
            .map_err(|e| format!("not the folder approved at publish ({e})"))
    }

    /// The password generated for `id` in this run; for the owner's surfaces only.
    pub fn password(&self, id: &str) -> Option<String> {
        self.passwords.lock().get(id).cloned()
    }

    /// Publish (§7.6) under the owner's [`Consent`].
    pub async fn publish(
        &self,
        plan: Plan,
        origin: CallOrigin,
        consent: Consent,
    ) -> anyhow::Result<Published> {
        self.publish_with(plan, origin, consent, None).await
    }

    /// Publish what the approval card showed ([`Self::publish_preview`]): a held name as is; a
    /// drawn one is claimed now, after the owner's `Allow`. When the edge answers `name_taken`
    /// meanwhile the publish fails and claims nothing (FIX6 N3): the owner approved that URL.
    pub async fn publish_previewed(
        &self,
        mut plan: Plan,
        origin: CallOrigin,
        consent: Consent,
        preview: PublishPreview,
    ) -> anyhow::Result<Published> {
        let candidate = if preview.drawn {
            Some(preview.name)
        } else {
            plan.name = Some(preview.name);
            None
        };
        self.publish_with(plan, origin, consent, candidate).await
    }

    async fn publish_with(
        &self,
        plan: Plan,
        origin: CallOrigin,
        consent: Consent,
        candidate: Option<String>,
    ) -> anyhow::Result<Published> {
        anyhow::ensure!(
            !consent.0.approval_id.is_empty(),
            "this needs the owner's own approval"
        );
        self.still_approved(&consent).await?;
        anyhow::ensure!(
            !self.store.edge().await?.tickets().is_empty(),
            NO_SHARING_EDGE
        );
        let pin = match &plan.source {
            Source::Port(p) => {
                drop(self.ports.connect(*p).await?);
                None
            }
            Source::Dir(d) => Some(sources::pin_root(
                d,
                &self.data_dir,
                std::env::home_dir().as_deref(),
            )?),
        };
        self.reserve().await?;
        let r = self.publish_admitted(plan, origin, consent, pin, candidate).await;
        let _g = self.admit.lock().await;
        self.reserved.fetch_sub(1, Ordering::SeqCst);
        r
    }

    /// `share.max_live` as one step with the store (m6): the count and the reservation happen
    /// under `admit`, and a reservation is released only after its row is stored.
    async fn reserve(&self) -> anyhow::Result<()> {
        let _g = self.admit.lock().await;
        let live = self.mine().await?.len() as u32 + self.reserved.load(Ordering::SeqCst);
        anyhow::ensure!(
            live < self.cfg.max_live,
            "{live} shares are live on this machine (share.max_live): stop one first"
        );
        self.reserved.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn publish_admitted(
        &self,
        plan: Plan,
        origin: CallOrigin,
        consent: Consent,
        pin: Option<DirPin>,
        candidate: Option<String>,
    ) -> anyhow::Result<Published> {
        let link = self.wait_attached().await?;
        let domain = link.info.domain.clone();
        let edge = link.info.edge_id.to_string();
        let (name, claimed) = self
            .pick_name(&link, plan.name.as_deref(), candidate.as_deref())
            .await?;
        // Every way out before the share row is stored (an error, the caller dropping this
        // future) gives a name claimed here back.
        let claim = Claim {
            engine: self.me.clone(),
            name: claimed.then(|| name.clone()),
        };
        let host = names::host(&name, plan.label.as_deref(), &domain);
        let url = names::share_url(&host, &plan.path);
        let prefix = if plan.path == "/" {
            ""
        } else {
            plan.path.as_str()
        };
        if let Lookup::Found(m, _) = self.table.lookup(&host, &plan.path)
            && m.prefix == prefix
        {
            anyhow::bail!("{url} is already shared ({})", m.share);
        }
        let id = uuid::Uuid::now_v7().to_string();
        // The certificate step runs on its own task: a fresh name key waits for the vault to
        // settle (about a minute), which a publish must not block on (E2E1 finding 9).
        let mut cert = {
            let me = self.me.upgrade().context("the share service is shutting down")?;
            let (link, name) = (link.clone(), name.clone());
            tokio::spawn(async move { me.ensure_cert(&link, &name, false).await })
        };
        // Hash first (nothing stored), store it only once the share row is certain (m6).
        let password = match plan.access {
            Access::Password => {
                let pw = gate::generate_password();
                let phc = tokio::task::spawn_blocking({
                    let pw = pw.clone();
                    move || gate::hash_password(&pw)
                })
                .await??;
                Some((pw, phc))
            }
            Access::Public => None,
        };
        let step = match tokio::time::timeout(PUBLISH_CERT_WAIT, &mut cert).await {
            Ok(joined) => Some(joined.context("certificate step")??),
            Err(_) => None,
        };
        let have_cert = matches!(step, Some(CertStep::Installed));
        let now = platform::time::now_ms();
        let expiry_ms = plan.expiry.duration().as_millis() as u64;
        // The last moment before anything is stored: the switch and Stop all may have changed
        // while the certificate step and the hash ran (§7.4).
        self.still_approved(&consent).await?;
        let share = Share {
            id,
            engine: self.engine_id.clone(),
            edge,
            domain,
            host,
            path: plan.path,
            url,
            source: plan.source,
            access: plan.access,
            interstitial: self.cfg.interstitial,
            created_ms: now,
            live_ms: have_cert.then_some(now),
            expires_ms: have_cert.then_some(now + expiry_ms),
            approved_by: consent.0,
            created_by: match origin {
                CallOrigin::Call { session, call_id } => CreatedBy::Call { session, call_id },
                CallOrigin::Settings => CreatedBy::Settings("settings".into()),
            },
            state: if have_cert {
                ShareState::Live
            } else {
                ShareState::Pending
            },
            stopped_ms: None,
            stopped_by: None,
        };
        if let Some((_, phc)) = &password {
            self.store.put_secret(&share.id, phc).await?;
        }
        let stored = async {
            if !have_cert {
                self.store.put_request(&share.id, expiry_ms).await?;
            }
            if let Some(pin) = &pin {
                self.store.put_pin(&share.id, pin).await?;
            }
            self.store.put_approval(&share, now).await?;
            self.store.put(&share).await
        }
        .await;
        if let Err(e) = stored {
            let _ = self.store.delete_secret(&share.id).await;
            let _ = self.store.delete_request(&share.id).await;
            let _ = self.store.delete_pin(&share.id).await;
            let _ = self.store.delete_approval(&share.id).await;
            return Err(e);
        }
        claim.keep();
        if let Some((pw, phc)) = &password {
            self.gate.set_password(&share.id, Some(phc.clone()));
            self.passwords.lock().insert(share.id.clone(), pw.clone());
        }
        if have_cert {
            self.table.insert(Mount::of(&share, pin));
            self.push_routes().await;
        }
        self.emit(&share).await;
        let pending = match step {
            Some(CertStep::Installed) => None,
            Some(CertStep::Pending(p)) => Some(p),
            None => {
                self.finish_cert_later(cert, name);
                Some(PendingCert::default())
            }
        };
        Ok(Published {
            share,
            password: password.map(|(pw, _)| pw),
            pending,
        })
    }

    /// The rest of a publish's certificate step, after the share row is stored: an installed
    /// chain takes the name's pending shares live (`share.changed`); a queued one waits for
    /// the edge's `cert` event like any pending share.
    fn finish_cert_later(
        &self,
        cert: tokio::task::JoinHandle<anyhow::Result<CertStep>>,
        name: String,
    ) {
        let Some(me) = self.me.upgrade() else { return };
        tokio::spawn(async move {
            match cert.await {
                Ok(Ok(CertStep::Installed)) => me.after_cert(&name, false).await,
                Ok(Ok(CertStep::Pending(_))) => {}
                // TODO(210-M3): surface a failed background certificate step on the share
                // (it stays `pending`; the next `cert` event or welcome retries it).
                Ok(Err(e)) => tracing::warn!("share: certificate for {name}: {e:#}"),
                Err(e) => tracing::warn!("share: certificate task for {name}: {e}"),
            }
        });
    }

    /// The names a publish may use without allocating, best first: the welcome's held names
    /// (not released/banned) and this mesh's `share_names` rows at the link's edge and domain
    /// not released. The rows cover a name claimed on a link whose welcome predates the claim
    /// (a welcome is only as new as its `hello`). The primary row comes first, then reserved
    /// names, then the rest.
    async fn held_names(&self, link: &Link) -> anyhow::Result<Vec<String>> {
        let (edge, domain) = (link.info.edge_id.to_string(), &link.info.domain);
        let rows: Vec<_> = self
            .store
            .names()
            .await?
            .into_iter()
            .filter(|r| r.edge == edge && r.domain == *domain && r.released_ms.is_none())
            .collect();
        let gone = |name: &str| {
            link.welcome.names.iter().any(|n| {
                n.name == name && matches!(n.state, NameState::Released | NameState::Banned)
            })
        };
        let rank = |name: &str, reserved: bool| {
            let row = rows.iter().find(|r| r.name == name);
            if row.is_some_and(|r| r.is_primary) {
                0
            } else if reserved || row.is_some_and(|r| r.kind == "reserved") {
                1
            } else {
                2
            }
        };
        let mut held: Vec<(u8, String)> = Vec::new();
        let seen = link
            .welcome
            .names
            .iter()
            .map(|n| (n.name.as_str(), n.kind == NameKind::Reserved))
            .chain(rows.iter().map(|r| (r.name.as_str(), r.kind == "reserved")));
        for (name, reserved) in seen {
            if !gone(name) && !held.iter().any(|(_, h)| h == name) {
                held.push((rank(name, reserved), name.to_owned()));
            }
        }
        held.sort_by_key(|(r, _)| *r);
        Ok(held.into_iter().map(|(_, n)| n).collect())
    }

    /// The accepted terms revision when it is the edge's current one (`info.terms_rev`); else
    /// the refusal the owner's surfaces parse (`accept the terms of … first (<url>, revision
    /// <rev>)`). An edge without terms (empty `terms_rev`) asks nothing, as the edge's own check.
    async fn current_terms(&self, link: &Link) -> anyhow::Result<Option<String>> {
        let accepted = self.store.terms_rev(&link.info.edge_id.to_string()).await?;
        if !link.info.terms_rev.is_empty() && accepted.as_deref() != Some(link.info.terms_rev.as_str()) {
            anyhow::bail!(
                "accept the terms of {} first ({}, revision {}): Settings › Sharing or the first-run card",
                link.info.domain,
                link.info.terms_url,
                link.info.terms_rev
            );
        }
        Ok(accepted)
    }

    /// TERMS-FIRST: what a name claim (reserve, a received transfer) or a CSR post is refused
    /// for before the owner is asked: no edge, the edge unreachable, or its current terms not
    /// accepted (the same refusal text as a publish). Run it before raising the approval row.
    pub async fn terms_precheck(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.store.edge().await?.tickets().is_empty(),
            NO_SHARING_EDGE
        );
        let link = self.wait_attached().await?;
        self.current_terms(&link).await.map(drop)
    }

    /// Right before a claim or CSR post: the terms accepted at the edge's current revision, and
    /// this session's `hello` carrying them (a `hello` sent before the owner accepted is sent
    /// again), else the edge answers `403 terms_required` (§7.6).
    pub(crate) async fn terms_ready(&self) -> anyhow::Result<()> {
        let link = self.wait_attached().await?;
        let accepted = self.current_terms(&link).await?;
        if !link.welcome.terms_ok {
            self.rehello(&link, accepted).await?;
        }
        Ok(())
    }

    /// After a name claim or release on the live link: `hello` again so the link's welcome
    /// lists the mesh's names as the edge now holds them (Domains' state, the held-name pick).
    pub(crate) async fn refresh_welcome(&self) -> anyhow::Result<()> {
        let link = self.wait_attached().await?;
        let accepted = self.store.terms_rev(&link.info.edge_id.to_string()).await?;
        self.rehello(&link, accepted).await.map(drop)
    }

    /// TERMS-FIRST: the configured edge's current terms and whether this mesh accepted them, for
    /// Settings › Sharing. `None` while sharing is off or no edge is configured (nothing dials
    /// for it then); otherwise attaches like a name check.
    pub async fn edge_terms(&self) -> anyhow::Result<Option<EdgeTerms>> {
        if !self.store.enabled().await? || self.store.edge().await?.tickets().is_empty() {
            return Ok(None);
        }
        let link = self.wait_attached().await?;
        let edge = link.info.edge_id.to_string();
        let accepted = self.store.terms_rev(&edge).await?;
        Ok(Some(EdgeTerms {
            accepted: link.info.terms_rev.is_empty()
                || accepted.as_deref() == Some(link.info.terms_rev.as_str()),
            edge,
            domain: link.info.domain.clone(),
            url: link.info.terms_url.clone(),
            rev: link.info.terms_rev.clone(),
        }))
    }

    /// What a publish can be refused for before the owner is asked (E2E1 finding 8): sharing
    /// off, no edge, the edge unreachable, or — when it would allocate a fresh name — terms not
    /// accepted at the edge's current revision. Run it before raising the approval row.
    pub async fn publish_precheck(&self, want: Option<&str>) -> anyhow::Result<()> {
        anyhow::ensure!(self.store.enabled().await?, SWITCH_OFF);
        anyhow::ensure!(
            !self.store.edge().await?.tickets().is_empty(),
            NO_SHARING_EDGE
        );
        let link = self.wait_attached().await?;
        if want.is_none() && self.held_names(&link).await?.is_empty() {
            self.current_terms(&link).await?;
        }
        Ok(())
    }

    /// A name the publish may use without claiming one: the requested held one, else the best
    /// held one ([`Self::held_names`]), else one claimed earlier in this run that no welcome
    /// lists yet. `None`: a fresh random name has to be claimed (§5.2, §7.6); never while the
    /// mesh holds a name here (the edge refuses a second random name, `quota`).
    async fn held_pick(&self, link: &Link, want: Option<&str>) -> anyhow::Result<Option<String>> {
        let held = self.held_names(link).await?;
        if let Some(w) = want {
            anyhow::ensure!(
                self.fresh.lock().contains(w) || held.iter().any(|n| n == w),
                "the mesh holds no name {w} at {}",
                link.info.domain
            );
            return Ok(Some(w.to_owned()));
        }
        if let Some(n) = held.into_iter().next() {
            return Ok(Some(n));
        }
        Ok(self.fresh.lock().iter().next().cloned())
    }

    /// The name for a publish (after the owner's approval): [`Self::held_pick`], else a random
    /// name claimed at the edge after the terms: `candidate` (the one the card showed) exactly,
    /// failing on `name_taken`; without one, a fresh draw whenever the edge answers
    /// `name_taken`. The flag is `true` when this call claimed the name.
    async fn pick_name(
        &self,
        link: &Arc<Link>,
        want: Option<&str>,
        candidate: Option<&str>,
    ) -> anyhow::Result<(String, bool)> {
        if let Some(n) = self.held_pick(link, want).await? {
            return Ok((n, false));
        }
        let accepted = self.current_terms(link).await?;
        if !link.welcome.terms_ok {
            self.rehello(link, accepted).await?;
        }
        for _ in 0..3 {
            let name = candidate
                .map(str::to_owned)
                .unwrap_or_else(|| names::random_name(random_bytes()));
            match link.control.allocate(&name).await {
                Ok(g) => {
                    self.fresh.lock().insert(g.name.clone());
                    return Ok((g.name, true));
                }
                Err(e) if e.code() == Some(ErrorCode::NameTaken) => {
                    anyhow::ensure!(candidate.is_none(), NAME_TAKEN_MEANWHILE);
                }
                Err(e) => return Err(e.into()),
            }
        }
        anyhow::bail!("could not allocate a name at {}", link.info.domain)
    }

    /// What the approval card shows before the ask (§7.3): the name, host and exact URL a
    /// publish of `plan` gets. Nothing is claimed at the edge here: a first publish draws its
    /// random name locally (checked with the read-only `GET /v1/names/{name}`), and
    /// [`Self::publish_previewed`] claims it only after the owner's `Allow`.
    pub async fn publish_preview(&self, plan: &Plan) -> anyhow::Result<PublishPreview> {
        let link = self.wait_attached().await?;
        let (name, drawn) = match self.held_pick(&link, plan.name.as_deref()).await? {
            Some(n) => (n, false),
            None => (Self::draw_name(&link).await, true),
        };
        let host = names::host(&name, plan.label.as_deref(), &link.info.domain);
        Ok(PublishPreview {
            url: names::share_url(&host, &plan.path),
            name,
            drawn,
        })
    }

    /// A random name the edge reports free right now (a few draws); the check is advisory (the
    /// claim after the approval fails on `name_taken`), so a failed check keeps the draw.
    async fn draw_name(link: &Link) -> String {
        let mut name = names::random_name(random_bytes());
        for _ in 0..3 {
            match link.control.check_name(&name).await {
                Ok(a) if !a.available => name = names::random_name(random_bytes()),
                Ok(_) => break,
                Err(e) => {
                    tracing::debug!("share: name check of {name}: {e:#}");
                    break;
                }
            }
        }
        name
    }

    /// Give back a name this run claimed for a publish that then failed or was dropped, unless
    /// a share of the family not stopped (replicated rows included) uses it, or the store
    /// cannot tell (FIX6 N4: a name in doubt stays claimed).
    async fn release_unused(&self, name: &str) {
        let host = |d: &str| names::host(name, None, d);
        match self.store.list(platform::time::now_ms()).await {
            Ok(all) => {
                if all.iter().any(|s| {
                    s.state != ShareState::Stopped
                        && name_host_of(&s.host, &s.domain) == host(&s.domain)
                }) {
                    return;
                }
            }
            Err(e) => {
                tracing::warn!("share: name {name} kept claimed, shares unreadable: {e:#}");
                return;
            }
        }
        self.fresh.lock().remove(name);
        if let Err(e) = self.with_control(|c, _| async move { c.release(name).await }).await {
            tracing::warn!("share: release of unused name {name}: {e:#}");
        }
    }

    /// `hello` again on `link`; its welcome replaces the link's (while `link` is the current
    /// one), so the held names and `terms_ok` are as new as this `hello`.
    async fn rehello(&self, link: &Arc<Link>, terms_rev: Option<String>) -> anyhow::Result<Arc<Link>> {
        let Some(d) = &self.hooks.delegation else {
            anyhow::bail!("no delegation source");
        };
        let delegation = d
            .delegate(link.info.edge_id, &link.info.domain, link.info.now_ms)
            .await?;
        let welcome = link
            .control
            .hello(&Hello {
                v: share_proto::PROTOCOL_MAJOR,
                delegation,
                invite: None,
                client: share_proto::version::client_agent(platform::version::get()),
                terms_rev,
            })
            .await?;
        let renewed = Arc::new(Link {
            control: link.control.clone(),
            info: link.info.clone(),
            welcome,
        });
        let mut cur = self.link.lock();
        if cur.as_ref().is_some_and(|l| Arc::ptr_eq(l, link)) {
            *cur = Some(renewed.clone());
        }
        Ok(renewed)
    }

    /// Have `<name>.D`'s pair installed: fetch the chain, or post the CSR.
    /// `renew` (a `cert` event) fetches and installs over the current chain (B2); without it a
    /// chain is fetched again when the welcome's `not_after`/SPKI differ from the installed one.
    async fn ensure_cert(&self, link: &Link, name: &str, renew: bool) -> anyhow::Result<CertStep> {
        let domain = &link.info.domain;
        let name_host = names::host(name, None, domain);
        let seen = link.welcome.names.iter().find(|n| n.name == name);
        let current = self.installed.lock().get(&name_host).is_some_and(|i| {
            seen.is_none_or(|n| n.not_after == i.not_after && n.spki == i.spki)
        });
        if !renew && current && self.certs.lookup(&name_host).is_some() {
            return Ok(CertStep::Installed);
        }
        let Some(store) = &self.hooks.name_keys else {
            anyhow::bail!("name keys are not available on this engine");
        };
        let key = keys::ensure(store.as_ref(), domain, name).await?;
        let valid = link
            .welcome
            .names
            .iter()
            .any(|n| n.name == name && n.state == NameState::Valid);
        let chain = if valid || renew {
            link.control.cert(name).await.ok()
        } else {
            None
        };
        let chain = match chain {
            Some(c) => c,
            None => match link.control.csr(name, key.csr_der.clone(), false).await? {
                CsrAnswer::Issued(c) => c,
                CsrAnswer::Pending(p) => {
                    return Ok(CertStep::Pending(PendingCert {
                        queue_pos: p.queue_pos,
                        eta_s: Some(p.eta_s),
                    }));
                }
            },
        };
        self.certs
            .install(&name_host, &chain.chain_pem, &key.key_pem)?;
        if let Some(n) = seen {
            self.installed.lock().insert(name_host, n.clone());
        }
        Ok(CertStep::Installed)
    }

    /// Drop `<name>.D`'s chain when no share of this engine is left under it.
    async fn release_cert_if_unused(&self, host: &str, domain: &str) {
        let name_host = name_host_of(host, domain);
        let Ok(mine) = self.mine().await else { return };
        if !mine
            .iter()
            .any(|s| name_host_of(&s.host, &s.domain) == name_host)
        {
            self.certs.remove(&name_host);
            self.installed.lock().remove(&name_host);
        }
    }

    /// Stop a share. One of another engine is forwarded to that engine over `theuth/rpc/1`
    /// (`share_stop {id}`), which stops it and announces it; the replicated row follows.
    pub async fn stop(&self, id: &str, by: StoppedBy) -> anyhow::Result<Share> {
        let mut s = self
            .store
            .get(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("no share {id}"))?;
        if s.engine != self.engine_id {
            if s.state == ShareState::Stopped {
                return Ok(s);
            }
            let _: crate::rpc_share::Stopped = crate::family::call(
                &s.engine,
                "share_stop",
                serde_json::json!({ "id": id }),
            )
            .await
            .with_context(|| format!("share {id} runs on another machine, which did not answer"))?;
            return Ok(self.store.get(id).await?.unwrap_or(s));
        }
        if s.state != ShareState::Stopped {
            s.state = ShareState::Stopped;
            s.stopped_ms = Some(platform::time::now_ms());
            s.stopped_by = Some(by);
            self.store.put(&s).await?;
            self.gate.set_password(id, None);
            self.passwords.lock().remove(id);
            let _ = self.store.delete_secret(id).await;
            let _ = self.store.delete_request(id).await;
            let _ = self.store.delete_pin(id).await;
            let _ = self.store.delete_approval(id).await;
            if self.table.remove(id) {
                self.push_routes().await;
            }
            self.release_cert_if_unused(&s.host, &s.domain).await;
            self.emit(&s).await;
        }
        self.refresh_need().await;
        Ok(s)
    }

    /// Stop all (§7.4): every local share created before now, the shares of other machines
    /// (each asked over `theuth/rpc/1`), then `POST /v1/mesh/stop` with retries. Detached or
    /// refused, the edge stop stays pending (sent again at the next `welcome`) and the outcome
    /// says so. Shares created before it never go live (M7).
    pub async fn stop_all(&self) -> anyhow::Result<StopAllOutcome> {
        self.stop_all_with(true).await
    }

    /// Stop all as asked by another machine of the family: no second fan-out.
    pub async fn stop_all_forwarded(&self) -> anyhow::Result<StopAllOutcome> {
        self.stop_all_with(false).await
    }

    async fn stop_all_with(&self, fan_out: bool) -> anyhow::Result<StopAllOutcome> {
        let now = platform::time::now_ms();
        self.store.stop_all_at(now).await?;
        // Pending publish cards are withdrawn; an `Allow` already given is refused by the
        // `stop_all_ms >= asked_ms` check in `publish` (§7.4, M7).
        ::tools::approval::dismiss_tool(PUBLISH_TOOL).await;
        let stopped = self.stop_local(StoppedBy::User, Some(now)).await;
        if fan_out {
            self.stop_all_elsewhere().await;
        }
        // §7.4: on the engine's own connection, attaching first when detached (bounded). With
        // no edge client running (no dialer, delegation source or ticket) there is nothing to
        // wait for.
        let dials = self.client.load(Ordering::SeqCst);
        let mut sent = if !dials && self.live_link().is_none() {
            Err(anyhow::anyhow!("not attached to the sharing edge"))
        } else {
            match tokio::time::timeout(STOP_ALL_ATTACH_WAIT, self.wait_attached()).await {
                Ok(Ok(_)) => self.send_stop_all().await,
                Ok(Err(e)) => Err(e),
                Err(_) => Err(anyhow::anyhow!(
                    "the sharing edge did not answer within {STOP_ALL_ATTACH_WAIT:?}"
                )),
            }
        };
        self.refresh_need().await;
        // An engine that cannot reach the edge hands Stop all to the lease holder (§7.4).
        if fan_out && let Err(e) = &sent {
            tracing::warn!("share: POST /v1/mesh/stop: {e:#}; handing Stop all to the lease holder");
            sent = self.stop_all_via_holder().await.map_err(|h| anyhow::anyhow!("{e:#}; {h:#}"));
        }
        let edge_pending = match sent {
            Ok(()) => None,
            Err(e) => {
                tracing::warn!("share: Stop all not confirmed by the edge: {e:#}");
                Some(format!("{e:#}"))
            }
        };
        let stopped = stopped?;
        Ok(StopAllOutcome {
            stopped,
            edge_pending,
        })
    }

    /// Ask the lease holder to Stop all on its own edge connection; `Ok` when it confirmed.
    async fn stop_all_via_holder(&self) -> anyhow::Result<()> {
        let peer = match crate::family::holder().await? {
            crate::family::Holder::Me => {
                anyhow::bail!("this machine holds the lease and cannot reach the edge")
            }
            crate::family::Holder::Peer(p) => p,
        };
        let r: crate::rpc_share::Stopped =
            crate::family::call(&peer, "share_stop", serde_json::json!({ "id": "all" })).await?;
        match r.edge_pending {
            None => self.store.stop_all_sent().await,
            Some(why) => anyhow::bail!("the lease holder {peer} could not reach the edge either: {why}"),
        }
    }

    /// `POST /v1/mesh/stop`, three tries; clears the pending mark on success.
    async fn send_stop_all(&self) -> anyhow::Result<()> {
        let mut last = anyhow::anyhow!("not attached to the sharing edge");
        for wait in [0u64, 500, 2000] {
            tokio::time::sleep(Duration::from_millis(wait)).await;
            let control = self.link.lock().as_ref().map(|l| l.control.clone());
            let Some(c) = control else { continue };
            match c.stop_all().await {
                Ok(_) => return self.store.stop_all_sent().await,
                Err(e) => last = e.into(),
            }
        }
        Err(last)
    }

    /// Ask every other machine with a share not stopped to Stop all (best effort, 10 s each, in
    /// parallel); the edge's `POST /v1/mesh/stop` closes their tunnels whatever they answer.
    async fn stop_all_elsewhere(&self) {
        let Ok(all) = self.store.list(platform::time::now_ms()).await else {
            return;
        };
        let mut engines: Vec<String> = all
            .into_iter()
            .filter(|s| s.engine != self.engine_id && s.state != ShareState::Stopped)
            .map(|s| s.engine)
            .collect();
        engines.sort();
        engines.dedup();
        let calls = engines.into_iter().map(|e| async move {
            let r = tokio::time::timeout(
                Duration::from_secs(10),
                crate::family::call::<crate::rpc_share::Stopped>(
                    &e,
                    "share_stop",
                    serde_json::json!({ "id": "all" }),
                ),
            )
            .await;
            if !matches!(r, Ok(Ok(_))) {
                tracing::warn!("share: Stop all on {e} did not answer; the edge stop covers it");
            }
        });
        futures::future::join_all(calls).await;
    }

    /// Stop every local share (created before `before`, when given), continuing past errors;
    /// the first error is returned after the rest were tried.
    async fn stop_local(&self, by: StoppedBy, before: Option<u64>) -> anyhow::Result<usize> {
        let mut n = 0;
        let mut first = None;
        for s in self.mine().await? {
            if before.is_some_and(|b| s.created_ms > b) {
                continue;
            }
            match self.stop(&s.id, by).await {
                Ok(_) => n += 1,
                Err(e) => {
                    tracing::warn!("share: stop {}: {e:#}", s.id);
                    first.get_or_insert(e);
                }
            }
        }
        first.map_or(Ok(n), Err)
    }

    /// The 30 s tick: expired shares stop; a routed share whose row no longer matches its local
    /// approval (a replicated edit of its expiry, say: R4) leaves the route table.
    async fn expire(&self) {
        let now = platform::time::now_ms();
        let Ok(mine) = self.mine().await else { return };
        let mut unrouted = false;
        for s in mine {
            if s.expires_ms.is_some_and(|e| e <= now) {
                let _ = self.stop(&s.id, StoppedBy::Expiry).await;
            } else if self.table.routes(&s.id) && !self.may_mount(&s).await {
                unrouted = true;
            }
        }
        if unrouted {
            self.push_routes().await;
        }
    }

    /// Every share of the mesh, newest first (30-day history included).
    pub async fn list(&self) -> anyhow::Result<Vec<Share>> {
        self.store.list(platform::time::now_ms()).await
    }

    /// Switch sharing on, under the owner's [`Consent`] (§7.2).
    pub async fn enable(&self, _consent: &Consent) -> anyhow::Result<()> {
        self.store.set_enabled(true).await
    }

    /// Switch sharing off and stop this machine's shares (`switch_off`). Never gated.
    pub async fn disable(&self) -> anyhow::Result<()> {
        self.store.set_enabled(false).await?;
        // A publish card still on screen must not outlive the switch (§7.4).
        ::tools::approval::dismiss_tool(PUBLISH_TOOL).await;
        self.stop_local(StoppedBy::SwitchOff, None).await?;
        Ok(())
    }

    /// Change the edge under the owner's [`Consent`]; takes effect at the next start.
    // TODO(210-M1): restart the state machine live on an edge change.
    pub async fn set_edge(&self, edge: &EdgeChoice, _consent: &Consent) -> anyhow::Result<()> {
        self.store.set_edge(edge).await
    }

    /// Record accepted terms for `edge` under the owner's [`Consent`]. `edge` is the edge's
    /// endpoint id ([`EdgeTerms::edge`]) or, as the refusal text names it, the attached edge's
    /// domain: both are stored under the endpoint id, the key [`Self::current_terms`] reads.
    pub async fn accept_terms(&self, edge: &str, rev: &str, _consent: &Consent) -> anyhow::Result<()> {
        let id = self
            .live_link()
            .filter(|l| l.info.domain == edge)
            .map_or_else(|| edge.to_owned(), |l| l.info.edge_id.to_string());
        self.store.set_terms_rev(&id, rev).await?;
        self.terms.send_replace(Some(rev.to_owned()));
        Ok(())
    }

    /// The attached edge's control client and info (attaching first, §5.2 "a name operation").
    pub(crate) async fn attached_control(&self) -> anyhow::Result<(Control, Info)> {
        let link = self.wait_attached().await?;
        Ok((link.control.clone(), link.info.clone()))
    }

    /// The `notAfter` (Unix ms) of `<name>.D`'s chain installed on this engine, as the welcome
    /// that installed it reported it (B2); `None` without an installed chain.
    pub(crate) fn installed_not_after(&self, name_host: &str) -> Option<u64> {
        self.certs.lookup(name_host)?;
        self.installed.lock().get(name_host).and_then(|i| i.not_after)
    }

    /// The last `welcome`'s names with its edge id and domain; `None` while detached.
    pub(crate) fn welcome_names(&self) -> Option<(String, String, Vec<NameInfo>)> {
        self.link.lock().as_ref().map(|l| {
            (l.info.edge_id.to_string(), l.info.domain.clone(), l.welcome.names.clone())
        })
    }

    /// This engine's id.
    pub fn engine_id(&self) -> &str {
        &self.engine_id
    }

    /// After a name-key rotation (§6.3): post the row's CSR with `?rotate=true` and install
    /// the chain when it is issued at once (otherwise the `cert` event installs it).
    pub(crate) async fn rotate_cert(&self, name: &str, key: &keys::NameKey) -> anyhow::Result<bool> {
        let link = self.wait_attached().await?;
        match link.control.csr(name, key.csr_der.clone(), true).await? {
            CsrAnswer::Issued(c) => {
                let host = names::host(name, None, &link.info.domain);
                self.certs.install(&host, &c.chain_pem, &key.key_pem)?;
                Ok(true)
            }
            CsrAnswer::Pending(_) => Ok(false),
        }
    }

    /// The `csr_mismatch` heal timer (§6.2, P0-6), one pass on the lease holder: a welcome name
    /// whose edge SPKI differs from the vault row for [`crate::holder::CSR_HEAL_MS`] gets the
    /// row's CSR posted with `?rotate=true`. `since` keeps first-seen times between passes;
    /// `posted` the SPKIs already posted, so a stale welcome does not post again.
    pub(crate) async fn csr_heal_tick(
        &self,
        since: &mut HashMap<String, u64>,
        posted: &mut HashMap<String, share_proto::SpkiHash>,
        now: u64,
    ) {
        let (Some((_, domain, seen)), Some(store)) = (self.welcome_names(), &self.hooks.name_keys) else {
            return;
        };
        if !matches!(crate::family::holder().await, Ok(crate::family::Holder::Me)) {
            since.clear();
            return;
        }
        since.retain(|n, _| seen.iter().any(|i| &i.name == n));
        for n in &seen {
            let Ok(Some(row)) = store.get(&domain, &n.name).await else { continue };
            let Ok(row_spki) = crate::holder::row_spki(&row, &n.name, &domain) else { continue };
            let edge = match n.spki {
                Some(e) if e != row_spki && posted.get(&n.name) == Some(&row_spki) => Some(row_spki),
                e => e,
            };
            let at = since.get(&n.name).copied();
            match crate::holder::csr_heal(edge.as_ref(), &row_spki, at, now, crate::holder::CSR_HEAL_MS) {
                crate::holder::CsrHeal::InSync => {
                    since.remove(&n.name);
                    if n.spki == Some(row_spki) {
                        posted.remove(&n.name);
                    }
                }
                crate::holder::CsrHeal::Wait { since: s } => {
                    since.insert(n.name.clone(), s);
                }
                crate::holder::CsrHeal::Rotate => {
                    since.remove(&n.name);
                    match self.rotate_cert(&n.name, &row).await {
                        Ok(_) => {
                            posted.insert(n.name.clone(), row_spki);
                        }
                        Err(e) => tracing::warn!("share: csr heal of {}: {e:#}", n.name),
                    }
                }
            }
        }
    }

    /// `GET /v1/names/{name}` through the current connection.
    pub async fn check_name(&self, name: &str) -> anyhow::Result<share_proto::NameAvailability> {
        names::check_reserved::<&str>(name, &[]).map_err(|e| anyhow::anyhow!(e.reason()))?;
        self.with_control(|c, _| async move { c.check_name(name).await })
            .await
    }

    async fn after_cert(&self, name: &str, renew: bool) {
        let link = self.link.lock().clone();
        let Some(link) = link else { return };
        match self.ensure_cert(&link, name, renew).await {
            Ok(CertStep::Installed) => {}
            Ok(CertStep::Pending(_)) => return,
            Err(e) => {
                tracing::warn!("share: certificate for {name}: {e:#}");
                return;
            }
        }
        let name_host = names::host(name, None, &link.info.domain);
        let now = platform::time::now_ms();
        let stop_all = self.store.stop_all_ms().await.ok().flatten();
        let Ok(mine) = self.mine().await else { return };
        for s in mine {
            let under = name_host_of(&s.host, &s.domain) == name_host;
            if !under || s.state != ShareState::Pending {
                continue;
            }
            // M7: a share asked for before a Stop all never goes live after it.
            if stop_all.is_some_and(|b| s.created_ms <= b) {
                let _ = self.stop(&s.id, StoppedBy::User).await;
                continue;
            }
            if !self.may_mount(&s).await {
                continue;
            }
            let expiry = match self.store.request_expiry(&s.id).await {
                Ok(Some(ms)) => ms,
                _ => self.cfg.default_expiry.duration().as_millis() as u64,
            };
            let pending = s.clone();
            let s = go_live(s, now, expiry);
            if s.access == Access::Password {
                match self.store.secret(&s.id).await {
                    Ok(Some(phc)) => self.gate.set_password(&s.id, Some(phc)),
                    _ => {
                        let _ = self.stop(&s.id, StoppedBy::User).await;
                        continue;
                    }
                }
            }
            let Some(m) = self.mount(&s).await else {
                continue;
            };
            // The local approval binds the expiry (R4): it moves with the row, else nothing.
            if !matches!(self.store.rebind_approval(&pending, &s).await, Ok(true)) {
                continue;
            }
            if self.store.put(&s).await.is_ok() {
                let _ = self.store.delete_request(&s.id).await;
                self.table.insert(m);
                self.emit(&s).await;
            } else {
                let _ = self.store.rebind_approval(&s, &pending).await;
            }
        }
        self.push_routes().await;
    }
}

/// A pending share goes live with the lifetime the owner approved (M8).
fn go_live(mut s: Share, now: u64, expiry_ms: u64) -> Share {
    s.state = ShareState::Live;
    s.live_ms = Some(now);
    s.expires_ms = Some(now + expiry_ms);
    s
}

#[async_trait::async_trait]
impl Attachment for Engine {
    fn hosts(&self) -> Vec<String> {
        self.table.hosts()
    }

    async fn routes(&self) -> share_proto::RoutesPut {
        self.routes_put().await
    }

    async fn welcome(&self, control: &Control, info: &Info, welcome: &Welcome) {
        self.gate.set_operator(Operator {
            name: info.domain.clone(),
            abuse_url: info.abuse_url.clone(),
        });
        let skew = share_proto::Delegation::clock_skew_ms(platform::time::now_ms(), info.now_ms);
        let _ = self
            .store
            .attached(&info.edge_id.to_string(), platform::time::now_ms(), skew)
            .await;
        *self.link.lock() = Some(Arc::new(Link {
            control: control.clone(),
            info: info.clone(),
            welcome: welcome.clone(),
        }));
        // D22: the mesh is known at this edge; the holder's removal duties dial it.
        if let Ok(choice) = self.store.edge().await
            && let Some(t) = choice.tickets().into_iter().find(|t| t.addr.id == info.edge_id)
        {
            let row = crate::rekey::EdgeRow::new(info.edge_id, &info.domain, &t.to_string());
            if let Err(e) = self.store.put_share_edge(&row, platform::time::now_ms()).await {
                tracing::warn!("share: share_edges row: {e:#}");
            }
        }
        if let Some(stop_all) = welcome.stop_all_ms {
            // Every share asked for before it, pending ones included (M7).
            let _ = self.stop_local(StoppedBy::User, Some(stop_all.saturating_sub(1))).await;
        }
        if self.store.stop_all_pending().await.ok().flatten().is_some()
            && let Err(e) = control.stop_all().await
        {
            tracing::warn!("share: pending Stop all, POST /v1/mesh/stop: {e}");
        } else {
            let _ = self.store.stop_all_sent().await;
        }
        // Released or banned names lose their chain (B2).
        for n in welcome
            .names
            .iter()
            .filter(|n| matches!(n.state, NameState::Released | NameState::Banned))
        {
            let name_host = names::host(&n.name, None, &info.domain);
            self.certs.remove(&name_host);
            self.installed.lock().remove(&name_host);
        }
        self.restore().await;
        for n in welcome.names.iter().filter(|n| n.state == NameState::Valid) {
            self.after_cert(&n.name, false).await;
        }
        // Restore routes of shares that were live before a reconnect.
        if let Ok(mine) = self.mine().await {
            for mut s in mine.into_iter().filter(|s| s.state == ShareState::Offline) {
                if !self.may_mount(&s).await {
                    continue;
                }
                s.state = ShareState::Live;
                let Some(m) = self.mount(&s).await else {
                    continue;
                };
                if self.store.put(&s).await.is_ok() {
                    self.table.insert(m);
                    self.emit(&s).await;
                }
            }
        }
        self.attached.send_replace(true);
    }

    async fn event(&self, _control: &Control, event: Event) {
        match event {
            Event::Cert(c) => self.after_cert(&c.name, true).await,
            Event::Kill(k) => {
                let by = match k.reason {
                    KillReason::EdgeBan => StoppedBy::EdgeBan,
                    KillReason::Budget => StoppedBy::Budget,
                    _ => StoppedBy::User,
                };
                let Ok(mine) = self.mine().await else { return };
                for s in mine {
                    if k.host.as_deref().is_none_or(|h| h == s.host) {
                        let _ = self.stop(&s.id, by).await;
                    }
                }
            }
            // TODO(210-M3): `queue` events feed the slot-queue UI; `stats` feed `share.traffic`;
            // `throttle` feeds the throttled badge.
            Event::Queue(_)
            | Event::Stats(_)
            | Event::Throttle(_)
            | Event::Routes(_)
            | Event::Drain(_) => {}
        }
    }

    fn visitor(&self, stream: Stream) {
        let (tls, proxy) = (self.tls.clone(), self.proxy.clone());
        tokio::spawn(async move {
            if let Err(e) = visitor::serve(stream, tls, proxy).await {
                tracing::debug!("share: visitor: {e:#}");
            }
        });
    }

    fn lost(&self) {
        *self.link.lock() = None;
        self.attached.send_replace(false);
        // TODO(210-M1): live shares turn `offline` after 10 s without a reconnect (§5.9).
    }

    async fn removed(&self) {
        let _ = self.stop_local(StoppedBy::User, None).await;
        // TODO(210-M2): drop the stale `mesh/share_key` and `share/*` rows. Not done: a
        // `revoked` answer alone does not prove this engine left the family, and deleting a
        // replicated vault row while still paired would delete the family's key.
    }

    fn state(&self, state: &EdgeState, clock_off_ms: Option<i64>) {
        let Some(rt) = self.rt.clone() else { return };
        let (edge, domain) = match self.link.lock().as_ref() {
            Some(l) => (l.info.edge_id.to_string(), Some(l.info.domain.clone())),
            None => (String::new(), None),
        };
        let ev = ShareStatus {
            edge,
            domain,
            state: state.clone(),
            clock_off_ms,
        };
        tokio::spawn(async move { rt.publish(ev).await });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Expiry;
    use crate::sources::{NotTheuth, OwnListeners};

    async fn engine(max_live: u32) -> (tempfile::TempDir, Arc<Engine>) {
        engine_with(max_live, None).await
    }

    async fn engine_with(
        max_live: u32,
        name_keys: Option<Arc<dyn NameKeyStore>>,
    ) -> (tempfile::TempDir, Arc<Engine>) {
        engine_full(max_live, name_keys, None).await
    }

    async fn engine_full(
        max_live: u32,
        name_keys: Option<Arc<dyn NameKeyStore>>,
        delegation: Option<Arc<dyn DelegationSource>>,
    ) -> (tempfile::TempDir, Arc<Engine>) {
        // No dot in the path: a dir share inside it must fail on the data-dir rule, not the dot rule.
        let dir = tempfile::Builder::new().prefix("data").tempdir().unwrap();
        let store = Store::open(&dir.path().join("share.sqlite")).await.unwrap();
        let cfg = ShareConfig {
            max_live,
            ..ShareConfig::default()
        };
        let hooks = Hooks {
            dialer: None,
            delegation,
            name_keys,
            own_ports: Arc::new(OwnListeners),
            process: Arc::new(NotTheuth),
        };
        let e = Engine::new(cfg, store, dir.path().to_path_buf(), "me".into(), hooks, None).unwrap();
        (dir, e)
    }

    /// One name key in memory.
    struct MemKeys(Mutex<Option<keys::NameKey>>);

    #[async_trait::async_trait]
    impl NameKeyStore for MemKeys {
        async fn get(&self, _: &str, _: &str) -> anyhow::Result<Option<keys::NameKey>> {
            Ok(self.0.lock().clone())
        }
        async fn create(&self, _: &str, _: &str, k: keys::NameKey) -> anyhow::Result<keys::NameKey> {
            Ok(self.0.lock().get_or_insert(k).clone())
        }
    }

    /// B2: a `cert` event fetches the edge's new chain and replaces the installed certificate.
    #[tokio::test]
    async fn cert_event_replaces_the_installed_chain() {
        use axum::routing::get;

        let (key_pem, csr_der) = keys::name_key("sky", "tun.zip").unwrap();
        let leaf = |serial: u64| {
            let key = rcgen::KeyPair::from_pem(&key_pem).unwrap();
            let mut p = rcgen::CertificateParams::new(keys::sans("sky", "tun.zip")).unwrap();
            p.serial_number = Some(serial.into());
            let c = p.self_signed(&key).unwrap();
            (c.pem(), c.der().to_vec())
        };
        let ((pem1, der1), (pem2, der2)) = (leaf(1), leaf(2));
        let chain = Arc::new(Mutex::new(pem1));
        let served = chain.clone();
        let app = axum::Router::new().route(
            &share_proto::paths::name_cert("sky"),
            get(move || {
                let pem = served.lock().clone();
                async move {
                    axum::Json(share_proto::CertChain {
                        chain_pem: pem,
                        not_after: u64::MAX / 2,
                    })
                }
            }),
        );
        let control = Control::new(crate::edge::tests::FakeConn::new(app));
        let keys = MemKeys(Mutex::new(Some(keys::NameKey {
            r#gen: 1,
            key_pem: key_pem.clone(),
            csr_der,
        })));
        let (_d, e) = engine_with(8, Some(Arc::new(keys))).await;
        let edge_id = share_proto::SecretKey::from_bytes(&[7; 32]).public();
        let info = Info {
            v: 1,
            edge: "theuth-share-edge/0.1.0".into(),
            domain: "tun.zip".into(),
            edge_id,
            now_ms: platform::time::now_ms(),
            min_client: "0.0.1".into(),
            admission: share_proto::Admission::Open,
            terms_url: String::new(),
            terms_rev: "1".into(),
            abuse_url: String::new(),
            full: false,
        };
        let welcome = Welcome {
            v: 1,
            now_ms: platform::time::now_ms(),
            min_client: "0.0.1".into(),
            mesh: share_proto::SecretKey::from_bytes(&[1; 32]).public(),
            names: vec![NameInfo {
                name: "sky".into(),
                kind: NameKind::Random,
                state: NameState::Valid,
                ca: None,
                spki: None,
                not_after: Some(u64::MAX / 2),
                queue: None,
                transfer_to: None,
                transfer_until_ms: None,
            }],
            routes: vec![],
            revoked: vec![],
            stop_all_ms: None,
            domain: "tun.zip".into(),
            limits: share_proto::WelcomeLimits::default(),
            terms_url: String::new(),
            abuse_url: String::new(),
            terms_rev: "1".into(),
            terms_ok: true,
            full: false,
            retire: None,
            throttled: false,
        };
        let leaf_of = |e: &Engine| {
            e.certs
                .lookup("app.sky.tun.zip")
                .map(|ck| ck.cert[0].as_ref().to_vec())
        };

        e.welcome(&control, &info, &welcome).await;
        assert_eq!(leaf_of(&e), Some(der1), "welcome installs the valid name's chain");
        let held = crate::names::list(&e).await.unwrap();
        let sky = held.iter().find(|h| h.name == "sky").unwrap();
        assert_eq!(sky.state.as_deref(), Some("valid"));
        assert_eq!(sky.cert_not_after_ms, Some(u64::MAX / 2), "the installed chain's notAfter");

        *chain.lock() = pem2;
        e.event(
            &control,
            Event::Cert(share_proto::events::CertEvent { name: "sky".into() }),
        )
        .await;
        assert_eq!(leaf_of(&e), Some(der2), "the renewed chain replaced the old one");
    }

    /// An edge answering `GET /v1/info` and `POST /v1/names/status` (one valid name) only.
    struct StatusDialer {
        edge: share_proto::SecretKey,
        dials: std::sync::atomic::AtomicU32,
    }

    #[async_trait::async_trait]
    impl EdgeDialer for StatusDialer {
        async fn dial(
            &self,
            _: share_proto::EndpointAddr,
            _: &[&[u8]],
        ) -> anyhow::Result<Arc<dyn crate::edge::EdgeConn>> {
            self.dials.fetch_add(1, Ordering::SeqCst);
            let info = Info {
                v: 1,
                edge: "theuth-share-edge/0.1.0".into(),
                domain: "tun.zip".into(),
                edge_id: self.edge.public(),
                now_ms: platform::time::now_ms(),
                min_client: "0.0.1".into(),
                admission: share_proto::Admission::Open,
                terms_url: String::new(),
                terms_rev: "1".into(),
                abuse_url: String::new(),
                full: false,
            };
            let status = share_proto::NamesStatus {
                now_ms: platform::time::now_ms(),
                mesh: share_proto::SecretKey::from_bytes(&[1; 32]).public(),
                names: vec![NameInfo {
                    name: "sky".into(),
                    kind: NameKind::Reserved,
                    state: NameState::Valid,
                    ca: None,
                    spki: None,
                    not_after: Some(1_900_000_000_000),
                    queue: None,
                    transfer_to: None,
                    transfer_until_ms: None,
                }],
            };
            let app = axum::Router::new()
                .route(
                    share_proto::paths::INFO,
                    axum::routing::get(move || std::future::ready(axum::Json(info.clone()))),
                )
                .route(
                    share_proto::paths::NAMES_STATUS,
                    axum::routing::post(move || std::future::ready(axum::Json(status.clone()))),
                );
            Ok(crate::edge::tests::FakeConn::new(app))
        }
    }

    /// CERT-STATE: a detached `share_names_list` reads the edge's view with one names status
    /// (cached), never attaches (`need` stays false), and asks nothing with the switch off.
    #[tokio::test]
    async fn detached_names_list_uses_the_names_status_and_never_attaches() {
        let dir = tempfile::Builder::new().prefix("data").tempdir().unwrap();
        let store = Store::open(&dir.path().join("share.sqlite")).await.unwrap();
        let edge = share_proto::SecretKey::from_bytes(&[7; 32]);
        let dialer = Arc::new(StatusDialer {
            edge: edge.clone(),
            dials: Default::default(),
        });
        let hooks = Hooks {
            dialer: Some(dialer.clone()),
            delegation: Some(Arc::new(crate::edge::SelfHolder {
                mesh_key: share_proto::SecretKey::from_bytes(&[1; 32]),
                engine: share_proto::SecretKey::from_bytes(&[2; 32]).public(),
            })),
            name_keys: None,
            own_ports: Arc::new(OwnListeners),
            process: Arc::new(NotTheuth),
        };
        let e = Engine::new(ShareConfig::default(), store, dir.path().to_path_buf(), "me".into(), hooks, None)
            .unwrap();
        let ticket = share_proto::ShareEdgeTicket {
            addr: share_proto::EndpointAddr::new(edge.public()),
            invite: None,
        };
        e.store.set_edge(&EdgeChoice::Ticket(Box::new(ticket))).await.unwrap();

        e.store.set_enabled(false).await.unwrap();
        assert!(crate::names::list(&e).await.unwrap().is_empty());
        assert_eq!(dialer.dials.load(Ordering::SeqCst), 0, "switch off: nothing dialled");

        e.store.set_enabled(true).await.unwrap();
        let held = crate::names::list(&e).await.unwrap();
        let sky = held.iter().find(|h| h.name == "sky").unwrap();
        assert_eq!(sky.state.as_deref(), Some("valid"));
        assert_eq!(sky.cert_not_after_ms, Some(1_900_000_000_000), "the edge's notAfter");
        assert!(!*e.need.borrow(), "a names status is no attach");
        assert!(e.welcome_names().is_none());

        crate::names::list(&e).await.unwrap();
        assert_eq!(dialer.dials.load(Ordering::SeqCst), 1, "the answer is cached");
    }

    /// A share row this engine's consent path wrote (row + local approval), switch on.
    async fn put_approved(e: &Engine, s: &Share) {
        e.store.set_enabled(true).await.unwrap();
        e.store.put(s).await.unwrap();
        e.store.put_approval(s, 0).await.unwrap();
    }

    fn share(id: &str, created_ms: u64, state: ShareState, access: Access) -> Share {
        Share {
            id: id.into(),
            engine: "me".into(),
            edge: "x".into(),
            domain: "tun.zip".into(),
            host: format!("{id}.sky.tun.zip"),
            path: "/".into(),
            url: format!("https://{id}.sky.tun.zip/"),
            source: Source::Port(5173),
            access,
            interstitial: true,
            created_ms,
            live_ms: (state == ShareState::Live).then_some(created_ms),
            expires_ms: (state == ShareState::Live).then_some(u64::MAX / 2),
            created_by: CreatedBy::Settings("settings".into()),
            approved_by: Consent::test().0,
            state,
            stopped_ms: None,
            stopped_by: None,
        }
    }

    /// B1: only a human-only (`owner`) row with an id is consent; a publish never runs without.
    #[test]
    fn consent_needs_an_owner_row() {
        let id = ::tools::ApprovalId(uuid::Uuid::now_v7());
        assert!(Consent::from_approved(::tools::Approved { id, surface: "human" }, None).is_err());
        let nil = ::tools::ApprovalId(uuid::Uuid::nil());
        assert!(
            Consent::from_approved(
                ::tools::Approved {
                    id: nil,
                    surface: "owner"
                },
                None
            )
            .is_err()
        );
        assert!(Consent::of_call().is_none(), "no row outside a gated call");
        let c = Consent::from_approved(::tools::Approved { id, surface: "owner" }, None).unwrap();
        let now = platform::time::now_ms();
        assert!(now.abs_diff(c.asked_ms()) < 5_000, "asked_ms from the row's v7 id");
        assert_eq!(c.approved_by().approval_id, id.0.to_string());
    }

    /// M7: Stop all stops pending shares too, keeps going past them, and reports a detached
    /// edge stop as pending (kept for the next attach).
    #[tokio::test]
    async fn stop_all_stops_pending_and_reports_the_edge_pending() {
        let (_d, e) = engine(8).await;
        e.store
            .put(&share("p", 1, ShareState::Pending, Access::Public))
            .await
            .unwrap();
        e.store
            .put(&share("l", 2, ShareState::Live, Access::Public))
            .await
            .unwrap();
        let o = e.stop_all().await.unwrap();
        assert_eq!(o.stopped, 2);
        assert!(o.edge_pending.is_some());
        assert!(e.mine().await.unwrap().is_empty());
        assert!(e.store.stop_all_pending().await.unwrap().is_some());
    }

    /// M8: a pending share goes live with the lifetime the owner approved.
    #[tokio::test]
    async fn pending_share_keeps_the_approved_expiry() {
        let (_d, e) = engine(8).await;
        let week = Expiry::D7.duration().as_millis() as u64;
        e.store.put_request("p", week).await.unwrap();
        let ms = e.store.request_expiry("p").await.unwrap().unwrap();
        let s = go_live(share("p", 1, ShareState::Pending, Access::Public), 100, ms);
        assert_eq!((s.state, s.expires_ms), (ShareState::Live, Some(100 + week)));
    }

    /// M9: after a restart live shares are routed again; a password share without its hash is
    /// stopped instead of serving 401 to everyone.
    #[tokio::test]
    async fn restore_remounts_live_shares_and_stops_hashless_password_shares() {
        let (_d, e) = engine(8).await;
        put_approved(&e, &share("a", 1, ShareState::Live, Access::Password)).await;
        e.store.put_secret("a", "$argon2id$x").await.unwrap();
        put_approved(&e, &share("b", 2, ShareState::Live, Access::Password)).await;
        e.restore().await;
        assert!(matches!(
            e.table.lookup("a.sky.tun.zip", "/"),
            Lookup::Found(m, _) if m.share == "a"
        ));
        assert!(!matches!(
            e.table.lookup("b.sky.tun.zip", "/"),
            Lookup::Found(..)
        ));
        let b = e.store.get("b").await.unwrap().unwrap();
        assert_eq!(b.state, ShareState::Stopped);
    }

    /// m6: the `max_live` count and the reservation are one step.
    #[tokio::test]
    async fn max_live_reservations_are_counted() {
        let (_d, e) = engine(2).await;
        e.store
            .put(&share("l", 1, ShareState::Live, Access::Public))
            .await
            .unwrap();
        e.reserve().await.unwrap();
        assert!(e.reserve().await.unwrap_err().to_string().contains("max_live"));
    }

    /// M1 after a restart: a dir share is mounted from the pin stored at publish; a folder
    /// swapped while the engine was down, a root inside this engine's own data dir, or a share
    /// without a pin stops (`source_gone`) instead of being pinned again.
    #[tokio::test]
    async fn restore_mounts_dirs_from_the_stored_pin_and_stops_swapped_roots() {
        let (data, e) = engine(8).await;
        let tmp = tempfile::Builder::new().prefix("share").tempdir().unwrap();
        let elsewhere = tmp.path().join("elsewhere");
        let (kept, swapped) = (tmp.path().join("kept"), tmp.path().join("swapped"));
        let inside = data.path().join("site");
        for d in [&kept, &swapped, &inside, &elsewhere] {
            std::fs::create_dir_all(d).unwrap();
        }
        let dir_share = |id: &str, root: &std::path::Path| Share {
            source: Source::Dir(root.to_str().unwrap().into()),
            ..share(id, 1, ShareState::Live, Access::Public)
        };
        for (id, root) in [("a", &kept), ("b", &swapped), ("c", &inside)] {
            put_approved(&e, &dir_share(id, root)).await;
            // `c` was pinned against another data dir (the platform default, say).
            let pin = sources::pin_root(root.to_str().unwrap(), &elsewhere, None).unwrap();
            e.store.put_pin(id, &pin).await.unwrap();
        }
        put_approved(&e, &dir_share("d", &kept)).await;
        std::fs::rename(&swapped, tmp.path().join("old")).unwrap();
        std::fs::create_dir(&swapped).unwrap();

        e.restore().await;
        let Lookup::Found(m, _) = e.table.lookup("a.sky.tun.zip", "/") else {
            panic!("a is not routed");
        };
        assert!(m.dir.is_some());
        assert_eq!(m.dir, e.store.pin("a").await.unwrap());
        for id in ["b", "c", "d"] {
            assert!(
                !matches!(
                    e.table.lookup(&format!("{id}.sky.tun.zip"), "/"),
                    Lookup::Found(..)
                ),
                "{id} is routed"
            );
            let s = e.store.get(id).await.unwrap().unwrap();
            assert_eq!(
                (s.state, s.stopped_by),
                (ShareState::Stopped, Some(StoppedBy::SourceGone)),
                "{id}"
            );
            assert!(e.store.pin(id).await.unwrap().is_none(), "{id}'s pin is forgotten");
        }
    }

    fn test_info() -> Info {
        Info {
            v: 1,
            edge: "theuth-share-edge/0.1.0".into(),
            domain: "tun.zip".into(),
            edge_id: share_proto::SecretKey::from_bytes(&[7; 32]).public(),
            now_ms: platform::time::now_ms(),
            min_client: "0.0.1".into(),
            admission: share_proto::Admission::Open,
            terms_url: "https://tun.zip/terms".into(),
            terms_rev: "2026-11-23".into(),
            abuse_url: String::new(),
            full: false,
        }
    }

    fn test_welcome(names: &[(&str, NameState)]) -> Welcome {
        Welcome {
            v: 1,
            now_ms: platform::time::now_ms(),
            min_client: "0.0.1".into(),
            mesh: share_proto::SecretKey::from_bytes(&[1; 32]).public(),
            names: names
                .iter()
                .map(|(n, state)| NameInfo {
                    name: (*n).into(),
                    kind: NameKind::Random,
                    state: *state,
                    ca: None,
                    spki: None,
                    not_after: None,
                    queue: None,
                    transfer_to: None,
                    transfer_until_ms: None,
                })
                .collect(),
            routes: vec![],
            revoked: vec![],
            stop_all_ms: None,
            domain: "tun.zip".into(),
            limits: share_proto::WelcomeLimits::default(),
            terms_url: String::new(),
            abuse_url: String::new(),
            terms_rev: "2026-11-23".into(),
            terms_ok: true,
            full: false,
            retire: None,
            throttled: false,
        }
    }

    /// An edge serving `GET /v1/info`.
    fn info_edge() -> Control {
        let app = axum::Router::new().route(
            share_proto::paths::INFO,
            axum::routing::get(|| async { axum::Json(test_info()) }),
        );
        Control::new(crate::edge::tests::FakeConn::new(app))
    }

    /// E2E1 finding 4: a control call never goes out on a connection the edge closed (after
    /// Stop all); one that fails at the transport drops the link and is tried once more after
    /// the redial.
    #[tokio::test]
    async fn control_calls_redial_a_closed_connection() {
        let (_d, e) = engine(8).await;
        let (info, welcome) = (test_info(), test_welcome(&[]));
        let closed = Control::new(crate::edge::tests::FakeConn::new(axum::Router::new()));
        closed.close();
        for stale in [closed, Control::new(crate::edge::tests::FakeConn::dead())] {
            e.welcome(&stale, &info, &welcome).await;
            // The state machine's redial, here by hand.
            let redial = tokio::spawn({
                let (e, info, welcome) = (e.clone(), info.clone(), welcome.clone());
                async move {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    e.welcome(&info_edge(), &info, &welcome).await;
                }
            });
            let got = tokio::time::timeout(
                Duration::from_secs(5),
                e.with_control(|c, _| async move { c.info().await }),
            )
            .await
            .expect("answers after the redial")
            .unwrap();
            assert_eq!(got.domain, "tun.zip");
            assert!(!e.link.lock().as_ref().unwrap().control.is_closed());
            redial.await.unwrap();
        }
    }

    /// E2E1 finding 8: terms are checked against the edge's current revision before the owner
    /// is asked; a held name (or a requested one) needs no fresh allocation and no terms.
    #[tokio::test]
    async fn publish_precheck_refuses_unaccepted_terms_before_the_ask() {
        let (_d, e) = engine(8).await;
        let info = test_info();
        assert!(
            e.publish_precheck(None)
                .await
                .unwrap_err()
                .to_string()
                .contains("Public sharing is off")
        );
        e.store.set_enabled(true).await.unwrap();
        e.welcome(&info_edge(), &info, &test_welcome(&[])).await;
        let err = e.publish_precheck(None).await.unwrap_err().to_string();
        assert!(
            err.contains("accept the terms of tun.zip first (https://tun.zip/terms, revision 2026-11-23)"),
            "{err}"
        );
        e.publish_precheck(Some("sky")).await.unwrap();
        e.store
            .set_terms_rev(&info.edge_id.to_string(), "2026-01-01")
            .await
            .unwrap();
        assert!(e.publish_precheck(None).await.is_err(), "an older revision is not enough");
        e.store
            .set_terms_rev(&info.edge_id.to_string(), "2026-11-23")
            .await
            .unwrap();
        e.publish_precheck(None).await.unwrap();

        let (_d2, e2) = engine(8).await;
        e2.store.set_enabled(true).await.unwrap();
        e2.welcome(&info_edge(), &info, &test_welcome(&[("sky", NameState::Allocated)]))
            .await;
        e2.publish_precheck(None).await.unwrap();
    }

    /// TERMS-FIRST: reserving a name with the edge's terms not accepted answers the terms
    /// refusal the desktop parses and never takes the owner's consent (no card is raised);
    /// accepting under the domain the refusal names (what the desktop sends) is enough.
    #[tokio::test]
    async fn reserve_refuses_unaccepted_terms_before_the_consent() {
        let (_d, e) = engine(8).await;
        let info = test_info();
        e.store.set_enabled(true).await.unwrap();
        e.welcome(&info_edge(), &info, &test_welcome(&[])).await;
        let asked = std::sync::atomic::AtomicU32::new(0);
        let ask = || async {
            asked.fetch_add(1, Ordering::SeqCst);
            Ok(Consent::test())
        };
        let err = format!("{:#}", crate::names::reserve(&e, "sky", ask).await.unwrap_err());
        assert!(
            err.contains("accept the terms of tun.zip first (https://tun.zip/terms, revision 2026-11-23)"),
            "{err}"
        );
        assert_eq!(asked.load(Ordering::SeqCst), 0, "no consent is asked for");
        assert_eq!(
            e.edge_terms().await.unwrap(),
            Some(EdgeTerms {
                edge: info.edge_id.to_string(),
                domain: "tun.zip".into(),
                url: "https://tun.zip/terms".into(),
                rev: "2026-11-23".into(),
                accepted: false,
            })
        );

        e.accept_terms("tun.zip", "2026-11-23", &Consent::test()).await.unwrap();
        assert!(e.edge_terms().await.unwrap().unwrap().accepted);
        e.terms_precheck().await.unwrap();
        // Past the terms the consent is taken, then the claim goes to the edge (this fake one
        // serves only `/v1/info`, so the claim itself fails).
        assert!(crate::names::reserve(&e, "sky", ask).await.is_err());
        assert_eq!(asked.load(Ordering::SeqCst), 1);

        e.store.set_enabled(false).await.unwrap();
        assert_eq!(e.edge_terms().await.unwrap(), None, "nothing dials while sharing is off");
    }

    /// TERMS-FIRST: the owner's own press stands for consent only from a connection that is
    /// neither a guest nor the internal peer (MCP console, ACP half, TUI, forwarded calls).
    #[tokio::test]
    async fn only_the_owners_client_presses_without_a_card() {
        let ext = |p: Option<rpc_core::RpcPeer>| {
            let mut x = jsonrpsee::Extensions::new();
            if let Some(p) = p {
                x.insert(p);
            }
            x
        };
        let (ui, _g) = rpc_core::RpcPeer::open();
        let (device, _g2) = rpc_core::RpcPeer::open_remote();
        let (guest, _g3) = rpc_core::RpcPeer::open_guest([1; 32], [2; 32]);
        let c = Consent::of_press(&ext(Some(ui))).expect("the owner's desktop socket presses");
        assert_eq!(c.approved_by().surface, PRESS);
        assert!(!c.approved_by().approval_id.is_empty());
        assert!(Consent::of_press(&ext(Some(device))).is_some());
        assert!(Consent::of_press(&ext(Some(rpc_core::RpcPeer::internal()))).is_none());
        assert!(Consent::of_press(&ext(Some(guest))).is_none());
        assert!(Consent::of_press(&ext(None)).is_none());
        // Not a press: the human-only row is raised (none runs here, so the ask itself fails).
        let err = Consent::press_or_ask(
            &ext(Some(rpc_core::RpcPeer::internal())),
            "share_names_reserve",
            serde_json::json!({ "name": "sky" }),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("share_names_reserve: no approval service is running"), "{err}");
    }

    /// TERMS-FIRST: the agent's tool path never takes a press; it acts only inside the
    /// registry's human-only approval (`HUMAN_ONLY`), and outside one it refuses.
    #[tokio::test]
    async fn the_agent_publish_tool_still_needs_the_approval_row() {
        use ::tools::Tool as _;
        const { assert!(crate::tools::SharePublishTool::HUMAN_ONLY) };
        let (d, e) = engine(8).await;
        e.store.set_enabled(true).await.unwrap();
        let tool = crate::tools::SharePublishTool::new(e.clone());
        let cx = ::tools::ToolCx {
            session: runtime::SessionId::new(),
            call_id: "c".into(),
            quiet: false,
            resources: ::tools::SessionResources::durable_arc(d.path()),
            cancel: ::tools::CancellationToken::new(),
            promote_after: None,
        };
        let args = serde_json::from_value(serde_json::json!({"source": {"port": 5173}})).unwrap();
        let out = tool.call(args, cx).await.unwrap();
        assert!(out.is_error, "{out:?}");
        assert!(e.list().await.unwrap().is_empty());
    }

    /// A name key that is minted only when `release` is notified (the vault settle).
    struct SlowKeys {
        key: keys::NameKey,
        release: tokio::sync::Notify,
        made: Mutex<bool>,
    }

    #[async_trait::async_trait]
    impl NameKeyStore for SlowKeys {
        async fn get(&self, _: &str, _: &str) -> anyhow::Result<Option<keys::NameKey>> {
            Ok(self.made.lock().then(|| self.key.clone()))
        }
        async fn create(&self, _: &str, _: &str, _: keys::NameKey) -> anyhow::Result<keys::NameKey> {
            self.release.notified().await;
            *self.made.lock() = true;
            Ok(self.key.clone())
        }
    }

    /// E2E1 finding 9: a cold publish (the name key still settling) answers `pending` within
    /// [`PUBLISH_CERT_WAIT`], and the share goes live by itself once the certificate step ends.
    #[tokio::test]
    async fn cold_publish_answers_pending_promptly_and_goes_live_later() {
        use axum::routing::{get, post};

        let (key_pem, csr_der) = keys::name_key("sky", "tun.zip").unwrap();
        let chain = {
            let key = rcgen::KeyPair::from_pem(&key_pem).unwrap();
            let p = rcgen::CertificateParams::new(keys::sans("sky", "tun.zip")).unwrap();
            share_proto::CertChain {
                chain_pem: p.self_signed(&key).unwrap().pem(),
                not_after: u64::MAX / 2,
            }
        };
        let (c1, c2) = (chain.clone(), chain);
        let app = axum::Router::new()
            .route(
                &share_proto::paths::name_csr("sky", false),
                post(move || async move { axum::Json(c1) }),
            )
            .route(
                &share_proto::paths::name_cert("sky"),
                get(move || async move { axum::Json(c2) }),
            );
        let slow = Arc::new(SlowKeys {
            key: keys::NameKey {
                r#gen: 1,
                key_pem,
                csr_der,
            },
            release: tokio::sync::Notify::new(),
            made: Mutex::new(false),
        });
        let (_d, e) = engine_with(8, Some(slow.clone() as Arc<dyn NameKeyStore>)).await;
        e.store.set_enabled(true).await.unwrap();
        let control = Control::new(crate::edge::tests::FakeConn::new(app));
        e.welcome(&control, &test_info(), &test_welcome(&[("sky", NameState::Allocated)]))
            .await;
        let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let plan = Plan {
            source: Source::Port(upstream.local_addr().unwrap().port()),
            label: Some("app".into()),
            name: None,
            path: "/".into(),
            expiry: Expiry::H1,
            access: Access::Public,
        };
        let started = tokio::time::Instant::now();
        let p = e
            .publish(plan, CallOrigin::Settings, Consent::test())
            .await
            .unwrap();
        assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
        assert_eq!(p.share.state, ShareState::Pending);
        assert_eq!(p.pending, Some(PendingCert::default()));

        slow.release.notify_one();
        let live = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let s = e.store.get(&p.share.id).await.unwrap().unwrap();
                if s.state == ShareState::Live {
                    break s;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the share goes live once the certificate step ends");
        assert!(live.expires_ms.is_some());
        assert!(
            e.store.locally_approved(&live).await.unwrap(),
            "the local approval moved with the expiry"
        );
        assert!(e.certs.lookup("app.sky.tun.zip").is_some());
        assert!(matches!(e.table.lookup("app.sky.tun.zip", "/"), Lookup::Found(..)));
    }

    /// FIX4 #1: a `shares` row naming this engine that arrived by replication (no local approval
    /// record), or whose approved host/source was altered elsewhere, is never mounted and is
    /// reported; with the switch off nothing mounts either.
    #[tokio::test]
    async fn replicated_rows_without_a_local_approval_stay_unmounted() {
        let (_d, e) = engine(8).await;
        e.store.set_enabled(true).await.unwrap();
        let foreign = share("r", 1, ShareState::Live, Access::Public);
        e.store.put(&foreign).await.unwrap();
        put_approved(&e, &share("a", 2, ShareState::Live, Access::Public)).await;
        let altered = share("x", 3, ShareState::Live, Access::Public);
        put_approved(&e, &altered).await;
        sqlx::query("UPDATE shares SET source = ? WHERE id = 'x'")
            .bind(serde_json::to_string(&Source::Port(22)).unwrap())
            .execute(e.store.pool())
            .await
            .unwrap();
        e.restore().await;
        let routed = |id: &str| matches!(e.table.lookup(&format!("{id}.sky.tun.zip"), "/"), Lookup::Found(..));
        assert!(routed("a"));
        assert!(!routed("r"), "replicated row mounted");
        assert!(!routed("x"), "altered row mounted");
        assert_eq!(e.unmounted(), vec!["r".to_owned(), "x".to_owned()]);
        assert_eq!(e.store.get("r").await.unwrap().unwrap().state, ShareState::Live, "left as is");
        // Offline → Live at a welcome goes through the same gate.
        e.store
            .put(&share("o", 4, ShareState::Offline, Access::Public))
            .await
            .unwrap();
        e.welcome(&info_edge(), &test_info(), &test_welcome(&[])).await;
        assert_eq!(e.store.get("o").await.unwrap().unwrap().state, ShareState::Offline);
        assert!(!routed("o"));

        e.table.remove("a");
        e.store.set_enabled(false).await.unwrap();
        e.restore().await;
        assert!(!routed("a"), "mounted with the switch off");
    }

    /// FIX4 #10 (§7.4): a restart stops this engine's shares except those created from
    /// Settings under a reserved name the mesh holds; an expired one stops as `expiry`.
    #[tokio::test]
    async fn start_stops_shares_except_reserved_settings_ones() {
        let (_d, e) = engine(8).await;
        e.store
            .put_name("x", "tun.zip", "sky", NameKind::Reserved)
            .await
            .unwrap();
        let call = Share {
            created_by: CreatedBy::Call {
                session: "s".into(),
                call_id: "c".into(),
            },
            ..share("c", 1, ShareState::Live, Access::Public)
        };
        let random_name = Share {
            host: "z.other.tun.zip".into(),
            ..share("z", 2, ShareState::Live, Access::Public)
        };
        let expired = Share {
            expires_ms: Some(5),
            ..share("e", 3, ShareState::Live, Access::Public)
        };
        let kept = share("k", 4, ShareState::Live, Access::Public);
        for s in [&call, &random_name, &expired, &kept] {
            put_approved(&e, s).await;
        }
        e.start().await.unwrap();
        for (id, want) in [
            ("c", (ShareState::Stopped, Some(StoppedBy::User))),
            ("z", (ShareState::Stopped, Some(StoppedBy::User))),
            ("e", (ShareState::Stopped, Some(StoppedBy::Expiry))),
            ("k", (ShareState::Live, None)),
        ] {
            let s = e.store.get(id).await.unwrap().unwrap();
            assert_eq!((s.state, s.stopped_by), want, "{id}");
        }
        assert!(matches!(e.table.lookup("k.sky.tun.zip", "/"), Lookup::Found(..)));
    }

    /// FIX4 #6: an approval given before a Stop all (or with the switch turned off since) does
    /// not publish.
    #[tokio::test]
    async fn publish_refused_after_stop_all_or_switch_off() {
        let (_d, e) = engine(8).await;
        e.store.set_enabled(true).await.unwrap();
        let plan = || Plan {
            source: Source::Port(5173),
            label: Some("app".into()),
            name: None,
            path: "/".into(),
            expiry: Expiry::H1,
            access: Access::Public,
        };
        let asked = platform::time::now_ms();
        e.store.stop_all_at(asked + 10).await.unwrap();
        let err = e
            .publish(plan(), CallOrigin::Settings, Consent::test_asked_at(asked))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("Stop all was pressed"), "{err}");
        e.store.set_enabled(false).await.unwrap();
        let err = e
            .publish(plan(), CallOrigin::Settings, Consent::test_asked_at(asked + 20))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("Public sharing is off"), "{err}");
    }

    /// FIX4 #3: Stop all from a detached engine whose edge client runs waits for the attach
    /// and sends `POST /v1/mesh/stop` on it.
    #[tokio::test]
    async fn stop_all_attaches_first_when_detached() {
        let (_d, e) = engine(8).await;
        e.client.store(true, Ordering::SeqCst);
        let app = axum::Router::new().route(
            share_proto::paths::MESH_STOP,
            axum::routing::post(|| async { axum::Json(share_proto::StopAll { stop_all_ms: 1 }) }),
        );
        let control = Control::new(crate::edge::tests::FakeConn::new(app));
        let redial = tokio::spawn({
            let e = e.clone();
            async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                e.welcome(&control, &test_info(), &test_welcome(&[])).await;
            }
        });
        let o = tokio::time::timeout(Duration::from_secs(5), e.stop_all())
            .await
            .expect("bounded")
            .unwrap();
        assert_eq!(o.edge_pending, None);
        assert!(e.store.stop_all_pending().await.unwrap().is_none());
        redial.await.unwrap();
    }

    /// A fake edge's name endpoints: every check, claim and release it saw; a claim of
    /// `taken1` answers `name_taken`.
    #[derive(Default)]
    struct NameEdge {
        checks: Mutex<Vec<String>>,
        claims: Mutex<Vec<String>>,
        released: Mutex<Vec<String>>,
    }

    fn name_edge(edge: &Arc<NameEdge>) -> Control {
        use axum::extract::Path;
        use axum::response::IntoResponse as _;
        use axum::routing::{get, post};

        let (claim, check, release) = (edge.clone(), edge.clone(), edge.clone());
        let app = axum::Router::new()
            .route(
                share_proto::paths::NAMES,
                post(move |axum::Json(b): axum::Json<serde_json::Value>| async move {
                    let name = b["name"].as_str().unwrap().to_owned();
                    claim.claims.lock().push(name.clone());
                    if name == "taken1" {
                        let b = share_proto::ErrorBody::new(ErrorCode::NameTaken);
                        return (axum::http::StatusCode::CONFLICT, axum::Json(b)).into_response();
                    }
                    axum::Json(share_proto::NameGranted { name }).into_response()
                }),
            )
            .route(
                share_proto::paths::NAME,
                get(move |Path(n): Path<String>| async move {
                    check.checks.lock().push(n);
                    axum::Json(share_proto::NameAvailability {
                        available: true,
                        reason: None,
                        reclaim_after_ms: None,
                    })
                })
                .delete(move |Path(n): Path<String>| async move {
                    release.released.lock().push(n.clone());
                    axum::Json(share_proto::NameReleased {
                        name: n,
                        reclaim_after_ms: 1,
                    })
                }),
            );
        Control::new(crate::edge::tests::FakeConn::new(app))
    }

    /// FIX5 R2: the card's URL is known before the ask, but nothing is claimed for it: a held
    /// name is used as is; a first publish draws its random name locally (read-only check).
    /// After the approval the publish claims exactly the shown name, fails without claiming
    /// anything when it was taken meanwhile (FIX6 N3), and gives the claimed name back when it
    /// then fails (or is dropped).
    #[tokio::test]
    async fn preview_claims_nothing_and_a_failed_publish_gives_its_claim_back() {
        let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let plan = Plan {
            source: Source::Port(upstream.local_addr().unwrap().port()),
            label: Some("app".into()),
            name: None,
            path: "/".into(),
            expiry: Expiry::H1,
            access: Access::Public,
        };

        let (_d, e) = engine(8).await;
        e.welcome(&info_edge(), &test_info(), &test_welcome(&[("sky", NameState::Valid)]))
            .await;
        let p = e.publish_preview(&plan).await.unwrap();
        assert_eq!(
            p,
            PublishPreview {
                name: "sky".into(),
                url: "https://app.sky.tun.zip/".into(),
                drawn: false
            }
        );

        let edge = Arc::new(NameEdge::default());
        let (_d2, e2) = engine(8).await;
        e2.store.set_enabled(true).await.unwrap();
        let info = test_info();
        e2.store
            .set_terms_rev(&info.edge_id.to_string(), &info.terms_rev)
            .await
            .unwrap();
        e2.welcome(&name_edge(&edge), &info, &test_welcome(&[])).await;
        let p = e2.publish_preview(&plan).await.unwrap();
        assert!(p.drawn);
        assert_eq!(p.url, format!("https://app.{}.tun.zip/", p.name));
        assert_eq!(*edge.checks.lock(), vec![p.name.clone()], "checked read-only");
        assert!(edge.claims.lock().is_empty(), "nothing claimed before the approval");

        // Approved, but the shown name was taken meanwhile: the publish fails, claims nothing
        // and never publishes a URL other than the one the card showed.
        let shown = PublishPreview {
            name: "taken1".into(),
            url: "https://app.taken1.tun.zip/".into(),
            drawn: true,
        };
        let err = e2
            .publish_previewed(plan.clone(), CallOrigin::Settings, Consent::test(), shown)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), NAME_TAKEN_MEANWHILE);
        assert_eq!(*edge.claims.lock(), vec!["taken1".to_owned()], "no fresh draw");
        assert!(e2.fresh.lock().is_empty());
        assert!(e2.store.list(platform::time::now_ms()).await.unwrap().is_empty());

        // The shown name claimed, the publish then fails (this engine has no name keys): the
        // claim is given back.
        let shown = PublishPreview {
            name: "free1".into(),
            url: "https://app.free1.tun.zip/".into(),
            drawn: true,
        };
        let err = e2
            .publish_previewed(plan, CallOrigin::Settings, Consent::test(), shown)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("name keys"), "{err:#}");
        assert_eq!(*edge.claims.lock(), vec!["taken1".to_owned(), "free1".to_owned()]);
        tokio::time::timeout(Duration::from_secs(5), async {
            while *edge.released.lock() != vec!["free1".to_owned()] {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the claimed name is released");
        assert!(!e2.fresh.lock().contains("free1"));

        // A publish dropped after its claim releases it the same way.
        e2.fresh.lock().insert("dropped".into());
        drop(Claim {
            engine: Arc::downgrade(&e2),
            name: Some("dropped".into()),
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !edge.released.lock().contains(&"dropped".to_owned()) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("a dropped claim is released");
        assert!(!e2.fresh.lock().contains("dropped"));
    }

    /// LAUNCH-CLIENT: the tool path's card preview (`share_publish` called by the model). The
    /// preview names the URL and claims nothing; a denied card never claims; the approved call
    /// claims exactly the shown name, fails with `NAME_TAKEN_MEANWHILE` when it was taken, and
    /// refuses an expired or missing preview without claiming.
    #[tokio::test]
    async fn tool_preview_binds_the_call_to_the_shown_name() {
        use crate::tools::SharePublishTool;
        use ::tools::Tool as _;
        let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = upstream.local_addr().unwrap().port();
        let edge = Arc::new(NameEdge::default());
        let (d, e) = engine(8).await;
        e.store.set_enabled(true).await.unwrap();
        let info = test_info();
        e.store
            .set_terms_rev(&info.edge_id.to_string(), &info.terms_rev)
            .await
            .unwrap();
        e.welcome(&name_edge(&edge), &info, &test_welcome(&[])).await;
        let tool = SharePublishTool::new(e.clone());
        let session = runtime::SessionId::new();
        let args = || -> crate::tools::PublishArgs {
            serde_json::from_value(serde_json::json!({"source": {"port": port}, "label": "app"}))
                .unwrap()
        };
        let preview = |call_id: &str| ::tools::PreviewCx {
            session,
            call_id: call_id.into(),
        };
        let cx = |call_id: &str| ::tools::ToolCx {
            session,
            call_id: call_id.into(),
            quiet: false,
            resources: ::tools::SessionResources::durable_arc(d.path()),
            cancel: ::tools::CancellationToken::new(),
            promote_after: None,
        };
        let approved = || {
            Some(::tools::Approved {
                id: ::tools::ApprovalId(uuid::Uuid::now_v7()),
                surface: "owner",
            })
        };

        // Shown, then denied: the call never runs and nothing is claimed.
        let card = tool.approval_preview(args(), preview("denied")).await.unwrap().unwrap();
        let name = card["name"].as_str().unwrap().to_owned();
        assert_eq!(card["url"], format!("https://app.{name}.tun.zip/"));
        assert_eq!(card["label"], "app");
        assert_eq!(*edge.checks.lock(), vec![name], "checked read-only");
        assert!(edge.claims.lock().is_empty(), "nothing claimed for the card");

        // Shown, approved: the call claims exactly that name (then fails: no name keys).
        let card = tool.approval_preview(args(), preview("ok")).await.unwrap().unwrap();
        let shown = card["name"].as_str().unwrap().to_owned();
        let out = ::tools::approval::approved_scope(approved(), tool.call(args(), cx("ok")))
            .await
            .unwrap();
        assert!(out.content.contains("name keys"), "{out:?}");
        assert_eq!(*edge.claims.lock(), vec![shown.clone()], "the shown name, no other");
        assert!(!tool.shown.lock().contains_key(&(session, "ok".into())), "taken once");

        // The shown name was taken meanwhile: refused, no fresh draw.
        tool.approval_preview(args(), preview("taken")).await.unwrap();
        tool.shown
            .lock()
            .get_mut(&(session, "taken".into()))
            .unwrap()
            .preview
            .name = "taken1".into();
        let out = ::tools::approval::approved_scope(approved(), tool.call(args(), cx("taken")))
            .await
            .unwrap();
        assert_eq!(out.content, format!("share_publish: {NAME_TAKEN_MEANWHILE}"));
        assert_eq!(*edge.claims.lock(), vec![shown.clone(), "taken1".into()]);

        // An expired preview, or none at all: refused before any claim.
        tool.approval_preview(args(), preview("old")).await.unwrap();
        tool.shown.lock().get_mut(&(session, "old".into())).unwrap().until =
            std::time::Instant::now();
        for id in ["old", "never"] {
            let out = ::tools::approval::approved_scope(approved(), tool.call(args(), cx(id)))
                .await
                .unwrap();
            assert!(out.content.contains("expired"), "{out:?}");
        }
        assert_eq!(*edge.claims.lock(), vec![shown, "taken1".into()]);
        assert!(e.store.list(platform::time::now_ms()).await.unwrap().is_empty());
    }

    /// FIX6 N4: an unused claim is released only when no share of the family that is not
    /// stopped uses the name (another engine's replicated row included), and never when the
    /// shares cannot be read.
    #[tokio::test]
    async fn release_unused_keeps_names_in_use_or_in_doubt() {
        let edge = Arc::new(NameEdge::default());
        let (_d, e) = engine(8).await;
        e.welcome(&name_edge(&edge), &test_info(), &test_welcome(&[])).await;
        let other = Share {
            engine: "other".into(),
            host: "app.busy.tun.zip".into(),
            ..share("o", 1, ShareState::Live, Access::Public)
        };
        e.store.put(&other).await.unwrap();
        let stopped = Share {
            host: "gone.tun.zip".into(),
            state: ShareState::Stopped,
            stopped_ms: Some(platform::time::now_ms()),
            stopped_by: Some(StoppedBy::User),
            ..share("s", 2, ShareState::Live, Access::Public)
        };
        e.store.put(&stopped).await.unwrap();
        for n in ["busy", "gone", "doubt"] {
            e.fresh.lock().insert(n.into());
        }

        e.release_unused("busy").await;
        e.release_unused("gone").await;
        assert_eq!(*edge.released.lock(), vec!["gone".to_owned()]);
        assert!(e.fresh.lock().contains("busy"));

        sqlx::query("DROP TABLE shares")
            .execute(e.store.pool())
            .await
            .unwrap();
        e.release_unused("doubt").await;
        assert_eq!(*edge.released.lock(), vec!["gone".to_owned()], "released in doubt");
        assert!(e.fresh.lock().contains("doubt"));
    }

    /// FIX6 N2: a store error in the mount check decides nothing: the tick keeps a routed share
    /// routed, and a restore leaves an unmounted one unmounted (and unreported).
    #[tokio::test]
    async fn a_store_error_leaves_mounts_as_they_are() {
        let (_d, e) = engine(8).await;
        put_approved(&e, &share("x", 1, ShareState::Live, Access::Public)).await;
        e.restore().await;
        assert!(e.table.routes("x"));
        put_approved(&e, &share("y", 2, ShareState::Live, Access::Public)).await;

        sqlx::query("DROP TABLE share_approvals")
            .execute(e.store.pool())
            .await
            .unwrap();
        e.expire().await;
        assert!(e.table.routes("x"), "unrouted on a store error");
        e.restore().await;
        assert!(e.table.routes("x"));
        assert!(!e.table.routes("y"), "mounted without a readable approval");
        assert!(e.unmounted().is_empty());
    }

    /// FIX5 R4: the local approval binds the expiry: a replicated edit extending it unbinds the
    /// row, and the next tick takes the routed share out of the table.
    #[tokio::test]
    async fn an_extended_expiry_unbinds_the_local_approval() {
        let (_d, e) = engine(8).await;
        let mut s = share("x", 1, ShareState::Live, Access::Public);
        s.expires_ms = Some(platform::time::now_ms() + 3_600_000);
        put_approved(&e, &s).await;
        e.restore().await;
        assert!(e.table.routes("x"));

        let mut longer = s.clone();
        longer.expires_ms = Some(s.expires_ms.unwrap() + 30 * 86_400_000);
        e.store.put(&longer).await.unwrap();
        assert!(!e.store.locally_approved(&longer).await.unwrap());
        e.expire().await;
        assert!(!e.table.routes("x"));
        assert_eq!(e.unmounted(), vec!["x".to_owned()]);
        e.restore().await;
        assert!(!e.table.routes("x"), "nor does a restore remount it");
    }

    #[test]
    fn name_host_of_strips_the_label() {
        assert_eq!(name_host_of("app.sky.tun.zip", "tun.zip"), "sky.tun.zip");
        assert_eq!(name_host_of("sky.tun.zip", "tun.zip"), "sky.tun.zip");
    }

    /// 210 live bug: a name reserved on the attached link is the publish's name, asked for or
    /// not, and no random name is claimed. The reserve's `hello` brings the link's welcome up
    /// to date; a welcome older than the claim alone is covered by the `share_names` row.
    #[tokio::test]
    async fn a_name_reserved_on_the_live_link_is_held_for_publishes() {
        use axum::extract::Path;
        use axum::routing::{post, put};

        let delegation = Arc::new(crate::edge::SelfHolder {
            mesh_key: share_proto::SecretKey::from_bytes(&[1; 32]),
            engine: share_proto::SecretKey::from_bytes(&[2; 32]).public(),
        });
        let (_d, e) = engine_full(8, None, Some(delegation)).await;
        e.store.set_enabled(true).await.unwrap();
        let info = test_info();
        e.store
            .set_terms_rev(&info.edge_id.to_string(), &info.terms_rev)
            .await
            .unwrap();
        let held = Arc::new(Mutex::new(Vec::<String>::new()));
        let randoms = Arc::new(AtomicU32::new(0));
        let (h1, h2, r) = (held.clone(), held.clone(), randoms.clone());
        let app = axum::Router::new()
            .route(
                share_proto::paths::HELLO,
                post(move || async move {
                    let names: Vec<(String, NameState)> =
                        h1.lock().iter().map(|n| (n.clone(), NameState::Allocated)).collect();
                    let names: Vec<(&str, NameState)> =
                        names.iter().map(|(n, s)| (n.as_str(), *s)).collect();
                    axum::Json(test_welcome(&names))
                }),
            )
            .route(
                share_proto::paths::NAME,
                put(move |Path(n): Path<String>| async move {
                    h2.lock().push(n.clone());
                    axum::Json(share_proto::NameGranted { name: n })
                }),
            )
            .route(
                share_proto::paths::NAMES,
                post(move || async move {
                    r.fetch_add(1, Ordering::SeqCst);
                    axum::http::StatusCode::FORBIDDEN
                }),
            );
        let control = Control::new(crate::edge::tests::FakeConn::new(app));
        e.welcome(&control, &info, &test_welcome(&[])).await;
        crate::names::reserve(&e, "sky", || async { Ok(Consent::test()) })
            .await
            .unwrap();
        let listed = crate::names::held(&e, "sky").await.unwrap();
        assert_eq!(listed.state.as_deref(), Some("allocated"), "the welcome lists the claim");

        let plan = |name: Option<&str>| Plan {
            source: Source::Port(1),
            label: Some("h3renow".into()),
            name: name.map(Into::into),
            path: "/".into(),
            expiry: Expiry::H1,
            access: Access::Public,
        };
        let sky = PublishPreview {
            name: "sky".into(),
            url: "https://h3renow.sky.tun.zip/".into(),
            drawn: false,
        };
        for stale in [false, true] {
            if stale {
                // The welcome of a session attached before the claim (what 2.26.1 kept).
                e.welcome(&control, &info, &test_welcome(&[])).await;
            }
            for want in [Some("sky"), None] {
                assert_eq!(e.publish_preview(&plan(want)).await.unwrap(), sky, "{stale} {want:?}");
                let link = e.live_link().unwrap();
                let (name, claimed) = e.pick_name(&link, want, None).await.unwrap();
                assert_eq!((name.as_str(), claimed), ("sky", false));
            }
        }
        assert_eq!(randoms.load(Ordering::SeqCst), 0, "never a random claim");

        // Another edge's row and a released row are not held here.
        e.store
            .put_name("other", "tun.zip", "abcdef", NameKind::Random)
            .await
            .unwrap();
        let sky_id = format!("{}/tun.zip/sky", info.edge_id);
        e.store.release_name(&sky_id, 1).await.unwrap();
        for want in ["abcdef", "sky"] {
            let err = e.publish_preview(&plan(Some(want))).await.unwrap_err();
            assert_eq!(err.to_string(), format!("the mesh holds no name {want} at tun.zip"));
        }
    }
}
