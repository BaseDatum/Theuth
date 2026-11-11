//! A2A plugin (design doc 50 §4, §6, §10): the network boundary. The official
//! SDK speaks the protocol; `agent` owns sessions, intake and tasks; this
//! crate owns the optional loopback listener, the outbound clients, the
//! configured peer/export bindings and `peers.yaml`.
//!
//! Public surface consumed by other plugins:
//! - commands on the [`A2A_ACTOR`] actor ([`A2aActor`]): [`GetStatus`], [`Reload`]
//! - DTOs: [`A2aStatus`], [`A2aPeerStatus`], [`A2aExportStatus`]
//! - event: [`A2aChanged`]
//!
//! The actor always registers and finishes initialisation in the background
//! once `agent` and `secrets` are up (plugin order is not assumed). 183 W-26:
//! contacts (`peers.yaml` entries with `endpoint`, reached over `theuth/a2a/1`)
//! need it whatever `a2a.enabled` says; `a2a.enabled: false` (the default)
//! keeps the loopback listener closed and HTTP peers dormant (no card fetch,
//! no credential load). A malformed `a2a:` section registers the actor with
//! the error and everything closed.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;

use agent::communication::SharedPolicy;
use agent::communication::network::{
    CommitPeerConfig, NetworkStatus, PeerPolicy, PeerRoute, PeerSender, ValidateExport,
};
use agent::{AGENT_ACTOR, Agents, CommunicationChanged};
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use parking_lot::Mutex;
use runtime::{Runtime, SessionId};
use secrets::Vault;
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub mod client;
pub mod config;
pub mod contacts;
pub mod server;

use client::{ConnectError, PeerClient, Sender};
use config::{A2aConfig, Listener, PEERS_FILE, PeersFile, Staged};
use server::{Live, Shared};

/// Registry name of the [`A2aActor`].
pub const A2A_ACTOR: &str = "a2a";

/// 183 §7.8: how often `peers.yaml` is checked for a hand edit.
pub const HOT_RELOAD: std::time::Duration = std::time::Duration::from_secs(2);

/// Network status for `/a2a` and settings (§10). Never carries a credential.
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct A2aStatus {
    /// `a2a.enabled`.
    pub enabled: bool,
    /// The loopback listener is bound (needs global communication on).
    pub listening: bool,
    /// Bound address while listening.
    pub address: Option<String>,
    /// Configured HTTPS base, when any.
    pub public_url: Option<String>,
    /// Configured peers.
    pub peers: Vec<A2aPeerStatus>,
    /// Configured exports.
    pub exports: Vec<A2aExportStatus>,
    /// Config/listener/reload problem; `None` when healthy. A failed reload
    /// keeps the previous configuration live and reports why here.
    pub error: Option<String>,
}

impl A2aStatus {
    /// The answer when `a2a.enabled` is false and no actor exists.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            listening: false,
            address: None,
            public_url: None,
            peers: Vec::new(),
            exports: Vec::new(),
            error: None,
        }
    }
}

/// One configured peer as the owner sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct A2aPeerStatus {
    /// Owner alias.
    pub name: String,
    /// Store binding identity, once committed.
    pub binding: Option<Uuid>,
    /// We can send to it.
    pub outbound: bool,
    /// It can call our exports.
    pub inbound: bool,
    /// Local roots allowed to send.
    pub sessions: Vec<SessionId>,
    /// Exports it may call.
    pub exports: Vec<String>,
    /// Remote card name once resolved.
    pub card: Option<String>,
    /// Card/transport problem with this peer.
    pub error: Option<String>,
}

/// One export as the owner sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct A2aExportStatus {
    /// Export name.
    pub name: String,
    /// Published session.
    pub session: SessionId,
    /// Advertised JSON-RPC URL.
    pub url: String,
    /// The one peer granted this export.
    pub peer: Option<String>,
}

/// `ask` → [`A2aStatus`].
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct GetStatus;

/// `ask` → `Result<A2aStatus, String>`: validate and atomically replace the
/// peer/export configuration (§6.1). `Err` installs nothing.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Reload;

/// Event: network configuration or listener state changed (§10). No payload:
/// reload [`GetStatus`].
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub struct A2aChanged {}

/// The plugin.
pub struct A2aPlugin;

/// The [`A2A_ACTOR`] actor.
#[derive(kameo::Actor)]
pub struct A2aActor {
    ctx: Arc<Ctx>,
}

struct Ctx {
    rt: Runtime,
    enabled: bool,
    /// Validated listener, or why the config is unusable.
    listener: Result<Listener, String>,
    public_url: Option<String>,
    shared: Shared,
    subscribers: Arc<Semaphore>,
    /// One reload at a time; the mailbox is not held while cards are fetched.
    reload_lock: tokio::sync::Mutex<()>,
    /// One listener transition at a time; the mailbox is not held while
    /// binding or reporting to the agent.
    listener_lock: tokio::sync::Mutex<()>,
    /// 183 §7.8: one `peers.yaml` rewrite (and its reload) at a time.
    edit_lock: tokio::sync::Mutex<()>,
    /// 183 §9.3: `a2a.contact_cap`, `.contact_window_secs`, `.mdns_types`.
    cfg: A2aConfig,
    state: Mutex<State>,
    /// 183 §7.2: the mesh endpoint `theuth/a2a/1` rides on, once bound.
    net: std::sync::OnceLock<Arc<mesh::net::Net>>,
    /// 183 §7.1: the Add contact window's advert and browse.
    mdns: Arc<dyn contacts::discovery::Mdns>,
    /// 183 §7.4: the window and the ceremonies.
    contacts: Mutex<contacts::Contacts>,
    /// 183 F1: each contact's last fetched offers, by endpoint id.
    offers: Mutex<std::collections::HashMap<String, contacts::offers::Cached>>,
}

#[derive(Default)]
struct State {
    agents: Option<ActorRef<Agents>>,
    vault: Option<Arc<Vault>>,
    policy: Option<SharedPolicy>,
    error: Option<String>,
    listening: Option<SocketAddr>,
    serve: Option<CancellationToken>,
    /// `listening` as last accepted by the agent. Every report it accepts
    /// comes back as a [`CommunicationChanged`], so re-sending an unchanged
    /// bit would feed the two actors to each other forever.
    reported: Option<bool>,
    /// Registered outbound clients by alias.
    clients: BTreeMap<String, Arc<PeerClient>>,
    /// Grant epoch per alias (§6.1): the route token every admission under
    /// that peer's current credentials/exports/roots is bound to. A reload
    /// that changes any of them mints a new one and cancels the old after the
    /// swap, ending streams, in-flight requests and tasks of the old grant.
    epochs: BTreeMap<String, CancellationToken>,
    /// Per-peer transport problems from the last connect attempt.
    peer_errors: BTreeMap<String, String>,
    /// 183 §7.8: `peers.yaml` as the last reload read or the last rewrite left it; the hot
    /// reload compares against it.
    stamp: Option<(std::time::SystemTime, u64)>,
    /// The HTTP peers of the last good reload.
    http_good: BTreeMap<String, config::PeerEntry>,
    /// Last `peers.yaml` rewrite per contact for its remembered addresses (debounce).
    addr_written: std::collections::HashMap<String, std::time::Instant>,
    /// The last address-write failure logged (log each distinct one once).
    addr_error: Option<String>,
    /// The mode warning was logged.
    mode_warned: bool,
    /// 190 (D10): this process tried the one-time notice.
    notice_tried: bool,
}

/// 190 (D10): unless the marker `a2a/.190-full-notice` exists beside `peers.yaml`, log one
/// INFO line per grant of `file` and write the marker (creating `a2a/`). A marker that cannot
/// be written is a warning, never an error. Returns the lines logged.
fn full_notice_once(data_dir: &std::path::Path, file: &PeersFile) -> Vec<String> {
    let marker = data_dir.join(config::FULL_NOTICE);
    if marker.exists() {
        return Vec::new();
    }
    let lines = config::full_notice(file);
    for line in &lines {
        tracing::info!("{line}");
    }
    let wrote = marker
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&marker, b""));
    if let Err(e) = wrote {
        tracing::warn!("a2a: {}: {e}", marker.display());
    }
    lines
}

impl Ctx {
    async fn status(&self) -> A2aStatus {
        let live = self.shared.read().clone();
        let (listening, error, clients, peer_errors) = {
            let s = self.state.lock();
            (
                s.listening,
                s.error.clone(),
                s.clients.clone(),
                s.peer_errors.clone(),
            )
        };
        let mut peers = Vec::with_capacity(live.staged.peers.len());
        for p in live.staged.peers.values() {
            let card = match clients.get(&p.name) {
                Some(c) => c.card_name().await,
                None => None,
            };
            peers.push(A2aPeerStatus {
                name: p.name.clone(),
                binding: live.bindings.get(&p.name).copied(),
                outbound: p.card.is_some(),
                inbound: p.inbound.is_some(),
                sessions: p.sessions.clone(),
                exports: p.exports.clone(),
                card,
                error: peer_errors.get(&p.name).cloned(),
            });
        }
        let base = self
            .listener
            .as_ref()
            .ok()
            .map(|l| l.base.as_str().trim_end_matches('/').to_owned());
        let exports = live
            .staged
            .exports
            .values()
            .map(|e| A2aExportStatus {
                name: e.name.clone(),
                session: e.session,
                url: format!("{}/agents/{}/rpc", base.as_deref().unwrap_or(""), e.name),
                peer: e.peer.clone(),
            })
            .collect();
        A2aStatus {
            enabled: self.enabled,
            listening: listening.is_some(),
            address: listening.map(|a| a.to_string()),
            public_url: self.public_url.clone(),
            peers,
            exports,
            error,
        }
    }

    fn agents(&self) -> Result<ActorRef<Agents>, String> {
        self.state
            .lock()
            .agents
            .clone()
            .ok_or_else(|| "agent not ready".to_owned())
    }

    /// Ask the agent, flattening actor failures into the same `Err(String)`.
    async fn ask<M, R>(&self, agents: &ActorRef<Agents>, msg: M) -> Result<R, String>
    where
        Agents: Message<M, Reply = Result<R, String>>,
        M: Send + 'static,
        R: Send + 'static,
    {
        match agents.ask(msg).await {
            Ok(r) => Ok(r),
            Err(kameo::error::SendError::HandlerError(e)) => Err(e),
            Err(e) => Err(format!("agent unavailable: {e}")),
        }
    }

    /// Read, validate and atomically install `peers.yaml` (§6.1). On `Err`
    /// the previous configuration stays live and the message lands in status.
    async fn reload(&self) -> Result<(), String> {
        self.reload_as(false).await
    }

    /// [`Self::reload`]; `keep_http` puts the last good HTTP peers where the file's are.
    async fn reload_as(&self, keep_http: bool) -> Result<(), String> {
        let _serial = self.reload_lock.lock().await;
        let agents = self.agents()?;
        let listener = self.listener.clone()?;
        let (vault, policy) = {
            let s = self.state.lock();
            (s.vault.clone(), s.policy.clone())
        };
        let policy = policy.ok_or_else(|| "agent policy not ready".to_owned())?;
        let path = self.rt.data_dir.join(PEERS_FILE);
        // The stamp comes first: a write landing between it and the read is read next tick.
        self.state.lock().stamp = contacts::store::stamp(&path);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        if let Some(mode) = contacts::store::loose_mode(&path)
            && !std::mem::replace(&mut self.state.lock().mode_warned, true)
        {
            tracing::warn!(
                "a2a: {} has mode {mode:o}; it names vault secrets, use 600",
                path.display()
            );
        }
        let (head, mut file) =
            contacts::store::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        // 183 W-41: a chat export whose session was deleted is dropped from the file.
        let mut gone = Vec::new();
        for (name, e) in &file.exports {
            if let Some(session) = e.session
                && let Err(err) = self.ask(&agents, ValidateExport { session }).await
                && err.starts_with(agent::communication::network::UNKNOWN_SESSION)
            {
                gone.push(name.clone());
            }
        }
        if !gone.is_empty() {
            for name in &gone {
                contacts::drop_export(&mut file, name);
            }
            tracing::info!(exports = ?gone, "a2a: exported chats deleted; exports removed");
            let text = contacts::store::render(&head, &file)?;
            contacts::store::write(&path, &text).map_err(|e| format!("{}: {e}", path.display()))?;
            self.state.lock().stamp = contacts::store::stamp(&path);
        }
        // 183 Z3-R2: after a failed full reload a contact edit retries with the last good
        // HTTP peers, so a broken HTTP entry never blocks it.
        if keep_http {
            file.peers.retain(|_, p| p.is_contact());
            for (k, p) in self.state.lock().http_good.clone() {
                file.peers.entry(k).or_insert(p);
            }
        }
        // 183 W-26: without `a2a.enabled` only contacts are live; HTTP peers stay dormant.
        if !self.enabled {
            file.peers.retain(|_, p| p.is_contact());
        }
        let console = std::fs::read_to_string(self.rt.data_dir.join("token"))
            .ok()
            .map(|t| t.trim().to_owned());
        let resolve = |name: &str| vault.as_ref().and_then(|v| v.get(name, 0));
        let staged = config::stage(&file, &resolve, console.as_deref(), self.cfg.contact_cap)
            .map_err(|e| e.join("; "))?;

        // Sessions are the agent's to validate (§6.3: native, existing).
        let mut errors = Vec::new();
        for e in staged.exports.values() {
            if let Err(err) = self
                .ask(&agents, ValidateExport { session: e.session })
                .await
            {
                errors.push(format!("export {}: {err}", e.name));
            }
        }
        for p in staged.peers.values() {
            for s in &p.sessions {
                if let Err(err) = self.ask(&agents, ValidateExport { session: *s }).await {
                    errors.push(format!("peer {}: session {s}: {err}", p.name));
                }
            }
        }
        if !errors.is_empty() {
            return Err(errors.join("; "));
        }

        // Card validation before any commit (§6.1): an incompatible card fails
        // the reload; a transport failure is a per-peer status. Unchanged
        // outbound peers keep their client (and running followers).
        let prev = self.shared.read().clone();
        let (old_clients, old_epochs) = {
            let s = self.state.lock();
            (s.clients.clone(), s.epochs.clone())
        };
        let (epochs, retired) = epochs(&prev.staged, &old_epochs, &staged);
        let mut clients = BTreeMap::new();
        let mut peer_errors = BTreeMap::new();
        let mut probed = BTreeMap::new();
        let mut incompatible = Vec::new();
        // An unchanged peer that probed fine last time is not probed again; the rest run
        // together, so one unreachable peer costs one timeout, not one per peer.
        let old_errors = self.state.lock().peer_errors.clone();
        let http: Vec<_> = staged.peers.values().filter(|p| p.card.is_some()).collect();
        let results = futures::future::join_all(http.iter().map(|p| {
            let unchanged = old_clients.get(&p.name).is_some_and(|c| c.matches(p))
                && !old_errors.contains_key(&p.name);
            async move {
                if unchanged {
                    return None;
                }
                Some(
                    tokio::time::timeout(
                        client::REQUEST_TIMEOUT + client::CONNECT_TIMEOUT,
                        client::probe(p),
                    )
                    .await,
                )
            }
        }))
        .await;
        for (p, result) in http.into_iter().zip(results) {
            match result {
                None => {}
                Some(Ok(Ok(card_name))) => {
                    probed.insert(p.name.clone(), card_name);
                }
                Some(Ok(Err(ConnectError::Incompatible(e)))) => {
                    incompatible.push(format!("peer {}: {e}", p.name))
                }
                Some(Ok(Err(ConnectError::Transport(e)))) => {
                    peer_errors.insert(p.name.clone(), e);
                }
                Some(Err(_)) => {
                    peer_errors.insert(p.name.clone(), "card fetch timed out".to_owned());
                }
            }
            let c = match old_clients.get(&p.name).filter(|c| c.matches(p)) {
                Some(c) => c.clone(),
                None => Arc::new(PeerClient::new(
                    p,
                    agents.clone(),
                    vault.clone(),
                    CancellationToken::new(),
                )?),
            };
            if let Some(name) = probed.get(&p.name) {
                c.set_probed_name(name);
            }
            clients.insert(p.name.clone(), c);
        }
        // 183 §7.7: a contact's route is `theuth/a2a/1` to its pinned endpoint, once this
        // engine's endpoint is attached (until then the contact is inbound-only).
        if let Some(net) = self.net.get() {
            for p in staged.peers.values().filter(|p| p.eid.is_some()) {
                let display = staged
                    .contacts
                    .get(&p.name)
                    .map_or(p.name.as_str(), |c| c.display.as_str());
                let c = match old_clients.get(&p.name).filter(|c| c.matches(p)) {
                    Some(c) => c.clone(),
                    None => Arc::new(PeerClient::contact(
                        p,
                        display,
                        net.clone(),
                        agents.clone(),
                        vault.clone(),
                        CancellationToken::new(),
                    )?),
                };
                clients.insert(p.name.clone(), c);
            }
        }
        if !incompatible.is_empty() {
            return Err(incompatible.join("; "));
        }

        // One commit: identities (idempotent per alias+endpoint, §5.1),
        // revocation of removed aliases and the route table, all-or-nothing.
        let routes: Vec<PeerRoute> = staged
            .peers
            .values()
            .map(|p| {
                let sender = clients
                    .get(&p.name)
                    .map(|c| Arc::new(Sender(c.clone())) as Arc<dyn PeerSender>);
                PeerRoute {
                    alias: p.name.clone(),
                    endpoint: p.endpoint(),
                    roots: p.sessions.clone(),
                    generation: epochs[&p.name].clone(),
                    sender,
                    // 190: a contact's exports, every one shared in full.
                    full: if p.eid.is_some() {
                        p.exports.clone()
                    } else {
                        Vec::new()
                    },
                }
            })
            .collect();
        let revoke: Vec<String> = prev
            .bindings
            .keys()
            .filter(|a| !staged.peers.contains_key(*a))
            .cloned()
            .collect();
        // 183 F4: a contact whose grant epoch rotates, or whose cap changes, keeps a held
        // first message only for an export whose definition is unchanged (it moves to the new
        // epoch); every other held message fails now, before an Allow could run it under
        // authority that is gone.
        let rebind: Vec<agent::communication::network::RebindHeld> = staged
            .contacts
            .iter()
            .filter(|(alias, f)| {
                let cap_changed = prev.staged.contacts.get(*alias).map(|c| c.cap) != Some(f.cap);
                !f.blocked
                    && prev.staged.peers.contains_key(*alias)
                    && (cap_changed || !prev.staged.same_grant(&staged, alias))
            })
            .filter_map(|(alias, _)| {
                Some(agent::communication::network::RebindHeld {
                    contact: alias.clone(),
                    exports: prev.staged.unchanged_exports(&staged, alias),
                    route: epochs.get(alias)?.clone(),
                })
            })
            .collect();
        let committed = self
            .ask(&agents, CommitPeerConfig { routes, revoke })
            .await
            .map_err(|e| format!("commit peer configuration: {e}"))?;
        for m in &rebind {
            if let Err(e) = agents.ask(m.clone()).await {
                tracing::warn!("a2a: held first messages across a grant change: {e}");
            }
        }
        let bindings: BTreeMap<String, Uuid> = staged
            .peers
            .keys()
            .cloned()
            .zip(committed.iter().map(|b| b.id))
            .collect();

        // Install routes, then retire every grant that did not carry over
        // (removed, rotated, remapped): nothing admitted under it emits again.
        let live = Arc::new(Live::build(
            staged,
            bindings,
            &epochs,
            &listener.base,
            agents.clone(),
            self.subscribers.clone(),
            policy,
        ));
        // 186 review 2: the agent exports each contact holds now; every other inbox of the
        // contact stops running uncapped at once (a running turn included).
        let limits: Vec<agent::communication::network::ContactLimits> = live
            .staged
            .contacts
            .iter()
            .filter(|(_, f)| !f.blocked)
            .map(|(alias, _)| agent::communication::network::ContactLimits {
                contact: alias.clone(),
                full: live
                    .staged
                    .peers
                    .get(alias)
                    .map(|p| {
                        p.exports
                            .iter()
                            .filter(|e| live.staged.agents.contains_key(*e))
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .collect();
        // 183 F1: a contact whose grants here changed hears it and fetches our card again.
        let notice = contacts::offers::changed(&prev.staged, &live.staged);
        *self.shared.write() = live;
        // 183 review 2: the old Live admitted until this swap, so a first message it held
        // after the first ask is checked again (the same answer: kept or failed).
        for m in rebind {
            if let Err(e) = agents.ask(m).await {
                tracing::warn!("a2a: held first messages across a grant change: {e}");
            }
        }
        let to = agents.clone();
        tokio::spawn(async move {
            for m in limits {
                let _ = to.tell(m).await;
            }
        });
        self.notify_offers(notice);
        // A rename, removal or block changes whose offers the agent lists, and under what name.
        self.push_offers().await;
        for old in retired {
            old.cancel();
        }
        for (alias, old) in &old_clients {
            if !clients.get(alias).is_some_and(|c| Arc::ptr_eq(c, old)) {
                old.generation().cancel();
            }
        }
        // 190 (D10): once, at the first good load after the upgrade, every grant says it is
        // full now (tried once per process, so an unwritable marker never repeats it).
        if !std::mem::replace(&mut self.state.lock().notice_tried, true) {
            full_notice_once(&self.rt.data_dir, &file);
        }
        {
            let mut s = self.state.lock();
            s.clients = clients;
            s.epochs = epochs;
            s.peer_errors = peer_errors;
            s.http_good = file
                .peers
                .into_iter()
                .filter(|(_, p)| !p.is_contact())
                .collect();
        }
        Ok(())
    }

    /// 183 §7.8: change `peers.yaml` with `f`, rewrite it atomically and reload. A reload
    /// that refuses the result puts the old text back (and reloads it), so the file on disk
    /// is never one the engine refused; `A2aChanged` follows a change that took.
    async fn edit<T>(
        &self,
        f: impl FnOnce(&mut PeersFile) -> Result<T, String>,
    ) -> Result<T, String> {
        let _serial = self.edit_lock.lock().await;
        let path = self.rt.data_dir.join(PEERS_FILE);
        let before = match std::fs::read_to_string(&path) {
            Ok(t) => Some(t),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let (head, mut file) = contacts::store::parse(before.as_deref().unwrap_or(""))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let out = f(&mut file)?;
        let text = contacts::store::render(&head, &file)?;
        let io = |e: std::io::Error| format!("{}: {e}", path.display());
        contacts::store::write(&path, &text).map_err(io)?;
        if let Err(e) = self.reload().await {
            // 183 Z3-R2: an HTTP peer's problem (a broken entry, an incompatible card) must
            // not undo a contact edit: retry with the last good HTTP peers in its place. A
            // contact's own problem fails that retry too, and the old text goes back.
            if self.reload_as(true).await.is_ok() {
                tracing::warn!(
                    "a2a: peers.yaml applied; its HTTP peers keep their old settings: {e}"
                );
                {
                    let mut s = self.state.lock();
                    s.error = Some(e);
                    // Let the hot reload retry the full file (and report it once).
                    s.stamp = None;
                }
                self.rt.publish(A2aChanged {}).await;
                return Ok(out);
            }
            match &before {
                Some(old) => contacts::store::write(&path, old).map_err(io)?,
                None => std::fs::remove_file(&path).map_err(io)?,
            }
            if let Err(again) = self.reload().await {
                tracing::warn!("a2a: peers.yaml restore: {again}");
            }
            return Err(e);
        }
        self.state.lock().error = None;
        self.rt.publish(A2aChanged {}).await;
        Ok(out)
    }

    /// 183 §7.8: hot reload: an owner's hand edit of `peers.yaml` is picked up within
    /// [`HOT_RELOAD`] (the RPCs' own writes reload at once and move the stamp).
    /// ponytail: mtime polling; a file watcher if two seconds is ever too slow.
    async fn watch_file(self: Arc<Self>) {
        let path = self.rt.data_dir.join(PEERS_FILE);
        let stop = self.rt.shutdown.clone();
        loop {
            tokio::select! {
                () = stop.cancelled() => return,
                () = tokio::time::sleep(HOT_RELOAD) => {}
            }
            // 183 §7.2: keep each contact's last live direct addresses for the next dial.
            self.remember_addrs().await;
            let now = contacts::store::stamp(&path);
            if now == self.state.lock().stamp {
                continue;
            }
            let _serial = self.edit_lock.lock().await;
            let outcome = self.reload().await;
            if let Err(e) = &outcome {
                tracing::warn!("a2a peers.yaml: {e}");
                // Do not retry an unchanged broken file every tick.
                self.state.lock().stamp = now;
            }
            self.state.lock().error = outcome.err();
            self.rt.publish(A2aChanged {}).await;
        }
    }

    /// Bind or stop the listener to match `a2a.enabled && global
    /// communication` (§2.2); report to the agent and the bus when something
    /// changed. Serialised: concurrent callers converge on the latest policy.
    async fn sync_listener(&self) {
        let _serial = self.listener_lock.lock().await;
        let (agents, policy, serving) = {
            let s = self.state.lock();
            (s.agents.clone(), s.policy.clone(), s.serve.clone())
        };
        let (Some(agents), Some(policy)) = (agents, policy) else {
            return;
        };
        let global = policy.0.read().global();
        // 183 W-26: the actor also runs for contacts alone; `a2a.enabled` gates the listener.
        let want = self.enabled && global && self.listener.is_ok();
        let changed = match transition(want, serving.is_some()) {
            Transition::Keep => false,
            Transition::Start => {
                let listener = self.listener.as_ref().expect("checked by `want`");
                match tokio::net::TcpListener::bind(listener.addr).await {
                    Ok(sock) => {
                        let addr = sock.local_addr().unwrap_or(listener.addr);
                        let token = self.rt.shutdown.child_token();
                        let shared = self.shared.clone();
                        let serve_token = token.clone();
                        tokio::spawn(async move {
                            if let Err(e) = server::serve(sock, shared, serve_token).await {
                                tracing::error!("a2a listener: {e}");
                            }
                        });
                        tracing::info!(%addr, production = listener.production, "a2a listening");
                        let mut s = self.state.lock();
                        s.serve = Some(token);
                        s.listening = Some(addr);
                        if s.error.as_deref().is_some_and(|e| e.starts_with("listen ")) {
                            s.error = None;
                        }
                    }
                    Err(e) => {
                        self.state.lock().error = Some(format!("listen {}: {e}", listener.addr));
                    }
                }
                true
            }
            Transition::Stop => {
                serving.expect("checked by `transition`").cancel();
                let mut s = self.state.lock();
                s.serve = None;
                s.listening = None;
                tracing::info!("a2a listener stopped (communication off)");
                true
            }
        };
        let (listening, reported) = {
            let s = self.state.lock();
            (s.listening.is_some(), s.reported)
        };
        if reported != Some(listening)
            && agents
                .ask(NetworkStatus {
                    enabled: self.enabled,
                    listening,
                })
                .await
                .is_ok()
        {
            self.state.lock().reported = Some(listening);
        }
        if changed {
            self.rt.publish(A2aChanged {}).await;
        }
    }

    /// Background initialisation: wait for `agent` and `secrets`, then the
    /// first reload and the listener.
    async fn init(self: Arc<Self>) {
        let Some(agents) = self.rt.lookup_when::<Agents>(AGENT_ACTOR).await else {
            return;
        };
        let vault = secrets::handle(&self.rt).await;
        if vault.is_none() {
            tracing::warn!("secrets vault unavailable; peer credentials cannot be resolved");
        }
        let policy = match agents.ask(PeerPolicy).await {
            Ok(p) => p,
            Err(e) => {
                self.state.lock().error = Some(format!("agent policy unavailable: {e}"));
                return;
            }
        };
        {
            let mut s = self.state.lock();
            s.agents = Some(agents);
            s.vault = vault;
            s.policy = Some(policy);
            s.error = None;
        }
        if let Err(e) = self.reload().await {
            tracing::warn!("a2a peers.yaml: {e}");
            self.state.lock().error = Some(e);
        }
        tokio::spawn(self.clone().watch_file());
        tokio::spawn(self.clone().attach_mesh());
        let source = Arc::new(contacts::FirstContactItems(Arc::downgrade(&self)));
        let rt = self.rt.clone();
        tokio::spawn(async move {
            if let Some(inbox) = rt
                .lookup_when::<roster::attention::Attention>(roster::attention::ATTENTION_ACTOR)
                .await
            {
                let _ = inbox.tell(roster::attention::RegisterSource(source)).await;
            }
        });
        self.sync_listener().await;
        self.rt.publish(A2aChanged {}).await;
    }

    /// 183 §7.2: once the mesh plugin bound its endpoint, serve `theuth/a2a/1` on it, and
    /// join the network (relay, address lookup) when this engine has a contact.
    async fn attach_mesh(self: Arc<Self>) {
        let stop = self.rt.shutdown.clone();
        let net = loop {
            if let Some(n) = mesh::net() {
                break n;
            }
            tokio::select! {
                () = stop.cancelled() => return,
                () = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
            }
        };
        self.attach(net).await;
    }

    /// Serve `theuth/a2a/1` on `net` (the mesh's endpoint; a test's own).
    async fn attach(self: &Arc<Self>, net: Arc<mesh::net::Net>) {
        net.set_contacts(Arc::new(contacts::reach::Gate(Arc::downgrade(self))));
        let _ = self.net.set(net.clone());
        if !self.shared.read().staged.contacts.is_empty() {
            net.join_network().await;
        }
        self.rewire_contacts().await;
        // 183 F1: what each contact shares with us is memory; fetch it again at boot.
        let eids: Vec<String> = self
            .shared
            .read()
            .staged
            .contacts
            .values()
            .filter(|f| !f.blocked)
            .map(|f| f.eid.clone())
            .collect();
        for eid in eids {
            self.refresh_offers_soon(&eid);
        }
    }

    /// Contacts' outbound routes ride on the endpoint: once it is attached, a reload
    /// installs them.
    async fn rewire_contacts(&self) {
        if self
            .shared
            .read()
            .staged
            .peers
            .values()
            .any(|p| p.eid.is_some())
            && let Err(e) = self.reload().await
        {
            tracing::warn!("a2a: contact routes: {e}");
        }
    }
}

impl Message<GetStatus> for A2aActor {
    type Reply = A2aStatus;

    async fn handle(&mut self, _: GetStatus, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ctx.status().await
    }
}

impl Message<Reload> for A2aActor {
    type Reply = DelegatedReply<Result<A2aStatus, String>>;

    async fn handle(&mut self, _: Reload, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let c = self.ctx.clone();
        // Card fetches and agent round trips: off the mailbox.
        tokio::spawn(async move {
            let outcome = c.reload().await;
            c.state.lock().error = outcome.as_ref().err().cloned();
            c.rt.publish(A2aChanged {}).await;
            let status = c.status().await;
            tx.send(outcome.map(|()| status));
        });
        reply
    }
}

impl Message<CommunicationChanged> for A2aActor {
    type Reply = ();

    async fn handle(&mut self, m: CommunicationChanged, _: &mut Context<Self, Self::Reply>) {
        if m.session.is_none() {
            // The agent publishes this while handling our `NetworkStatus`;
            // awaiting it here would let the two mailboxes wait on each other.
            let c = self.ctx.clone();
            tokio::spawn(async move { c.sync_listener().await });
        }
    }
}

impl Message<agent::communication::network::ContactOffersStale> for A2aActor {
    type Reply = ();

    /// 183 F1: `discover_peers` found the contacts' offers older than the TTL.
    async fn handle(
        &mut self,
        _: agent::communication::network::ContactOffersStale,
        _: &mut Context<Self, Self::Reply>,
    ) {
        let c = self.ctx.clone();
        tokio::spawn(async move { c.refresh_stale_offers().await });
    }
}

impl Message<agent::communication::network::ContactsHeldChanged> for A2aActor {
    type Reply = ();

    /// 183 F3, F4, F11: a first-contact hold began or ended, wherever in the agent (held,
    /// released, failed, revoked, expired): Attention derives again and every
    /// `contact_first_waiting` reader refetches (`A2aChanged`).
    async fn handle(
        &mut self,
        _: agent::communication::network::ContactsHeldChanged,
        _: &mut Context<Self, Self::Reply>,
    ) {
        contacts::nudge_attention();
        self.ctx.rt.publish(A2aChanged {}).await;
    }
}

#[async_trait::async_trait]
impl runtime::Plugin for A2aPlugin {
    fn name(&self) -> &'static str {
        "a2a"
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        // Absent section = defaults (off). A present but malformed section
        // fails closed and stays visible through the actor's status.
        let (cfg, config_error) = if rt.config.find_value("a2a").is_ok() {
            match rt.section::<A2aConfig>("a2a") {
                Ok(c) => (c, None),
                Err(e) => (A2aConfig::default(), Some(format!("a2a config: {e}"))),
            }
        } else {
            (A2aConfig::default(), None)
        };
        // 183 W-26: the actor runs without `a2a.enabled` too: contacts (and adding the first
        // one) need it. `enabled: false` keeps HTTP peers dormant and the listener closed.
        if !cfg.enabled && config_error.is_none() {
            tracing::info!("a2a: HTTP listener and peers disabled by config; contacts only");
        }
        let listener = match &config_error {
            Some(e) => Err(e.clone()),
            None => cfg.listener(),
        };
        if let Err(e) = &listener {
            tracing::error!("a2a: {e}; listener stays off");
        }
        let ctx = Arc::new(Ctx {
            rt: rt.clone(),
            enabled: cfg.enabled && config_error.is_none(),
            public_url: cfg.public_url.clone(),
            state: Mutex::new(State {
                error: listener.as_ref().err().cloned(),
                ..State::default()
            }),
            listener,
            shared: Arc::new(parking_lot::RwLock::new(Arc::new(Live::empty()))),
            subscribers: Arc::new(Semaphore::new(server::SUBSCRIBERS_MAX)),
            reload_lock: tokio::sync::Mutex::new(()),
            listener_lock: tokio::sync::Mutex::new(()),
            edit_lock: tokio::sync::Mutex::new(()),
            cfg,
            net: std::sync::OnceLock::new(),
            mdns: contacts::discovery::default_mdns(),
            contacts: Mutex::new(contacts::Contacts::new(
                cfg!(debug_assertions) && std::env::var_os("THEUTH_DEV_CONTACTS_DIR").is_some(),
            )),
            offers: Mutex::default(),
        });
        let actor = A2aActor::spawn_with_mailbox(
            A2aActor { ctx: ctx.clone() },
            runtime::subscriber_mailbox(),
        );
        actor.register(A2A_ACTOR)?;
        rt.subscribe::<A2aActor, CommunicationChanged>(&actor).await;
        rt.subscribe::<A2aActor, agent::communication::network::ContactOffersStale>(&actor)
            .await;
        rt.subscribe::<A2aActor, agent::communication::network::ContactsHeldChanged>(&actor)
            .await;
        if config_error.is_none() {
            tokio::spawn(ctx.init());
        }
        Ok(())
    }
}

/// 183 tests: one engine's A2A context over `dir`, its mesh endpoint `net`, a fake LAN and,
/// when given, its live agent (then `peers.yaml` reloads and the ceremony pins).
#[cfg(test)]
async fn test_ctx(
    dir: &std::path::Path,
    net: Arc<mesh::net::Net>,
    mdns: Arc<dyn contacts::discovery::Mdns>,
    cfg: A2aConfig,
    agents: Option<ActorRef<Agents>>,
) -> Arc<Ctx> {
    let rt = Runtime::new(figment::Figment::new(), dir.to_path_buf());
    let listener = cfg.listener();
    let ctx = Arc::new(Ctx {
        rt,
        enabled: cfg.enabled,
        public_url: None,
        state: Mutex::new(State::default()),
        listener,
        shared: Arc::new(parking_lot::RwLock::new(Arc::new(Live::empty()))),
        subscribers: Arc::new(Semaphore::new(server::SUBSCRIBERS_MAX)),
        reload_lock: tokio::sync::Mutex::new(()),
        listener_lock: tokio::sync::Mutex::new(()),
        edit_lock: tokio::sync::Mutex::new(()),
        cfg,
        net: std::sync::OnceLock::new(),
        mdns,
        contacts: Mutex::new(contacts::Contacts::new(true)),
        offers: Mutex::default(),
    });
    if let Some(agents) = agents {
        let policy = agents.ask(PeerPolicy).await.unwrap();
        let mut s = ctx.state.lock();
        s.agents = Some(agents);
        s.policy = Some(policy);
    }
    ctx.attach(net).await;
    ctx
}

/// Listener transition for the desired (`want`) and current (`serving`) state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Transition {
    Start,
    Stop,
    Keep,
}

fn transition(want: bool, serving: bool) -> Transition {
    match (want, serving) {
        (true, false) => Transition::Start,
        (false, true) => Transition::Stop,
        _ => Transition::Keep,
    }
}

/// Grant epochs for the next configuration: an alias whose authorization is
/// unchanged keeps its token (open streams survive a no-op reload); any other
/// alias gets a fresh one. Returns the new map and the tokens to cancel once
/// the new configuration is live. Token rotation keeps the binding identity;
/// only the epoch changes.
fn epochs(
    prev: &Staged,
    old: &BTreeMap<String, CancellationToken>,
    next: &Staged,
) -> (BTreeMap<String, CancellationToken>, Vec<CancellationToken>) {
    let mut retired = old.clone();
    let epochs = next
        .peers
        .keys()
        .map(|alias| {
            let carried = prev
                .same_grant(next, alias)
                .then(|| retired.remove(alias))
                .flatten();
            (alias.clone(), carried.unwrap_or_default())
        })
        .collect();
    (epochs, retired.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listener_transitions_are_idempotent() {
        assert_eq!(transition(true, false), Transition::Start);
        assert_eq!(transition(true, true), Transition::Keep);
        assert_eq!(transition(false, true), Transition::Stop);
        assert_eq!(transition(false, false), Transition::Keep);
    }

    /// A 186-written file: `bob` holds `proj` (no `modes`).
    fn granted_file() -> PeersFile {
        let bob = iroh::SecretKey::from_bytes(&[7; 32]).public().to_string();
        serde_yaml::from_str(&format!(
            "peers:\n  bob:\n    endpoint: {bob}\n    exports: [proj]\nexports:\n  proj: {{ session: \"00000000-0000-0000-0000-000000000003\", name: Proj }}\n"
        ))
        .unwrap()
    }

    const NOTICE: &str = "a2a: share proj to bob is full (cautious mode removed)";

    /// 190 (D10): the notice runs once: the first load logs each grant and writes the
    /// marker; a second load logs nothing.
    #[test]
    fn the_full_notice_logs_once() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("a2a")).unwrap();
        assert_eq!(full_notice_once(dir.path(), &granted_file()), [NOTICE]);
        assert!(dir.path().join(config::FULL_NOTICE).is_file());
        assert!(full_notice_once(dir.path(), &granted_file()).is_empty());
    }

    /// 190 (D10): with no `a2a/` yet, the notice creates it beside `peers.yaml` and logs.
    #[test]
    fn the_full_notice_creates_a_missing_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(full_notice_once(dir.path(), &granted_file()), [NOTICE]);
        assert!(dir.path().join(config::FULL_NOTICE).is_file());
    }

    /// 190 (D10): a marker that cannot be written is only a warning: the lines are logged,
    /// nothing panics, and the reload goes on (it is tried once per process).
    #[cfg(unix)]
    #[test]
    fn an_unwritable_marker_warns_and_never_fails() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let a2a = dir.path().join("a2a");
        std::fs::create_dir(&a2a).unwrap();
        std::fs::set_permissions(&a2a, std::fs::Permissions::from_mode(0o500)).unwrap();
        let lines = full_notice_once(dir.path(), &granted_file());
        std::fs::set_permissions(&a2a, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(lines, [NOTICE]);
        assert!(!dir.path().join(config::FULL_NOTICE).exists());
    }

    fn staged(peers: &[(&str, &str)]) -> Staged {
        let mut s = Staged::default();
        for (alias, token) in peers {
            s.peers.insert(
                (*alias).to_owned(),
                config::Peer {
                    name: (*alias).to_owned(),
                    card: None,
                    outbound: None,
                    inbound: Some(config::Token::for_test(token)),
                    sessions: vec![],
                    exports: vec![],
                    allow_private: false,
                    eid: None,
                },
            );
        }
        s
    }

    /// Unchanged grants keep their epoch (open streams survive); rotation
    /// or removal retires the old one and the old token alone is cancelled
    /// (the binding identity is the store's and unaffected here).
    #[test]
    fn epochs_carry_only_unchanged_grants() {
        let prev = staged(&[("keep", "k"), ("rotate", "r1"), ("drop", "d")]);
        let old: BTreeMap<String, CancellationToken> = prev
            .peers
            .keys()
            .map(|a| (a.clone(), CancellationToken::new()))
            .collect();
        let next = staged(&[("keep", "k"), ("rotate", "r2"), ("new", "n")]);
        let (epochs, retired) = epochs(&prev, &old, &next);
        assert_eq!(epochs.len(), 3);
        assert_eq!(retired.len(), 2);
        for t in &retired {
            t.cancel();
        }
        assert!(!old["keep"].is_cancelled());
        assert!(old["rotate"].is_cancelled() && old["drop"].is_cancelled());
        assert!(!epochs["rotate"].is_cancelled() && !epochs["new"].is_cancelled());
        // The kept alias carries the very same token, not a lookalike.
        epochs["keep"].cancel();
        assert!(old["keep"].is_cancelled());
    }
}
