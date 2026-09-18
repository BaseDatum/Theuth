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
//! `a2a.enabled: false` (the default) starts nothing: no actor, listener,
//! card fetch or credential load; `A2aStatus::disabled()` is the RPC answer
//! then. A malformed `a2a:` section registers the actor with the error and
//! everything closed. With `enabled: true` the actor registers immediately
//! and finishes initialisation in the background once `agent` and `secrets`
//! are up (plugin order is not assumed).

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
pub mod server;

use client::{ConnectError, PeerClient, Sender};
use config::{A2aConfig, Listener, PEERS_FILE, PeersFile, Staged};
use server::{Live, Shared};

/// Registry name of the [`A2aActor`].
pub const A2A_ACTOR: &str = "a2a";

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
    state: Mutex<State>,
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
        let _serial = self.reload_lock.lock().await;
        let agents = self.agents()?;
        let listener = self.listener.clone()?;
        let (vault, policy) = {
            let s = self.state.lock();
            (s.vault.clone(), s.policy.clone())
        };
        let policy = policy.ok_or_else(|| "agent policy not ready".to_owned())?;
        let path = self.rt.data_dir.join(PEERS_FILE);
        let file: PeersFile = match std::fs::read_to_string(&path) {
            Ok(text) => {
                serde_yaml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => PeersFile::default(),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let console = std::fs::read_to_string(self.rt.data_dir.join("token"))
            .ok()
            .map(|t| t.trim().to_owned());
        let resolve = |name: &str| vault.as_ref().and_then(|v| v.get(name, 0));
        let staged =
            config::stage(&file, &resolve, console.as_deref()).map_err(|e| e.join("; "))?;

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
        for p in staged.peers.values().filter(|p| p.card.is_some()) {
            match tokio::time::timeout(
                client::REQUEST_TIMEOUT + client::CONNECT_TIMEOUT,
                client::probe(p),
            )
            .await
            {
                Ok(Ok(card_name)) => {
                    probed.insert(p.name.clone(), card_name);
                }
                Ok(Err(ConnectError::Incompatible(e))) => {
                    incompatible.push(format!("peer {}: {e}", p.name))
                }
                Ok(Err(ConnectError::Transport(e))) => {
                    peer_errors.insert(p.name.clone(), e);
                }
                Err(_) => {
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
                }
            })
            .collect();
        let revoke: Vec<String> = prev
            .bindings
            .keys()
            .filter(|a| !staged.peers.contains_key(*a))
            .cloned()
            .collect();
        let committed = self
            .ask(&agents, CommitPeerConfig { routes, revoke })
            .await
            .map_err(|e| format!("commit peer configuration: {e}"))?;
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
        *self.shared.write() = live;
        for old in retired {
            old.cancel();
        }
        for (alias, old) in &old_clients {
            if !clients.get(alias).is_some_and(|c| Arc::ptr_eq(c, old)) {
                old.generation().cancel();
            }
        }
        {
            let mut s = self.state.lock();
            s.clients = clients;
            s.epochs = epochs;
            s.peer_errors = peer_errors;
        }
        Ok(())
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
        let want = global && self.listener.is_ok();
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
                    enabled: true,
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
        self.sync_listener().await;
        self.rt.publish(A2aChanged {}).await;
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
        if !cfg.enabled && config_error.is_none() {
            tracing::info!("a2a disabled by config");
            return Ok(());
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
        });
        let actor = A2aActor::spawn_with_mailbox(
            A2aActor { ctx: ctx.clone() },
            runtime::subscriber_mailbox(),
        );
        actor.register(A2A_ACTOR)?;
        rt.subscribe::<A2aActor, CommunicationChanged>(&actor).await;
        if ctx.enabled {
            tokio::spawn(ctx.init());
        }
        Ok(())
    }
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
