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
use crate::edge::{Attachment, Control, CsrAnswer, DelegationSource, EdgeDialer, Stream};
use crate::gate::{self, Gate, Operator};
use crate::keys::{self, NameKeyStore};
use crate::model::{
    Access, ApprovedBy, CreatedBy, EdgeState, Share, ShareChanged, ShareState, ShareStatus, Source,
    StoppedBy,
};
use crate::proxy::Proxy;
use crate::routes::{Lookup, Mount, Table};
use crate::sources::{self, DirPin, OwnPorts, PortGuard, ProcessCheck};
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
    },
    /// Settings or the card.
    Settings,
}

/// Proof that the owner, in person, approved an act through a human-only row (§7.2): the only
/// way to publish, switch sharing on, change the edge or accept terms. Built from the tool
/// registry's scope ([`Consent::of_call`]) or by raising the row ([`Consent::ask`]); an empty or
/// non-owner row is refused, so the ask cannot be skipped.
#[derive(Clone, Debug)]
pub struct Consent(ApprovedBy);

impl Consent {
    fn from_approved(a: ::tools::Approved) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !a.id.0.is_nil() && a.surface == "owner",
            "this needs the owner's own approval"
        );
        Ok(Self(ApprovedBy {
            approval_id: a.id.0.to_string(),
            surface: a.surface.to_owned(),
        }))
    }

    /// The human-only row the running tool call passed; `None` outside a gated body.
    pub fn of_call() -> Option<Self> {
        ::tools::approved().and_then(|a| Self::from_approved(a).ok())
    }

    /// Raise a human-only row for `what` (an RPC acting for the owner) and wait for `Allow`.
    pub async fn ask(what: &str, args: serde_json::Value) -> anyhow::Result<Self> {
        let a = ::tools::ask_human_only(::tools::approval::RequestApproval {
            session: runtime::SessionId(uuid::Uuid::nil()),
            tool: what.to_owned(),
            args,
            risk: ::tools::Risk::Destructive,
            call_id: None,
        })
        .await?;
        Self::from_approved(a)
    }

    /// The row, for `approved_by`.
    pub fn approved_by(&self) -> &ApprovedBy {
        &self.0
    }

    #[cfg(test)]
    pub(crate) fn test() -> Self {
        Self::from_approved(::tools::Approved {
            id: ::tools::ApprovalId(uuid::Uuid::now_v7()),
            surface: "owner",
        })
        .unwrap()
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
    /// Publishes admitted but not yet stored; read with the store under `admit` (m6).
    reserved: AtomicU32,
    admit: tokio::sync::Mutex<()>,
    /// `<name>.D` → the welcome's `(not_after, spki)` of the installed chain (B2).
    installed: Mutex<HashMap<String, NameInfo>>,
    /// Generated passwords of this run, for the owner's `share_password` (M9); never logged.
    passwords: Mutex<HashMap<String, String>>,
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
            reserved: AtomicU32::new(0),
            admit: tokio::sync::Mutex::new(()),
            installed: Mutex::new(HashMap::new()),
            passwords: Mutex::new(HashMap::new()),
        }))
    }

    /// Start the edge state machine for the configured edge (one per edge, §5.9) and the expiry
    /// ticker. Without a dialer, a delegation source or a ticket to dial (edge `none`, or the
    /// default while its tickets are placeholders) no client runs and publishing says so.
    pub async fn start(self: &Arc<Self>) -> anyhow::Result<()> {
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
    /// opened and is stopped.
    async fn restore(&self) {
        let Ok(mine) = self.mine().await else { return };
        for s in mine
            .into_iter()
            .filter(|s| matches!(s.state, ShareState::Live | ShareState::Offline))
        {
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
        anyhow::ensure!(
            !consent.0.approval_id.is_empty(),
            "this needs the owner's own approval"
        );
        anyhow::ensure!(self.store.enabled().await?, SWITCH_OFF);
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
        let r = self.publish_admitted(plan, origin, consent, pin).await;
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
    ) -> anyhow::Result<Published> {
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
        let have_cert = self.ensure_cert(&link, &name, false).await?;
        let now = platform::time::now_ms();
        let expiry_ms = plan.expiry.duration().as_millis() as u64;
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
            self.store.put(&share).await
        }
        .await;
        if let Err(e) = stored {
            let _ = self.store.delete_secret(&share.id).await;
            let _ = self.store.delete_request(&share.id).await;
            let _ = self.store.delete_pin(&share.id).await;
            return Err(e);
        }
        if let Some((pw, phc)) = &password {
            self.gate.set_password(&share.id, Some(phc.clone()));
            self.passwords.lock().insert(share.id.clone(), pw.clone());
        }
        if have_cert {
            self.table.insert(Mount::of(&share, pin));
            self.push_routes().await;
        }
        self.emit(&share).await;
        Ok(Published {
            share,
            password: password.map(|(pw, _)| pw),
        })
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
    /// `renew` (a `cert` event) fetches and installs over the current chain (B2); without it a
    /// chain is fetched again when the welcome's `not_after`/SPKI differ from the installed one.
    async fn ensure_cert(&self, link: &Link, name: &str, renew: bool) -> anyhow::Result<bool> {
        let domain = &link.info.domain;
        let name_host = names::host(name, None, domain);
        let seen = link.welcome.names.iter().find(|n| n.name == name);
        let current = self.installed.lock().get(&name_host).is_some_and(|i| {
            seen.is_none_or(|n| n.not_after == i.not_after && n.spki == i.spki)
        });
        if !renew && current && self.certs.lookup(&name_host).is_some() {
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
        let chain = if valid || renew {
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
        if let Some(n) = seen {
            self.installed.lock().insert(name_host, n.clone());
        }
        Ok(true)
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
        let stopped = self.stop_local(StoppedBy::User, Some(now)).await;
        if fan_out {
            self.stop_all_elsewhere().await;
        }
        // TODO(210-M2): attach first when detached; hand Stop all to the lease holder when this
        // engine cannot reach the edge.
        let edge_pending = match self.send_stop_all().await {
            Ok(()) => None,
            Err(e) => {
                tracing::warn!("share: POST /v1/mesh/stop: {e:#}");
                Some(format!("{e:#}"))
            }
        };
        let stopped = stopped?;
        Ok(StopAllOutcome {
            stopped,
            edge_pending,
        })
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

    /// Switch sharing on, under the owner's [`Consent`] (§7.2).
    pub async fn enable(&self, _consent: &Consent) -> anyhow::Result<()> {
        self.store.set_enabled(true).await
    }

    /// Switch sharing off and stop this machine's shares (`switch_off`). Never gated.
    pub async fn disable(&self) -> anyhow::Result<()> {
        self.store.set_enabled(false).await?;
        self.stop_local(StoppedBy::SwitchOff, None).await?;
        Ok(())
    }

    /// Change the edge under the owner's [`Consent`]; takes effect at the next start.
    // TODO(210-M1): restart the state machine live on an edge change.
    pub async fn set_edge(&self, edge: &EdgeChoice, _consent: &Consent) -> anyhow::Result<()> {
        self.store.set_edge(edge).await
    }

    /// Record accepted terms for `edge` under the owner's [`Consent`].
    pub async fn accept_terms(&self, edge: &str, rev: &str, _consent: &Consent) -> anyhow::Result<()> {
        self.store.set_terms_rev(edge, rev).await?;
        self.terms.send_replace(Some(rev.to_owned()));
        Ok(())
    }

    /// `GET /v1/names/{name}` through the current connection.
    /// The attached edge's control client and info (attaching first, §5.2 "a name operation").
    pub(crate) async fn attached_control(&self) -> anyhow::Result<(Control, Info)> {
        let link = self.wait_attached().await?;
        Ok((link.control.clone(), link.info.clone()))
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

    pub async fn check_name(&self, name: &str) -> anyhow::Result<share_proto::NameAvailability> {
        names::check_reserved::<&str>(name, &[]).map_err(|e| anyhow::anyhow!(e.reason()))?;
        let link = self.wait_attached().await?;
        Ok(link.control.check_name(name).await?)
    }

    async fn after_cert(&self, name: &str, renew: bool) {
        let link = self.link.lock().clone();
        let Some(link) = link else { return };
        match self.ensure_cert(&link, name, renew).await {
            Ok(true) => {}
            Ok(false) => return,
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
            let expiry = match self.store.request_expiry(&s.id).await {
                Ok(Some(ms)) => ms,
                _ => self.cfg.default_expiry.duration().as_millis() as u64,
            };
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
            if self.store.put(&s).await.is_ok() {
                let _ = self.store.delete_request(&s.id).await;
                self.table.insert(m);
                self.emit(&s).await;
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
    use crate::sources::{NoProcessCheck, OwnListeners};

    async fn engine(max_live: u32) -> (tempfile::TempDir, Arc<Engine>) {
        engine_with(max_live, None).await
    }

    async fn engine_with(
        max_live: u32,
        name_keys: Option<Arc<dyn NameKeyStore>>,
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
            delegation: None,
            name_keys,
            own_ports: Arc::new(OwnListeners),
            process: Arc::new(NoProcessCheck),
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

        *chain.lock() = pem2;
        e.event(
            &control,
            Event::Cert(share_proto::events::CertEvent { name: "sky".into() }),
        )
        .await;
        assert_eq!(leaf_of(&e), Some(der2), "the renewed chain replaced the old one");
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
        assert!(Consent::from_approved(::tools::Approved { id, surface: "human" }).is_err());
        let nil = ::tools::ApprovalId(uuid::Uuid::nil());
        assert!(
            Consent::from_approved(::tools::Approved {
                id: nil,
                surface: "owner"
            })
            .is_err()
        );
        assert!(Consent::of_call().is_none(), "no row outside a gated call");
        let c = Consent::from_approved(::tools::Approved { id, surface: "owner" }).unwrap();
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
        e.store
            .put(&share("a", 1, ShareState::Live, Access::Password))
            .await
            .unwrap();
        e.store.put_secret("a", "$argon2id$x").await.unwrap();
        e.store
            .put(&share("b", 2, ShareState::Live, Access::Password))
            .await
            .unwrap();
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
            e.store.put(&dir_share(id, root)).await.unwrap();
            // `c` was pinned against another data dir (the platform default, say).
            let pin = sources::pin_root(root.to_str().unwrap(), &elsewhere, None).unwrap();
            e.store.put_pin(id, &pin).await.unwrap();
        }
        e.store.put(&dir_share("d", &kept)).await.unwrap();
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

    #[test]
    fn name_host_of_strips_the_label() {
        assert_eq!(name_host_of("app.sky.tun.zip", "tun.zip"), "sky.tun.zip");
        assert_eq!(name_host_of("sky.tun.zip", "tun.zip"), "sky.tun.zip");
    }
}
