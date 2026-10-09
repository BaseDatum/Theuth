//! Link/connect supervisor, inbound envelopes → agent asks, agent events →
//! outbound text / reactions / typing (design doc 09 §3.1, §5–8, §10).
//!
//! Shape: [`spawn`] runs one supervisor task (linking phase, then a
//! connected phase over `transport.incoming()`), two ordered worker tasks
//! (envelopes, bus events) and a small bus-subscriber actor feeding the
//! event worker. [`Handle`] is what the `"signal"` actor in `lib.rs` talks to.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use agent::{
    AGENT_ACTOR, AgentEvent, Agents, Ancestors, NewRootSession, PeerScope, SendInput, Steer,
};
use artifact::ArtifactShown;
use async_trait::async_trait;
use futures::StreamExt;
use kameo::Actor;
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use runtime::{InputId, Runtime, SessionId};
use std::sync::{Mutex, MutexGuard, PoisonError};
use tokio::sync::{mpsc, oneshot};
use tokio::task::{AbortHandle, JoinHandle};
use tokio::time::Instant;
use tools::{
    APPROVAL_ACTOR, Answer, Approval, ApprovalId, ApprovalRequested, ApprovalResolved, AskId,
    AskRequested, AskResolved, Decision, Resolve,
};
use uuid::Uuid;

use crate::commands::{self, Cx, Reply};
use crate::transport::{IncomingAttachment, Recipient, SignalEnvelope, SignalTransport};
use crate::{SIGNAL_ACTOR, SignalConfig, SignalEvent, SignalStatus, approvals, asks, text};

/// How long the Signal server keeps a provisioning URL scannable.
pub const LINK_URL_TTL: Duration = Duration::from_secs(90);
/// Pause between an expired provisioning URL and the next one.
pub const LINK_CHAIN_PAUSE: Duration = Duration::from_secs(3);
/// After this many unscanned URLs in a row, rotate at [`LINK_SLOW_PAUSE`].
pub const MAX_FAST_LINK_ATTEMPTS: u32 = 10;
/// Slow rotation cadence once nobody is scanning.
pub const LINK_SLOW_PAUSE: Duration = Duration::from_secs(300);
/// A connection that lived this long resets the reconnect backoff.
pub const STABLE_CONNECTION: Duration = Duration::from_secs(30);
/// Extra `send_text` attempts per chunk after the first failure.
pub const SEND_RETRIES: u32 = 2;
/// Pause between send attempts.
pub const RETRY_DELAY: Duration = Duration::from_millis(200);
/// Typing "started" is re-sent this often while a turn runs (clients clear
/// it after ~15 s).
pub const TYPING_REFRESH_INTERVAL: Duration = Duration::from_secs(10);
/// A failed number → ACI lookup is not retried before this.
pub const ACI_RESOLVE_COOLDOWN: Duration = Duration::from_secs(300);

/// Reconnect backoff: `min(2^min(attempt, 6), 60)` seconds.
#[must_use]
pub fn backoff_delay(attempt: u32) -> Duration {
    Duration::from_secs((1u64 << attempt.min(6)).min(60))
}

/// The 37 §7 line, plain text: where to enter a requested secret. Never answerable here.
#[must_use]
pub fn secret_notice(name: &str, note: &str) -> String {
    format!(
        "theuth asks for secret {name} — {note}. Enter it in the desktop app, the TUI (/secrets add {name}) or `t --secret-set {name}`; it will not be read from this chat."
    )
}

/// Strips ` -.()` and prefixes `+` when what remains is all digits, so
/// `"+1 (409) 519-3333"` and `"14095193333"` both match `"+14095193333"`.
#[must_use]
pub fn normalize_number(s: &str) -> String {
    let stripped: String = s
        .trim()
        .chars()
        .filter(|c| !matches!(c, ' ' | '-' | '.' | '(' | ')'))
        .collect();
    if !stripped.is_empty() && stripped.chars().all(|c| c.is_ascii_digit()) {
        format!("+{stripped}")
    } else {
        stripped
    }
}

/// One bound thread: the session plus the contact's ACI once a message from
/// it has been seen, so replies never depend on the presage contact store.
/// 67 §10: `agent` is who the contact is talking to; a Signal thread is per
/// contact (not per message), so the binding holds — every message goes to
/// that agent's task — until an explicit `theuth …` message.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Thread {
    /// The live session; `None` when bound to an agent that has no desk yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionId>,
    /// The contact's Signal account UUID, from the last inbound envelope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aci: Option<Uuid>,
    /// The roster agent this contact addressed (67 §10); `None` = theuth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// `agent`'s title, the outbound prefix (`"<Title> — "`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_title: Option<String>,
    /// The task the thread's messages are appended to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

/// On-disk value; files written before ACIs were kept hold the bare session id.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum StoredThread {
    Session(SessionId),
    Thread(Thread),
}

/// Contact → live session (+ ACI), persisted at `<data_dir>/signal/threads.json`
/// on every change (09 §6).
#[derive(Debug)]
pub struct Threads {
    path: PathBuf,
    map: BTreeMap<String, Thread>,
}

impl Threads {
    /// Reads the file; missing or corrupt → empty.
    #[must_use]
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("signal").join("threads.json");
        let map = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<BTreeMap<String, StoredThread>>(&bytes).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|(n, t)| {
                let t = match t {
                    StoredThread::Session(session) => Thread {
                        session: Some(session),
                        ..Thread::default()
                    },
                    StoredThread::Thread(t) => t,
                };
                (n, t)
            })
            .collect();
        Self { path, map }
    }

    fn save(&self) {
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let result = serde_json::to_vec_pretty(&self.map)
            .map_err(std::io::Error::other)
            .and_then(|bytes| std::fs::write(&self.path, bytes));
        if let Err(e) = result {
            tracing::warn!(path = %self.path.display(), error = %e, "failed to save signal threads");
        }
    }

    /// The session bound to `number`.
    #[must_use]
    pub fn get(&self, number: &str) -> Option<SessionId> {
        self.map
            .get(&normalize_number(number))
            .and_then(|t| t.session)
    }

    /// Whether some number's thread is `session`.
    #[must_use]
    pub fn is_bound(&self, session: SessionId) -> bool {
        self.map.values().any(|t| t.session == Some(session))
    }

    /// Whom to send to for `session`: its number plus the ACI when known.
    #[must_use]
    pub fn recipient_for(&self, session: SessionId) -> Option<Recipient> {
        self.map
            .iter()
            .find(|(_, t)| t.session == Some(session))
            .map(|(n, t)| Recipient {
                number: n.clone(),
                uuid: t.aci,
            })
    }

    /// `number` as a recipient, with the ACI its thread learned, if any.
    #[must_use]
    pub fn recipient(&self, number: &str) -> Recipient {
        Recipient {
            uuid: self.map.get(&normalize_number(number)).and_then(|t| t.aci),
            number: number.to_owned(),
        }
    }

    /// Binds `who` to `session` for theuth (any agent binding is dropped),
    /// keeps a previously learned ACI when `who` has none, and saves if
    /// anything changed.
    pub fn bind(&mut self, who: &Recipient, session: SessionId) {
        let key = normalize_number(&who.number);
        let aci = who.uuid.or_else(|| self.map.get(&key).and_then(|t| t.aci));
        let thread = Thread {
            session: Some(session),
            aci,
            ..Thread::default()
        };
        if self.map.get(&key) != Some(&thread) {
            self.map.insert(key, thread);
            self.save();
        }
    }

    /// Binds `who` to `agent` (67 §10): `session` = the agent's desk when
    /// given, else the thread keeps the one it had; `task` replaces the old.
    pub fn bind_agent(
        &mut self,
        who: &Recipient,
        session: Option<SessionId>,
        agent: String,
        title: String,
        task: Option<String>,
    ) {
        let key = normalize_number(&who.number);
        let old = self.map.get(&key);
        let thread = Thread {
            session: session.or_else(|| old.and_then(|t| t.session)),
            aci: who.uuid.or_else(|| old.and_then(|t| t.aci)),
            agent: Some(agent),
            agent_title: Some(title),
            task,
        };
        if old != Some(&thread) {
            self.map.insert(key, thread);
            self.save();
        }
    }

    /// Back to theuth: drops `number`'s agent, title and task; the session
    /// stays (today's path rebinds it on the next reply).
    pub fn clear_agent(&mut self, number: &str) {
        if let Some(t) = self.map.get_mut(&normalize_number(number))
            && t.agent.is_some()
        {
            t.agent = None;
            t.agent_title = None;
            t.task = None;
            self.save();
        }
    }

    /// `number`'s agent binding: `(agent, title, task)`; title = agent when unset.
    #[must_use]
    pub fn agent_of(&self, number: &str) -> Option<(&str, &str, Option<&str>)> {
        let t = self.map.get(&normalize_number(number))?;
        let agent = t.agent.as_deref()?;
        Some((
            agent,
            t.agent_title.as_deref().unwrap_or(agent),
            t.task.as_deref(),
        ))
    }

    /// Drops `number`'s binding and saves.
    pub fn unbind(&mut self, number: &str) {
        if self.map.remove(&normalize_number(number)).is_some() {
            self.save();
        }
    }

    /// Every session binding.
    #[must_use]
    pub fn all(&self) -> Vec<(String, SessionId)> {
        self.map
            .iter()
            .filter_map(|(n, t)| Some((n.clone(), t.session?)))
            .collect()
    }
}

/// The asks the channel makes of other plugins, behind a trait so the
/// inbound/outbound paths are testable without live actors.
#[async_trait]
trait Port: Send + Sync {
    /// An empty root with an explicit scope (50 §2.3); only for an
    /// authenticated owner, whose thread roots are `owner` from birth.
    async fn new_root(&self, scope: PeerScope) -> Result<SessionId, String>;
    async fn send_input(
        &self,
        session: Option<SessionId>,
        text: String,
        files: Vec<PathBuf>,
    ) -> Result<SessionId, String>;
    async fn steer(
        &self,
        session: SessionId,
        id: InputId,
        text: String,
        files: Vec<PathBuf>,
    ) -> Result<(), String>;
    async fn resolve(&self, id: ApprovalId, decision: Decision) -> Result<bool, String>;
    async fn answer_ask(&self, id: AskId, answers: Option<Vec<Answer>>) -> Result<bool, String>;
    /// `session`, then its parent, …, the root (24 §2); `[session]` when unknown.
    async fn ancestors(&self, session: SessionId) -> Result<Vec<SessionId>, String>;
}

/// [`Port`] over the registered `agent` and `approval` actors.
struct Actors;

fn agents() -> Result<ActorRef<Agents>, String> {
    ActorRef::<Agents>::lookup(AGENT_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(|| "agent actor is not running".to_string())
}

#[async_trait]
impl Port for Actors {
    async fn new_root(&self, scope: PeerScope) -> Result<SessionId, String> {
        agents()?
            .ask(NewRootSession { cwd: None, scope })
            .await
            .map_err(|e| e.to_string())
    }

    async fn send_input(
        &self,
        session: Option<SessionId>,
        text: String,
        files: Vec<PathBuf>,
    ) -> Result<SessionId, String> {
        agents()?
            .ask(SendInput {
                session,
                text,
                files,
                delivery: agent::Delivery::default(),
                id: None,
            })
            .await
            .map_err(|e| e.to_string())
    }

    async fn steer(
        &self,
        session: SessionId,
        id: InputId,
        text: String,
        files: Vec<PathBuf>,
    ) -> Result<(), String> {
        agents()?
            .ask(Steer {
                session,
                id,
                text,
                files,
                delivery: agent::Delivery::default(),
            })
            .await
            .map_err(|e| e.to_string())
    }

    async fn resolve(&self, id: ApprovalId, decision: Decision) -> Result<bool, String> {
        ActorRef::<Approval>::lookup(APPROVAL_ACTOR)
            .ok()
            .flatten()
            .ok_or_else(|| "approval actor is not running".to_string())?
            .ask(Resolve { id, decision })
            .await
            .map_err(|e| e.to_string())
    }

    async fn answer_ask(&self, id: AskId, answers: Option<Vec<Answer>>) -> Result<bool, String> {
        agent::answer_ask(id, answers)
            .await
            .map_err(|e| e.to_string())
    }

    async fn ancestors(&self, session: SessionId) -> Result<Vec<SessionId>, String> {
        agents()?
            .ask(Ancestors { session })
            .await
            .map(|chain| chain.into_iter().map(|a| a.id).collect())
            .map_err(|e| e.to_string())
    }
}

/// Bus events the channel renders, in bus order.
enum Event {
    Agent(AgentEvent),
    Requested(ApprovalRequested),
    Resolved(ApprovalResolved),
    AskRequested(AskRequested),
    AskResolved(AskResolved),
    Artifact(ArtifactShown),
    Secret(secrets::SecretsEvent),
    /// 67 §4.7: the hub lease moved; answering follows it.
    Lease(mesh::LeaseState),
    /// 126 §10: the double's prediction for a pending card.
    Predicted(double::DoublePredicted),
    /// 126 §10: the double answered a card at L2.
    DoubleAnswered(double::DoubleAnswered),
}

/// Bus subscriber: forwards into the ordered event worker.
#[derive(Actor)]
struct Bus {
    tx: mpsc::UnboundedSender<Event>,
}

impl Message<mesh::MeshEvent> for Bus {
    type Reply = ();
    async fn handle(&mut self, m: mesh::MeshEvent, _: &mut Context<Self, ()>) {
        if let mesh::MeshEvent::Lease(lease) = m {
            let _ = self.tx.send(Event::Lease(lease));
        }
    }
}

impl Message<AgentEvent> for Bus {
    type Reply = ();
    async fn handle(&mut self, m: AgentEvent, _: &mut Context<Self, ()>) {
        let _ = self.tx.send(Event::Agent(m));
    }
}

impl Message<ApprovalRequested> for Bus {
    type Reply = ();
    async fn handle(&mut self, m: ApprovalRequested, _: &mut Context<Self, ()>) {
        let _ = self.tx.send(Event::Requested(m));
    }
}

impl Message<ApprovalResolved> for Bus {
    type Reply = ();
    async fn handle(&mut self, m: ApprovalResolved, _: &mut Context<Self, ()>) {
        let _ = self.tx.send(Event::Resolved(m));
    }
}

impl Message<AskRequested> for Bus {
    type Reply = ();
    async fn handle(&mut self, m: AskRequested, _: &mut Context<Self, ()>) {
        let _ = self.tx.send(Event::AskRequested(m));
    }
}

impl Message<AskResolved> for Bus {
    type Reply = ();
    async fn handle(&mut self, m: AskResolved, _: &mut Context<Self, ()>) {
        let _ = self.tx.send(Event::AskResolved(m));
    }
}

impl Message<ArtifactShown> for Bus {
    type Reply = ();
    async fn handle(&mut self, m: ArtifactShown, _: &mut Context<Self, ()>) {
        let _ = self.tx.send(Event::Artifact(m));
    }
}

impl Message<double::DoublePredicted> for Bus {
    type Reply = ();
    async fn handle(&mut self, m: double::DoublePredicted, _: &mut Context<Self, ()>) {
        let _ = self.tx.send(Event::Predicted(m));
    }
}

impl Message<double::DoubleAnswered> for Bus {
    type Reply = ();
    async fn handle(&mut self, m: double::DoubleAnswered, _: &mut Context<Self, ()>) {
        let _ = self.tx.send(Event::DoubleAnswered(m));
    }
}

impl Message<secrets::SecretsEvent> for Bus {
    type Reply = ();
    async fn handle(&mut self, m: secrets::SecretsEvent, _: &mut Context<Self, ()>) {
        let _ = self.tx.send(Event::Secret(m));
    }
}

/// Requests from [`Handle`] to the supervisor.
enum Control {
    Link(oneshot::Sender<Result<String, String>>),
    Unlink(oneshot::Sender<Result<(), String>>),
}

/// Sealed-sender attribution (09 §5.2): ACI → configured number.
#[derive(Default)]
struct AciDirectory {
    by_aci: HashMap<Uuid, String>,
    failed_at: HashMap<String, Instant>,
}

/// Per-session outbound state between `TurnStarted` and `TurnFinished`.
/// `text` is the *unflushed* markdown (45 §4.5.3).
#[derive(Default)]
struct Turn {
    in_flight: bool,
    text: String,
    /// User messages (author, ts) waiting for the terminal reaction (§8.2).
    marks: Vec<(Recipient, u64)>,
    typing: Option<AbortHandle>,
    /// The last input was a voice note: flushed blocks also go out as clips (45 §4.5.2).
    voice: bool,
    /// The previous block's clip send; the next clip waits for it (45 §4.5.4).
    clip_tail: Option<JoinHandle<()>>,
    /// The turn's first block already carries the agent's `"<Title> — "` (67 §10).
    prefixed: bool,
}

pub(crate) struct State {
    allowed: Vec<String>,
    pub(crate) threads: Threads,
    aci: AciDirectory,
    turns: HashMap<SessionId, Turn>,
    last_event: Option<SignalEvent>,
    link_url: Option<String>,
    /// 67 decision 22: only the hub answers; the lease says who that is.
    pub(crate) lease: roster::lease::Follower,
}

pub(crate) struct Channel {
    pub(crate) transport: Arc<dyn SignalTransport>,
    port: Arc<dyn Port>,
    rt: Runtime,
    pub(crate) owner: String,
    device_name: String,
    control: mpsc::UnboundedSender<Control>,
    // ponytail: one std mutex for all state, never held across an await; split if it ever contends.
    state: Mutex<State>,
    /// Async lock: `commands::handle` needs `&mut Pending` across its asks.
    approvals: tokio::sync::Mutex<approvals::Pending>,
    /// Open `ask_user` questions per number.
    asks: tokio::sync::Mutex<asks::Pending>,
}

/// Control handle the `"signal"` actor uses. Cheap to clone.
#[derive(Clone)]
pub struct Handle {
    pub(crate) chan: Arc<Channel>,
}

impl Handle {
    /// The current provisioning URL (starting the linking phase if it is
    /// paused), or `Err("already linked as <number>")`.
    pub async fn link(&self) -> Result<String, String> {
        self.chan.control_link().await
    }

    /// The 09 §4 snapshot.
    pub async fn status(&self) -> SignalStatus {
        self.chan.status().await
    }

    /// Adds `number` to the live allowlist (the config file is `lib.rs`'s job).
    pub async fn allow(&self, number: String) -> Result<(), String> {
        let number = normalize_number(&number);
        if number.is_empty() {
            return Err("not a phone number".into());
        }
        if number == self.chan.owner {
            return Err(format!("{number} is the owner"));
        }
        let mut s = self.chan.lock();
        if !s.allowed.contains(&number) {
            s.allowed.push(number);
        }
        Ok(())
    }

    /// Removes `number` from the live allowlist.
    pub async fn remove_allowed(&self, number: String) -> Result<(), String> {
        let number = normalize_number(&number);
        let mut s = self.chan.lock();
        let before = s.allowed.len();
        s.allowed.retain(|n| *n != number);
        if s.allowed.len() == before {
            return Err(format!("{number} is not in the allowed list"));
        }
        Ok(())
    }

    /// Stops the pump, deletes `presage.db`, publishes `Unlinked` and
    /// re-enters the linking phase. Threads are kept.
    pub async fn unlink(&self) -> Result<(), String> {
        self.chan.control_unlink().await
    }
}

/// Spawns the supervisor under `rt.shutdown`; returns immediately.
pub fn spawn(transport: Arc<dyn SignalTransport>, cfg: SignalConfig, rt: Runtime) -> Handle {
    spawn_with(transport, cfg, rt, Arc::new(Actors))
}

fn spawn_with(
    transport: Arc<dyn SignalTransport>,
    cfg: SignalConfig,
    rt: Runtime,
    port: Arc<dyn Port>,
) -> Handle {
    let (tx, rx) = mpsc::unbounded_channel();
    let chan = Arc::new(Channel::new(transport, cfg, rt, port, tx));
    tokio::spawn(run(Arc::clone(&chan), rx));
    Handle { chan }
}

/// An unstarted channel over `transport` (no supervisor task); for the
/// outbound tests in `user.rs`.
#[cfg(test)]
pub(crate) fn unstarted(
    transport: Arc<dyn SignalTransport>,
    cfg: SignalConfig,
    rt: Runtime,
) -> Handle {
    let (tx, _rx) = mpsc::unbounded_channel();
    Handle {
        chan: Arc::new(Channel::new(transport, cfg, rt, Arc::new(Actors), tx)),
    }
}

/// Epoch milliseconds (URL expiry stamps).
fn now_ms() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or_default()
}

const STOPPED: &str = "signal channel stopped";
/// Caption on an app's source attachment (46 §8.2): the file, not controls.
const APP_NOTE: &str = "Interactive app — open on desktop/web";

/// Supervisor: linking phase ↔ connected phase until shutdown (09 §3.1).
async fn run(chan: Arc<Channel>, mut ctrl: mpsc::UnboundedReceiver<Control>) {
    let (env_tx, mut env_rx) = mpsc::unbounded_channel::<SignalEnvelope>();
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel::<Event>();
    let bus = Bus::spawn_with_mailbox(Bus { tx: ev_tx }, runtime::subscriber_mailbox());
    chan.rt.subscribe::<Bus, AgentEvent>(&bus).await;
    chan.rt.subscribe::<Bus, ApprovalRequested>(&bus).await;
    chan.rt.subscribe::<Bus, ApprovalResolved>(&bus).await;
    chan.rt.subscribe::<Bus, AskRequested>(&bus).await;
    chan.rt.subscribe::<Bus, AskResolved>(&bus).await;
    chan.rt.subscribe::<Bus, ArtifactShown>(&bus).await;
    chan.rt.subscribe::<Bus, secrets::SecretsEvent>(&bus).await;
    chan.rt.subscribe::<Bus, mesh::MeshEvent>(&bus).await;
    chan.rt
        .subscribe::<Bus, double::DoublePredicted>(&bus)
        .await;
    chan.rt.subscribe::<Bus, double::DoubleAnswered>(&bus).await;
    // 67 §4.7: who "we" are, once the mesh is up; the follower re-reads the
    // last lease it saw against that id.
    let identity = {
        let chan = Arc::clone(&chan);
        tokio::spawn(async move {
            if let Some(mesh) = chan.rt.lookup_when::<mesh::Mesh>(mesh::MESH_ACTOR).await
                && let Ok(s) = mesh.ask(mesh::Status).await
                && let Some(holding) = chan.lock().lease.set_self(s.self_id.0)
            {
                tracing::info!(holding, "signal: hub duties re-evaluated for this engine");
            }
        })
    };
    let inbound = {
        let chan = Arc::clone(&chan);
        tokio::spawn(async move {
            while let Some(env) = env_rx.recv().await {
                chan.handle_envelope(env).await;
            }
        })
    };
    let events = {
        let chan = Arc::clone(&chan);
        tokio::spawn(async move {
            while let Some(ev) = ev_rx.recv().await {
                chan.handle_event(ev).await;
            }
        })
    };
    let mut skip_check = false;
    loop {
        if chan.linking(&mut ctrl, skip_check).await.is_none() {
            break;
        }
        match chan.connected(&mut ctrl, &env_tx).await {
            Some(force) => skip_check = force,
            None => break,
        }
    }
    drop(env_tx);
    inbound.abort();
    events.abort();
    identity.abort();
    bus.kill();
}

impl Channel {
    fn new(
        transport: Arc<dyn SignalTransport>,
        cfg: SignalConfig,
        rt: Runtime,
        port: Arc<dyn Port>,
        control: mpsc::UnboundedSender<Control>,
    ) -> Self {
        let owner = normalize_number(&cfg.owner);
        let allowed = cfg
            .allowed
            .iter()
            .map(|n| normalize_number(n))
            .filter(|n| !n.is_empty() && *n != owner)
            .collect();
        Self {
            transport,
            port,
            owner,
            device_name: cfg.device_name,
            control,
            state: Mutex::new(State {
                allowed,
                threads: Threads::load(&rt.data_dir),
                aci: AciDirectory::default(),
                turns: HashMap::new(),
                last_event: None,
                link_url: None,
                lease: roster::lease::Follower::default(),
            }),
            approvals: tokio::sync::Mutex::new(approvals::Pending::default()),
            asks: tokio::sync::Mutex::new(asks::Pending::default()),
            rt,
        }
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    async fn publish(&self, ev: SignalEvent) {
        self.lock().last_event = Some(ev.clone());
        self.rt.publish(ev).await;
    }

    async fn account_number(&self) -> Option<String> {
        self.transport
            .account()
            .await
            .ok()
            .flatten()
            .map(|r| r.number)
            .filter(|n| !n.is_empty())
    }

    async fn already_linked(&self) -> String {
        format!(
            "already linked as {}",
            self.account_number()
                .await
                .unwrap_or_else(|| "unknown".into())
        )
    }

    async fn status(&self) -> SignalStatus {
        let registered = self.transport.is_registered().await.unwrap_or(false);
        let number = self.account_number().await;
        let s = self.lock();
        SignalStatus {
            state: s.last_event.clone(),
            registered,
            number,
            owner: self.owner.clone(),
            allowed: s.allowed.clone(),
            threads: s.threads.all(),
        }
    }

    async fn control_link(&self) -> Result<String, String> {
        let (tx, rx) = oneshot::channel();
        self.control
            .send(Control::Link(tx))
            .map_err(|_| STOPPED.to_string())?;
        rx.await.map_err(|_| STOPPED.to_string())?
    }

    async fn control_unlink(&self) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.control
            .send(Control::Unlink(tx))
            .map_err(|_| STOPPED.to_string())?;
        rx.await.map_err(|_| STOPPED.to_string())?
    }

    // -- supervisor ------------------------------------------------------

    /// Linking phase: returns `Some` once registered, `None` on shutdown.
    /// `skip_check` (after an unlink) provisions without consulting
    /// `is_registered`, which the frozen transport keeps in memory.
    async fn linking(
        &self,
        ctrl: &mut mpsc::UnboundedReceiver<Control>,
        mut skip_check: bool,
    ) -> Option<()> {
        let mut fault: u32 = 0;
        let mut expired: u32 = 0;
        let mut waiters: Vec<oneshot::Sender<Result<String, String>>> = Vec::new();
        loop {
            if self.rt.shutdown.is_cancelled() {
                return None;
            }
            if !std::mem::take(&mut skip_check) {
                match self.transport.is_registered().await {
                    Ok(true) => {
                        let msg = self.already_linked().await;
                        for w in waiters.drain(..) {
                            let _ = w.send(Err(msg.clone()));
                        }
                        return Some(());
                    }
                    Ok(false) => {}
                    Err(e) => {
                        tracing::warn!(error = %e, "signal registration check failed");
                        self.sleep_linking(ctrl, backoff_delay(fault), &mut waiters)
                            .await?;
                        fault = fault.saturating_add(1);
                        continue;
                    }
                }
            }
            let attempt = match self.transport.link_device(&self.device_name).await {
                Ok(attempt) => attempt,
                Err(e) => {
                    tracing::warn!(error = %e, "signal provisioning could not start");
                    for w in waiters.drain(..) {
                        let _ = w.send(Err(format!("provisioning failed to start: {e}")));
                    }
                    self.sleep_linking(ctrl, backoff_delay(fault), &mut waiters)
                        .await?;
                    fault = fault.saturating_add(1);
                    continue;
                }
            };
            fault = 0;
            let url = attempt.url.0;
            let expires_at_ms = now_ms() + u64::try_from(LINK_URL_TTL.as_millis()).unwrap_or(0);
            tracing::warn!(%url, "signal provisioning required (scan from Signal → Settings → Linked devices)");
            self.lock().link_url = Some(url.clone());
            self.publish(SignalEvent::LinkUrl {
                url: url.clone(),
                expires_at_ms,
            })
            .await;
            for w in waiters.drain(..) {
                let _ = w.send(Ok(url.clone()));
            }
            let mut done = attempt.done;
            let outcome = loop {
                tokio::select! {
                    () = self.rt.shutdown.cancelled() => return None,
                    result = &mut done => break result,
                    c = ctrl.recv() => match c {
                        Some(Control::Link(reply)) => { let _ = reply.send(Ok(url.clone())); }
                        Some(Control::Unlink(reply)) => { let _ = reply.send(Err("not linked".into())); }
                        None => return None,
                    },
                }
            };
            self.lock().link_url = None;
            match outcome {
                Ok(()) => {
                    let number = self.account_number().await.unwrap_or_default();
                    tracing::info!(%number, "signal device linked");
                    self.publish(SignalEvent::Linked { number }).await;
                    return Some(());
                }
                Err(e) => {
                    expired = expired.saturating_add(1);
                    tracing::warn!(error = %e, attempts = expired, "signal provisioning URL expired unscanned; rotating");
                    let pause = if expired >= MAX_FAST_LINK_ATTEMPTS {
                        LINK_SLOW_PAUSE
                    } else {
                        LINK_CHAIN_PAUSE
                    };
                    self.sleep_linking(ctrl, pause, &mut waiters).await?;
                }
            }
        }
    }

    /// Sleeps in the linking phase. A `Link` request cuts the pause short
    /// (its reply is queued for the next URL); `Unlink` is refused.
    /// `None` on shutdown.
    async fn sleep_linking(
        &self,
        ctrl: &mut mpsc::UnboundedReceiver<Control>,
        delay: Duration,
        waiters: &mut Vec<oneshot::Sender<Result<String, String>>>,
    ) -> Option<()> {
        let sleep = tokio::time::sleep(delay);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                () = self.rt.shutdown.cancelled() => return None,
                () = &mut sleep => return Some(()),
                c = ctrl.recv() => match c {
                    Some(Control::Link(reply)) => { waiters.push(reply); return Some(()); }
                    Some(Control::Unlink(reply)) => { let _ = reply.send(Err("not linked".into())); }
                    None => return None,
                },
            }
        }
    }

    /// Connected phase: pumps `incoming()` into the envelope worker,
    /// reconnecting with backoff. Returns `Some(force_relink)` when the
    /// registration is gone (`true` after an explicit unlink), `None` on
    /// shutdown.
    async fn connected(
        &self,
        ctrl: &mut mpsc::UnboundedReceiver<Control>,
        env_tx: &mpsc::UnboundedSender<SignalEnvelope>,
    ) -> Option<bool> {
        let mut attempt: u32 = 0;
        loop {
            let mut stream = self.transport.incoming();
            self.publish(SignalEvent::Connected).await;
            let connected_at = Instant::now();
            let unlink = loop {
                tokio::select! {
                    () = self.rt.shutdown.cancelled() => return None,
                    env = stream.next() => match env {
                        Some(env) => {
                            attempt = 0;
                            if env_tx.send(env).is_err() {
                                return None;
                            }
                        }
                        None => break None,
                    },
                    c = ctrl.recv() => match c {
                        Some(Control::Link(reply)) => { let _ = reply.send(Err(self.already_linked().await)); }
                        Some(Control::Unlink(reply)) => break Some(reply),
                        None => return None,
                    },
                }
            };
            drop(stream);
            if let Some(reply) = unlink {
                self.unlink().await;
                let _ = reply.send(Ok(()));
                return Some(true);
            }
            if matches!(self.transport.is_registered().await, Ok(false)) {
                tracing::warn!("signal account no longer registered; back to linking");
                self.publish(SignalEvent::Unlinked).await;
                return Some(false);
            }
            if connected_at.elapsed() >= STABLE_CONNECTION {
                attempt = 0;
            }
            let delay = backoff_delay(attempt);
            attempt = attempt.saturating_add(1);
            if attempt == 1 {
                tracing::info!(
                    delay_s = delay.as_secs(),
                    "signal stream ended; reconnecting"
                );
            } else {
                tracing::warn!(
                    delay_s = delay.as_secs(),
                    attempt,
                    "signal stream disconnected; reconnecting"
                );
                self.publish(SignalEvent::Disconnected {
                    retry_in_secs: delay.as_secs(),
                })
                .await;
            }
            let sleep = tokio::time::sleep(delay);
            tokio::pin!(sleep);
            loop {
                tokio::select! {
                    () = self.rt.shutdown.cancelled() => return None,
                    () = &mut sleep => break,
                    c = ctrl.recv() => match c {
                        Some(Control::Link(reply)) => { let _ = reply.send(Err(self.already_linked().await)); }
                        Some(Control::Unlink(reply)) => {
                            self.unlink().await;
                            let _ = reply.send(Ok(()));
                            return Some(true);
                        }
                        None => return None,
                    },
                }
            }
        }
    }

    /// Deletes the presage store files and publishes `Unlinked` (09 §11.4).
    async fn unlink(&self) {
        let dir = self.rt.data_dir.join("signal");
        for name in ["presage.db", "presage.db-wal", "presage.db-shm"] {
            match tokio::fs::remove_file(dir.join(name)).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => tracing::warn!(file = name, error = %e, "failed to delete presage store"),
            }
        }
        tracing::info!("signal device unlinked; back to linking");
        self.publish(SignalEvent::Unlinked).await;
    }

    // -- trust -----------------------------------------------------------

    fn is_owner(&self, number: &str) -> bool {
        !self.owner.is_empty() && normalize_number(number) == self.owner
    }

    fn is_allowed(&self, number: &str) -> bool {
        if self.is_owner(number) {
            return true;
        }
        let number = normalize_number(number);
        !number.is_empty() && self.lock().allowed.contains(&number)
    }

    /// Rewrites a sealed-sender (ACI only) sender to its configured number
    /// via the [`AciDirectory`] (09 §5.2). Allowed senders come back
    /// normalised.
    async fn resolve_sender(&self, mut sender: Recipient) -> Recipient {
        if self.is_allowed(&sender.number) {
            sender.number = normalize_number(&sender.number);
            if let Some(aci) = sender.uuid {
                self.lock().aci.by_aci.insert(aci, sender.number.clone());
            }
            return sender;
        }
        let Some(aci) = sender.uuid else {
            return sender;
        };
        if let Some(n) = self.lock().aci.by_aci.get(&aci).cloned() {
            sender.number = n;
            return sender;
        }
        let candidates: Vec<String> = {
            let s = self.lock();
            std::iter::once(&self.owner)
                .chain(s.allowed.iter())
                .filter(|n| !n.is_empty())
                .filter(|n| !s.aci.by_aci.values().any(|v| v == *n))
                .filter(|n| {
                    s.aci
                        .failed_at
                        .get(*n)
                        .is_none_or(|at| at.elapsed() >= ACI_RESOLVE_COOLDOWN)
                })
                .cloned()
                .collect()
        };
        for number in candidates {
            match self.transport.resolve_uuid(&number).await {
                Ok(Some(uuid)) => {
                    tracing::info!(%number, %uuid, "resolved allowed signal number to ACI");
                    let mut s = self.lock();
                    s.aci.failed_at.remove(&number);
                    s.aci.by_aci.insert(uuid, number);
                }
                Ok(None) => {
                    tracing::info!(%number, "signal number has no known ACI; save it as a contact on the primary phone");
                    self.lock().aci.failed_at.insert(number, Instant::now());
                }
                Err(e) => {
                    tracing::debug!(%number, error = %e, "signal ACI resolution failed");
                    self.lock().aci.failed_at.insert(number, Instant::now());
                }
            }
        }
        if let Some(n) = self.lock().aci.by_aci.get(&aci).cloned() {
            sender.number = n;
        }
        sender
    }

    // -- sends -----------------------------------------------------------

    /// `send_text` with [`SEND_RETRIES`]; the sent timestamp on success.
    async fn send_text(&self, to: &Recipient, text: &str) -> Option<u64> {
        for attempt in 0..=SEND_RETRIES {
            match self.transport.send_text(to, text).await {
                Ok(ts) => return Some(ts),
                Err(e) => {
                    tracing::warn!(to = %to.number, attempt, error = %e, "signal send failed");
                    if attempt < SEND_RETRIES {
                        tokio::time::sleep(RETRY_DELAY).await;
                    }
                }
            }
        }
        None
    }

    /// `send_attachment` with [`SEND_RETRIES`]; `true` on success (24 §6).
    pub(crate) async fn send_attachment(
        &self,
        to: &Recipient,
        bytes: &[u8],
        mime: &str,
        file_name: Option<&str>,
        caption: Option<&str>,
    ) -> bool {
        for attempt in 0..=SEND_RETRIES {
            match self
                .transport
                .send_attachment(to, bytes, mime, file_name, caption)
                .await
            {
                Ok(()) => return true,
                Err(e) => {
                    tracing::warn!(to = %to.number, attempt, error = %e, "signal attachment send failed");
                    if attempt < SEND_RETRIES {
                        tokio::time::sleep(RETRY_DELAY).await;
                    }
                }
            }
        }
        false
    }

    /// Chunks and sends plain text; the last chunk's timestamp.
    pub(crate) async fn send_chunks(&self, to: &Recipient, plain: &str) -> Option<u64> {
        let mut last = None;
        for chunk in text::chunk_text(plain, text::MAX_CHUNK_CHARS) {
            last = self.send_text(to, &chunk).await;
        }
        last
    }

    async fn react(&self, to: &Recipient, emoji: &str, author: &Recipient, ts: u64) {
        if let Err(e) = self.transport.send_reaction(to, emoji, author, ts).await {
            tracing::debug!(error = %e, emoji, "failed to send reaction");
        }
    }

    async fn typing(&self, to: &Recipient, active: bool) {
        if let Err(e) = self.transport.send_typing(to, active).await {
            tracing::debug!(error = %e, "failed to send typing indicator");
        }
    }

    // -- inbound (09 §7) -------------------------------------------------

    async fn handle_envelope(&self, env: SignalEnvelope) {
        let sender = self.resolve_sender(env.sender).await;
        if !self.is_allowed(&sender.number) {
            tracing::warn!(
                sender = %if sender.number.is_empty() { "unknown" } else { sender.number.as_str() },
                aci = ?sender.uuid,
                "dropping signal message from a number not in the allowlist"
            );
            return;
        }
        // 67 decision 22: every engine stays linked and listening; only the
        // lease holder answers. Outbound (`UserSink::deliver`) is not gated: a
        // session running here that calls `message_user` still reaches the user.
        if !self.lock().lease.holding {
            tracing::debug!(sender = %sender.number, ts = env.timestamp, "not the hub; recorded, silent");
            return;
        }
        // 67 §4.7 answered markers: the first hub to mark it answers. Tasks
        // down (`None` / `Err`) must not mute Signal, so only `Ok(false)` stops.
        if let Some(tasks) = tasks::actor()
            && let Ok(false) = tasks
                .ask(tasks::MarkAnswered {
                    sender: sender.number.clone(),
                    timestamp: env.timestamp,
                })
                .await
        {
            tracing::debug!(sender = %sender.number, ts = env.timestamp, "already answered by another hub");
            return;
        }
        if let Err(e) = self.transport.send_receipt(&sender, env.timestamp).await {
            tracing::debug!(error = %e, "failed to send read receipt");
        }
        if let Some(reaction) = env.reaction {
            if reaction.remove {
                return;
            }
            let id = self
                .approvals
                .lock()
                .await
                .by_reaction(&sender.number, reaction.target_timestamp);
            if let (Some(id), Some(decision)) = (id, approvals::decision_for_emoji(&reaction.emoji))
                && let Err(e) = self.port.resolve(id, decision).await
            {
                tracing::warn!(error = %e, "approval resolve by reaction failed");
            }
            return;
        }
        let body = env.text.as_deref().map(str::trim).unwrap_or_default();
        if !body.is_empty()
            && !body.starts_with('/')
            && let Some(reply) = self.ask_reply(&sender.number, body).await
        {
            self.send_chunks(&sender, &reply).await;
            return;
        }
        if !body.is_empty() {
            let reply = {
                let status = self.status().await;
                let session = self.lock().threads.get(&sender.number);
                let mut pending = self.approvals.lock().await;
                let cx = Cx {
                    session,
                    is_owner: self.is_owner(&sender.number),
                    status,
                    approvals: &mut pending,
                };
                commands::handle(cx, body).await
            };
            if let Some(reply) = reply {
                self.apply_reply(&sender, reply).await;
                return;
            }
        }
        // 67 §10: `<agent>: …` (or a thread already bound to one) goes to that
        // agent's task; `theuth …` (or nothing) is today's path.
        let (addressee, body) = self.addressee(&sender.number, body).await;
        let mut lines: Vec<String> = Vec::new();
        if let Some(quoted) = env
            .quote_text
            .as_deref()
            .map(str::trim)
            .filter(|q| !q.is_empty())
        {
            lines.push(format!("(replying to: \"{quoted}\")"));
        }
        if !body.is_empty() {
            lines.push(body.to_owned());
        }
        let (attachment_lines, files, voice) =
            self.save_attachments(env.timestamp, &env.attachments).await;
        lines.extend(attachment_lines);
        if lines.is_empty() {
            return;
        }
        let input = lines.join("\n");
        self.react(&sender, "👀", &sender, env.timestamp).await;
        if let Addressee::Agent(who) = addressee {
            // The desk answers through the outbound path (its session is the
            // thread's); image paths ride in the attachment lines.
            self.tell_agent(&sender, env.timestamp, who, input).await;
            return;
        }
        let (session, in_flight) = {
            let s = self.lock();
            let session = s.threads.get(&sender.number);
            let in_flight = session.is_some_and(|id| s.turns.get(&id).is_some_and(|t| t.in_flight));
            (session, in_flight)
        };
        // Mid-turn text queues behind the running tool batch; the phone hears
        // `queued` now and the ✅ when the turn that took it finishes.
        // A new thread root is `owner` only when the owner opened it; every
        // other allowed contact gets the isolated default (50 §2.3).
        let result = match session {
            Some(id) if in_flight => {
                let queued = self
                    .port
                    .steer(id, InputId::new(), input, files)
                    .await
                    .map(|()| id);
                if queued.is_ok() {
                    self.send_chunks(&sender, "queued").await;
                }
                queued
            }
            None if self.is_owner(&sender.number) => {
                match self.port.new_root(PeerScope::Owner).await {
                    Ok(root) => self.port.send_input(Some(root), input, files).await,
                    Err(e) => Err(e),
                }
            }
            _ => self.port.send_input(session, input, files).await,
        };
        match result {
            Ok(id) => {
                let mut s = self.lock();
                s.threads.bind(&sender, id);
                // The entry outlives this call: `TurnStarted` picks it up, and a
                // steer updates the running turn in place (last input wins).
                let turn = s.turns.entry(id).or_default();
                turn.voice = voice;
                turn.marks.push((sender.clone(), env.timestamp));
            }
            Err(e) => {
                tracing::warn!(error = %e, "agent ask failed");
                self.send_chunks(&sender, &format!("⚠️ {e}")).await;
                self.react(&sender, "❌", &sender, env.timestamp).await;
            }
        }
    }

    /// Who `body` is for (67 §10): the named agent, the agent the thread is
    /// already bound to, or theuth. `theuth: …` rebinds the thread to theuth.
    /// Returns the text minus the address. Roster down → everything is theuth's.
    async fn addressee<'a>(&self, number: &str, body: &'a str) -> (Addressee, &'a str) {
        let agents = match roster::actor() {
            Some(r) => r.ask(roster::RoutableAgents).await.unwrap_or_default(),
            None => Vec::new(),
        };
        let mut names: Vec<String> = agents.iter().map(|a| a.name.clone()).collect();
        names.push("theuth".into());
        let bound = self
            .lock()
            .threads
            .agent_of(number)
            .map(|(a, t, k)| (a.to_owned(), t.to_owned(), k.map(str::to_owned)));
        let (name, rest) = match roster::routing::route(body, &names) {
            Some(r) if r.agent == "theuth" => {
                self.lock().threads.clear_agent(number);
                return (Addressee::Theuth, r.rest);
            }
            Some(r) => (r.agent, r.rest),
            None => match &bound {
                Some((a, ..)) => (a.clone(), body),
                None => return (Addressee::Theuth, body),
            },
        };
        let task = bound
            .as_ref()
            .filter(|(a, ..)| *a == name)
            .and_then(|(_, _, k)| k.clone());
        // A bound agent no longer routable (paused, gone) keeps its title and
        // task; its desk is whatever the thread still has.
        let (title, desk) = match agents.into_iter().find(|a| a.name == name) {
            Some(a) => (a.title, a.desk.and_then(|d| d.parse().ok())),
            None => (bound.map_or_else(|| name.clone(), |(_, t, _)| t), None),
        };
        (
            Addressee::Agent(Named {
                name,
                title,
                desk,
                task,
            }),
            rest,
        )
    }

    /// `text` to `name`'s task: the first message files it (the `assigned`
    /// wake carries the brief), later ones are mentions. Then the thread is
    /// bound to the agent and its desk; the desk's next turn closes the 👀.
    async fn tell_agent(&self, sender: &Recipient, ts: u64, who: Named, text: String) {
        let Named {
            name,
            title,
            desk,
            task,
        } = who;
        let result = match (tasks::actor(), task) {
            (None, _) => Err("tasks plugin is not running".to_owned()),
            (Some(tasks), Some(id)) => tasks
                .ask(tasks::Mention {
                    id: tasks::TaskId(id.clone()),
                    text,
                    actor: "user".into(),
                    session: None,
                })
                .await
                .map_err(|e| e.to_string())
                .map(|()| Some(id)),
            (Some(tasks), None) => tasks
                .ask(tasks::CreateTask {
                    input: tasks::TaskCreate {
                        state: tasks::TaskState::Todo,
                        title: first_line(&text),
                        brief: text,
                        owner: tasks::Owner::Agent(name.clone()),
                        reviewer: None,
                        parent: None,
                        project: None,
                        where_: None,
                        harness: None,
                        external: vec![],
                        done_when: None,
                        attachments: vec![],
                        priority: None,
                    },
                    created_by: "user".into(),
                    session: None,
                })
                .await
                .map_err(|e| e.to_string())
                .map(|t| Some(t.id.0)),
        };
        match result {
            Ok(task) => {
                let mut s = self.lock();
                s.threads.bind_agent(sender, desk, name, title, task);
                if let Some(desk) = desk {
                    s.turns
                        .entry(desk)
                        .or_default()
                        .marks
                        .push((sender.clone(), ts));
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, agent = %name, "task for the agent failed");
                self.send_chunks(sender, &format!("⚠️ {e}")).await;
                self.react(sender, "❌", sender, ts).await;
            }
        }
    }

    /// `body` as the answer to `number`'s open question, if one is waiting:
    /// the next question, the outcome of sending the answers, or a hint.
    async fn ask_reply(&self, number: &str, body: &str) -> Option<String> {
        let mut asks = self.asks.lock().await;
        let entry = asks.answerable(number)?;
        let answer = match asks::parse_reply(body, entry.question()) {
            Err(hint) => return Some(hint),
            Ok(asks::Reply::Dismiss) => {
                entry.done = true;
                let res = self.port.answer_ask(entry.req.id, None).await;
                return Some(asks::outcome(res, "🚫 dismissed; turn cancelled"));
            }
            Ok(asks::Reply::Pick(index)) => Answer::Choice { index },
            Ok(asks::Reply::Text(text)) => Answer::Text { text },
            Ok(asks::Reply::Skip) => Answer::Skipped,
        };
        if !entry.record(answer) {
            return Some(asks::question_text(&entry.req, entry.current));
        }
        entry.done = true;
        let res = self
            .port
            .answer_ask(entry.req.id, Some(entry.answers.clone()))
            .await;
        Some(asks::outcome(res, "✅ answers sent"))
    }

    /// Acts on a slash-command outcome (09 §9).
    async fn apply_reply(&self, sender: &Recipient, reply: Reply) {
        let text = match reply {
            Reply::Text(t) => t,
            Reply::NewThread => {
                self.lock().threads.unbind(&sender.number);
                "new thread; next message starts a session".into()
            }
            Reply::Resume(id) => {
                self.lock().threads.bind(sender, id);
                format!("resumed {id}")
            }
            Reply::Allow(n) => match ask_signal(crate::Allow(n.clone())).await {
                Ok(()) => format!("{n} allowed"),
                Err(e) => e,
            },
            Reply::RemoveAllowed(n) => match ask_signal(crate::RemoveAllowed(n.clone())).await {
                Ok(()) => format!("{n} removed"),
                Err(e) => e,
            },
            Reply::Unlink => match ask_signal(crate::Unlink).await {
                // Not registered any more: nothing to send.
                Ok(()) => return,
                Err(e) => e,
            },
            Reply::Link => match self.control_link().await {
                Ok(url) => url,
                Err(e) => e,
            },
        };
        self.send_chunks(sender, &text).await;
    }

    /// Writes attachments to `<data_dir>/signal/inbox/<ts>-<n>.<ext>` and
    /// returns one `[attachment: …]` line each (09 §7.4) plus the paths of
    /// the `image/*` ones and PDFs up to 32 MB, which ride along as model
    /// input (17 §5, 20 §2.5), plus whether the message was a voice note:
    /// audio is transcribed into a `[voice note, N s] …` line (45 §4.5.1).
    async fn save_attachments(
        &self,
        ts: u64,
        attachments: &[IncomingAttachment],
    ) -> (Vec<String>, Vec<PathBuf>, bool) {
        let mut lines = Vec::with_capacity(attachments.len());
        let mut images = Vec::new();
        let mut voice = false;
        if attachments.is_empty() {
            return (lines, images, voice);
        }
        let dir = self.rt.data_dir.join("signal").join("inbox");
        if let Err(e) = tokio::fs::create_dir_all(&dir).await {
            tracing::warn!(error = %e, "failed to create signal inbox dir");
            return (lines, images, voice);
        }
        for (n, a) in attachments.iter().enumerate() {
            let mime = effective_mime(&a.mime, &a.bytes);
            let path = dir.join(format!("{ts}-{n}.{}", extension_for(&mime)));
            if let Err(e) = tokio::fs::write(&path, &a.bytes).await {
                tracing::warn!(error = %e, path = %path.display(), "failed to save attachment");
                continue;
            }
            let mut detail = format!("{mime}, {}", fmt_size(a.bytes.len()));
            if a.voice_note {
                detail.push_str(", voice note");
                if let Some(ms) = a.duration_ms {
                    detail.push_str(&format!(", {} s", ms.div_ceil(1000)));
                }
            }
            let mut suffix = "";
            if a.voice_note || mime.starts_with("audio/") {
                let heard = match ActorRef::<voice::Transcriber>::lookup(voice::TRANSCRIBER_ACTOR) {
                    Ok(Some(stt)) => stt
                        .ask(voice::Transcribe { path: path.clone() })
                        .await
                        .map_err(|e| e.to_string()),
                    _ => Err("voice plugin is not running".to_owned()),
                };
                match heard {
                    Ok(t) if !t.text.trim().is_empty() => {
                        voice = true;
                        lines.push(voice::note_text(&t.text, t.duration_ms));
                        continue;
                    }
                    Ok(t) => {
                        voice = true;
                        lines.push(format!(
                            "[voice note, {} s] (nothing recognized)",
                            t.duration_ms.div_ceil(1000).max(1)
                        ));
                        continue;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, path = %path.display(), "voice note not transcribed");
                        suffix = " (transcription unavailable)";
                    }
                }
            }
            lines.push(format!(
                "[attachment: {} ({detail})]{suffix}",
                path.display()
            ));
            if mime.starts_with("image/")
                || (mime == "application/pdf" && a.bytes.len() <= 32 * 1024 * 1024)
            {
                images.push(path);
            }
        }
        (lines, images, voice)
    }

    // -- outbound (09 §8, §10) -------------------------------------------

    async fn handle_event(self: &Arc<Self>, ev: Event) {
        match ev {
            Event::Agent(ev) => self.handle_agent(ev).await,
            Event::Requested(req) => {
                let to = self.lock().threads.recipient_for(req.session);
                let Some(to) = to else { return };
                let code = self.approvals.lock().await.insert(&req, &to.number);
                if let Some(ts) = self
                    .send_chunks(&to, &approvals::message(&req, &code))
                    .await
                {
                    self.approvals.lock().await.mark_sent(&code, ts);
                }
            }
            Event::Resolved(res) => {
                let Some(entry) = self.approvals.lock().await.resolve(&res) else {
                    return;
                };
                let to = self.lock().threads.recipient(&entry.number);
                if let Some(ts) = entry.sent_ts {
                    match self.transport.account().await {
                        Ok(Some(me)) => {
                            self.react(&to, approvals::reaction_for(&res), &me, ts)
                                .await
                        }
                        _ => tracing::debug!("own account unknown; skipping approval reaction"),
                    }
                }
                if res.timed_out {
                    self.send_chunks(&to, &format!("⌛ {} expired", entry.code))
                        .await;
                }
            }
            Event::AskRequested(req) => {
                let to = self.lock().threads.recipient_for(req.session);
                let Some(to) = to else { return };
                let first = asks::question_text(&req, 0);
                if self.asks.lock().await.insert(req, &to.number) {
                    self.send_chunks(&to, &first).await;
                }
            }
            Event::AskResolved(res) => {
                let Some((entry, next)) = self.asks.lock().await.resolve(res.id) else {
                    return;
                };
                let to = self.lock().threads.recipient(&entry.number);
                if let Some(note) = asks::closing_note(&entry, &res) {
                    self.send_chunks(&to, note).await;
                }
                if let Some(req) = next {
                    self.send_chunks(&to, &asks::question_text(&req, 0)).await;
                }
            }
            // 126 §10: the double's line follows the card it is about, as its own text
            // (Signal messages are not edited); only for the card on the phone now.
            Event::Predicted(p) => {
                let to = p.session.and_then(|s| self.lock().threads.recipient_for(s));
                let Some(to) = to else { return };
                let shown = match p.kind {
                    double::CardKind::Approval => true,
                    double::CardKind::Ask => self
                        .asks
                        .lock()
                        .await
                        .answerable(&to.number)
                        .is_some_and(|e| e.req.id.0.to_string() == p.question_id),
                    _ => false,
                };
                if shown {
                    let now = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0);
                    self.send_chunks(&to, &format!("🤖 {}", double::chip::would(&p, now)))
                        .await;
                }
            }
            Event::DoubleAnswered(a) => {
                let to = a.session.and_then(|s| self.lock().threads.recipient_for(s));
                let Some(to) = to else { return };
                self.send_chunks(&to, &format!("🤖 {}", double::chip::answered(&a)))
                    .await;
            }
            // 35 §8: the session's number, else the nearest bound ancestor's. An
            // app (46 §8.2) is its `.openui` source, captioned with where it runs.
            Event::Artifact(m) => {
                let chain = match self.port.ancestors(m.session).await {
                    Ok(chain) => chain,
                    Err(e) => {
                        tracing::debug!(error = %e, "ancestors for the artifact failed; using the session alone");
                        vec![m.session]
                    }
                };
                let to = {
                    let s = self.lock();
                    chain.iter().find_map(|id| s.threads.recipient_for(*id))
                };
                let Some(to) = to else {
                    tracing::debug!(session = %m.session, name = %m.artifact.name, "artifact for an unbound session; not sent");
                    return;
                };
                let a = m.artifact;
                let bytes = match tokio::fs::read(&a.path).await {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::warn!(error = %e, path = %a.path.display(), "artifact not readable");
                        return;
                    }
                };
                let title = a.title.trim();
                let caption = match (a.kind, title.is_empty()) {
                    (artifact::Kind::App, true) => Some(APP_NOTE.to_owned()),
                    (artifact::Kind::App, false) => Some(format!("{title} · {APP_NOTE}")),
                    (_, true) => None,
                    (_, false) => Some(title.to_owned()),
                };
                let caption = caption.as_deref();
                if !self
                    .send_attachment(&to, &bytes, &a.mime, Some(&a.name), caption)
                    .await
                {
                    tracing::warn!(name = %a.name, to = %to.number, "artifact send failed");
                }
            }
            // 37 §7: the model asked the user for a secret; tell the owner where
            // to enter it (never here — the value would sit in Signal's history).
            Event::Secret(secrets::SecretsEvent::Requested { name, note, .. }) => {
                if self.owner.is_empty() {
                    return;
                }
                let to = self.lock().threads.recipient(&self.owner);
                self.send_chunks(&to, &secret_notice(&name, &note)).await;
            }
            Event::Secret(_) => {}
            Event::Lease(lease) => {
                if let Some(holding) = self.lock().lease.observe(&lease) {
                    tracing::info!(holding, holder = ?lease.holder, reason = %lease.reason, "signal: hub duties follow the lease");
                }
            }
        }
    }

    async fn handle_agent(self: &Arc<Self>, ev: AgentEvent) {
        match ev {
            AgentEvent::TurnStarted { session, .. } => {
                let to = {
                    let mut s = self.lock();
                    let Some(to) = s.threads.recipient_for(session) else {
                        return;
                    };
                    let turn = s.turns.entry(session).or_default();
                    turn.in_flight = true;
                    if let Some(h) = turn.typing.take() {
                        h.abort();
                    }
                    to
                };
                self.typing(&to, true).await;
                let transport = Arc::clone(&self.transport);
                let typing = tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(TYPING_REFRESH_INTERVAL).await;
                        if let Err(e) = transport.send_typing(&to, true).await {
                            tracing::debug!(error = %e, "failed to refresh typing indicator");
                        }
                    }
                })
                .abort_handle();
                self.lock().turns.entry(session).or_default().typing = Some(typing);
            }
            AgentEvent::TextDelta { session, text, .. } => {
                let mut s = self.lock();
                if s.threads.is_bound(session) {
                    s.turns.entry(session).or_default().text.push_str(&text);
                }
            }
            // 45 §0.3: the prose before a tool batch is a block of its own, for
            // every turn, spoken or typed. The typing indicator keeps running.
            AgentEvent::ToolCallPending { session, .. } => {
                let to = self.lock().threads.recipient_for(session);
                if let Some(to) = to {
                    self.flush(&to, session).await;
                }
            }
            AgentEvent::TurnFinished {
                session,
                stop_reason,
                ..
            } => {
                let (to, typing) = {
                    let mut s = self.lock();
                    let Some(turn) = s.turns.get_mut(&session) else {
                        return;
                    };
                    let typing = turn.typing.take();
                    (s.threads.recipient_for(session), typing)
                };
                if let Some(h) = typing {
                    h.abort();
                }
                let Some(to) = to else {
                    self.lock().turns.remove(&session);
                    return;
                };
                self.typing(&to, false).await;
                self.flush(&to, session).await;
                let marks = self
                    .lock()
                    .turns
                    .remove(&session)
                    .map(|t| t.marks)
                    .unwrap_or_default();
                let emoji = match stop_reason.as_str() {
                    "cancelled" => "🛑",
                    "error" => "❌",
                    _ => "✅",
                };
                for (author, ts) in &marks {
                    self.react(&to, emoji, author, *ts).await;
                }
            }
            AgentEvent::Error {
                session, message, ..
            } => {
                let (to, marks) = {
                    let s = self.lock();
                    (
                        s.threads.recipient_for(session),
                        s.turns
                            .get(&session)
                            .map(|t| t.marks.clone())
                            .unwrap_or_default(),
                    )
                };
                let Some(to) = to else { return };
                self.flush(&to, session).await;
                self.send_chunks(&to, &format!("⚠️ {message}")).await;
                for (author, ts) in &marks {
                    self.react(&to, "❌", author, *ts).await;
                }
            }
            // 125 §9: a concern or blocker is one plain line after what is
            // buffered so far; nits post nothing.
            AgentEvent::AdvisoryAdded {
                session, advisory, ..
            } => {
                let Some(line) = advisory.channel_line() else {
                    return;
                };
                let to = self.lock().threads.recipient_for(session);
                let Some(to) = to else { return };
                self.flush(&to, session).await;
                self.send_chunks(&to, &line).await;
            }
            _ => {}
        }
    }

    /// One prose block out (45 §4.5.3–4): the buffered markdown as text now
    /// and, on a voice turn, as a voice message once synthesized — after the
    /// previous block's clip, never before it.
    async fn flush(self: &Arc<Self>, to: &Recipient, session: SessionId) {
        let (md, voice, prefix) = {
            let mut s = self.lock();
            let State { turns, threads, .. } = &mut *s;
            let Some(turn) = turns.get_mut(&session) else {
                return;
            };
            if turn.text.trim().is_empty() {
                return;
            }
            // 67 §10: an agent's thread hears `"<Title> — "` once per turn.
            let prefix = (!turn.prefixed)
                .then(|| {
                    threads
                        .agent_of(&to.number)
                        .map(|(_, t, _)| format!("{t} — "))
                })
                .flatten();
            turn.prefixed = true;
            (std::mem::take(&mut turn.text), turn.voice, prefix)
        };
        // 30 §9: json-render fences as their rendering; `markdown_to_plain`
        // keeps the ``` lines, so the box arrives monospace-readable.
        let plain = text::markdown_to_plain(&surface::flatten(&md, 60));
        if !plain.trim().is_empty() {
            match prefix {
                Some(p) => self.send_chunks(to, &(p + &plain)).await,
                None => self.send_chunks(to, &plain).await,
            };
        }
        if !voice {
            return;
        }
        let Ok(Some(tts)) = ActorRef::<voice::Voice>::lookup(voice::VOICE_ACTOR) else {
            return;
        };
        let chan = Arc::clone(self);
        let to = to.clone();
        let mut s = self.lock();
        let Some(turn) = s.turns.get_mut(&session) else {
            return;
        };
        let prev = turn.clip_tail.take();
        // ponytail: the chain ends with the turn; a clip still uploading when the
        // next turn's first block is ready may land after it. Park the tail on
        // the session if that is ever heard.
        turn.clip_tail = Some(tokio::spawn(async move {
            // Synthesis starts at once; only the send waits for the previous clip.
            let clip = tts
                .ask(voice::Synthesize { text: md })
                .await
                .map_err(|e| e.to_string());
            if let Some(p) = prev {
                let _ = p.await;
            }
            match clip {
                Ok(Some(c)) => {
                    if !chan
                        .send_attachment(&to, &c.bytes, c.mime, None, None)
                        .await
                    {
                        tracing::warn!(to = %to.number, "voice clip send failed");
                    }
                }
                Ok(None) => {}
                Err(e) => tracing::warn!(error = %e, "voice clip synthesis failed"),
            }
        }));
    }
}

/// Asks the `"signal"` actor so `lib.rs` keeps config persistence in one place.
async fn ask_signal<M>(m: M) -> Result<(), String>
where
    M: Send + 'static,
    crate::Signal: Message<M, Reply = Result<(), String>>,
{
    let actor = ActorRef::<crate::Signal>::lookup(SIGNAL_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(|| "signal actor is not running".to_string())?;
    actor.ask(m).await.map_err(|e| e.to_string())
}

/// The declared MIME unless it is generic, then the magic-byte guess.
fn effective_mime(declared: &str, bytes: &[u8]) -> String {
    let base = declared
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let generic = matches!(
        base.as_str(),
        "" | "application/octet-stream" | "binary/octet-stream" | "application/binary"
    );
    if !generic {
        return base;
    }
    sniff(bytes)
        .unwrap_or(if base.is_empty() {
            "application/octet-stream"
        } else {
            base.as_str()
        })
        .to_owned()
}

fn sniff(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if b.len() >= 12 && b.starts_with(b"RIFF") && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else if b.starts_with(b"%PDF") {
        Some("application/pdf")
    } else if b.starts_with(b"OggS") {
        Some("audio/ogg")
    } else {
        None
    }
}

fn extension_for(mime: &str) -> String {
    match mime {
        "image/jpeg" => "jpg".into(),
        "text/plain" => "txt".into(),
        "audio/mpeg" => "mp3".into(),
        "audio/mp4" => "m4a".into(),
        "application/octet-stream" => "bin".into(),
        _ => {
            let sub: String = mime
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            if sub.is_empty() { "bin".into() } else { sub }
        }
    }
}

/// Where an inbound message goes (67 §10).
enum Addressee {
    Theuth,
    Agent(Named),
}

/// The roster agent addressed: `desk` and `title` from the roster, `task`
/// from the thread when it is already bound to this agent.
struct Named {
    name: String,
    title: String,
    desk: Option<SessionId>,
    task: Option<String>,
}

/// A task title from a brief: its first line, at most 80 chars (char-safe).
fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    match line.char_indices().nth(80) {
        Some((i, _)) => line[..i].to_owned(),
        None => line.to_owned(),
    }
}

fn fmt_size(bytes: usize) -> String {
    const KIB: usize = 1024;
    const MIB: usize = KIB * 1024;
    if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{} KiB", bytes.div_ceil(KIB))
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use tools::Risk;

    use super::*;
    use crate::mock::{MockTransport, SentItem};
    use crate::transport::IncomingReaction;

    const OWNER: &str = "+15550001111";
    const FRIEND: &str = "+15550002222";

    #[derive(Debug, PartialEq)]
    enum Call {
        NewRoot(PeerScope),
        SendInput(Option<SessionId>, String, Vec<PathBuf>),
        Steer(SessionId, String, Vec<PathBuf>),
        Resolve(ApprovalId, Decision),
        AnswerAsk(AskId, Option<Vec<Answer>>),
    }

    struct Stub {
        session: SessionId,
        calls: Mutex<Vec<Call>>,
        /// child → parent, for `ancestors`.
        parents: Mutex<HashMap<SessionId, SessionId>>,
    }

    impl Stub {
        fn calls(&self) -> Vec<Call> {
            std::mem::take(&mut *self.calls.lock().unwrap())
        }
    }

    #[async_trait]
    impl Port for Stub {
        async fn new_root(&self, scope: PeerScope) -> Result<SessionId, String> {
            self.calls.lock().unwrap().push(Call::NewRoot(scope));
            Ok(self.session)
        }
        async fn send_input(
            &self,
            session: Option<SessionId>,
            text: String,
            files: Vec<PathBuf>,
        ) -> Result<SessionId, String> {
            self.calls
                .lock()
                .unwrap()
                .push(Call::SendInput(session, text, files));
            Ok(session.unwrap_or(self.session))
        }
        async fn steer(
            &self,
            session: SessionId,
            _: InputId,
            text: String,
            files: Vec<PathBuf>,
        ) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push(Call::Steer(session, text, files));
            Ok(())
        }
        async fn resolve(&self, id: ApprovalId, decision: Decision) -> Result<bool, String> {
            self.calls.lock().unwrap().push(Call::Resolve(id, decision));
            Ok(true)
        }
        async fn answer_ask(
            &self,
            id: AskId,
            answers: Option<Vec<Answer>>,
        ) -> Result<bool, String> {
            self.calls
                .lock()
                .unwrap()
                .push(Call::AnswerAsk(id, answers));
            Ok(true)
        }
        async fn ancestors(&self, session: SessionId) -> Result<Vec<SessionId>, String> {
            let parents = self.parents.lock().unwrap();
            let mut chain = vec![session];
            while let Some(p) = parents.get(chain.last().unwrap()) {
                chain.push(*p);
            }
            Ok(chain)
        }
    }

    #[derive(Actor)]
    struct Recorder {
        events: Arc<Mutex<Vec<SignalEvent>>>,
    }

    impl Message<SignalEvent> for Recorder {
        type Reply = ();
        async fn handle(&mut self, m: SignalEvent, _: &mut Context<Self, ()>) {
            self.events.lock().unwrap().push(m);
        }
    }

    struct Fixture {
        transport: MockTransport,
        stub: Arc<Stub>,
        chan: Arc<Channel>,
        handle: Handle,
        events: Arc<Mutex<Vec<SignalEvent>>>,
        _dir: tempfile::TempDir,
    }

    impl Fixture {
        fn events(&self) -> Vec<SignalEvent> {
            self.events.lock().unwrap().clone()
        }

        fn disconnects(&self) -> Vec<u64> {
            self.events()
                .into_iter()
                .filter_map(|e| match e {
                    SignalEvent::Disconnected { retry_in_secs } => Some(retry_in_secs),
                    _ => None,
                })
                .collect()
        }

        /// Spawns the supervisor on this fixture's runtime.
        fn start(self) -> Self {
            let handle = spawn_with(
                Arc::new(self.transport.clone()),
                cfg(),
                self.chan.rt.clone(),
                Arc::clone(&self.stub) as Arc<dyn Port>,
            );
            Self {
                chan: Arc::clone(&handle.chan),
                handle,
                ..self
            }
        }

        fn link_urls(&self) -> usize {
            self.events()
                .iter()
                .filter(|e| matches!(e, SignalEvent::LinkUrl { .. }))
                .count()
        }

        /// Polls (advancing paused time) until `pred` holds.
        async fn until(&self, mut pred: impl FnMut(&Self) -> bool) {
            tokio::time::timeout(Duration::from_secs(3600), async {
                while !pred(self) {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            })
            .await
            .expect("condition not reached");
        }
    }

    fn cfg() -> SignalConfig {
        SignalConfig {
            owner: OWNER.into(),
            allowed: vec![FRIEND.into()],
            ..SignalConfig::default()
        }
    }

    /// An unstarted channel (call `start` for the supervisor tests).
    async fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let events = Arc::new(Mutex::new(Vec::new()));
        let rec = Recorder::spawn_with_mailbox(
            Recorder {
                events: Arc::clone(&events),
            },
            runtime::subscriber_mailbox(),
        );
        rt.subscribe::<Recorder, SignalEvent>(&rec).await;
        let transport = MockTransport::new();
        transport.set_account(Some(Recipient::new("+15559990000")));
        let stub = Arc::new(Stub {
            session: SessionId::new(),
            calls: Mutex::new(Vec::new()),
            parents: Mutex::new(HashMap::new()),
        });
        let (tx, _rx) = mpsc::unbounded_channel();
        let handle = Handle {
            chan: Arc::new(Channel::new(
                Arc::new(transport.clone()),
                cfg(),
                rt,
                Arc::clone(&stub) as Arc<dyn Port>,
                tx,
            )),
        };
        Fixture {
            transport,
            stub,
            chan: Arc::clone(&handle.chan),
            handle,
            events,
            _dir: dir,
        }
    }

    fn env(from: &str, ts: u64, text: &str) -> SignalEnvelope {
        SignalEnvelope {
            sender: Recipient::new(from),
            timestamp: ts,
            text: Some(text.into()),
            quote_text: None,
            attachments: Vec::new(),
            reaction: None,
        }
    }

    fn finished(session: SessionId, stop: &str) -> AgentEvent {
        AgentEvent::TurnFinished {
            session,
            turn: runtime::TurnId(Uuid::nil()),
            stop_reason: stop.into(),
            usage: agent::Usage::default(),
            context: 0,
        }
    }

    fn started(session: SessionId) -> AgentEvent {
        AgentEvent::TurnStarted {
            session,
            turn: runtime::TurnId(Uuid::nil()),
            text: String::new(),
            model: String::new(),
            turn_seq: 0,
        }
    }

    #[test]
    fn backoff_and_numbers() {
        let secs: Vec<u64> = (0..8).map(|a| backoff_delay(a).as_secs()).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16, 32, 60, 60]);
        assert_eq!(normalize_number(" +1 (409) 519-3333 "), "+14095193333");
        assert_eq!(normalize_number("14095193333"), "+14095193333");
        assert_eq!(normalize_number("bob"), "bob");
        assert_eq!(normalize_number(""), "");
    }

    #[test]
    fn threads_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let id = SessionId::new();
        let aci = Uuid::new_v4();
        // Pre-ACI file shape still loads.
        std::fs::create_dir_all(dir.path().join("signal")).unwrap();
        std::fs::write(
            dir.path().join("signal/threads.json"),
            format!("{{\"{OWNER}\": \"{id}\"}}"),
        )
        .unwrap();
        let mut t = Threads::load(dir.path());
        assert_eq!(t.get(OWNER), Some(id));
        assert_eq!(t.recipient_for(id), Some(Recipient::new(OWNER)));
        // Learning the ACI persists; rebinding without one keeps it.
        t.bind(
            &Recipient {
                number: "+1 555 000 1111".into(),
                uuid: Some(aci),
            },
            id,
        );
        let id2 = SessionId::new();
        t.bind(&Recipient::new(OWNER), id2);
        let t2 = Threads::load(dir.path());
        assert_eq!(t2.get(OWNER), Some(id2));
        assert_eq!(
            t2.recipient_for(id2),
            Some(Recipient {
                number: OWNER.into(),
                uuid: Some(aci)
            })
        );
        assert_eq!(t2.recipient(OWNER).uuid, Some(aci));
        t.unbind(OWNER);
        assert!(Threads::load(dir.path()).all().is_empty());
        std::fs::write(dir.path().join("signal/threads.json"), b"{nope").unwrap();
        assert!(Threads::load(dir.path()).all().is_empty());
    }

    #[test]
    fn threads_agent_binding_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut t = Threads::load(dir.path());
        let first = SessionId::new();
        t.bind(&Recipient::new(OWNER), first);
        // No desk: the session it had stays; the agent, title and task land.
        t.bind_agent(
            &Recipient::new(OWNER),
            None,
            "ops".into(),
            "Ops".into(),
            None,
        );
        assert_eq!(t.get(OWNER), Some(first));
        assert_eq!(t.agent_of(OWNER), Some(("ops", "Ops", None)));
        let desk = SessionId::new();
        t.bind_agent(
            &Recipient::new(OWNER),
            Some(desk),
            "ops".into(),
            "Ops".into(),
            Some("t1".into()),
        );
        let t2 = Threads::load(dir.path());
        assert_eq!(t2.get(OWNER), Some(desk));
        assert_eq!(t2.agent_of(OWNER), Some(("ops", "Ops", Some("t1"))));
        assert_eq!(t2.recipient_for(desk), Some(Recipient::new(OWNER)));
        t.clear_agent(OWNER);
        let t3 = Threads::load(dir.path());
        assert_eq!(t3.agent_of(OWNER), None);
        assert_eq!(t3.get(OWNER), Some(desk));
        // A theuth bind drops any agent binding.
        t.bind_agent(
            &Recipient::new(OWNER),
            None,
            "ops".into(),
            "Ops".into(),
            None,
        );
        t.bind(&Recipient::new(OWNER), first);
        assert_eq!(Threads::load(dir.path()).agent_of(OWNER), None);
        // Agent-only thread with no session yet is not a session binding.
        t.bind_agent(
            &Recipient::new(FRIEND),
            None,
            "dev".into(),
            "Dev".into(),
            None,
        );
        assert_eq!(t.get(FRIEND), None);
        assert_eq!(t.all().len(), 1);
        assert_eq!(first_line("  fix prod\nmore"), "fix prod");
        assert_eq!(first_line(&"é".repeat(100)).chars().count(), 80);
    }

    // -- supervisor (09 §15.2) ------------------------------------------

    #[tokio::test(start_paused = true)]
    async fn reconnect_backoff_and_stable_reset() {
        let f = fixture().await;
        for _ in 0..8 {
            f.transport.script_connection(Vec::new(), false);
        }
        let live = f.transport.script_live();
        f.transport.script_connection(Vec::new(), true);
        let start = Instant::now();
        let f = f.start();

        f.until(|f| f.transport.incoming_calls() == 9).await;
        // 1 (silent), 2, 4, 8, 16, 32, 60, 60
        assert_eq!(f.disconnects(), [2, 4, 8, 16, 32, 60, 60]);
        assert!(start.elapsed() >= Duration::from_secs(183));
        // Stable for 30 s, then dropped: backoff resets to a silent 1 s.
        let before = Instant::now();
        tokio::time::sleep(STABLE_CONNECTION + Duration::from_secs(1)).await;
        drop(live);
        f.until(|f| f.transport.incoming_calls() == 10).await;
        assert_eq!(f.disconnects().len(), 7);
        assert!(before.elapsed() < Duration::from_secs(34));
        assert_eq!(
            f.handle.link().await,
            Err("already linked as +15559990000".into())
        );
    }

    #[tokio::test(start_paused = true)]
    async fn link_url_rotation_slows_after_ten() {
        let f = fixture().await;
        f.transport.set_registered(false);
        for i in 0..11 {
            drop(f.transport.script_link_attempt(format!("sgnl://u{i}")));
        }
        let start = Instant::now();
        let f = f.start();
        f.until(|f| f.link_urls() == 10).await;
        let at_ten = start.elapsed();
        assert!(
            at_ten >= Duration::from_secs(27) && at_ten < Duration::from_secs(60),
            "{at_ten:?}"
        );
        f.until(|f| f.link_urls() == 11).await;
        assert!(start.elapsed() >= Duration::from_secs(327));
        // The 12th (unscripted) attempt hangs: `link` returns its URL.
        f.until(|f| f.link_urls() == 12).await;
        assert_eq!(
            f.handle.link().await.unwrap(),
            "sgnl://linkdevice?uuid=mock&device=theuth"
        );
        assert!(
            f.transport
                .linked_device_names()
                .iter()
                .all(|n| n == "theuth")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn unregistered_after_drop_returns_to_linking() {
        let f = fixture().await;
        let live = f.transport.script_live();
        let f = f.start();
        f.until(|f| f.transport.incoming_calls() == 1).await;
        f.transport.set_registered(false);
        drop(live);
        f.until(|f| f.transport.linked_device_names().len() == 1)
            .await;
        assert!(f.events().contains(&SignalEvent::Unlinked));
        assert_eq!(
            f.handle
                .status()
                .await
                .state
                .map(|e| matches!(e, SignalEvent::LinkUrl { .. })),
            Some(true)
        );
    }

    // -- trust + inbound (09 §15.3–15.4) --------------------------------

    #[tokio::test]
    async fn stranger_dropped_silently() {
        let f = fixture().await;
        f.chan.handle_envelope(env("+19998887777", 1, "hi")).await;
        assert!(f.transport.sent().is_empty());
        assert!(f.stub.calls().is_empty());
    }

    // -- hub duties (67 decision 22) -------------------------------------

    #[tokio::test]
    async fn non_hub_is_silent() {
        let f = fixture().await;
        {
            let mut s = f.chan.lock();
            s.lease.set_self("me".into());
            s.lease.observe(&mesh::LeaseState {
                holder: Some(mesh::EngineId("other".into())),
                since_ms: 0,
                reason: String::new(),
            });
            assert!(!s.lease.holding);
        }
        f.chan.handle_envelope(env(OWNER, 1, "hi")).await;
        assert!(f.transport.sent().is_empty());
        assert!(f.stub.calls().is_empty());
    }

    #[tokio::test]
    async fn hub_answers_by_default() {
        let f = fixture().await;
        assert!(f.chan.lock().lease.holding);
        f.chan.handle_envelope(env(OWNER, 1, "hi")).await;
        assert_eq!(
            f.stub.calls(),
            [
                Call::NewRoot(PeerScope::Owner),
                Call::SendInput(Some(f.stub.session), "hi".into(), vec![])
            ]
        );
    }

    #[tokio::test]
    async fn name_route_without_roster_falls_back_to_theuth() {
        let f = fixture().await;
        f.chan.handle_envelope(env(OWNER, 1, "ops: hi")).await;
        assert_eq!(
            f.stub.calls(),
            [
                Call::NewRoot(PeerScope::Owner),
                Call::SendInput(Some(f.stub.session), "ops: hi".into(), vec![])
            ]
        );
        assert_eq!(f.chan.lock().threads.agent_of(OWNER), None);
        // `theuth:` is always a name: the address is stripped.
        f.chan.handle_envelope(env(OWNER, 2, "theuth: hello")).await;
        assert_eq!(
            f.stub.calls(),
            [Call::SendInput(
                Some(f.stub.session),
                "hello".into(),
                vec![]
            )]
        );
    }

    #[tokio::test]
    async fn sealed_sender_resolved_and_cached() {
        let f = fixture().await;
        let aci = Uuid::new_v4();
        f.transport.script_uuid(OWNER, aci);
        let mut e = env("", 1, "hi");
        e.sender.uuid = Some(aci);
        f.chan.handle_envelope(e.clone()).await;
        assert_eq!(
            f.transport.sent()[0],
            SentItem::Receipt {
                to: OWNER.into(),
                ts: 1
            }
        );
        assert_eq!(
            f.chan.lock().aci.by_aci.get(&aci).map(String::as_str),
            Some(OWNER)
        );
        // Resolved to the owner: opens an owner-scoped root, dispatched once.
        assert_eq!(
            f.stub.calls(),
            [
                Call::NewRoot(PeerScope::Owner),
                Call::SendInput(Some(f.stub.session), "hi".into(), vec![])
            ]
        );
        // Unknown ACI: every configured number is tried, failures cached,
        // nothing reaches the agent.
        let mut other = env("", 2, "hi");
        other.sender.uuid = Some(Uuid::new_v4());
        f.chan.handle_envelope(other).await;
        assert!(f.chan.lock().aci.failed_at.contains_key(FRIEND));
        assert!(f.stub.calls().is_empty());
    }

    #[tokio::test]
    async fn receipt_first_then_eyes_then_send_input_and_bind() {
        let f = fixture().await;
        f.chan.handle_envelope(env(OWNER, 42, "hi")).await;
        let sent = f.transport.sent();
        assert_eq!(
            sent[0],
            SentItem::Receipt {
                to: OWNER.into(),
                ts: 42
            }
        );
        assert_eq!(
            sent[1],
            SentItem::Reaction {
                to: OWNER.into(),
                emoji: "👀".into(),
                target_author: OWNER.into(),
                target_ts: 42
            }
        );
        let session = f.stub.session;
        assert_eq!(
            f.stub.calls(),
            [
                Call::NewRoot(PeerScope::Owner),
                Call::SendInput(Some(session), "hi".into(), vec![])
            ]
        );
        assert_eq!(f.chan.lock().threads.get(OWNER), Some(f.stub.session));
    }

    #[tokio::test]
    async fn slash_never_reaches_the_agent() {
        let f = fixture().await;
        f.chan.handle_envelope(env(OWNER, 1, "/help")).await;
        assert!(f.stub.calls().is_empty());
        // Receipt, then only text replies to the sender: no 👀, nothing
        // forwarded. Help may span several chunks; its wording is incidental.
        let sent = f.transport.sent();
        assert!(matches!(&sent[0], SentItem::Receipt { to, ts: 1 } if to == OWNER));
        assert!(sent.len() > 1);
        assert!(sent[1..].iter().all(
            |s| matches!(s, SentItem::Text { to, text, .. } if to == OWNER && !text.is_empty())
        ));
    }

    #[tokio::test]
    async fn attachment_saved_and_quote_prefixed() {
        let f = fixture().await;
        let mut e = env(OWNER, 7, "look");
        e.quote_text = Some("earlier".into());
        e.attachments.push(IncomingAttachment {
            bytes: vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
            mime: "application/octet-stream".into(),
            file_name: None,
            voice_note: false,
            duration_ms: None,
        });
        f.chan.handle_envelope(e).await;
        let path = f.chan.rt.data_dir.join("signal/inbox/7-0.png");
        assert!(path.exists());
        assert_eq!(
            f.stub.calls(),
            [
                Call::NewRoot(PeerScope::Owner),
                Call::SendInput(
                    Some(f.stub.session),
                    format!(
                        "(replying to: \"earlier\")\nlook\n[attachment: {} (image/png, 8 B)]",
                        path.display()
                    ),
                    vec![path]
                )
            ]
        );
    }

    // -- reactions + typing (09 §15.5) -----------------------------------

    #[tokio::test]
    async fn eyes_then_check_on_same_message_and_steer_in_flight() {
        let f = fixture().await;
        let session = f.stub.session;
        f.chan.handle_envelope(env(OWNER, 10, "hi")).await;
        f.chan.handle_event(Event::Agent(started(session))).await;
        f.chan.handle_envelope(env(OWNER, 11, "also")).await;
        assert_eq!(
            f.stub.calls(),
            [
                Call::NewRoot(PeerScope::Owner),
                Call::SendInput(Some(session), "hi".into(), vec![]),
                Call::Steer(session, "also".into(), vec![])
            ]
        );
        for piece in ["**hel", "lo**"] {
            f.chan
                .handle_event(Event::Agent(AgentEvent::TextDelta {
                    session,
                    turn: runtime::TurnId(Uuid::nil()),
                    text: piece.into(),
                }))
                .await;
        }
        f.chan
            .handle_event(Event::Agent(finished(session, "end_turn")))
            .await;
        let sent = f.transport.sent();
        assert!(sent.contains(&SentItem::Typing {
            to: OWNER.into(),
            active: true
        }));
        let tail = &sent[sent.len() - 4..];
        assert_eq!(
            tail[0],
            SentItem::Typing {
                to: OWNER.into(),
                active: false
            }
        );
        assert!(matches!(&tail[1], SentItem::Text { text, .. } if text == "hello"));
        let check = |ts| SentItem::Reaction {
            to: OWNER.into(),
            emoji: "✅".into(),
            target_author: OWNER.into(),
            target_ts: ts,
        };
        assert_eq!(tail[2], check(10));
        assert_eq!(tail[3], check(11));
        assert!(
            sent.iter()
                .any(|s| matches!(s, SentItem::Text { text, .. } if text == "queued"))
        );
        assert!(!f.chan.lock().turns.contains_key(&session));

        f.chan.handle_envelope(env(OWNER, 12, "more")).await;
        f.chan
            .handle_event(Event::Agent(finished(session, "cancelled")))
            .await;
        assert!(
            matches!(f.transport.sent().last(), Some(SentItem::Reaction { emoji, target_ts: 12, .. }) if emoji == "🛑")
        );
    }

    /// 45 §0.3: the prose before a tool batch is its own message; the
    /// typing indicator survives the mid-turn flush; the tail goes at the end.
    #[tokio::test]
    async fn prose_before_tool_batch_is_flushed_as_its_own_message() {
        let f = fixture().await;
        let session = f.stub.session;
        let delta = |text: &str| {
            Event::Agent(AgentEvent::TextDelta {
                session,
                turn: runtime::TurnId(Uuid::nil()),
                text: text.into(),
            })
        };
        let pending = || {
            Event::Agent(AgentEvent::ToolCallPending {
                session,
                turn: runtime::TurnId(Uuid::nil()),
                call_id: "c1".into(),
                name: "shell".into(),
            })
        };
        f.chan.handle_envelope(env(OWNER, 10, "hi")).await;
        f.chan.handle_event(Event::Agent(started(session))).await;
        f.chan.handle_event(pending()).await; // nothing buffered: no message
        f.chan.handle_event(delta("Let me check.")).await;
        f.chan.handle_event(pending()).await;
        f.chan.handle_event(pending()).await; // second call of the batch: nothing new
        let texts = |sent: &[SentItem]| {
            sent.iter()
                .filter_map(|s| match s {
                    SentItem::Text { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(texts(&f.transport.sent()), ["Let me check."]);
        assert!(
            f.chan
                .lock()
                .turns
                .get(&session)
                .is_some_and(|t| t.typing.is_some())
        );
        assert!(!f.transport.sent().contains(&SentItem::Typing {
            to: OWNER.into(),
            active: false
        }));
        f.chan.handle_event(delta("Done.")).await;
        f.chan
            .handle_event(Event::Agent(finished(session, "end_turn")))
            .await;
        assert_eq!(texts(&f.transport.sent()), ["Let me check.", "Done."]);
        assert!(!f.chan.lock().turns.contains_key(&session));
    }

    /// 125 §9: a blocker lands as one plain line after the buffered prose;
    /// a nit sends nothing.
    #[tokio::test]
    async fn advisory_blocker_is_one_plain_line_after_the_prose() {
        use agent::{AdvisorSeverity, Advisory, AdvisoryNote};
        let f = fixture().await;
        let session = f.stub.session;
        let card = |severity: AdvisorSeverity, note: &str| {
            Event::Agent(AgentEvent::AdvisoryAdded {
                session,
                seq: 9,
                advisory: Advisory {
                    advisor: SessionId::new(),
                    step: 1,
                    turn: None,
                    at_ms: 0,
                    notes: vec![AdvisoryNote {
                        severity,
                        note: note.into(),
                    }],
                    kind: Default::default(),
                },
            })
        };
        let texts = |sent: &[SentItem]| {
            sent.iter()
                .filter_map(|s| match s {
                    SentItem::Text { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        f.chan.handle_envelope(env(OWNER, 10, "hi")).await;
        f.chan.handle_event(Event::Agent(started(session))).await;
        f.chan
            .handle_event(Event::Agent(AgentEvent::TextDelta {
                session,
                turn: runtime::TurnId(Uuid::nil()),
                text: "On it.".into(),
            }))
            .await;
        f.chan.handle_event(card(AdvisorSeverity::Nit, "nit")).await;
        assert!(texts(&f.transport.sent()).is_empty());
        f.chan
            .handle_event(card(AdvisorSeverity::Blocker, "tests deleted\nmore"))
            .await;
        assert_eq!(
            texts(&f.transport.sent()),
            ["On it.", "advisor · blocker — tests deleted"]
        );
    }

    /// 30 §9: a `json-render` fence arrives as a monospace block of its
    /// rendering, not its JSON.
    #[tokio::test]
    async fn json_render_fence_arrives_flattened() {
        let f = fixture().await;
        let session = f.stub.session;
        f.chan.handle_envelope(env(OWNER, 10, "status?")).await;
        f.chan.handle_event(Event::Agent(started(session))).await;
        let reply = "Status:\n\n```json-render\n{\"root\":\"c\",\"elements\":{\
            \"c\":{\"type\":\"Card\",\"props\":{\"title\":\"Rollout\"},\"children\":[\"l\"]},\
            \"l\":{\"type\":\"Checklist\",\"props\":{\"items\":[{\"text\":\"schema migration\",\"done\":true}]}}}}\n```\n";
        f.chan
            .handle_event(Event::Agent(AgentEvent::TextDelta {
                session,
                turn: runtime::TurnId(Uuid::nil()),
                text: reply.into(),
            }))
            .await;
        f.chan
            .handle_event(Event::Agent(finished(session, "end_turn")))
            .await;
        let sent = f.transport.sent();
        let text = sent
            .iter()
            .find_map(|s| match s {
                SentItem::Text { text, .. } if text.starts_with("Status:") => Some(text),
                _ => None,
            })
            .expect("reply sent");
        assert!(text.contains("```\n╭"), "{text}");
        assert!(text.contains("[x] schema migration"), "{text}");
        assert!(!text.contains("\"type\""), "{text}");
    }

    /// Every send driven by a bus event carries the ACI the inbound envelope
    /// had, so presage takes the UUID path instead of a contact-store lookup
    /// that fails on a freshly linked device.
    #[tokio::test]
    async fn bus_sends_keep_inbound_aci() {
        let f = fixture().await;
        let session = f.stub.session;
        let aci = Uuid::new_v4();
        let friend = Recipient {
            number: FRIEND.into(),
            uuid: Some(aci),
        };
        let mut e = env(FRIEND, 10, "hi");
        e.sender.uuid = Some(aci);
        f.chan.handle_envelope(e).await;
        // An allowed contact is not the owner: its root keeps the isolated default.
        assert_eq!(f.stub.calls(), [Call::SendInput(None, "hi".into(), vec![])]);
        // Restart: the ACI must come back from disk, not process memory.
        f.chan.lock().threads = Threads::load(&f.chan.rt.data_dir);
        f.chan.handle_event(Event::Agent(started(session))).await;
        f.chan
            .handle_event(Event::Agent(AgentEvent::TextDelta {
                session,
                turn: runtime::TurnId(Uuid::nil()),
                text: "hello".into(),
            }))
            .await;
        f.chan
            .handle_event(Event::Agent(finished(session, "end_turn")))
            .await;
        f.chan
            .handle_event(Event::Agent(AgentEvent::Error {
                session,
                turn: None,
                message: "boom".into(),
            }))
            .await;
        let recipients = f.transport.sent_recipients();
        // receipt, 👀, typing on, typing off, text, ✅, ⚠️ text
        assert_eq!(recipients.len(), 7);
        assert!(recipients.iter().all(|r| *r == friend), "{recipients:#?}");

        // Sealed sender (no number) mapped through the ACI directory: same.
        let mut e = env("", 11, "again");
        e.sender.uuid = Some(aci);
        f.chan.handle_envelope(e).await;
        f.chan
            .handle_event(Event::Agent(finished(session, "end_turn")))
            .await;
        assert!(
            f.transport.sent_recipients()[7..]
                .iter()
                .all(|r| *r == friend),
            "{:#?}",
            f.transport.sent_recipients()
        );
    }

    #[tokio::test]
    async fn approval_reaction_resolves_allow() {
        let f = fixture().await;
        let session = f.stub.session;
        f.chan.lock().threads.bind(&Recipient::new(OWNER), session);
        let id = ApprovalId(Uuid::new_v4());
        f.chan
            .handle_event(Event::Requested(ApprovalRequested {
                id,
                session,
                tool: "shell".into(),
                args: serde_json::json!({ "command": "rm -rf target/" }),
                risk: Risk::Destructive,
            }))
            .await;
        let Some(SentItem::Text { timestamp, .. }) = f.transport.sent().last().cloned() else {
            panic!("approval message not sent");
        };
        let react = |from: &str, emoji: &str| SignalEnvelope {
            reaction: Some(IncomingReaction {
                emoji: emoji.into(),
                target_timestamp: timestamp,
                remove: false,
            }),
            text: None,
            ..env(from, 99, "")
        };
        f.chan.handle_envelope(react(FRIEND, "👍")).await;
        f.chan.handle_envelope(react(OWNER, "🎉")).await;
        assert!(f.stub.calls().is_empty());
        f.chan.handle_envelope(react(OWNER, "👍")).await;
        assert_eq!(f.stub.calls(), [Call::Resolve(id, Decision::Allow)]);

        f.chan
            .handle_event(Event::Resolved(ApprovalResolved {
                id,
                decision: Decision::Allow,
                timed_out: false,
            }))
            .await;
        assert_eq!(
            f.transport.sent().last(),
            Some(&SentItem::Reaction {
                to: OWNER.into(),
                emoji: "✅".into(),
                target_author: "+15559990000".into(),
                target_ts: timestamp,
            })
        );
    }

    #[tokio::test]
    async fn asks_answered_by_text_replies() {
        let f = fixture().await;
        let session = f.stub.session;
        f.chan.lock().threads.bind(&Recipient::new(OWNER), session);
        let choice = |label: &str| tools::Choice {
            label: label.into(),
            description: None,
        };
        let ask = |questions: Vec<tools::Question>| AskRequested {
            id: AskId(Uuid::new_v4()),
            session,
            questions,
        };
        let question = |q: &str, options: Vec<tools::Choice>| tools::Question {
            id: q.into(),
            question: q.into(),
            options,
            recommended: None,
        };
        let last_text = || {
            f.transport
                .sent()
                .into_iter()
                .rev()
                .find_map(|s| match s {
                    SentItem::Text { text, .. } => Some(text),
                    _ => None,
                })
                .expect("no text sent")
        };

        let req = ask(vec![
            question("lang?", vec![choice("Rust"), choice("Go")]),
            question("name?", Vec::new()),
        ]);
        let id = req.id;
        f.chan.handle_event(Event::AskRequested(req)).await;
        assert!(last_text().starts_with("❓ Question 1/2: lang?\n1. Rust\n2. Go\n"));

        f.chan.handle_envelope(env(OWNER, 1, "/status")).await;
        assert!(f.stub.calls().is_empty());
        assert!(!last_text().starts_with('❓'), "{}", last_text());

        f.chan.handle_envelope(env(OWNER, 2, "3")).await;
        assert_eq!(last_text(), "pick 1–2, your own answer, skip or dismiss");
        f.chan.handle_envelope(env(OWNER, 3, "2")).await;
        assert!(last_text().starts_with("❓ Question 2/2: name?\n"));
        assert!(f.stub.calls().is_empty());
        f.chan.handle_envelope(env(OWNER, 4, "my own")).await;
        assert_eq!(last_text(), "✅ answers sent");
        assert_eq!(
            f.stub.calls(),
            [Call::AnswerAsk(
                id,
                Some(vec![
                    Answer::Choice { index: 1 },
                    Answer::Text {
                        text: "my own".into()
                    },
                ]),
            )]
        );
        // Sent: later text is input again, not an answer.
        f.chan.handle_envelope(env(OWNER, 5, "hello")).await;
        assert!(matches!(f.stub.calls()[..], [Call::SendInput(..)]));

        // A second ask queues behind the first until it resolves.
        let next = ask(vec![question("more?", Vec::new())]);
        let next_id = next.id;
        f.chan.handle_event(Event::AskRequested(next)).await;
        assert!(!last_text().starts_with("❓ Question 1/1: more?"));
        f.chan
            .handle_event(Event::AskResolved(AskResolved {
                id,
                dismissed: false,
                timed_out: false,
            }))
            .await;
        assert!(last_text().starts_with("❓ Question 1/1: more?\n"));
        f.chan.handle_envelope(env(OWNER, 6, "dismiss")).await;
        assert_eq!(last_text(), "🚫 dismissed; turn cancelled");
        assert_eq!(f.stub.calls(), [Call::AnswerAsk(next_id, None)]);
        f.chan
            .handle_event(Event::AskResolved(AskResolved {
                id: next_id,
                dismissed: true,
                timed_out: false,
            }))
            .await;
        assert_eq!(last_text(), "🚫 dismissed; turn cancelled");
    }

    fn artifact(session: SessionId, dir: &Path) -> ArtifactShown {
        let path = dir.join("report.pdf");
        std::fs::write(&path, b"%PDF").unwrap();
        ArtifactShown {
            session,
            call_id: "call_1".into(),
            artifact: artifact::Artifact {
                id: "a1".into(),
                session,
                call_id: "call_1".into(),
                name: "report.pdf".into(),
                title: "Weekly report".into(),
                kind: artifact::Kind::Pdf,
                mime: "application/pdf".into(),
                bytes: 4,
                version: 1,
                path,
                source: None,
                preview: None,
                pages: None,
                origin: None,
                created_at: 0,
            },
        }
    }

    #[tokio::test]
    async fn artifact_for_a_bound_session_is_one_attachment_with_its_title() {
        let f = fixture().await;
        f.chan
            .lock()
            .threads
            .bind(&Recipient::new(FRIEND), f.stub.session);
        f.chan
            .handle_event(Event::Artifact(artifact(f.stub.session, f._dir.path())))
            .await;
        let sent = f.transport.sent();
        assert_eq!(sent.len(), 1);
        assert!(matches!(
            &sent[0],
            SentItem::Attachment { to, mime, bytes, file_name, caption }
                if to == FRIEND
                    && mime == "application/pdf"
                    && bytes == b"%PDF"
                    && file_name.as_deref() == Some("report.pdf")
                    && caption.as_deref() == Some("Weekly report")
        ));
    }

    #[tokio::test]
    async fn app_for_a_bound_session_is_its_source_captioned_where_it_runs() {
        let f = fixture().await;
        f.chan
            .lock()
            .threads
            .bind(&Recipient::new(FRIEND), f.stub.session);
        let mut ev = artifact(f.stub.session, f._dir.path());
        let path = f._dir.path().join("panel.openui");
        std::fs::write(&path, b"root = Card(TextContent(\"hi\"))").unwrap();
        ev.artifact.kind = artifact::Kind::App;
        ev.artifact.name = "panel.openui".into();
        ev.artifact.title = "Panel".into();
        ev.artifact.mime = "text/plain".into();
        ev.artifact.path = path;
        f.chan.handle_event(Event::Artifact(ev)).await;
        let sent = f.transport.sent();
        // One attachment, no text, no pretend controls.
        assert_eq!(sent.len(), 1);
        assert!(matches!(
            &sent[0],
            SentItem::Attachment { to, bytes, file_name, caption, .. }
                if to == FRIEND
                    && bytes == b"root = Card(TextContent(\"hi\"))"
                    && file_name.as_deref() == Some("panel.openui")
                    && caption.as_deref() == Some("Panel · Interactive app — open on desktop/web")
        ));
    }

    #[tokio::test]
    async fn artifact_of_an_unbound_child_goes_to_the_bound_parents_number() {
        let f = fixture().await;
        f.chan
            .lock()
            .threads
            .bind(&Recipient::new(FRIEND), f.stub.session);
        let child = SessionId::new();
        f.stub.parents.lock().unwrap().insert(child, f.stub.session);
        f.chan
            .handle_event(Event::Artifact(artifact(child, f._dir.path())))
            .await;
        let sent = f.transport.sent();
        assert!(matches!(
            &sent[..],
            [SentItem::Attachment { to, file_name, .. }]
                if to == FRIEND && file_name.as_deref() == Some("report.pdf")
        ));
    }

    #[tokio::test]
    async fn artifact_for_an_unbound_session_sends_nothing() {
        let f = fixture().await;
        f.chan
            .handle_event(Event::Artifact(artifact(SessionId::new(), f._dir.path())))
            .await;
        assert!(f.transport.sent().is_empty());
    }

    #[test]
    fn mime_helpers() {
        assert_eq!(effective_mime("image/PNG; charset=x", b""), "image/png");
        assert_eq!(effective_mime("", b"%PDF-1.4"), "application/pdf");
        assert_eq!(
            effective_mime("application/octet-stream", b"zzz"),
            "application/octet-stream"
        );
        assert_eq!(extension_for("image/jpeg"), "jpg");
        assert_eq!(extension_for("application/pdf"), "pdf");
        assert_eq!(extension_for("weird"), "weird");
        assert_eq!(fmt_size(83_000), "82 KiB");
        assert_eq!(fmt_size(3 * 1024 * 1024), "3.0 MiB");
    }
}
