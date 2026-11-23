//! The hosting engine's share service (210 §5.1, §5.9, §7.4, §7.6): publish, stop, Stop all,
//! the routing table and certificates, and the [`Attachment`] side of the edge state machine.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use share_proto::{ErrorCode, Event, Hello, Info, KillReason, NameKind, NameState, Welcome, names};
use tokio::sync::watch;

use crate::config::{EdgeChoice, NO_SHARING_EDGE, ShareConfig};
use crate::edge::{Attachment, Control, CsrAnswer, DelegationSource, EdgeDialer, Stream};
use crate::gate::{self, Gate, Operator};
use crate::keys::{self, NameKeyStore};
use crate::model::{
    Access, ApprovedBy, CreatedBy, EdgeState, Share, ShareChanged, ShareState, ShareStatus, Source,
    StoppedBy,
};
use crate::proxy::Proxy;
use crate::routes::{Lookup, Mount, Table};
use crate::sources::{self, OwnPorts, PortGuard, ProcessCheck};
use crate::store::Store;
use crate::tools::Plan;
use crate::visitor::{self, Certs};

/// "Public sharing is off" (§7.1).
pub const SWITCH_OFF: &str = "Public sharing is off in Settings › Sharing";
/// How long a publish waits for the edge.
pub const ATTACH_WAIT: Duration = Duration::from_secs(30);

/// Who asked for a publish.
#[derive(Clone, Debug)]
pub enum CallOrigin {
    /// A tool call.
    Call {
        /// Session.
        session: String,
        /// Call id.
        call_id: String,
        /// The approval row the call passed; `None` outside the registry (tests).
        approved: Option<ApprovedBy>,
    },
    /// Settings or the card.
    Settings,
}

/// A publish result; `password` only for `access: password`, for the owner's surfaces.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Published {
    /// The record.
    pub share: Share,
    /// The generated password.
    pub password: Option<String>,
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
        Ok(Arc::new(Self {
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
        }))
    }

    /// Start the edge state machine for the configured edge (one per edge, §5.9) and the expiry
    /// ticker. Without a dialer, a delegation source or a ticket to dial (edge `none`, or the
    /// default while its tickets are placeholders) no client runs and publishing says so.
    pub async fn start(self: &Arc<Self>) -> anyhow::Result<()> {
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

    async fn emit(&self, share: &Share) {
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

    async fn wait_attached(&self) -> anyhow::Result<Arc<Link>> {
        self.need.send_replace(true);
        let mut rx = self.attached.subscribe();
        let _ = tokio::time::timeout(ATTACH_WAIT, rx.wait_for(|a| *a)).await;
        self.link
            .lock()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Can't reach the sharing edge; see Settings › Sharing"))
    }

    async fn push_routes(&self) {
        let control = self.link.lock().as_ref().map(|l| l.control.clone());
        if let Some(c) = control
            && let Err(e) = c.put_routes(self.table.hosts()).await
        {
            tracing::warn!("share: PUT /v1/routes: {e}");
        }
    }

    /// Publish (§7.6). The owner's approval has already passed in the registry or RPC layer.
    pub async fn publish(&self, plan: Plan, origin: CallOrigin) -> anyhow::Result<Published> {
        anyhow::ensure!(self.store.enabled().await?, SWITCH_OFF);
        anyhow::ensure!(
            !self.store.edge().await?.tickets().is_empty(),
            NO_SHARING_EDGE
        );
        match &plan.source {
            Source::Port(p) => {
                drop(self.ports.connect(*p).await?);
            }
            Source::Dir(d) => {
                sources::check_root(d, &self.data_dir, std::env::home_dir().as_deref())?;
            }
        }
        let live = self.mine().await?.len();
        anyhow::ensure!(
            (live as u32) < self.cfg.max_live,
            "{live} shares are live on this machine (share.max_live): stop one first"
        );
        let link = self.wait_attached().await?;
        let domain = link.info.domain.clone();
        let edge = link.info.edge_id.to_string();
        let name = self.pick_name(&link, plan.name.as_deref()).await?;
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
        let password = match plan.access {
            Access::Password => {
                let pw = gate::generate_password();
                let phc = tokio::task::spawn_blocking({
                    let pw = pw.clone();
                    move || gate::hash_password(&pw)
                })
                .await??;
                self.store.put_secret(&id, &phc).await?;
                self.gate.set_password(&id, Some(phc));
                Some(pw)
            }
            Access::Public => None,
        };
        let have_cert = self.ensure_cert(&link, &name).await?;
        let now = platform::time::now_ms();
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
            expires_ms: have_cert.then(|| now + plan.expiry.duration().as_millis() as u64),
            approved_by: match &origin {
                CallOrigin::Call {
                    approved: Some(a), ..
                } => a.clone(),
                // TODO(210-M1): the RPC path (Settings, card) resolves its own owner-only row.
                _ => ApprovedBy {
                    approval_id: String::new(),
                    surface: String::new(),
                },
            },
            created_by: match origin {
                CallOrigin::Call {
                    session, call_id, ..
                } => CreatedBy::Call { session, call_id },
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
        self.store.put(&share).await?;
        if have_cert {
            self.table.insert(Mount::of(&share));
            self.push_routes().await;
        }
        self.emit(&share).await;
        Ok(Published { share, password })
    }

    /// The name for a publish: the requested held one, else the primary (newest reserved, else
    /// random), else a fresh random allocation after the terms (§5.2, §7.6).
    async fn pick_name(&self, link: &Link, want: Option<&str>) -> anyhow::Result<String> {
        let held: Vec<_> = link
            .welcome
            .names
            .iter()
            .filter(|n| !matches!(n.state, NameState::Released | NameState::Banned))
            .collect();
        if let Some(w) = want {
            return held
                .iter()
                .find(|n| n.name == w)
                .map(|n| n.name.clone())
                .ok_or_else(|| {
                    anyhow::anyhow!("the mesh holds no name {w} at {}", link.info.domain)
                });
        }
        if let Some(n) = held
            .iter()
            .find(|n| n.kind == NameKind::Reserved)
            .or_else(|| held.first())
        {
            return Ok(n.name.clone());
        }
        // First publish: terms, then a client-drawn random name.
        let edge = link.info.edge_id.to_string();
        let accepted = self.store.terms_rev(&edge).await?;
        if accepted.as_deref() != Some(link.info.terms_rev.as_str()) {
            anyhow::bail!(
                "accept the terms of {} first ({}, revision {}): Settings › Sharing or the first-run card",
                link.info.domain,
                link.info.terms_url,
                link.info.terms_rev
            );
        }
        if !link.welcome.terms_ok {
            self.rehello(link, accepted).await?;
        }
        for _ in 0..3 {
            let name = names::random_name(random_bytes());
            match link.control.allocate(&name).await {
                Ok(g) => return Ok(g.name),
                Err(e) if e.code() == Some(ErrorCode::NameTaken) => continue,
                Err(e) => return Err(e.into()),
            }
        }
        anyhow::bail!("could not allocate a name at {}", link.info.domain)
    }

    async fn rehello(&self, link: &Link, terms_rev: Option<String>) -> anyhow::Result<()> {
        let Some(d) = &self.hooks.delegation else {
            anyhow::bail!("no delegation source");
        };
        let delegation = d
            .delegate(link.info.edge_id, &link.info.domain, link.info.now_ms)
            .await?;
        link.control
            .hello(&Hello {
                v: share_proto::PROTOCOL_MAJOR,
                delegation,
                invite: None,
                client: share_proto::version::client_agent(platform::version::get()),
                terms_rev,
            })
            .await?;
        Ok(())
    }

    /// Have `<name>.D`'s pair installed: fetch the chain, or post the CSR. `false` = pending.
    async fn ensure_cert(&self, link: &Link, name: &str) -> anyhow::Result<bool> {
        let domain = &link.info.domain;
        let name_host = names::host(name, None, domain);
        if self.certs.lookup(&name_host).is_some() {
            return Ok(true);
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
        let chain = if valid {
            link.control.cert(name).await.ok()
        } else {
            None
        };
        let chain = match chain {
            Some(c) => c,
            None => match link.control.csr(name, key.csr_der.clone(), false).await? {
                CsrAnswer::Issued(c) => c,
                CsrAnswer::Pending(_) => return Ok(false),
            },
        };
        self.certs
            .install(&name_host, &chain.chain_pem, &key.key_pem)?;
        Ok(true)
    }

    /// Stop one of this engine's shares.
    // TODO(210-M2): a share of another engine is forwarded with `rpc_forward::to_engine`.
    pub async fn stop(&self, id: &str, by: StoppedBy) -> anyhow::Result<Share> {
        let mut s = self
            .store
            .get(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("no share {id}"))?;
        anyhow::ensure!(
            s.engine == self.engine_id,
            "share {id} runs on another machine (stopping it from here is M2)"
        );
        if s.state != ShareState::Stopped {
            s.state = ShareState::Stopped;
            s.stopped_ms = Some(platform::time::now_ms());
            s.stopped_by = Some(by);
            self.store.put(&s).await?;
            self.gate.set_password(id, None);
            if self.table.remove(id) {
                self.push_routes().await;
            }
            self.emit(&s).await;
        }
        self.refresh_need().await;
        Ok(s)
    }

    /// Stop all (§7.4): `POST /v1/mesh/stop` on the current connection, then every local share.
    pub async fn stop_all(&self) -> anyhow::Result<usize> {
        let link = self.link.lock().clone();
        if let Some(l) = link
            && let Err(e) = l.control.stop_all().await
        {
            tracing::warn!("share: POST /v1/mesh/stop: {e}");
        }
        // TODO(210-M2): attach first when detached; forward `share.stop all` to reachable
        // engines; hand Stop all to the lease holder when this engine cannot reach the edge.
        self.stop_local(StoppedBy::User).await
    }

    async fn stop_local(&self, by: StoppedBy) -> anyhow::Result<usize> {
        let mine = self.mine().await?;
        for s in &mine {
            self.stop(&s.id, by).await?;
        }
        Ok(mine.len())
    }

    async fn expire(&self) {
        let now = platform::time::now_ms();
        let Ok(mine) = self.mine().await else { return };
        for s in mine {
            if s.expires_ms.is_some_and(|e| e <= now) {
                let _ = self.stop(&s.id, StoppedBy::Expiry).await;
            }
        }
    }

    /// Every share of the mesh, newest first (30-day history included).
    pub async fn list(&self) -> anyhow::Result<Vec<Share>> {
        self.store.list(platform::time::now_ms()).await
    }

    /// The master switch; off stops this machine's shares (`switch_off`).
    pub async fn set_enabled(&self, on: bool) -> anyhow::Result<()> {
        self.store.set_enabled(on).await?;
        if !on {
            self.stop_local(StoppedBy::SwitchOff).await?;
        }
        Ok(())
    }

    /// Change the edge; takes effect at the next start.
    // TODO(210-M1): restart the state machine live on an edge change.
    pub async fn set_edge(&self, edge: &EdgeChoice) -> anyhow::Result<()> {
        self.store.set_edge(edge).await
    }

    /// Record accepted terms for `edge`.
    pub async fn accept_terms(&self, edge: &str, rev: &str) -> anyhow::Result<()> {
        self.store.set_terms_rev(edge, rev).await?;
        self.terms.send_replace(Some(rev.to_owned()));
        Ok(())
    }

    /// `GET /v1/names/{name}` through the current connection.
    pub async fn check_name(&self, name: &str) -> anyhow::Result<share_proto::NameAvailability> {
        names::check_reserved::<&str>(name, &[]).map_err(|e| anyhow::anyhow!(e.reason()))?;
        let link = self.wait_attached().await?;
        Ok(link.control.check_name(name).await?)
    }

    async fn after_cert(&self, name: &str) {
        let link = self.link.lock().clone();
        let Some(link) = link else { return };
        match self.ensure_cert(&link, name).await {
            Ok(true) => {}
            Ok(false) => return,
            Err(e) => {
                tracing::warn!("share: certificate for {name}: {e:#}");
                return;
            }
        }
        let name_host = names::host(name, None, &link.info.domain);
        let now = platform::time::now_ms();
        let Ok(mine) = self.mine().await else { return };
        for mut s in mine {
            let under = s.host == name_host || s.host.ends_with(&format!(".{name_host}"));
            if under && s.state == ShareState::Pending {
                s.state = ShareState::Live;
                s.live_ms = Some(now);
                s.expires_ms = Some(now + self.cfg.default_expiry.duration().as_millis() as u64);
                // TODO(210-M1): keep the expiry the owner approved instead of the default.
                if self.store.put(&s).await.is_ok() {
                    self.table.insert(Mount::of(&s));
                    self.emit(&s).await;
                }
            }
        }
        self.push_routes().await;
    }
}

#[async_trait::async_trait]
impl Attachment for Engine {
    fn hosts(&self) -> Vec<String> {
        self.table.hosts()
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
        if let Some(stop_all) = welcome.stop_all_ms
            && let Ok(mine) = self.mine().await
        {
            for s in mine
                .iter()
                .filter(|s| s.live_ms.is_some_and(|l| l < stop_all))
            {
                let _ = self.stop(&s.id, StoppedBy::User).await;
            }
        }
        for n in welcome.names.iter().filter(|n| n.state == NameState::Valid) {
            self.after_cert(&n.name).await;
        }
        // Restore routes of shares that were live before a reconnect.
        if let Ok(mine) = self.mine().await {
            for mut s in mine.into_iter().filter(|s| s.state == ShareState::Offline) {
                s.state = ShareState::Live;
                if self.store.put(&s).await.is_ok() {
                    self.table.insert(Mount::of(&s));
                    self.emit(&s).await;
                }
            }
        }
        self.attached.send_replace(true);
    }

    async fn event(&self, _control: &Control, event: Event) {
        match event {
            Event::Cert(c) => self.after_cert(&c.name).await,
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
            // TODO(210-M3): `queue` events feed the slot-queue UI; `stats` feed `share.traffic`.
            Event::Queue(_) | Event::Stats(_) | Event::Routes(_) | Event::Drain(_) => {}
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
        let _ = self.stop_local(StoppedBy::User).await;
        // TODO(210-M2): delete the stale `mesh/share_key` and `share/*` vault rows.
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
