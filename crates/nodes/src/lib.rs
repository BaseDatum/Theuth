//! Nodes plugin: pair this harness to harnesses on other machines and keep one
//! control conn to each, so the model can run children there and the UIs can
//! introduce devices to them (design doc 32 §6).
//!
//! Public surface consumed by other plugins:
//! - commands on the [`NODES_ACTOR`] actor ([`NodesActor`]): [`Status`],
//!   [`Add`], [`Remove`], [`Rename`], [`Invite`]
//! - events: [`NodeEvent`]
//!
//! Modules: [`types`] (the contract), [`store`] (`nodes.yaml`, §6.4), [`conn`]
//! (device socket, initiator handshakes, record codec, §4), [`client`]
//! (jsonrpsee over a mode-0 conn, §6.3), [`dialer`] (mode-1 conns for ACP
//! children, §6.5).
//!
//! This harness is a *device* of every node: its `link/identity` is the
//! static in both roles (§4.1). What `agent` needs arrives as `RegisterNode` /
//! `UnregisterNode` messages, driven by presence (§6.5).

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use agent::{RegisterNode, UnregisterNode};
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use link::LinkEvent;
use link::conn::PairHello;
use link::identity::{Identity, b64, now_ms};
use link::pairing::Parsed;
use runtime::Runtime;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

pub mod client;
pub mod conn;
pub mod dialer;
pub mod store;
pub mod types;
/// Actor lookup shared by the network RPC surface.
pub mod rpc_support;
/// Network RPC methods and typed client.
pub mod rpc;
pub use rpc::{NetworkApiClient, rpc_module};

pub use types::*;

use client::Rpc;
use conn::{Channel, HandshakeError};
use dialer::NodeDialer;
use store::{Row, Store};

/// Presence wait for `Add` and `Remove`'s fresh sockets.
const PRESENCE_TIMEOUT: Duration = Duration::from_secs(10);
/// `Remove`'s best-effort self-revoke on the node.
const REVOKE_TIMEOUT: Duration = Duration::from_secs(3);
/// `link_status` cadence on an idle control conn: half the relay's 600 s
/// device idle timer, so "zero bytes at rest" costs one tiny request.
const KEEPALIVE: Duration = Duration::from_secs(300);
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
const STABLE_AFTER: Duration = Duration::from_secs(60);

/// The plugin.
pub struct NodesPlugin;

/// Declare wire contracts before starting producers, including disabled services.
pub async fn declare_wire(rt: &Runtime) -> anyhow::Result<()> {
    use wire::RuntimeWire as _;
    rt.register_wire_event::<NodeEvent>().await
}

/// The [`NODES_ACTOR`] actor: the store, one control conn task per node.
#[derive(kameo::Actor)]
pub struct NodesActor {
    ctx: Arc<Ctx>,
}

/// What the actor and every control task share.
struct Ctx {
    rt: Runtime,
    /// Our static; the device key on every node (32 §4.1).
    identity: Arc<Identity>,
    /// `PairHello.name`: `link.name` or the hostname.
    hello_name: String,
    state: Mutex<State>,
}

struct State {
    store: Store,
    /// Per-node runtime bits, keyed by node id; absent while `Remove` runs.
    live: HashMap<String, Live>,
}

/// One node's control conn task as the actor and the dialer see it.
struct Live {
    cancel: CancellationToken,
    online: Arc<AtomicBool>,
    children: Arc<AtomicU32>,
    /// The mode-0 client while the conn is up.
    rpc: Option<Arc<Rpc>>,
}

/// `"<platform> · theuth <version>"`, or `"<platform> · offline"` (32 §6.5, §7).
fn describe(row: &Row, online: bool) -> String {
    match (online, row.platform.is_empty()) {
        (true, _) => format!("{} · theuth {}", row.platform, row.version),
        (false, true) => "offline".to_owned(),
        (false, false) => format!("{} · offline", row.platform),
    }
}

impl Ctx {
    /// Lock the state. Never hold across an `await`.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn node(st: &State, row: &Row) -> Node {
        let live = st.live.get(&row.id);
        row.node(
            live.is_some_and(|l| l.online.load(Ordering::Relaxed)),
            live.map_or(0, |l| l.children.load(Ordering::Relaxed)),
        )
    }

    /// Flip the presence bit; `true` when it changed.
    fn set_online(&self, id: &str, online: bool) -> bool {
        let mut st = self.lock();
        let Some(live) = st.live.get_mut(id) else {
            return false;
        };
        if !online {
            live.rpc = None;
        }
        live.online.swap(online, Ordering::Relaxed) != online
    }

    fn register_msg(&self, id: &str) -> Option<RegisterNode> {
        let st = self.lock();
        let row = st.store.by_id(id)?;
        if row.revoked {
            return None;
        }
        let live = st.live.get(id)?;
        Some(RegisterNode {
            name: row.name.clone(),
            describe: describe(row, live.online.load(Ordering::Relaxed)),
            dialer: Arc::new(NodeDialer {
                name: row.name.clone(),
                relay: row.relay.clone(),
                harness_pub: row.key,
                identity: self.identity.clone(),
                online: live.online.clone(),
                children: live.children.clone(),
            }),
        })
    }

    async fn agents(&self) -> Option<ActorRef<agent::Agents>> {
        self.rt
            .lookup_when::<agent::Agents>(agent::AGENT_ACTOR)
            .await
    }

    /// (Re)register the node with `agent` from its current row and presence.
    async fn register(&self, id: &str) {
        let Some(m) = self.register_msg(id) else {
            return;
        };
        if let Some(a) = self.agents().await {
            let _ = a.ask(m).await;
        }
    }

    async fn unregister(&self, name: String) {
        if let Some(a) = self.agents().await {
            let _ = a.ask(UnregisterNode { name }).await;
        }
    }

    /// Conn down: `last_seen_ms`, `NodeOnline { false }` and an `· offline`
    /// registration — each only when the node was online.
    async fn offline(&self, id: &str) {
        if !self.set_online(id, false) {
            return;
        }
        {
            let mut st = self.lock();
            if let Some(row) = st.store.by_id_mut(id) {
                row.last_seen_ms = now_ms();
                if let Err(e) = st.store.save() {
                    tracing::warn!("nodes.yaml: {e}");
                }
            }
        }
        self.rt
            .publish(NodeEvent::NodeOnline {
                id: id.to_owned(),
                online: false,
            })
            .await;
        self.register(id).await;
    }

    /// Queue a device revoke for a node that is not reachable now (32 §8.4).
    fn defer_revoke(&self, node: &str, device: &str) {
        let mut st = self.lock();
        let Some(row) = st.store.by_id_mut(node) else {
            return;
        };
        if !row.revoke_pending.iter().any(|d| d == device) {
            row.revoke_pending.push(device.to_owned());
            if let Err(e) = st.store.save() {
                tracing::warn!("nodes.yaml: {e}");
            }
        }
    }
}

/// Create the node's [`Live`] entry and spawn its control conn task (32 §6.3).
fn start_control(ctx: &Arc<Ctx>, id: String) {
    let cancel = CancellationToken::new();
    ctx.lock().live.insert(
        id.clone(),
        Live {
            cancel: cancel.clone(),
            online: Arc::default(),
            children: Arc::default(),
            rpc: None,
        },
    );
    tokio::spawn(control(ctx.clone(), id, cancel));
}

/// How one control session ended.
enum Ended {
    /// Relay or node closed; `Some` = when the session came up.
    Closed(Option<Instant>),
    /// `CLOSE 3` at the handshake: the node no longer knows us.
    Revoked,
}

/// One node's control conn for the process lifetime: park, `IK` mode 0,
/// `info`, hold, back off, repeat — until `Remove` cancels or the node revokes.
async fn control(ctx: Arc<Ctx>, id: String, cancel: CancellationToken) {
    ctx.register(&id).await;
    let mut backoff = BACKOFF_MIN;
    while !cancel.is_cancelled() {
        match session(&ctx, &id, &cancel).await {
            Ok(Ended::Revoked) => {
                revoked(&ctx, &id).await;
                return;
            }
            Ok(Ended::Closed(since)) => {
                if since.is_some_and(|t| t.elapsed() >= STABLE_AFTER) {
                    backoff = BACKOFF_MIN;
                }
            }
            Err(e) => tracing::info!("node {id}: {e:#}"),
        }
        ctx.offline(&id).await;
        if cancel.is_cancelled() {
            break;
        }
        let jitter = backoff.mul_f64(rand::random_range(0.8..1.2));
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep(jitter) => {}
        }
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// One socket: park until `PRESENCE 1`, handshake, `info`, then hold until
/// the conn closes or `cancel`.
async fn session(ctx: &Arc<Ctx>, id: &str, cancel: &CancellationToken) -> anyhow::Result<Ended> {
    let (relay, key) = {
        let st = ctx.lock();
        let row = st
            .store
            .by_id(id)
            .ok_or_else(|| anyhow::anyhow!("row gone"))?;
        (row.relay.clone(), row.key)
    };
    let mut sock = tokio::select! {
        _ = cancel.cancelled() => return Ok(Ended::Closed(None)),
        r = conn::open(&relay, &key) => r?,
    };
    loop {
        let p = tokio::select! {
            _ = cancel.cancelled() => return Ok(Ended::Closed(None)),
            p = conn::presence(&mut sock) => p,
        };
        match p {
            Some(true) => break,
            Some(false) => ctx.offline(id).await,
            None => anyhow::bail!("relay closed the socket while parked"),
        }
    }
    let (transport, _) = match conn::handshake(&mut sock, &ctx.identity, &key, 0).await {
        Ok(t) => t,
        Err(HandshakeError::Refused) => return Ok(Ended::Revoked),
        Err(HandshakeError::Offline) => return Ok(Ended::Closed(None)),
        Err(HandshakeError::Other(e)) => return Err(e),
    };
    let rpc = Arc::new(Rpc::new(Channel::new(sock, transport)));
    let info = rpc.info().await.map_err(anyhow::Error::msg)?;
    let since = Instant::now();
    let pending = {
        let mut st = ctx.lock();
        let State { store, live } = &mut *st;
        let row = store
            .by_id_mut(id)
            .ok_or_else(|| anyhow::anyhow!("row gone"))?;
        row.platform = info.platform;
        row.version = info.version;
        row.revoked = false;
        let pending = row.revoke_pending.clone();
        if let Err(e) = store.save() {
            tracing::warn!("nodes.yaml: {e}");
        }
        let live = live
            .get_mut(id)
            .ok_or_else(|| anyhow::anyhow!("node removed"))?;
        live.rpc = Some(rpc.clone());
        live.online.store(true, Ordering::Relaxed);
        pending
    };
    ctx.rt
        .publish(NodeEvent::NodeOnline {
            id: id.to_owned(),
            online: true,
        })
        .await;
    ctx.register(id).await;
    flush_revokes(ctx, id, &rpc, pending).await;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = rpc.closed() => break,
            _ = tokio::time::sleep(KEEPALIVE) => {
                if let Err(e) = rpc.link_status().await {
                    tracing::info!("node {id}: keepalive: {e}");
                    break;
                }
            }
        }
    }
    Ok(Ended::Closed(Some(since)))
}

/// `link_revoke_device` for every id queued while the node was offline (32 §8.4).
async fn flush_revokes(ctx: &Ctx, id: &str, rpc: &Rpc, pending: Vec<String>) {
    if pending.is_empty() {
        return;
    }
    let mut done = Vec::new();
    for dev in &pending {
        match rpc.link_revoke_device(dev).await {
            Ok(()) => done.push(dev.clone()),
            Err(e) => tracing::warn!("node {id}: revoke {dev}: {e}"),
        }
    }
    let mut st = ctx.lock();
    if let Some(row) = st.store.by_id_mut(id) {
        row.revoke_pending.retain(|d| !done.contains(d));
        if let Err(e) = st.store.save() {
            tracing::warn!("nodes.yaml: {e}");
        }
    }
}

/// The node deleted us from its `devices.yaml`: keep the row, stop retrying.
async fn revoked(ctx: &Ctx, id: &str) {
    let name = {
        let mut st = ctx.lock();
        if let Some(live) = st.live.get_mut(id) {
            live.rpc = None;
            live.online.store(false, Ordering::Relaxed);
        }
        let Some(row) = st.store.by_id_mut(id) else {
            return;
        };
        row.revoked = true;
        row.name.clone()
    };
    tracing::warn!("node {name}: revoked by the node");
    ctx.rt
        .publish(NodeEvent::NodeRevoked { id: id.to_owned() })
        .await;
    ctx.unregister(name).await;
}

/// Everything `Add` checks before touching the network (32 §6.2): the string,
/// our own key, an existing row, and the one namespace shared with `agents.yaml`.
/// Returns the parsed string and the name the node will be registered under.
fn precheck(
    our_pub: &[u8; 32],
    rows: &[Row],
    agents: &BTreeMap<String, acp::Entry>,
    key: &str,
    given: Option<&str>,
) -> Result<(Parsed, String), String> {
    let parsed =
        link::pairing::parse(key).ok_or("not a pairing key (expected https://<relay>/link#1.…)")?;
    if parsed.relay_host.is_empty() {
        return Err("pairing key has no relay host".into());
    }
    if &parsed.harness_pub == our_pub {
        return Err("this key is our own harness".into());
    }
    let id = b64(&parsed.harness_pub);
    if let Some(row) = rows.iter().find(|r| r.id == id) {
        return Err(format!("already paired as {}", row.name));
    }
    let name = given.unwrap_or(&parsed.name).to_owned();
    if agents.contains_key(&name) {
        return Err(format!(
            "{name} is an external harness in agents.yaml; rename with name:"
        ));
    }
    if rows.iter().any(|r| r.name == name) {
        return Err(format!("{name} is already a node; rename with name:"));
    }
    Ok((parsed, name))
}

/// `Add` off the mailbox: `IKpsk2`, `info` as the enrolling record, row, event, control task.
async fn add(ctx: Arc<Ctx>, m: Add) -> Result<Node, String> {
    let agents = acp::load(&ctx.rt.data_dir).map_err(|e| format!("{e:#}"))?;
    let (parsed, _) = {
        let st = ctx.lock();
        precheck(
            &ctx.identity.public,
            st.store.rows(),
            &agents,
            &m.key,
            m.name.as_deref(),
        )?
    };
    let relay = conn::relay_url(&parsed);
    let mut sock = conn::open(&relay, &parsed.harness_pub)
        .await
        .map_err(|e| format!("relay unreachable: {e:#}"))?;
    match tokio::time::timeout(PRESENCE_TIMEOUT, conn::presence(&mut sock)).await {
        Ok(Some(true)) => {}
        Ok(Some(false)) => return Err(format!("{} is offline", parsed.name)),
        Ok(None) | Err(_) => return Err("relay unreachable: no answer".into()),
    }
    let hello = PairHello {
        name: ctx.hello_name.clone(),
        platform: "theuth".to_owned(),
        pending: parsed.pending.clone(),
    };
    let (transport, accept) = match conn::pair(&mut sock, &ctx.identity, &parsed, &hello).await {
        Ok(x) => x,
        Err(HandshakeError::Refused) => return Err("pairing key expired or already used".into()),
        Err(HandshakeError::Offline) => return Err(format!("{} is offline", parsed.name)),
        Err(HandshakeError::Other(e)) => return Err(format!("pairing failed: {e:#}")),
    };
    // The first decrypted record enrols us (31 §4.3); discovery now precedes info.
    let rpc = Rpc::new(Channel::new(sock, transport));
    let info = rpc
        .info()
        .await
        .map_err(|e| format!("pairing failed: {e}"))?;
    // The control task opens its own IK session; this pairing conn is done.
    drop(rpc);
    let row = Row {
        id: b64(&parsed.harness_pub),
        key: parsed.harness_pub,
        name: m.name.unwrap_or(accept.name),
        relay,
        platform: info.platform,
        version: info.version,
        paired_at_ms: now_ms(),
        last_seen_ms: 0,
        revoke_pending: Vec::new(),
        revoked: false,
    };
    let node = row.node(false, 0);
    ctx.lock()
        .store
        .insert(row)
        .map_err(|e| format!("nodes.yaml: {e}"))?;
    ctx.rt
        .publish(NodeEvent::NodeAdded { node: node.clone() })
        .await;
    start_control(&ctx, node.id.clone());
    Ok(node)
}

/// A one-off mode-0 client for a node without a live control conn.
async fn fresh_rpc(ctx: &Ctx, relay: &str, key: &[u8; 32]) -> Result<Rpc, String> {
    let mut sock = conn::open(relay, key).await.map_err(|e| format!("{e:#}"))?;
    if conn::presence(&mut sock).await != Some(true) {
        return Err("offline".into());
    }
    let (transport, _) = conn::handshake(&mut sock, &ctx.identity, key, 0)
        .await
        .map_err(|e| e.to_string())?;
    Ok(Rpc::new(Channel::new(sock, transport)))
}

/// `Remove` off the mailbox: self-revoke on the node (best effort), then row,
/// task, registration, event.
async fn remove(ctx: Arc<Ctx>, id: String) -> Result<(), String> {
    let (name, live, relay, key) = {
        let mut st = ctx.lock();
        let row = st
            .store
            .by_id(&id)
            .ok_or_else(|| format!("unknown node {id}"))?;
        let (name, relay, key) = (row.name.clone(), row.relay.clone(), row.key);
        (name, st.live.remove(&id), relay, key)
    };
    let our = link::devices::device_id(&ctx.identity.public);
    let revoke = async {
        let rpc = match live.as_ref().and_then(|l| l.rpc.clone()) {
            Some(r) => r,
            None => Arc::new(fresh_rpc(&ctx, &relay, &key).await?),
        };
        rpc.link_revoke_device(&our).await
    };
    match tokio::time::timeout(REVOKE_TIMEOUT, revoke).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::info!("node {name}: self-revoke skipped: {e}"),
        Err(_) => tracing::info!("node {name}: self-revoke timed out"),
    }
    ctx.lock()
        .store
        .remove(&id)
        .map_err(|e| format!("nodes.yaml: {e}"))?;
    if let Some(l) = live {
        l.cancel.cancel();
    }
    ctx.unregister(name).await;
    ctx.rt.publish(NodeEvent::NodeRemoved { id }).await;
    Ok(())
}

impl Message<Status> for NodesActor {
    type Reply = Vec<Node>;

    async fn handle(&mut self, _: Status, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let st = self.ctx.lock();
        let nodes: Vec<Node> = st.store.rows().iter().map(|r| Ctx::node(&st, r)).collect();
        drop(st);
        nodes
    }
}

impl Message<Add> for NodesActor {
    type Reply = DelegatedReply<Result<Node, String>>;

    async fn handle(&mut self, m: Add, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let c = self.ctx.clone();
        // Relay round trips: off the mailbox.
        tokio::spawn(async move { tx.send(add(c, m).await) });
        reply
    }
}

impl Message<Remove> for NodesActor {
    type Reply = DelegatedReply<Result<(), String>>;

    async fn handle(&mut self, m: Remove, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let c = self.ctx.clone();
        tokio::spawn(async move { tx.send(remove(c, m.id).await) });
        reply
    }
}

impl Message<Rename> for NodesActor {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: Rename, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let name = m.name.trim();
        if name.is_empty() {
            return Err("name must not be empty".into());
        }
        let old = {
            let mut st = self.ctx.lock();
            if st
                .store
                .rows()
                .iter()
                .any(|r| r.name == name && r.id != m.id)
            {
                return Err(format!("{name} is already a node"));
            }
            let row = st
                .store
                .by_id_mut(&m.id)
                .ok_or_else(|| format!("unknown node {}", m.id))?;
            let old = std::mem::replace(&mut row.name, name.to_owned());
            st.store.save().map_err(|e| format!("nodes.yaml: {e}"))?;
            old
        };
        // The model's namespace follows the alias.
        let c = self.ctx.clone();
        tokio::spawn(async move {
            c.unregister(old).await;
            c.register(&m.id).await;
        });
        Ok(())
    }
}

impl Message<Invite> for NodesActor {
    type Reply = DelegatedReply<Result<link::Pairing, String>>;

    async fn handle(&mut self, m: Invite, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let target = {
            let st = self.ctx.lock();
            st.store.by_id(&m.id).map(|row| {
                (
                    row.name.clone(),
                    st.live.get(&m.id).and_then(|l| l.rpc.clone()),
                )
            })
        };
        tokio::spawn(async move {
            tx.send(match target {
                None => Err(format!("unknown node {}", m.id)),
                Some((name, None)) => Err(format!("{name} is offline")),
                Some((_, Some(rpc))) => rpc.link_pair().await,
            });
        });
        reply
    }
}

impl Message<LinkEvent> for NodesActor {
    type Reply = ();

    async fn handle(&mut self, e: LinkEvent, _: &mut Context<Self, Self::Reply>) {
        let LinkEvent::LinkRevoked { id: device } = e else {
            return;
        };
        // Revoke cascade (32 §8.4): online nodes now, offline ones on their next connect.
        let targets: Vec<(String, Option<Arc<Rpc>>)> = {
            let st = self.ctx.lock();
            st.store
                .rows()
                .iter()
                .map(|r| (r.id.clone(), st.live.get(&r.id).and_then(|l| l.rpc.clone())))
                .collect()
        };
        for (node, rpc) in targets {
            match rpc {
                None => self.ctx.defer_revoke(&node, &device),
                Some(rpc) => {
                    let c = self.ctx.clone();
                    let device = device.clone();
                    tokio::spawn(async move {
                        if let Err(e) = rpc.link_revoke_device(&device).await {
                            tracing::info!("node {node}: revoke {device}: {e}; queued");
                            c.defer_revoke(&node, &device);
                        }
                    });
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl runtime::Plugin for NodesPlugin {
    fn name(&self) -> &'static str {
        "nodes"
    }

    async fn declare_wire(&mut self, rt: &runtime::Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await?;
        let cfg: NodesConfig = rt.section("nodes").unwrap_or_else(|e| {
            tracing::debug!("no [nodes] config section, using defaults: {e}");
            NodesConfig::default()
        });
        if !cfg.enabled {
            tracing::info!("nodes disabled by config");
            return Ok(());
        }
        let identity = match Identity::load_or_create(&rt.data_dir.join("link")) {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!("link identity unavailable: {e:#}; nodes stay off");
                return Ok(());
            }
        };
        // The name a node lists us under (32 §4.1): link's display name or the hostname.
        let link_cfg: link::LinkConfig = rt.section("link").unwrap_or_default();
        let hello_name = if link_cfg.name.is_empty() {
            platform::process::hostname().to_owned()
        } else {
            link_cfg.name
        };
        let ctx = Arc::new(Ctx {
            rt: rt.clone(),
            identity: Arc::new(identity),
            hello_name,
            state: Mutex::new(State {
                store: Store::load(&rt.data_dir),
                live: HashMap::new(),
            }),
        });
        let actor = NodesActor::spawn_with_mailbox(
            NodesActor { ctx: ctx.clone() },
            runtime::subscriber_mailbox(),
        );
        actor.register(NODES_ACTOR)?;
        rt.subscribe::<NodesActor, LinkEvent>(&actor).await;
        let ids: Vec<String> = ctx
            .lock()
            .store
            .rows()
            .iter()
            .map(|r| r.id.clone())
            .collect();
        for id in ids {
            start_control(&ctx, id);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn our() -> [u8; 32] {
        [9u8; 32]
    }

    #[test]
    fn add_refuses_own_key() {
        let key = link::pairing::build("wss://relay.example", &our(), &[2; 32], "Mac", None);
        let e = precheck(&our(), &[], &BTreeMap::new(), &key, None).unwrap_err();
        assert_eq!(e, "this key is our own harness");
    }

    #[test]
    fn add_refuses_malformed_key() {
        let e = precheck(&our(), &[], &BTreeMap::new(), "hello there", None).unwrap_err();
        assert!(e.starts_with("not a pairing key"), "{e}");
        let e = precheck(&our(), &[], &BTreeMap::new(), "1.AAAA.bm9kZQ", None).unwrap_err();
        assert!(e.starts_with("not a pairing key"), "{e}");
    }

    #[test]
    fn add_refuses_agents_yaml_clash_unless_renamed() {
        let key = link::pairing::build("wss://relay.example", &[1; 32], &[2; 32], "codex", None);
        let mut agents = BTreeMap::new();
        agents.insert(
            "codex".to_owned(),
            acp::Entry {
                command: "codex".into(),
                args: vec![],
                env: BTreeMap::new(),
                description: None,
                model: None,
                skills: None,
                skills_dir: None,
                mcp: None,
                reports: None,
                steering: Default::default(),
            },
        );
        let e = precheck(&our(), &[], &agents, &key, None).unwrap_err();
        assert_eq!(
            e,
            "codex is an external harness in agents.yaml; rename with name:"
        );
        let (p, name) = precheck(&our(), &[], &agents, &key, Some("box")).unwrap();
        assert_eq!(
            (p.relay_host.as_str(), name.as_str()),
            ("relay.example", "box")
        );
    }

    #[test]
    fn describe_follows_presence() {
        let mut row = Row {
            id: "x".into(),
            key: [1; 32],
            name: "n".into(),
            relay: "wss://r".into(),
            platform: String::new(),
            version: String::new(),
            paired_at_ms: 0,
            last_seen_ms: 0,
            revoke_pending: vec![],
            revoked: false,
        };
        assert_eq!(describe(&row, false), "offline");
        row.platform = "linux".into();
        row.version = "0.4.1".into();
        assert_eq!(describe(&row, true), "linux · theuth 0.4.1");
        assert_eq!(describe(&row, false), "linux · offline");
    }
}
