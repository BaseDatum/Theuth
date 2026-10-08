//! Inbound routing, per-thread outbound workers and the supervisor (design
//! doc 21 §3.1, §5–§8, §11).
//!
//! Shape: [`spawn`] builds the [`Api`], one [`Channel`], an ordered inbound
//! worker, a bus-subscriber actor feeding an ordered router, and the
//! supervisor task (`auth.test` → owner DM → stop stale streams →
//! [`socket::run`]). Every bound thread gets one [`Worker`] task on first
//! use; the router sends it the session's `AgentEvent`s. [`Handle`] is what
//! the `"slack"` actor in `lib.rs` talks to.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use agent::{
    AGENT_ACTOR, AgentEvent, AgentInfo, Agents, Cancel, ListAgents, ListSessions, NewRootSession,
    SendInput, SessionSummary, Steer,
    communication::{PeerDelivered, PeerScope},
};
use artifact::{Artifact, ArtifactShown};
use async_trait::async_trait;
use kameo::Actor;
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use runtime::{InputId, Runtime, SessionId};
use slack_morphism::prelude::*;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tools::{
    APPROVAL_ACTOR, Answer, Approval, ApprovalId, ApprovalRequested, ApprovalResolved, AskId,
    AskRequested, AskResolved, Decision, Resolve,
};

use crate::api::{Api, ApiError, MARKDOWN_TEXT_MAX, SlackApi};
use crate::bubble::{self, Footer, Target, Turn};
use crate::commands::{self, CommandCx, Outcome, Reply};
use crate::threads::{Thread, ThreadKey, Threads, thread_key};
use crate::{
    ApprovalsScope, SlackConfig, SlackEvent, SlackStatus, ToolCards, approvals, asks, board, plan,
    socket, ui,
};

/// Dedup set size for `event_id` / `trigger_id` (21 §3.1).
pub const RECENT_KEYS: usize = 1024;
/// `agents.sessions.rename` title cap (21 §8.6).
pub const TITLE_MAX: usize = 200;
/// A child's task text cap in its `▸ **task**` line (21 §9.1).
pub const TASK_MAX: usize = 3000;
/// How long an instant `/theuth` answer may take before it goes through `response_url` (21 §10.2).
pub const ACK_BUDGET: Duration = Duration::from_millis(2500);

/// `<data_dir>/slack/threads.json`.
#[must_use]
pub fn threads_path(data_dir: &Path) -> PathBuf {
    data_dir.join("slack").join("threads.json")
}

/// The 37 §7 line: where to enter a requested secret. Never answerable here.
#[must_use]
pub fn secret_notice(name: &str, note: &str) -> String {
    format!(
        "theuth asks for secret `{name}` — {note}. Enter it in the desktop app, the TUI (/secrets add {name}) or `t --secret-set {name}`; it will not be read from this chat."
    )
}

/// Bounded FIFO set of recently seen envelope keys.
#[derive(Debug, Default)]
pub struct RecentKeys {
    order: VecDeque<String>,
    set: HashSet<String>,
}

impl RecentKeys {
    /// Records `key`; `true` when it was already present.
    pub fn seen(&mut self, key: &str) -> bool {
        if self.set.contains(key) {
            return true;
        }
        if self.order.len() >= RECENT_KEYS
            && let Some(old) = self.order.pop_front()
        {
            self.set.remove(&old);
        }
        self.order.push_back(key.to_owned());
        self.set.insert(key.to_owned());
        false
    }
}

/// The asks the channel makes of other plugins, behind a trait so the
/// inbound/outbound paths are testable without live actors.
#[async_trait]
trait Port: Send + Sync {
    async fn send_input(
        &self,
        session: Option<SessionId>,
        text: String,
        files: Vec<PathBuf>,
    ) -> Result<SessionId, String>;
    /// `agent::NewRootSession`: an empty root with an explicit scope. Only
    /// called after the channel authenticated the owner (50 §2.3).
    async fn new_root(&self, scope: PeerScope) -> Result<SessionId, String>;
    async fn steer(
        &self,
        session: SessionId,
        id: InputId,
        text: String,
        files: Vec<PathBuf>,
    ) -> Result<(), String>;
    async fn cancel(&self, session: SessionId) -> Result<(), String>;
    async fn list_sessions(&self) -> Result<Vec<SessionSummary>, String>;
    /// `tools::Resolve`: `true` when the id was pending.
    async fn resolve(&self, id: ApprovalId, decision: Decision) -> Result<bool, String>;
    /// `agent::answer_ask`: `true` when the id was pending; `None` dismisses (cancels the turn).
    async fn answer_ask(&self, id: AskId, answers: Option<Vec<Answer>>) -> Result<bool, String>;
    /// `agent::ListAgents`: the subtree under `session`.
    async fn list_agents(&self, session: SessionId) -> Result<Vec<AgentInfo>, String>;
}

/// [`Port`] over the registered `agent` and `approval` actors.
struct Actors;

/// 67 §10: the agents a message may be addressed to; empty without the roster.
async fn routable() -> Vec<roster::Routable> {
    let Some(roster) = roster::actor() else {
        return Vec::new();
    };
    roster.ask(roster::RoutableAgents).await.unwrap_or_default()
}

fn agents() -> Result<ActorRef<Agents>, String> {
    ActorRef::<Agents>::lookup(AGENT_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(|| "agent actor is not running".to_string())
}

#[async_trait]
impl Port for Actors {
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

    async fn new_root(&self, scope: PeerScope) -> Result<SessionId, String> {
        agents()?
            .ask(NewRootSession { cwd: None, scope })
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

    async fn cancel(&self, session: SessionId) -> Result<(), String> {
        agents()?
            .ask(Cancel {
                session,
                keep: None,
                scope: agent::CancelScope::Root,
            })
            .await
            .map_err(|e| e.to_string())
    }

    async fn list_sessions(&self) -> Result<Vec<SessionSummary>, String> {
        agents()?.ask(ListSessions).await.map_err(|e| e.to_string())
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

    async fn list_agents(&self, session: SessionId) -> Result<Vec<AgentInfo>, String> {
        agents()?
            .ask(ListAgents { session })
            .await
            .map_err(|e| e.to_string())
    }
}

/// Bus events the channel renders, in bus order.
enum Event {
    Agent(AgentEvent),
    Requested(ApprovalRequested),
    Resolved(ApprovalResolved),
    Ask(AskRequested),
    AskDone(AskResolved),
    Artifact(ArtifactShown),
    Secret(secrets::SecretsEvent),
    /// 67 §4.4: the hub lease moved.
    Lease(mesh::LeaseState),
    /// 110 §6: a child's or peer's result reached a session.
    Peer(PeerDelivered),
    /// 126 §10: the double's prediction for a pending card.
    Predicted(double::DoublePredicted),
    /// 126 §10: the double answered a card at L2.
    DoubleAnswered(double::DoubleAnswered),
}

/// Bus subscriber: forwards into the ordered router.
#[derive(Actor)]
struct Bus {
    tx: mpsc::UnboundedSender<Event>,
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
        let _ = self.tx.send(Event::Ask(m));
    }
}

impl Message<AskResolved> for Bus {
    type Reply = ();
    async fn handle(&mut self, m: AskResolved, _: &mut Context<Self, ()>) {
        let _ = self.tx.send(Event::AskDone(m));
    }
}

impl Message<ArtifactShown> for Bus {
    type Reply = ();
    async fn handle(&mut self, m: ArtifactShown, _: &mut Context<Self, ()>) {
        let _ = self.tx.send(Event::Artifact(m));
    }
}

impl Message<secrets::SecretsEvent> for Bus {
    type Reply = ();
    async fn handle(&mut self, m: secrets::SecretsEvent, _: &mut Context<Self, ()>) {
        let _ = self.tx.send(Event::Secret(m));
    }
}

impl Message<PeerDelivered> for Bus {
    type Reply = ();
    async fn handle(&mut self, m: PeerDelivered, _: &mut Context<Self, ()>) {
        let _ = self.tx.send(Event::Peer(m));
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

impl Message<mesh::MeshEvent> for Bus {
    type Reply = ();
    async fn handle(&mut self, m: mesh::MeshEvent, _: &mut Context<Self, ()>) {
        if let mesh::MeshEvent::Lease(l) = m {
            let _ = self.tx.send(Event::Lease(l));
        }
    }
}

/// An acked, deduplicated Socket Mode envelope (21 §7).
pub enum Inbound {
    /// Events API payload.
    Push(Box<SlackPushEventCallback>),
    /// Interactivity payload (buttons).
    Action(Box<SlackInteractionEvent>),
}

/// Router → worker.
enum Out {
    Agent(AgentEvent),
    /// A user message `ts` awaiting the terminal reaction (21 §8.6); `voice`
    /// when it was a voice note (45 §4.5.2).
    Mark(String, bool),
    /// An approval for this thread's session: post the card (21 §10.1).
    Approval(Box<ApprovalRequested>),
    /// The card for this id was answered: replace it with the outcome; `by` = the clicker.
    Resolved(ApprovalResolved, Option<String>),
    /// 126 §10: the double predicted this thread's approval card: add its context line.
    Predicted(Box<double::DoublePredicted>),
    /// Question `i` of an `ask_user` for this thread's session: post its card.
    Ask(Box<AskRequested>, usize),
    /// A `show` artifact for this thread's session or a descendant (35 §8): upload it.
    Artifact(Box<Artifact>),
    /// A descendant was bound to its own thread at `link` (21 §9.1); `depth` 1 = direct child.
    Spawned {
        child: SessionId,
        name: String,
        link: String,
        depth: u8,
    },
    /// A child's event, for the board row (21 §9.2).
    Child(SessionId, AgentEvent),
    /// A child's approval pending (`Some(tool)`) or answered (`None`).
    ChildApproval(SessionId, Option<String>),
    /// One markdown line for this thread (the 37 §7 secret notice).
    Note(String),
    /// 110 §6: a peer's result for this thread's session, as a collapsed container.
    Peer(Box<PeerDelivered>),
}

/// `auth.test` identity plus the owner DM (21 §11).
pub(crate) struct Identity {
    bot_user: String,
    team: String,
    pub(crate) owner_dm: Option<String>,
}

/// A pending approval card (21 §10.1): who may click it and, for owner-DM
/// cards, where it is (thread cards live in their worker).
struct Card {
    session: SessionId,
    /// The thread's user, allowed to click beside the owner; `None` for owner-DM cards.
    thread_user: Option<String>,
    /// Who clicked, once a click was accepted.
    by: Option<String>,
    /// Owner-DM cards only: `(ts, body, session context line)`.
    dm: Option<(String, String, String)>,
}

/// A pending `ask_user`: one message per question, the current one replaced
/// in place as it is answered. Lives here for thread and owner-DM cards alike.
struct AskCard {
    req: AskRequested,
    /// The thread's user, allowed to click beside the owner; `None` for owner-DM cards.
    thread_user: Option<String>,
    /// Index of the question whose card is up.
    current: usize,
    answers: Vec<Answer>,
    channel: String,
    /// The current question's message; empty until the worker has posted it.
    ts: String,
    /// Owner-DM cards only: the `session <id> · <title>` context line.
    context: Option<String>,
    /// Answered or dismissed from here; the message is already final.
    done: bool,
}

/// What follows an accepted question-card click.
enum AskNext {
    /// Post question `i`.
    Post(usize),
    /// Every question answered: hand these to the agent.
    Answer(Vec<Answer>),
    /// Dismiss the set (cancels the turn).
    Dismiss,
}

struct State {
    allowed: Vec<String>,
    channels: Vec<String>,
    threads: Threads,
    workers: HashMap<ThreadKey, mpsc::UnboundedSender<Out>>,
    /// Sessions between `TurnStarted` and `TurnFinished`.
    running: HashSet<SessionId>,
    recent: RecentKeys,
    last_event: Option<SlackEvent>,
    dropped: u64,
    /// 110 §6: `MessageDelivered` events not rendered.
    hidden: u64,
    /// Pending approval cards by id (thread and owner-DM alike).
    cards: HashMap<ApprovalId, Card>,
    /// Pending `ask_user` questions by id.
    asks: HashMap<AskId, AskCard>,
    /// child → (parent, child's name), from `SubagentSpawned` and `list_agents`.
    parent_of: HashMap<SessionId, (SessionId, String)>,
    /// `chat.getPermalink` results by thread key.
    permalinks: HashMap<ThreadKey, String>,
    /// 67 §4.4: the socket is a hub duty; it follows the lease.
    follower: roster::lease::Follower,
    /// The open socket's token; `None` while closed.
    socket: Option<CancellationToken>,
    /// Set once identity resolved: the socket may open.
    app_token: Option<String>,
}

/// Everything shared by the listener callbacks, the inbound worker, the
/// router and the per-thread workers.
pub struct Channel {
    pub(crate) api: Arc<dyn SlackApi>,
    /// The live façade, for call counters; `None` under a fake.
    live: Option<Arc<Api>>,
    port: Arc<dyn Port>,
    rt: Runtime,
    owner: String,
    tools: ToolCards,
    reasoning: bool,
    /// 110 §5: the plan message is on.
    plan: bool,
    /// 110 §8: `json-render` fences as Block Kit in the bubble.
    blocks: bool,
    approvals: ApprovalsScope,
    pub(crate) identity: OnceLock<Identity>,
    // ponytail: one std mutex for all state, never held across an await; split if it ever contends.
    state: Mutex<State>,
    inbound_tx: mpsc::UnboundedSender<Inbound>,
}

/// Control handle the `"slack"` actor uses. Cheap to clone.
#[derive(Clone)]
pub struct Handle {
    pub(crate) chan: Arc<Channel>,
}

impl Handle {
    /// The 21 §13 snapshot.
    #[must_use]
    pub fn status(&self) -> SlackStatus {
        self.chan.status()
    }

    /// Adds `user` to the live allowlist (the config file is `lib.rs`'s job).
    pub fn allow(&self, user: String) -> Result<(), String> {
        if user == self.chan.owner {
            return Err(format!("{user} is the owner"));
        }
        let mut s = self.chan.lock();
        if !s.allowed.contains(&user) {
            s.allowed.push(user);
        }
        Ok(())
    }

    /// Removes `user` from the live allowlist.
    pub fn remove_allowed(&self, user: String) -> Result<(), String> {
        let mut s = self.chan.lock();
        let before = s.allowed.len();
        s.allowed.retain(|u| *u != user);
        if s.allowed.len() == before {
            return Err(format!("{user} is not in the allowed list"));
        }
        Ok(())
    }

    /// Adds `channel` to the live channel list.
    pub fn allow_channel(&self, channel: String) -> Result<(), String> {
        let mut s = self.chan.lock();
        if !s.channels.contains(&channel) {
            s.channels.push(channel);
        }
        Ok(())
    }

    /// Removes `channel` from the live channel list.
    pub fn remove_channel(&self, channel: String) -> Result<(), String> {
        let mut s = self.chan.lock();
        let before = s.channels.len();
        s.channels.retain(|c| *c != channel);
        if s.channels.len() == before {
            return Err(format!("{channel} is not in the channel list"));
        }
        Ok(())
    }
}

/// Spawns the channel under `rt.shutdown`; returns immediately. `tokens` =
/// `(bot xoxb-…, app xapp-…)`. Fails only when the TLS client cannot be built.
pub fn spawn(cfg: SlackConfig, tokens: (String, String), rt: Runtime) -> anyhow::Result<Handle> {
    let (bot, app) = tokens;
    let api = Arc::new(Api::new(&bot)?);
    let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
    let chan = Arc::new(Channel::new(
        Arc::clone(&api) as Arc<dyn SlackApi>,
        Some(api),
        cfg,
        rt,
        Arc::new(Actors),
        inbound_tx,
    ));
    tokio::spawn(run(Arc::clone(&chan), app, inbound_rx));
    Ok(Handle { chan })
}

/// Supervisor (21 §3.1): workers, identity, boot cleanup, then the socket
/// until shutdown.
async fn run(
    chan: Arc<Channel>,
    app_token: String,
    mut inbound_rx: mpsc::UnboundedReceiver<Inbound>,
) {
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
    chan.rt.subscribe::<Bus, PeerDelivered>(&bus).await;
    chan.rt.subscribe::<Bus, double::DoublePredicted>(&bus).await;
    chan.rt.subscribe::<Bus, double::DoubleAnswered>(&bus).await;
    // 67 §4.7: learn our engine id once the mesh is up; the lease resolves then.
    let me = {
        let chan = Arc::clone(&chan);
        tokio::spawn(async move {
            let Some(mesh) = chan.rt.lookup_when::<mesh::Mesh>(mesh::MESH_ACTOR).await else {
                return;
            };
            if let Ok(st) = mesh.ask(mesh::Status).await {
                chan.lock().follower.set_self(st.self_id.0);
                chan.sync_socket().await;
            }
        })
    };
    let inbound = {
        let chan = Arc::clone(&chan);
        tokio::spawn(async move {
            while let Some(ev) = inbound_rx.recv().await {
                chan.handle_inbound(ev).await;
            }
        })
    };
    let router = {
        let chan = Arc::clone(&chan);
        tokio::spawn(async move {
            while let Some(ev) = ev_rx.recv().await {
                chan.route(ev).await;
            }
        })
    };
    let shutdown = chan.rt.shutdown.clone();
    if let Some(api) = chan.live.as_deref() {
        let mut attempt: u32 = 0;
        let identity = loop {
            match api.auth_test().await {
                Ok((bot_user, team, _url)) => break Some((bot_user, team)),
                Err(e) => {
                    let delay = socket::backoff_delay(attempt);
                    attempt = attempt.saturating_add(1);
                    tracing::warn!(error = %e, delay_s = delay.as_secs(), "slack auth.test failed; retrying");
                    chan.publish(SlackEvent::Disconnected {
                        reason: format!("auth.test: {e}"),
                    })
                    .await;
                    tokio::select! {
                        () = tokio::time::sleep(delay) => {},
                        () = shutdown.cancelled() => break None,
                    }
                }
            }
        };
        if let Some((bot_user, team)) = identity {
            let owner_dm = if chan.owner.is_empty() {
                None
            } else {
                api.open_dm(&chan.owner)
                    .await
                    .inspect_err(|e| tracing::warn!(error = %e, "slack conversations.open for the owner failed"))
                    .ok()
            };
            tracing::info!(%bot_user, %team, "slack identity resolved");
            let _ = chan.identity.set(Identity {
                bot_user,
                team,
                owner_dm,
            });
            // 21 §11: streams a previous process left open.
            let stale = chan.lock().threads.take_streams();
            for (channel, ts) in stale {
                if let Err(e) = chan
                    .api
                    .stop_stream(&channel, &ts, Vec::new(), None, Some("active"))
                    .await
                {
                    tracing::debug!(%channel, %ts, error = %e, "stale slack stream not stopped");
                }
            }
            // 67 §4.4: the socket opens while this engine holds the lease
            // (an engine without a mesh holds it by default).
            chan.lock().app_token = Some(app_token);
            chan.sync_socket().await;
            shutdown.cancelled().await;
        }
    }
    me.abort();
    inbound.abort();
    router.abort();
    bus.kill();
}

/// Reaction names (21 §8.6).
const EYES: &str = "eyes";
const DONE: &str = "white_check_mark";
const STOPPED: &str = "octagonal_sign";
const FAILED: &str = "x";
/// Subtitle of an app's `card` (46 §8.2, 110 §8): the thread gets a card, not controls.
const APP_NOTE: &str = "Interactive in theuth · not runnable in Slack";

impl Channel {
    fn new(
        api: Arc<dyn SlackApi>,
        live: Option<Arc<Api>>,
        cfg: SlackConfig,
        rt: Runtime,
        port: Arc<dyn Port>,
        inbound_tx: mpsc::UnboundedSender<Inbound>,
    ) -> Self {
        let owner = cfg.owner.trim().to_owned();
        let allowed = cfg
            .allowed
            .iter()
            .map(|u| u.trim().to_owned())
            .filter(|u| !u.is_empty() && *u != owner)
            .collect();
        Self {
            api,
            live,
            port,
            owner,
            tools: cfg.tools,
            reasoning: cfg.reasoning,
            plan: cfg.plan,
            blocks: cfg.blocks,
            approvals: cfg.approvals,
            identity: OnceLock::new(),
            state: Mutex::new(State {
                allowed,
                channels: cfg.channels,
                threads: Threads::load(&threads_path(&rt.data_dir)),
                workers: HashMap::new(),
                running: HashSet::new(),
                recent: RecentKeys::default(),
                last_event: None,
                dropped: 0,
                hidden: 0,
                cards: HashMap::new(),
                asks: HashMap::new(),
                parent_of: HashMap::new(),
                permalinks: HashMap::new(),
                follower: roster::lease::Follower::default(),
                socket: None,
                app_token: None,
            }),
            inbound_tx,
            rt,
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Publishes `ev` on the bus and remembers it for `Status`.
    pub async fn publish(&self, ev: SlackEvent) {
        self.lock().last_event = Some(ev.clone());
        self.rt.publish(ev).await;
    }

    /// 67 §4.4: opens the socket when this engine holds the lease and
    /// identity is up; closes it when the lease went elsewhere. Idempotent.
    async fn sync_socket(self: &Arc<Self>) {
        let closed = {
            let mut s = self.lock();
            let want = s.follower.holding && s.app_token.is_some();
            match (want, s.socket.take()) {
                (true, Some(tok)) => {
                    s.socket = Some(tok);
                    false
                }
                (true, None) => {
                    let tok = self.rt.shutdown.child_token();
                    let app = s.app_token.clone().unwrap_or_default();
                    tokio::spawn(socket::run(Arc::clone(self), app, tok.clone()));
                    s.socket = Some(tok);
                    false
                }
                (false, Some(tok)) => {
                    tok.cancel();
                    true
                }
                (false, None) => false,
            }
        };
        if closed {
            tracing::info!("slack socket closed: another engine holds the hub lease");
            self.publish(SlackEvent::Disconnected {
                reason: "hub lease held elsewhere".into(),
            })
            .await;
        }
    }

    /// The 21 §13 snapshot.
    pub fn status(&self) -> SlackStatus {
        let id = self.identity.get();
        let s = self.lock();
        SlackStatus {
            state: s.last_event.clone(),
            bot_user: id.map(|i| i.bot_user.clone()),
            team: id.map(|i| i.team.clone()),
            owner: self.owner.clone(),
            allowed: s.allowed.clone(),
            channels: s.channels.clone(),
            threads: s.threads.all(),
            open_streams: s.threads.open_streams(),
            calls: self.live.as_ref().map(|a| a.stats()).unwrap_or_default(),
            dropped_envelopes: s.dropped,
            hidden_deliveries: s.hidden,
        }
    }

    // -- listener callbacks (socket.rs) ----------------------------------

    /// `hello`: the socket is up.
    pub async fn connected(&self, connections: u32) {
        tracing::info!(connections, "slack socket mode connected");
        self.publish(SlackEvent::Connected {
            connections: u8::try_from(connections).unwrap_or(u8::MAX),
        })
        .await;
    }

    /// An envelope morphism could not parse even with `Unknown`.
    pub fn protocol_error(&self) {
        self.lock().dropped += 1;
    }

    /// Dedup: `true` when `key` was already seen (21 §3.1).
    pub fn recent_seen(&self, key: &str) -> bool {
        self.lock().recent.seen(key)
    }

    /// Queues an envelope for the ordered inbound worker.
    pub fn inbound(&self, ev: Inbound) {
        let _ = self.inbound_tx.send(ev);
    }

    /// `/theuth …` inside Slack's 3 s ack window (21 §10.2): instant answers
    /// come back in the ack, the rest through `response_url`.
    pub async fn command(self: &Arc<Self>, ev: SlackCommandEvent) -> SlackCommandEventResponse {
        let user = ev.user_id.0;
        let channel = ev.channel_id.0;
        let team = ev.team_id.0;
        let text = ev.text.unwrap_or_default();
        tracing::debug!(%user, %channel, %text, "slack command received");
        if !self.is_allowed(&user) {
            return ack(Reply::text("not allowed"));
        }
        let cmd = commands::classify(&text);
        if commands::is_instant(&cmd) {
            let cx = self.command_cx(&user, &channel);
            if let Ok(out) =
                tokio::time::timeout(ACK_BUDGET, commands::handle(cmd.clone(), cx)).await
            {
                return ack(self.after_command(out, &channel, &user, &team).await);
            }
        }
        let chan = Arc::clone(self);
        let url = ev.response_url.0.to_string();
        tokio::spawn(async move {
            let out = commands::handle(cmd, chan.command_cx(&user, &channel)).await;
            let reply = chan.after_command(out, &channel, &user, &team).await;
            chan.respond(&url, reply, false).await;
        });
        SlackCommandEventResponse::new(SlackMessageContent::new())
    }

    /// What a command or click by `user` in `channel` runs against: the
    /// user's latest thread here and the permalinks of bound threads.
    fn command_cx<'a>(&'a self, user: &'a str, channel: &'a str) -> CommandCx<'a> {
        let (session, links) = {
            let s = self.lock();
            let session = s.threads.latest_for(channel, user).map(|(_, t)| t.session);
            let links = s
                .permalinks
                .iter()
                .filter_map(|(k, url)| s.threads.get(k).map(|t| (t.session, url.clone())))
                .collect();
            (session, links)
        };
        CommandCx {
            user,
            owner: &self.owner,
            channel,
            session,
            status: self.status(),
            page: 0,
            links,
        }
    }

    /// Side effects of a command outcome; the reply to send.
    async fn after_command(&self, out: Outcome, channel: &str, user: &str, team: &str) -> Reply {
        match out {
            Outcome::Text(reply) | Outcome::Cancelled { reply, .. } => reply,
            Outcome::Resume { session, reply, .. } => {
                // A fresh root message in this channel becomes the session's thread.
                match self.api.post_markdown(channel, None, &reply.text).await {
                    Ok(ts) => self.lock().threads.bind(
                        thread_key(channel, &ts),
                        Thread::new(session, user.to_owned(), team.to_owned()),
                    ),
                    Err(e) => tracing::warn!(error = %e, "slack resume root post failed"),
                }
                reply
            }
        }
    }

    /// Posts `reply` to a `response_url`; `replace` updates the clicked
    /// panel in place (21 §10.2), which needs blocks, so text-only replies
    /// become one section there.
    async fn respond(&self, url: &str, reply: Reply, replace: bool) {
        let r = match (reply.blocks, replace) {
            (None, false) => self.api.respond(url, &reply.text).await,
            (blocks, replace) => {
                let blocks =
                    blocks.unwrap_or_else(|| ui::result(&reply.text).blocks.unwrap_or_default());
                self.api
                    .respond_blocks(url, &reply.text, blocks, replace)
                    .await
            }
        };
        if let Err(e) = r {
            tracing::warn!(error = %e, "slack response_url post failed");
        }
    }

    // -- trust (21 §5) ---------------------------------------------------

    fn is_allowed(&self, user: &str) -> bool {
        !user.is_empty() && (user == self.owner || self.lock().allowed.iter().any(|u| u == user))
    }

    // -- sends -----------------------------------------------------------

    /// `chat.postMessage` with `markdown_text` into a thread; the new ts.
    pub(crate) async fn post_markdown(
        &self,
        channel: &str,
        thread_ts: &str,
        md: &str,
    ) -> Option<String> {
        match self.api.post_markdown(channel, Some(thread_ts), md).await {
            Ok(ts) => Some(ts),
            Err(e) => {
                tracing::warn!(%channel, %thread_ts, error = %e, "slack post failed");
                None
            }
        }
    }

    async fn react(&self, channel: &str, ts: &str, name: &str) {
        if let Err(e) = self.api.react(channel, ts, name).await {
            tracing::debug!(%channel, %ts, name, error = %e, "slack reaction failed");
        }
    }

    // -- inbound (21 §7) -------------------------------------------------

    async fn handle_inbound(self: &Arc<Self>, ev: Inbound) {
        match ev {
            Inbound::Push(ev) => self.handle_push(*ev).await,
            Inbound::Action(ev) => {
                if let Some(click) = approvals::click(&ev) {
                    return self.handle_click(click).await;
                }
                if let Some(click) = asks::click(&ev) {
                    return self.handle_ask_click(click).await;
                }
                match *ev {
                    SlackInteractionEvent::BlockActions(ev) => self.handle_cmd_click(ev).await,
                    SlackInteractionEvent::ViewSubmission(ev) => self.handle_view(ev).await,
                    other => {
                        tracing::debug!(kind = ?std::mem::discriminant(&other), "slack interaction ignored")
                    }
                }
            }
        }
    }

    /// A `cmd:*` click (21 §10.2): trust check, then Add… buttons open
    /// their modal at once (the `trigger_id` lives 3 s) and everything else
    /// runs as the command it stands for, answering through the click's
    /// `response_url` with `replace_original`.
    async fn handle_cmd_click(self: &Arc<Self>, ev: SlackInteractionBlockActionsEvent) {
        let Some(a) = ev
            .actions
            .as_deref()
            .and_then(|a| a.iter().find(|a| a.action_id.0.starts_with("cmd:")))
        else {
            tracing::debug!("slack block_actions without a cmd action ignored");
            return;
        };
        // Overflow menus carry the command in the chosen option; selects
        // and radios carry the chosen value; buttons their `value`.
        let (action, value) = match &a.selected_option {
            Some(o) if o.value.starts_with("cmd:") => (o.value.clone(), None),
            Some(o) => (a.action_id.0.clone(), Some(o.value.clone())),
            None => (a.action_id.0.clone(), a.value.clone()),
        };
        let Some(user) = ev.user.map(|u| u.id.0) else {
            return;
        };
        let container_channel = match &ev.container {
            SlackInteractionActionContainer::Message(m) => m.channel_id.clone(),
            SlackInteractionActionContainer::MessageAttachment(m) => m.channel_id.clone(),
            SlackInteractionActionContainer::View(_) => None,
        };
        let channel = ev
            .channel
            .map(|c| c.id)
            .or(container_channel)
            .map(|c| c.0)
            .unwrap_or_default();
        if !self.is_allowed(&user) {
            tracing::warn!(%user, %action, "slack cmd click from a user who is not allowed");
            if !channel.is_empty() {
                self.ephemeral(&channel, &user, "not allowed").await;
            }
            return;
        }
        if action == "cmd:link" {
            return;
        }
        if let Some(kind @ ("skills" | "mcp")) = action
            .strip_prefix("cmd:")
            .and_then(|r| r.strip_suffix(":add"))
        {
            let view = ui::modal_add(kind, &channel);
            if let Err(e) = self.api.views_open(&ev.trigger_id.0, view).await {
                tracing::warn!(error = %e, "slack views.open failed");
            }
            return;
        }
        let Some(url) = ev.response_url.map(|u| u.0.to_string()) else {
            tracing::debug!(%action, "slack cmd click without response_url ignored");
            return;
        };
        let team = ev.team.id.0;
        let chan = Arc::clone(self);
        tokio::spawn(async move {
            let cx = chan.command_cx(&user, &channel);
            let out = commands::action(&action, value.as_deref(), cx).await;
            let reply = chan.after_command(out, &channel, &user, &team).await;
            chan.respond(&url, reply, true).await;
        });
    }

    /// A submitted Add… modal ([`ui::modal_add`]): the `url` input runs as
    /// `skills add` / `mcp add`; the answer is an ephemeral in the channel
    /// the modal was opened from (its `private_metadata`).
    async fn handle_view(self: &Arc<Self>, ev: SlackInteractionViewSubmissionEvent) {
        let SlackView::Modal(view) = ev.view.view else {
            return;
        };
        let callback = view.callback_id.map(|c| c.0).unwrap_or_default();
        let channel = view.private_metadata.unwrap_or_default();
        let input = ev
            .view
            .state_params
            .state
            .and_then(|s| {
                s.values
                    .get(&"url".into())?
                    .get(&"url".into())?
                    .value
                    .clone()
            })
            .unwrap_or_default();
        let user = ev.user.id.0;
        if channel.is_empty() {
            tracing::warn!(%callback, "slack view_submission without a channel ignored");
            return;
        }
        if !self.is_allowed(&user) {
            self.ephemeral(&channel, &user, "not allowed").await;
            return;
        }
        let team = ev.team.id.0;
        let cmd = commands::view_submission(&callback, &input);
        let chan = Arc::clone(self);
        tokio::spawn(async move {
            let out = commands::handle(cmd, chan.command_cx(&user, &channel)).await;
            let reply = chan.after_command(out, &channel, &user, &team).await;
            chan.ephemeral(&channel, &user, &reply.text).await;
        });
    }

    /// An Allow/Deny click (21 §10.1): owner or the card's thread user may
    /// answer; the card itself is updated when `ApprovalResolved` arrives.
    async fn handle_click(&self, click: approvals::Click) {
        let permitted = click.user == self.owner
            || self
                .lock()
                .cards
                .get(&click.id)
                .is_some_and(|c| c.thread_user.as_deref() == Some(click.user.as_str()));
        if !permitted {
            tracing::warn!(user = %click.user, "slack approval click from a user who may not answer it");
            self.ephemeral(&click.channel, &click.user, "not allowed")
                .await;
            return;
        }
        match self.port.resolve(click.id, click.decision).await {
            Ok(true) => {
                if let Some(c) = self.lock().cards.get_mut(&click.id) {
                    c.by = Some(click.user);
                }
            }
            Ok(false) => {
                self.ephemeral(&click.channel, &click.user, "already resolved")
                    .await
            }
            Err(e) => {
                tracing::warn!(error = %e, "resolve failed");
                self.ephemeral(&click.channel, &click.user, &format!("⚠️ {e}"))
                    .await;
            }
        }
    }

    /// A question-card click (21 §10.1 shape): owner or the card's thread user
    /// may answer. The clicked message is finalised at once; the next question
    /// is posted or the answers handed to the agent.
    async fn handle_ask_click(self: &Arc<Self>, click: asks::Click) {
        let (blocks, next) = match self.ask_step(&click) {
            Ok(step) => step,
            Err(msg) => {
                tracing::debug!(user = %click.user, msg, "slack ask click refused");
                return self.ephemeral(&click.channel, &click.user, msg).await;
            }
        };
        if let Err(e) = self
            .api
            .update_blocks(&click.channel, &click.message_ts, blocks)
            .await
        {
            tracing::debug!(error = %e, "ask card update failed");
        }
        let answers = match next {
            AskNext::Post(i) => return self.post_question(click.id, i).await,
            AskNext::Answer(answers) => Some(answers),
            AskNext::Dismiss => None,
        };
        match self.port.answer_ask(click.id, answers).await {
            Ok(true) => {}
            Ok(false) => {
                self.ephemeral(&click.channel, &click.user, "already answered")
                    .await
            }
            Err(e) => {
                tracing::warn!(error = %e, "answer_ask failed");
                self.ephemeral(&click.channel, &click.user, &format!("⚠️ {e}"))
                    .await;
            }
        }
    }

    /// Applies `click` to its card under the lock: the blocks replacing the
    /// clicked message and what follows; `Err` = the ephemeral to send instead.
    fn ask_step(&self, click: &asks::Click) -> Result<(serde_json::Value, AskNext), &'static str> {
        let mut s = self.lock();
        let card = s.asks.get_mut(&click.id);
        let permitted = click.user == self.owner
            || card
                .as_deref()
                .is_some_and(|c| c.thread_user.as_deref() == Some(click.user.as_str()));
        if !permitted {
            return Err("not allowed");
        }
        let Some(card) = card else {
            return Err("already answered");
        };
        if card.done || click.question.is_some_and(|q| q != card.current) {
            return Err("already answered");
        }
        let i = card.current;
        let answer = match &click.kind {
            asks::Kind::Dismiss => {
                card.done = true;
                return Ok((
                    asks::closed_card(&card.req, i, true, false),
                    AskNext::Dismiss,
                ));
            }
            asks::Kind::Pick(index) if *index >= card.req.questions[i].options.len() => {
                return Err("unknown option");
            }
            asks::Kind::Pick(index) => Answer::Choice { index: *index },
            asks::Kind::Text(text) => Answer::Text { text: text.clone() },
            asks::Kind::Skip => Answer::Skipped,
        };
        let blocks = asks::answered_card(&card.req, i, &answer);
        card.answers.push(answer);
        card.current += 1;
        let next = if card.current < card.req.questions.len() {
            card.ts.clear();
            AskNext::Post(card.current)
        } else {
            card.done = true;
            AskNext::Answer(card.answers.clone())
        };
        Ok((blocks, next))
    }

    /// Posts question `i` of `id`: through the thread's worker for a bound
    /// session, straight into the owner DM otherwise.
    async fn post_question(self: &Arc<Self>, id: AskId, i: usize) {
        let card = self
            .lock()
            .asks
            .get(&id)
            .map(|c| (c.req.clone(), c.context.clone(), c.channel.clone()));
        let Some((req, context, channel)) = card else {
            return;
        };
        let Some(context) = context else {
            if let Some((key, thread)) = self.bound(req.session) {
                let _ = self.worker(&key, &thread).send(Out::Ask(Box::new(req), i));
            }
            return;
        };
        match self
            .api
            .post_blocks(&channel, None, asks::question_card(&req, i, Some(&context)))
            .await
        {
            Ok(ts) => {
                if let Some(c) = self.lock().asks.get_mut(&id) {
                    c.ts = ts;
                }
            }
            Err(e) => tracing::warn!(error = %e, "owner ask card post failed"),
        }
    }

    async fn ephemeral(&self, channel: &str, user: &str, text: &str) {
        if let Err(e) = self.api.ephemeral(channel, user, text).await {
            tracing::debug!(error = %e, "slack ephemeral failed");
        }
    }

    async fn handle_push(self: &Arc<Self>, ev: SlackPushEventCallback) {
        let team = ev.team_id.0;
        match ev.event {
            SlackEventCallbackBody::Message(m) => {
                // 45 §6: `file_share` is how voice clips arrive on some workspaces.
                if m.sender.bot_id.is_some()
                    || m.subtype
                        .as_ref()
                        .is_some_and(|s| *s != SlackMessageEventType::FileShare)
                {
                    return;
                }
                if m.origin.channel_type.as_ref().map(|t| t.0.as_str()) != Some("im") {
                    return;
                }
                let (Some(channel), Some(user)) = (m.origin.channel, m.sender.user) else {
                    return;
                };
                if !self.is_allowed(&user.0) {
                    tracing::warn!(user = %user.0, "dropping slack DM from a user not in the allowlist");
                    return;
                }
                let content = m.content.unwrap_or_else(SlackMessageContent::new);
                self.handle_message(Msg {
                    channel: channel.0,
                    ts: m.origin.ts.0,
                    thread_ts: m.origin.thread_ts.map(|t| t.0),
                    user: user.0,
                    team,
                    text: content.text.unwrap_or_default(),
                    files: content.files.unwrap_or_default(),
                    dm: true,
                })
                .await;
            }
            SlackEventCallbackBody::AppMention(a) => {
                if !self.lock().channels.contains(&a.channel.0) {
                    tracing::debug!(channel = %a.channel.0, "slack mention in an unlisted channel ignored");
                    return;
                }
                if !self.is_allowed(&a.user.0) {
                    self.ephemeral(&a.channel.0, &a.user.0, "not allowed").await;
                    return;
                }
                self.handle_message(Msg {
                    channel: a.channel.0,
                    ts: a.origin.ts.0,
                    thread_ts: a.origin.thread_ts.map(|t| t.0),
                    user: a.user.0,
                    team,
                    text: a.content.text.unwrap_or_default(),
                    files: a.content.files.unwrap_or_default(),
                    dm: false,
                })
                .await;
            }
            SlackEventCallbackBody::AgentSessionStopped(e) => {
                let Some(thread_ts) = e.thread_ts else { return };
                let key = thread_key(&e.channel.0, &thread_ts.0);
                let session = self.lock().threads.get(&key).map(|t| t.session);
                let Some(session) = session else { return };
                tracing::info!(%session, "slack stop button → cancel");
                if let Err(e) = self.port.cancel(session).await {
                    tracing::warn!(error = %e, "cancel failed");
                }
            }
            SlackEventCallbackBody::AgentSessionTitleChanged(e) => {
                let Some(thread_ts) = e.thread_ts else { return };
                let key = thread_key(&e.channel.0, &thread_ts.0);
                self.lock().threads.set_title(&key, Some(e.title));
            }
            SlackEventCallbackBody::Unknown(_) => {
                self.lock().dropped += 1;
                tracing::debug!("unknown slack event type acked and dropped");
            }
            other => tracing::debug!(kind = ?std::mem::discriminant(&other), "slack event ignored"),
        }
    }

    /// A user message in a DM or a listed channel: files, 👀, ask, bind (21 §7).
    /// `<name>: …` addresses a roster agent, `theuth: …` the default (67 §10);
    /// a thread stays with the agent it was last addressed to.
    async fn handle_message(self: &Arc<Self>, m: Msg) {
        let mut lines: Vec<String> = Vec::new();
        let thread_ts = m.thread_ts.clone().unwrap_or_else(|| m.ts.clone());
        let key = thread_key(&m.channel, &thread_ts);
        let bound = self.lock().threads.get(&key).cloned();
        let agents = routable().await;
        let names: Vec<String> = agents
            .iter()
            .map(|a| a.name.clone())
            .chain(std::iter::once("theuth".to_owned()))
            .collect();
        let text = strip_mentions(&m.text).trim().to_owned();
        let (text, agent) = match roster::routing::route(&text, &names) {
            Some(r) if r.agent == "theuth" => (r.rest.to_owned(), None),
            Some(r) => (r.rest.to_owned(), Some(r.agent)),
            None => (text, bound.as_ref().and_then(|t| t.agent.clone())),
        };
        if !text.is_empty() {
            // 110 §7.1: one marker line so the model writes for Slack. File and
            // voice-note lines carry their own markers and none of this one.
            let marker = if m.dm {
                "[slack dm]"
            } else {
                "[slack channel]"
            };
            lines.push(format!("{marker}\n{text}"));
        }
        let (file_lines, files, voice) = self.save_files(&m.ts, &m.files).await;
        lines.extend(file_lines);
        if lines.is_empty() {
            return;
        }
        let input = lines.join("\n");
        self.react(&m.channel, &m.ts, EYES).await;
        if let Some(agent) = agent {
            let info = agents.into_iter().find(|a| a.name == agent);
            let before = bound.as_ref().map(|t| (t.session, t.agent_title.clone()));
            match self.to_agent(&m, bound, agent, info, input).await {
                Ok(thread) => {
                    {
                        let mut s = self.lock();
                        // A new title or session: the worker holds both; respawn it.
                        if before != Some((thread.session, thread.agent_title.clone())) {
                            s.workers.remove(&key);
                        }
                        s.threads.bind(key.clone(), thread.clone());
                    }
                    let _ = self.worker(&key, &thread).send(Out::Mark(m.ts, voice));
                }
                Err(e) => self.failed(&m, &thread_ts, &e).await,
            }
            return;
        }
        if bound.as_ref().is_some_and(|t| t.agent.is_some()) {
            // `theuth: …` in an agent's thread: back to a theuth root of its own.
            let mut s = self.lock();
            s.threads.unbind(&key);
            s.workers.remove(&key);
        }
        let (session, running) = {
            let s = self.lock();
            let session = s.threads.get(&key).map(|t| t.session);
            (session, session.is_some_and(|id| s.running.contains(&id)))
        };
        let result = match session {
            Some(id) if running => self
                .port
                .steer(id, InputId::new(), input, files)
                .await
                .map(|()| id),
            Some(id) => self.port.send_input(Some(id), input, files).await,
            // A new root: the authenticated owner's is `owner` scope, anyone
            // else's stays `isolated` (50 §2.3). An existing thread's session
            // is never promoted here; `/theuth peers scope` does that explicitly.
            None if m.user == self.owner => match self.port.new_root(PeerScope::Owner).await {
                Ok(id) => self.port.send_input(Some(id), input, files).await,
                Err(e) => Err(e),
            },
            None => self.port.send_input(None, input, files).await,
        };
        match result {
            Ok(id) => {
                let thread = Thread::new(id, m.user.clone(), m.team.clone());
                {
                    let mut s = self.lock();
                    if session == Some(id) {
                        s.threads.touch(&key);
                    } else {
                        s.threads.bind(key.clone(), thread.clone());
                    }
                }
                let _ = self.worker(&key, &thread).send(Out::Mark(m.ts, voice));
            }
            Err(e) => self.failed(&m, &thread_ts, &e).await,
        }
    }

    /// The ⚠️ line and ❌ for a message no agent took.
    async fn failed(&self, m: &Msg, thread_ts: &str, e: &str) {
        tracing::warn!(error = %e, "agent ask failed");
        self.post_markdown(&m.channel, thread_ts, &format!("⚠️ {e}"))
            .await;
        self.react(&m.channel, &m.ts, FAILED).await;
    }

    /// 67 §10: `input` for `agent` — a task on the first message of the
    /// thread, a mention on its thread afterwards; the thread record now
    /// renders the agent's desk (`info.desk`) so its replies land here.
    async fn to_agent(
        &self,
        m: &Msg,
        bound: Option<Thread>,
        agent: String,
        info: Option<roster::Routable>,
        input: String,
    ) -> Result<Thread, String> {
        let desk: Option<SessionId> = info
            .as_ref()
            .and_then(|a| a.desk.as_deref())
            .and_then(|d| d.parse().ok());
        // ponytail: no desk and no prior thread → a placeholder session nobody
        // emits for; the next message in the thread re-reads the desk.
        let mut thread = bound.unwrap_or_else(|| {
            Thread::new(desk.unwrap_or_default(), m.user.clone(), m.team.clone())
        });
        if let Some(d) = desk {
            thread.session = d;
        }
        if let Some(a) = &info {
            thread.agent_title = Some(a.title.clone());
        }
        thread.agent = Some(agent.clone());
        let tasks = tasks::actor().ok_or("tasks actor is not running")?;
        match &thread.task {
            None => {
                let task = tasks
                    .ask(tasks::CreateTask {
                        input: tasks::TaskCreate {
                            state: tasks::TaskState::Todo,
                            title: input
                                .lines()
                                .next()
                                .unwrap_or("")
                                .chars()
                                .take(80)
                                .collect(),
                            brief: input,
                            owner: tasks::Owner::Agent(agent),
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
                    .map_err(|e| e.to_string())?;
                thread.task = Some(task.id.0);
            }
            Some(id) => tasks
                .ask(tasks::Mention {
                    id: tasks::TaskId(id.clone()),
                    text: input,
                    actor: "user".into(),
                    session: None,
                })
                .await
                .map_err(|e| e.to_string())?,
        }
        Ok(thread)
    }

    /// Downloads `files` to `<data_dir>/slack/inbox/<ts>-<name>`; one
    /// `[attachment: …]` line each plus the paths of images and PDFs (21 §7.1).
    /// Audio is transcribed into a `[voice note, N s]` line (45 §4.5.1); the
    /// flag says at least one was.
    async fn save_files(&self, ts: &str, files: &[SlackFile]) -> (Vec<String>, Vec<PathBuf>, bool) {
        let mut lines = Vec::with_capacity(files.len());
        let mut paths = Vec::new();
        let mut voice = false;
        if files.is_empty() {
            return (lines, paths, voice);
        }
        let dir = self.rt.data_dir.join("slack").join("inbox");
        if let Err(e) = tokio::fs::create_dir_all(&dir).await {
            tracing::warn!(error = %e, "failed to create slack inbox dir");
            return (lines, paths, voice);
        }
        for f in files {
            let name: String = f
                .name
                .clone()
                .or_else(|| f.title.clone())
                .unwrap_or_else(|| f.id.0.clone())
                .chars()
                .map(|c| if c == '/' || c == '\\' { '_' } else { c })
                .collect();
            let mime = f
                .mimetype
                .as_ref()
                .map_or("application/octet-stream", |m| m.0.as_str());
            let Some(url) = &f.url_private_download else {
                lines.push(format!(
                    "[attachment: {name} — download failed: no url_private_download]"
                ));
                continue;
            };
            let saved = match self.api.download(url.as_str()).await {
                Ok(bytes) => {
                    let path = dir.join(format!("{ts}-{name}"));
                    tokio::fs::write(&path, &bytes)
                        .await
                        .map(|()| (path, bytes.len()))
                        .map_err(|e| e.to_string())
                }
                Err(e) => Err(e.to_string()),
            };
            match saved {
                Ok((path, len)) => {
                    if mime.starts_with("audio/") {
                        match transcribe(&path).await {
                            Ok(t) if t.text.is_empty() => lines.push(format!(
                                "[voice note, {} s] (nothing recognized)",
                                t.duration_ms.div_ceil(1000).max(1)
                            )),
                            Ok(t) => {
                                lines.push(voice::note_text(&t.text, t.duration_ms));
                                voice = true;
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, path = %path.display(), "voice note not transcribed");
                                lines.push(format!(
                                    "[attachment: {} ({mime}, {})] (transcription unavailable)",
                                    path.display(),
                                    fmt_size(len)
                                ));
                            }
                        }
                        continue;
                    }
                    lines.push(format!(
                        "[attachment: {} ({mime}, {})]",
                        path.display(),
                        fmt_size(len)
                    ));
                    if mime.starts_with("image/") || mime == "application/pdf" {
                        paths.push(path);
                    }
                }
                Err(e) => lines.push(format!("[attachment: {name} — download failed: {e}]")),
            }
        }
        (lines, paths, voice)
    }

    // -- outbound (21 §8) ------------------------------------------------

    /// Bus event → the bound thread's worker, plus every bound ancestor's
    /// worker for a child's events (21 §9.2); unbound sessions are dropped.
    async fn route(self: &Arc<Self>, ev: Event) {
        match ev {
            Event::Agent(AgentEvent::SubagentSpawned {
                parent,
                child,
                name,
                ..
            }) => {
                self.spawned(parent, child, name).await;
            }
            Event::Agent(ev) => {
                let Some(session) = ev.session() else { return };
                {
                    let mut s = self.lock();
                    match &ev {
                        AgentEvent::TurnStarted { .. } => {
                            s.running.insert(session);
                        }
                        AgentEvent::TurnFinished { .. } => {
                            s.running.remove(&session);
                        }
                        _ => {}
                    }
                }
                for (key, thread, _) in self.ancestors(session) {
                    let _ = self
                        .worker(&key, &thread)
                        .send(Out::Child(session, ev.clone()));
                }
                let Some((key, thread)) = self.bound(session) else {
                    return;
                };
                let _ = self.worker(&key, &thread).send(Out::Agent(ev));
            }
            Event::Requested(req) => {
                for (key, thread, _) in self.ancestors(req.session) {
                    let _ = self
                        .worker(&key, &thread)
                        .send(Out::ChildApproval(req.session, Some(req.tool.clone())));
                }
                match self.bound(req.session) {
                    Some((key, thread)) => {
                        self.lock().cards.insert(
                            req.id,
                            Card {
                                session: req.session,
                                thread_user: Some(thread.user.clone()),
                                by: None,
                                dm: None,
                            },
                        );
                        let _ = self
                            .worker(&key, &thread)
                            .send(Out::Approval(Box::new(req)));
                    }
                    None => self.owner_card(req).await,
                }
            }
            Event::Resolved(res) => {
                let card = self.lock().cards.remove(&res.id);
                let Some(card) = card else { return };
                for (key, thread, _) in self.ancestors(card.session) {
                    let _ = self
                        .worker(&key, &thread)
                        .send(Out::ChildApproval(card.session, None));
                }
                match (card.dm, self.bound(card.session)) {
                    (Some((ts, body, _)), _) => {
                        let Some(dm) = self.identity.get().and_then(|i| i.owner_dm.as_deref())
                        else {
                            return;
                        };
                        let blocks = approvals::resolved_card(
                            &body,
                            res.decision,
                            res.timed_out,
                            card.by.as_deref(),
                        );
                        if let Err(e) = self.api.update_blocks(dm, &ts, blocks).await {
                            tracing::debug!(error = %e, "owner approval card update failed");
                        }
                    }
                    (None, Some((key, thread))) => {
                        let _ = self.worker(&key, &thread).send(Out::Resolved(res, card.by));
                    }
                    (None, None) => {}
                }
            }
            Event::Ask(req) => {
                let (thread_user, channel, context) = match self.bound(req.session) {
                    Some((key, thread)) => (Some(thread.user), split_key(&key).0.to_owned(), None),
                    None => {
                        if self.approvals != ApprovalsScope::All {
                            return;
                        }
                        let Some(dm) = self.identity.get().and_then(|i| i.owner_dm.clone()) else {
                            return;
                        };
                        let context = self.session_context(req.session).await;
                        (None, dm, Some(context))
                    }
                };
                let id = req.id;
                self.lock().asks.insert(
                    id,
                    AskCard {
                        req,
                        thread_user,
                        current: 0,
                        answers: Vec::new(),
                        channel,
                        ts: String::new(),
                        context,
                        done: false,
                    },
                );
                self.post_question(id, 0).await;
            }
            Event::AskDone(res) => {
                let card = self.lock().asks.remove(&res.id);
                let Some(card) = card else { return };
                if card.done || card.ts.is_empty() {
                    return;
                }
                let blocks =
                    asks::closed_card(&card.req, card.current, res.dismissed, res.timed_out);
                if let Err(e) = self
                    .api
                    .update_blocks(&card.channel, &card.ts, blocks)
                    .await
                {
                    tracing::debug!(error = %e, "ask card update failed");
                }
            }
            // 126 §10: the chip lands after the card — the card is rebuilt with the
            // double's context line. Owner-DM approval cards and every ask card are
            // updated here; a thread approval card by the worker that holds its ts.
            Event::Predicted(p) => {
                let Ok(id) = uuid::Uuid::parse_str(&p.question_id) else {
                    return;
                };
                let approval = self
                    .lock()
                    .cards
                    .get(&ApprovalId(id))
                    .map(|c| (c.session, c.dm.clone()));
                if let Some((session, dm)) = approval {
                    match (dm, self.bound(session)) {
                        (Some((ts, body, context)), _) => {
                            let Some(dm) = self.identity.get().and_then(|i| i.owner_dm.as_deref())
                            else {
                                return;
                            };
                            let blocks = approvals::predicted_card(
                                &body,
                                ApprovalId(id),
                                &p,
                                Some(context.as_str()),
                            );
                            if let Err(e) = self.api.update_blocks(dm, &ts, blocks).await {
                                tracing::debug!(error = %e, "owner approval card double update failed");
                            }
                        }
                        (None, Some((key, thread))) => {
                            let _ = self.worker(&key, &thread).send(Out::Predicted(Box::new(p)));
                        }
                        (None, None) => {}
                    }
                    return;
                }
                let ask = self.lock().asks.get(&AskId(id)).filter(|c| !c.ts.is_empty()).map(|c| {
                    (
                        c.channel.clone(),
                        c.ts.clone(),
                        asks::question_card(&c.req, c.current, c.context.as_deref()),
                    )
                });
                if let Some((channel, ts, blocks)) = ask
                    && let Err(e) = self
                        .api
                        .update_blocks(&channel, &ts, approvals::with_double(blocks, &p))
                        .await
                {
                    tracing::debug!(error = %e, "ask card double update failed");
                }
            }
            // 126 §10: one line where the card is — the session's thread, else the owner DM.
            Event::DoubleAnswered(a) => {
                match a.session.and_then(|s| self.bound(s)) {
                    Some((key, thread)) => {
                        let _ = self
                            .worker(&key, &thread)
                            .send(Out::Note(approvals::answered_text(&a)));
                    }
                    None => {
                        let Some(dm) = self.identity.get().and_then(|i| i.owner_dm.as_deref())
                        else {
                            return;
                        };
                        let blocks = approvals::answered_blocks(&a);
                        if let Err(e) = self.api.post_blocks(dm, None, blocks).await {
                            tracing::debug!(error = %e, "owner double answered line failed");
                        }
                    }
                }
            }
            // 35 §8: the session's own thread, else the nearest bound ancestor's.
            Event::Artifact(m) => {
                let target = self.bound(m.session).or_else(|| {
                    self.ancestors(m.session)
                        .into_iter()
                        .next()
                        .map(|(k, t, _)| (k, t))
                });
                let Some((key, thread)) = target else {
                    tracing::debug!(session = %m.session, name = %m.artifact.name, "artifact for an unbound session; not posted");
                    return;
                };
                let _ = self
                    .worker(&key, &thread)
                    .send(Out::Artifact(Box::new(m.artifact)));
            }
            // 37 §7: the model asked the user for a secret; say where to enter it
            // (never here). The session's thread, else the nearest bound
            // ancestor's, else the owner DM (Ask's routing).
            Event::Secret(secrets::SecretsEvent::Requested {
                session,
                name,
                note,
                ..
            }) => {
                let md = secret_notice(&name, &note);
                let target = self.bound(session).or_else(|| {
                    self.ancestors(session)
                        .into_iter()
                        .next()
                        .map(|(k, t, _)| (k, t))
                });
                match target {
                    Some((key, thread)) => {
                        let _ = self.worker(&key, &thread).send(Out::Note(md));
                    }
                    None => {
                        if self.approvals != ApprovalsScope::All {
                            return;
                        }
                        let Some(dm) = self.identity.get().and_then(|i| i.owner_dm.as_deref())
                        else {
                            return;
                        };
                        if let Err(e) = self.api.post_markdown(dm, None, &md).await {
                            tracing::debug!(error = %e, "secret notice to the owner DM failed");
                        }
                    }
                }
            }
            Event::Secret(_) => {}
            Event::Lease(lease) => {
                self.lock().follower.observe(&lease);
                self.sync_socket().await;
            }
            Event::Peer(p) => {
                if let Some((key, thread)) = self.bound(p.session) {
                    let _ = self.worker(&key, &thread).send(Out::Peer(Box::new(p)));
                }
            }
        }
    }

    /// The thread rendering `session`, cloned out of the lock.
    pub(crate) fn bound(&self, session: SessionId) -> Option<(ThreadKey, Thread)> {
        self.lock()
            .threads
            .by_session(session)
            .map(|(k, t)| (k.clone(), t.clone()))
    }

    /// `session`'s bound ancestors, nearest first, each with `session`'s
    /// depth below it (1 = direct child).
    fn ancestors(&self, session: SessionId) -> Vec<(ThreadKey, Thread, u8)> {
        let s = self.lock();
        let mut out = Vec::new();
        let mut cur = session;
        let mut depth: u8 = 0;
        while let Some((parent, _)) = s.parent_of.get(&cur)
            && depth < 16
        {
            depth += 1;
            cur = *parent;
            if let Some((k, t)) = s.threads.by_session(cur) {
                out.push((k.clone(), t.clone(), depth));
            }
        }
        out
    }

    /// An approval for a session no thread renders: the owner's DM when
    /// `slack.approvals: all` (21 §10.1).
    async fn owner_card(&self, req: ApprovalRequested) {
        if self.approvals != ApprovalsScope::All {
            return;
        }
        let Some(dm) = self.identity.get().and_then(|i| i.owner_dm.clone()) else {
            return;
        };
        let context = self.session_context(req.session).await;
        let body = approvals::body(&req);
        match self
            .api
            .post_blocks(&dm, None, approvals::card(&req, Some(&context)))
            .await
        {
            Ok(ts) => {
                self.lock().cards.insert(
                    req.id,
                    Card {
                        session: req.session,
                        thread_user: None,
                        by: None,
                        dm: Some((ts, body, context)),
                    },
                );
            }
            Err(e) => tracing::warn!(error = %e, "owner approval card post failed"),
        }
    }

    /// `session <last 8> · <title>` for owner-DM cards; title from `ListSessions`,
    /// `Unknown` when the ask fails.
    async fn session_context(&self, session: SessionId) -> String {
        let title = match self.port.list_sessions().await {
            Ok(sessions) => sessions
                .into_iter()
                .find(|s| s.id == session)
                .map(|s| s.title)
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| "Unknown".to_owned()),
            Err(e) => {
                tracing::debug!(error = %e, "list_sessions for the card context failed");
                "Unknown".to_owned()
            }
        };
        let id = session.to_string();
        format!("session {} · {title}", &id[id.len().saturating_sub(8)..])
    }

    /// `SubagentSpawned` for a bound parent (21 §9.1): a new top-level
    /// message in the parent's channel becomes the child's thread.
    async fn spawned(self: &Arc<Self>, parent: SessionId, child: SessionId, name: String) {
        let Some((pkey, pthread)) = self.bound(parent) else {
            return;
        };
        let (channel, parent_ts) = split_key(&pkey);
        let parent_link = self.permalink(channel, parent_ts).await;
        let root = format!("↳ **{name}** · child of <{parent_link}|this thread>");
        let ts = match self.api.post_markdown(channel, None, &root).await {
            Ok(ts) => ts,
            Err(e) => {
                tracing::warn!(error = %e, %name, "child root post failed");
                return;
            }
        };
        let key = thread_key(channel, &ts);
        let thread = Thread::new(child, pthread.user.clone(), pthread.team.clone());
        {
            let mut s = self.lock();
            s.parent_of.insert(child, (parent, name.clone()));
            s.threads.bind(key.clone(), thread.clone());
        }
        let link = self.permalink(channel, &ts).await;
        let _ = self.worker(&key, &thread);
        for (akey, athread, depth) in self.ancestors(child) {
            let _ = self.worker(&akey, &athread).send(Out::Spawned {
                child,
                name: name.clone(),
                link: link.clone(),
                depth,
            });
        }
    }

    /// `chat.getPermalink`, cached per thread; a built archive URL when it fails.
    async fn permalink(&self, channel: &str, ts: &str) -> String {
        let key = thread_key(channel, ts);
        if let Some(link) = self.lock().permalinks.get(&key) {
            return link.clone();
        }
        match self.api.permalink(channel, ts).await {
            Ok(link) => {
                self.lock().permalinks.insert(key, link.clone());
                link
            }
            Err(e) => {
                tracing::debug!(error = %e, "chat.getPermalink failed; building the link");
                format!(
                    "https://slack.com/archives/{channel}/p{}",
                    ts.replace('.', "")
                )
            }
        }
    }

    /// The worker for `key`, spawned on first use.
    fn worker(self: &Arc<Self>, key: &str, thread: &Thread) -> mpsc::UnboundedSender<Out> {
        let mut s = self.lock();
        if let Some(tx) = s.workers.get(key)
            && !tx.is_closed()
        {
            return tx.clone();
        }
        let (channel, thread_ts) = split_key(key);
        let (tx, rx) = mpsc::unbounded_channel();
        let worker = Worker {
            chan: Arc::clone(self),
            key: key.to_owned(),
            channel: channel.to_owned(),
            thread_ts: thread_ts.to_owned(),
            user: thread.user.clone(),
            team: thread.team.clone(),
            session: thread.session,
            name: s.parent_of.get(&thread.session).map(|(_, n)| n.clone()),
            agent_title: thread.agent_title.clone(),
            task_posted: false,
            turn: None,
            started: Instant::now(),
            model: String::new(),
            context_size: None,
            marks: Vec::new(),
            status_supported: true,
            title_sent: None,
            open_stream: None,
            approvals: HashMap::new(),
            rows: Vec::new(),
            rows_dirty: false,
            board: None,
            voice: false,
            unspoken: String::new(),
            clip_tail: None,
            plan: plan::Plan::default(),
            compacted: None,
            board_creates: HashMap::new(),
        };
        tokio::spawn(worker.run(rx));
        s.workers.insert(key.to_owned(), tx.clone());
        tx
    }
}

/// `"C…:ts"` → `(channel, ts)`.
pub(crate) fn split_key(key: &str) -> (&str, &str) {
    key.split_once(':').map_or((key, ""), |(c, t)| (c, t))
}

/// `Transcribe` on the `voice` plugin's actor; `Err` when it is not running (45 §4.5.1).
async fn transcribe(path: &Path) -> Result<voice::Transcript, String> {
    let stt = ActorRef::<voice::Transcriber>::lookup(voice::TRANSCRIBER_ACTOR)
        .ok()
        .flatten()
        .ok_or("voice plugin not running")?;
    stt.ask(voice::Transcribe {
        path: path.to_path_buf(),
    })
    .await
    .map_err(|e| e.to_string())
}

/// An ephemeral slash-command ack; blocks that morphism cannot type are
/// dropped for the text (logged).
fn ack(reply: Reply) -> SlackCommandEventResponse {
    let blocks = reply.blocks.and_then(|b| match crate::api::blocks(b) {
        Ok(b) => Some(b),
        Err(e) => {
            tracing::warn!(error = %e, "slack ack blocks dropped");
            None
        }
    });
    SlackCommandEventResponse::new(
        SlackMessageContent::new()
            .with_text(reply.text)
            .opt_blocks(blocks),
    )
    .with_response_type(SlackMessageResponseType::Ephemeral)
}

/// A user message after trust and type checks.
struct Msg {
    channel: String,
    ts: String,
    thread_ts: Option<String>,
    user: String,
    team: String,
    text: String,
    files: Vec<SlackFile>,
    /// A direct message (`channel_type == "im"`), not a channel mention (110 §7.1).
    dm: bool,
}

/// One ordered task per bound thread holding all per-thread state (21 §8).
struct Worker {
    chan: Arc<Channel>,
    key: ThreadKey,
    channel: String,
    thread_ts: String,
    user: String,
    team: String,
    session: SessionId,
    /// `TurnStarted` … `TurnFinished`.
    turn: Option<Turn>,
    started: Instant,
    model: String,
    /// The session's context window from `ContextUsed.size` (75 §5), for
    /// the footer's ` · ctx used/size`; `None` until a harness reports one.
    context_size: Option<u32>,
    /// User message ts awaiting ✅ / 🛑 / ❌.
    marks: Vec<String>,
    /// Cleared once `agents.sessions.setStatus` says the app is not an agent.
    status_supported: bool,
    title_sent: Option<String>,
    /// Stream ts last written to `threads.json.streams`.
    open_stream: Option<String>,
    /// The subagent's name when this thread renders a child (21 §9.1).
    name: Option<String>,
    /// The roster agent's title when this thread is addressed to one (67 §10).
    agent_title: Option<String>,
    /// The child's `▸ **task**` line went out.
    task_posted: bool,
    /// Approval cards posted here: id → `(ts, body)` (21 §10.1).
    approvals: HashMap<ApprovalId, (String, String)>,
    /// One row per descendant (21 §9.2).
    rows: Vec<board::Row>,
    /// A row changed since the board was last rendered.
    rows_dirty: bool,
    /// The board message `(ts, last edit)` while a descendant is live.
    board: Option<(String, Instant)>,
    /// The last input was a voice note: prose blocks also go out as clips (45 §4.5.2).
    voice: bool,
    /// Markdown streamed since the last clip (45 §6).
    unspoken: String,
    /// The previous block's clip upload; the next waits for it (45 §4.5.4).
    clip_tail: Option<tokio::task::JoinHandle<()>>,
    /// The plan message (110 §5).
    plan: plan::Plan,
    /// `Compacted { epoch }` seen between turns; handed to the next `Turn` for its footer (110 §6).
    compacted: Option<u32>,
    /// 110 §8: open `doc_create kind: board` calls by `call_id` → title; the
    /// result posts a `card`.
    board_creates: HashMap<String, String>,
}

/// `agents.sessions.setStatus` codes meaning "not an agent app" (21 §8.6).
const STATUS_UNSUPPORTED: [&str; 3] = [
    "missing_scope",
    "not_allowed_token_type",
    "invalid_arguments",
];

impl Worker {
    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Out>) {
        self.seed().await;
        let mut tick = tokio::time::interval(bubble::TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                m = rx.recv() => match m {
                    None => return,
                    Some(Out::Mark(ts, voice)) => {
                        self.marks.push(ts);
                        self.voice = voice;
                    }
                    Some(Out::Agent(ev)) => self.apply(ev).await,
                    Some(Out::Approval(req)) => self.approval(*req).await,
                    Some(Out::Resolved(res, by)) => self.resolved(res, by.as_deref()).await,
                    Some(Out::Predicted(p)) => self.predicted(&p).await,
                    Some(Out::Ask(req, i)) => self.ask(*req, i).await,
                    Some(Out::Spawned { child, name, link, depth }) => self.spawned(child, name, link, depth),
                    Some(Out::Child(session, ev)) => self.child(session, &ev).await,
                    Some(Out::Artifact(a)) => self.artifact(*a).await,
                    Some(Out::Note(md)) => { self.post_markdown_in_thread(&md).await; }
                    Some(Out::Peer(p)) => self.peer(*p).await,
                    Some(Out::ChildApproval(session, tool)) => {
                        if let Some(row) = self.rows.iter_mut().find(|r| r.session == session) {
                            self.rows_dirty |= row.approval(tool.as_deref());
                        }
                    }
                },
                _ = tick.tick() => self.tick().await,
            }
        }
    }

    /// Descendants that already exist (a restart): parent links for routing
    /// and seeded board rows, from one `list_agents` (21 §9.2).
    async fn seed(&mut self) {
        let infos = match self.chan.port.list_agents(self.session).await {
            Ok(infos) => infos,
            Err(e) => {
                tracing::debug!(error = %e, "list_agents for the board failed");
                return;
            }
        };
        if infos.is_empty() {
            return;
        }
        let mut s = self.chan.lock();
        for info in &infos {
            s.parent_of
                .insert(info.id, (info.parent, info.name.clone()));
            let link = s
                .threads
                .by_session(info.id)
                .and_then(|(k, _)| s.permalinks.get(k).cloned());
            let mut row = board::Row::new(info.id, &info.name, info.depth, link);
            row.seed(info);
            self.rows.push(row);
        }
        self.rows_dirty = board::any_live(&self.rows);
    }

    async fn tick(&mut self) {
        if let Some(t) = &mut self.turn {
            t.tick().await;
        }
        self.sync_stream();
        if self.rows_dirty
            && (self.board.is_some() || board::any_live(&self.rows))
            && self
                .board
                .as_ref()
                .is_none_or(|(_, at)| at.elapsed() >= board::BOARD_MIN_GAP)
        {
            self.render_board().await;
        }
        self.flush_plan(false).await;
    }

    /// 110 §5: posts or edits the plan message when an edit is due; `force`
    /// on `TurnFinished`.
    async fn flush_plan(&mut self, force: bool) {
        let Some(edit) = self.plan.take_due(Instant::now(), force) else {
            return;
        };
        match edit.ts {
            Some(ts) => {
                if let Err(e) = self
                    .chan
                    .api
                    .update_blocks(&self.channel, &ts, edit.blocks)
                    .await
                {
                    tracing::debug!(error = %e, "plan update failed");
                }
            }
            None => match self
                .chan
                .api
                .post_blocks(&self.channel, Some(&self.thread_ts), edit.blocks)
                .await
            {
                Ok(ts) => self.plan.posted(ts),
                Err(e) => tracing::debug!(error = %e, "plan post failed"),
            },
        }
    }

    /// 110 §6: `blocks` into the thread; `md` as a plain message when Slack
    /// refuses the blocks (a real `ApiError`), so nothing is lost.
    async fn post_blocks_or_markdown(&self, blocks: serde_json::Value, md: &str) {
        if let Err(e) = self
            .chan
            .api
            .post_blocks(&self.channel, Some(&self.thread_ts), blocks)
            .await
        {
            tracing::debug!(error = %e, "blocks refused; posting markdown instead");
            self.post_markdown_in_thread(md).await;
        }
    }

    /// 110 §6: a child's or peer's result as one collapsed container titled
    /// from its board row (`↳ name · finished · age`), else `↳ <from> · delivered`.
    async fn peer(&self, p: PeerDelivered) {
        let row = p
            .origin
            .from
            .strip_prefix("session:")
            .and_then(|id| id.parse::<SessionId>().ok())
            .and_then(|id| self.rows.iter().find(|r| r.session == id));
        let title = match row {
            Some(r) => format!(
                "↳ {} · finished · {}",
                r.name,
                board::fmt_age(std::time::Instant::now().saturating_duration_since(r.since))
            ),
            None => format!("↳ {} · delivered", p.origin.from),
        };
        let body = clip(&p.text, MARKDOWN_TEXT_MAX);
        let md = clip(&format!("{title}\n\n{}", p.text), MARKDOWN_TEXT_MAX);
        self.post_blocks_or_markdown(ui::collapsed(&title, &body), &md)
            .await;
    }

    /// Posts or edits the board message; drops it once every row is idle so
    /// the next spawn posts a fresh one (21 §9.2).
    async fn render_board(&mut self) {
        let text = board::render(&self.rows, std::time::Instant::now());
        let ts = match &self.board {
            Some((ts, _)) => {
                if let Err(e) = self
                    .chan
                    .api
                    .update_markdown(&self.channel, ts, &text)
                    .await
                {
                    tracing::debug!(error = %e, "board update failed");
                }
                ts.clone()
            }
            None => match self.post_markdown_in_thread(&text).await {
                Some(ts) => ts,
                None => return,
            },
        };
        self.rows_dirty = false;
        self.board = board::any_live(&self.rows).then(|| (ts, Instant::now()));
    }

    // -- approvals (21 §10.1) --------------------------------------------

    async fn approval(&mut self, req: ApprovalRequested) {
        let body = approvals::body(&req);
        match self
            .chan
            .api
            .post_blocks(
                &self.channel,
                Some(&self.thread_ts),
                approvals::card(&req, None),
            )
            .await
        {
            Ok(ts) => {
                self.approvals.insert(req.id, (ts, body));
            }
            Err(e) => tracing::warn!(error = %e, "approval card post failed"),
        }
    }

    // -- artifacts (35 §8) -------------------------------------------------

    /// Uploads a `show` artifact into the thread, `title` as the comment. An
    /// app (46 §8.2, 110 §8) is one `card` block instead: Slack cannot run
    /// it and its source is noise here — no upload, no pretended controls.
    async fn artifact(&self, a: Artifact) {
        let title = a.title.trim();
        if a.kind == artifact::Kind::App {
            let title = if title.is_empty() { "App" } else { title };
            self.card(ui::card(title, APP_NOTE)).await;
            return;
        }
        let bytes = match tokio::fs::read(&a.path).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(error = %e, path = %a.path.display(), "artifact not readable");
                return;
            }
        };
        let file = crate::api::UploadFile {
            name: a.name,
            mime: a.mime,
            bytes,
        };
        let comment = (!title.is_empty()).then_some(title);
        if let Err(e) = self
            .chan
            .api
            .upload_file(
                &self.channel,
                Some(&self.thread_ts),
                std::slice::from_ref(&file),
                comment,
            )
            .await
        {
            tracing::warn!(error = %e, name = %file.name, "artifact upload failed");
        }
    }

    /// 110 §8: a board create as one `card`; the `theuth://` scheme has no
    /// desktop handler yet (§16), so the path is plain text, not a link.
    async fn board_card(&self, title: &str, id: &str) {
        let title = if title.is_empty() { "Board" } else { title };
        self.card(ui::card(
            &format!("Board · {title}"),
            &format!("Open in theuth: notes/{id}"),
        ))
        .await;
    }

    async fn card(&self, blocks: serde_json::Value) {
        if let Err(e) = self
            .chan
            .api
            .post_blocks(&self.channel, Some(&self.thread_ts), blocks)
            .await
        {
            tracing::warn!(error = %e, "deliverable card post failed");
        }
    }

    async fn resolved(&mut self, res: ApprovalResolved, by: Option<&str>) {
        let Some((ts, body)) = self.approvals.remove(&res.id) else {
            return;
        };
        let blocks = approvals::resolved_card(&body, res.decision, res.timed_out, by);
        if let Err(e) = self
            .chan
            .api
            .update_blocks(&self.channel, &ts, blocks)
            .await
        {
            tracing::debug!(error = %e, "approval card update failed");
        }
    }

    /// 126 §10: rebuild the pending card with the double's context line.
    async fn predicted(&self, p: &double::DoublePredicted) {
        let Some(id) = uuid::Uuid::parse_str(&p.question_id).ok().map(ApprovalId) else {
            return;
        };
        let Some((ts, body)) = self.approvals.get(&id) else {
            return;
        };
        let blocks = approvals::predicted_card(body, id, p, None);
        if let Err(e) = self.chan.api.update_blocks(&self.channel, ts, blocks).await {
            tracing::debug!(error = %e, "approval card double update failed");
        }
    }

    // -- asks --------------------------------------------------------------

    /// Posts question `i`'s card in the thread and records its `ts` on the card.
    async fn ask(&mut self, req: AskRequested, i: usize) {
        match self
            .chan
            .api
            .post_blocks(
                &self.channel,
                Some(&self.thread_ts),
                asks::question_card(&req, i, None),
            )
            .await
        {
            Ok(ts) => {
                if let Some(c) = self.chan.lock().asks.get_mut(&req.id) {
                    c.ts = ts;
                }
            }
            Err(e) => tracing::warn!(error = %e, "ask card post failed"),
        }
    }

    // -- descendants (21 §9) ---------------------------------------------

    /// A descendant got its thread: an idle board row. No `↳ spawned` line
    /// (110 §6): the row and the child root's `child of` link carry it.
    fn spawned(&mut self, child: SessionId, name: String, link: String, depth: u8) {
        self.rows.retain(|r| r.session != child);
        self.rows
            .push(board::Row::new(child, name, depth, Some(link)));
    }

    /// A descendant's event: its board row, plus one `alert` for a direct
    /// child's error or cancellation (21 §9.1, 110 §6).
    async fn child(&mut self, session: SessionId, ev: &AgentEvent) {
        let Some(row) = self.rows.iter_mut().find(|r| r.session == session) else {
            return;
        };
        self.rows_dirty |= row.apply(ev);
        if row.depth != 1 {
            return;
        }
        let name = row.name.clone();
        let text = match ev {
            AgentEvent::Error { message, .. } => {
                format!(
                    "{name} · error: {}",
                    message.lines().next().unwrap_or_default()
                )
            }
            AgentEvent::TurnFinished { stop_reason, .. } if stop_reason == "cancelled" => {
                format!("{name} · cancelled")
            }
            _ => return,
        };
        self.alert(&text).await;
    }

    /// 110 §6: one `alert` block, level `error`; `⚠️ <text>` when refused.
    async fn alert(&self, text: &str) {
        let line = text.lines().next().unwrap_or_default();
        self.post_blocks_or_markdown(ui::alert_error(line), &format!("⚠️ {line}"))
            .await;
    }

    /// Mirrors the open stream ts into `threads.json.streams` on change (21 §11).
    fn sync_stream(&mut self) {
        let now = self
            .turn
            .as_ref()
            .and_then(|t| t.open_stream().map(str::to_owned));
        if now == self.open_stream {
            return;
        }
        let mut s = self.chan.lock();
        if let Some(old) = &self.open_stream {
            s.threads.remove_stream(&self.channel, old);
        }
        if let Some(new) = &now {
            s.threads.add_stream(&self.channel, new);
        }
        self.open_stream = now;
    }

    async fn apply(&mut self, ev: AgentEvent) {
        if let Some(session) = ev.session() {
            self.session = session;
        }
        match ev {
            AgentEvent::TurnStarted {
                session,
                model,
                text,
                ..
            } => {
                self.model = model;
                self.started = Instant::now();
                if self.name.is_some() && !self.task_posted {
                    // 21 §9.1: the child's task, before its first bubble.
                    self.task_posted = true;
                    self.post_markdown_in_thread(&format!(
                        "▸ **task**\n\n{}",
                        clip(&text, TASK_MAX)
                    ))
                    .await;
                }
                self.turn = Some(Turn::new(
                    Target {
                        channel: self.channel.clone(),
                        thread_ts: self.thread_ts.clone(),
                        user: self.user.clone(),
                        team: self.team.clone(),
                    },
                    self.chan.tools,
                    self.chan.reasoning,
                    self.chan.blocks,
                    Arc::clone(&self.chan.api),
                    self.chan.rt.clone(),
                    session,
                ));
                if let Some(t) = &mut self.turn {
                    if let Some(epoch) = self.compacted.take() {
                        t.compacted(epoch);
                    }
                    if let Some(title) = &self.agent_title {
                        t.text(&format!("{title} — ")).await;
                    }
                }
                self.set_status("processing").await;
            }
            AgentEvent::TextDelta { text, .. } => {
                if let Some(t) = &mut self.turn {
                    t.text(&text).await;
                }
                self.unspoken.push_str(&text);
            }
            AgentEvent::ReasoningDelta { text, .. } => {
                if let Some(t) = &mut self.turn {
                    t.reasoning(&text).await;
                }
            }
            // 110 §4.1: the time behind a trailing `Thought N s` card.
            AgentEvent::ReasoningDone { ms, .. } => {
                if let Some(t) = &mut self.turn {
                    t.reasoning_done(ms);
                }
            }
            AgentEvent::ToolCallPending { call_id, name, .. } => {
                if let Some(t) = &mut self.turn {
                    t.tool_pending(&call_id, &name).await;
                }
                self.speak();
            }
            AgentEvent::ToolCallStarted {
                call_id,
                name,
                args,
                ..
            } => {
                if let Some(t) = &mut self.turn {
                    t.tool_started(&call_id, &name, &args).await;
                }
                // 110 §8: `isBoardCreate` (desktop `BoardCard.tsx`).
                if name == "doc_create" && args["kind"] == "board" {
                    let title = args["title"].as_str().unwrap_or_default().trim();
                    self.board_creates.insert(call_id, title.to_owned());
                }
            }
            AgentEvent::ToolCallFinished {
                call_id,
                output,
                is_error,
                ..
            } => {
                if let Some(t) = &mut self.turn {
                    t.tool_finished(&call_id, &output, is_error).await;
                }
                if let Some(title) = self.board_creates.remove(&call_id)
                    && !is_error
                    && let Some(id) = output
                        .strip_prefix("created board ")
                        .and_then(|r| r.split_whitespace().next())
                {
                    self.board_card(&title, id).await;
                }
            }
            AgentEvent::TurnFinished {
                stop_reason,
                usage,
                context,
                ..
            } => {
                self.speak();
                if let Some(mut t) = self.turn.take() {
                    let streamed = t.open_stream().is_some();
                    let footer = Footer {
                        model: self.model.clone(),
                        usage,
                        elapsed: self.started.elapsed(),
                        tools: t.tool_count(),
                        context: self.context_size.map(|size| (context, size)),
                    };
                    t.finish(footer).await;
                    self.sync_stream();
                    if !streamed {
                        // Nothing streamed, so no stopStream carried `session_status`.
                        self.set_status("active").await;
                    }
                }
                // 110 §5: the plan's last edit, kept as history.
                self.flush_plan(true).await;
                self.plan.finish();
                let emoji = match stop_reason.as_str() {
                    "cancelled" => STOPPED,
                    "error" => FAILED,
                    _ => DONE,
                };
                self.react_marks(emoji).await;
                self.marks.clear();
                self.title().await;
            }
            AgentEvent::Error { message, .. } => {
                self.speak();
                tracing::warn!(session = %self.session, %message, "slack thread: agent error");
                self.alert(&message).await;
                self.react_marks(FAILED).await;
            }
            // 110 §6: model input and the human's own echo; nothing to show,
            // the count is in `SlackStatus.hidden_deliveries`.
            AgentEvent::MessageDelivered { from, text, .. } => {
                self.chan.lock().hidden += 1;
                tracing::debug!(%from, chars = text.chars().count(), "slack delivery hidden");
            }
            // 110 §6: no message; `⟲ epoch n` rides on this turn's footer, or
            // the next one's when no turn is open.
            AgentEvent::Compacted { epoch, .. } => match &mut self.turn {
                Some(t) => t.compacted(epoch),
                None => self.compacted = Some(epoch),
            },
            // 110 §5: the plan message.
            AgentEvent::TodoChanged { snapshot, .. } => {
                if self.chan.plan && self.plan.update(snapshot) {
                    self.flush_plan(false).await;
                }
            }
            // Intercepted by the router (`Channel::spawned`), or session
            // bookkeeping with nothing to say in a thread (`ContextChanged`
            // is a projection revision notice, 48 §3.2 — not a reply).
            // 75 §5: an external harness's window size, for the footer.
            AgentEvent::ContextUsed {
                size: Some(size), ..
            } => self.context_size = Some(size),
            // 141 C stub: W4 keeps this in the silent arm (no thread for a
            // watch-only child); the board row comes from `route()`.
            AgentEvent::ObservedChildChanged { .. }
            | AgentEvent::SubagentSpawned { .. }
            | AgentEvent::InputQueued { .. }
            | AgentEvent::InputInjected { .. }
            | AgentEvent::InputWithdrawn { .. }
            | AgentEvent::ModelChanged { .. }
            | AgentEvent::DefaultModelChanged { .. }
            | AgentEvent::HarnessModelChanged { .. }
            | AgentEvent::EffortChanged { .. }
            | AgentEvent::DefaultEffortChanged { .. }
            | AgentEvent::FavoriteModelsChanged { .. }
            | AgentEvent::Compacting { .. }
            | AgentEvent::Harvesting { .. }
            | AgentEvent::Harvested { .. }
            | AgentEvent::SkillsChanged { .. }
            | AgentEvent::TitleChanged { .. }
            | AgentEvent::ToolCallProgress { .. }
            | AgentEvent::SessionDeleted { .. }
            | AgentEvent::SessionOwnerChanged { .. }
            | AgentEvent::ContextChanged { .. }
            | AgentEvent::VisionProgress { .. }
            | AgentEvent::SessionMetadataChanged { .. }
            | AgentEvent::ModelUsageChanged { .. }
            | AgentEvent::HarnessCommandsChanged { .. }
            | AgentEvent::ContextUsed { .. }
            | AgentEvent::SessionClosing { .. }
            // 109: resume/suspend banners are the desktop's (W5); nothing to say in a thread yet.
            | AgentEvent::TurnResumed { .. }
            | AgentEvent::ResumePending { .. }
            | AgentEvent::ResumeReport { .. }
            | AgentEvent::TurnSuspended { .. }
            | AgentEvent::PartialDropped { .. }
            // 125 §9: the pill is the TUI's and the app's; the thread sees nothing.
            | AgentEvent::AdvisorChanged { .. } => {}
            // 125 §9: a concern or blocker is one quiet line; nits post nothing.
            AgentEvent::AdvisoryAdded { advisory, .. } => {
                if let Some(line) = advisory.channel_line() {
                    self.post_blocks_or_markdown(ui::advisory_line(&line), &line)
                        .await;
                }
            }
        }
    }

    /// Posts `md` into this worker's thread; the new ts.
    pub(crate) async fn post_markdown_in_thread(&self, md: &str) -> Option<String> {
        self.chan
            .post_markdown(&self.channel, &self.thread_ts, md)
            .await
    }

    async fn react_marks(&self, name: &str) {
        for ts in &self.marks {
            self.chan.react(&self.channel, ts, name).await;
        }
    }

    /// End of a prose block (45 §4.5.3-4): the text is already in the bubble;
    /// on a voice turn the block is synthesized now and uploaded as `reply.mp3`
    /// after the previous block's clip.
    fn speak(&mut self) {
        let text = std::mem::take(&mut self.unspoken);
        if !self.voice || text.trim().is_empty() {
            return;
        }
        let Ok(Some(voice)) = ActorRef::<voice::Voice>::lookup(voice::VOICE_ACTOR) else {
            return;
        };
        let prev = self.clip_tail.take();
        let api = Arc::clone(&self.chan.api);
        let channel = self.channel.clone();
        let thread_ts = self.thread_ts.clone();
        self.clip_tail = Some(tokio::spawn(async move {
            let clip = voice.ask(voice::Synthesize { text }).await;
            if let Some(p) = prev {
                let _ = p.await;
            }
            let c = match clip.map_err(|e| e.to_string()) {
                Ok(Some(c)) => c,
                Ok(None) => return,
                Err(e) => {
                    tracing::warn!(error = %e, "voice clip synthesis failed");
                    return;
                }
            };
            let file = crate::api::UploadFile {
                name: "reply.mp3".to_owned(),
                mime: c.mime.to_owned(),
                bytes: c.bytes,
            };
            if let Err(e) = api
                .upload_file(&channel, Some(&thread_ts), &[file], None)
                .await
            {
                tracing::warn!(error = %e, "voice clip upload failed");
            }
        }));
    }

    /// `agents.sessions.setStatus`, latching off on "not an agent app" (21 §8.6).
    async fn set_status(&mut self, status: &str) {
        if !self.status_supported {
            return;
        }
        match self
            .chan
            .api
            .set_session_status(&self.channel, &self.thread_ts, status)
            .await
        {
            Ok(()) => {}
            Err(ApiError::Slack(code)) if STATUS_UNSUPPORTED.contains(&code.as_str()) => {
                self.status_supported = false;
                tracing::warn!(
                    code,
                    "agents.sessions.setStatus refused; create the Slack app from the manifest with agent_view to get Working… and the stop button"
                );
            }
            Err(e) => tracing::debug!(error = %e, status, "agents.sessions.setStatus failed"),
        }
    }

    /// Renames the thread to the session's title once per title unless the
    /// user renamed it (21 §8.6).
    async fn title(&mut self) {
        let user_named = self
            .chan
            .lock()
            .threads
            .get(&self.key)
            .is_some_and(|t| t.title.is_some());
        if user_named {
            return;
        }
        let sessions = match self.chan.port.list_sessions().await {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!(error = %e, "list_sessions for the thread title failed");
                return;
            }
        };
        let Some(title) = sessions
            .into_iter()
            .find(|s| s.id == self.session)
            .map(|s| s.title)
            .filter(|t| !t.trim().is_empty())
            .map(|t| match &self.name {
                Some(name) => format!("{name} · {}", t.lines().next().unwrap_or_default()),
                None => t,
            })
        else {
            return;
        };
        if self.title_sent.as_deref() == Some(title.as_str()) {
            return;
        }
        let short: String = title.chars().take(TITLE_MAX).collect();
        match self
            .chan
            .api
            .rename_session(&self.channel, &self.thread_ts, &short)
            .await
        {
            Ok(()) => self.title_sent = Some(title),
            Err(e) => tracing::debug!(error = %e, "agents.sessions.rename failed"),
        }
    }
}

/// Removes `<@U…>` mention tokens.
fn strip_mentions(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("<@") {
        out.push_str(&rest[..i]);
        match rest[i..].find('>') {
            Some(j) => rest = &rest[i + j + 1..],
            None => {
                rest = &rest[i..];
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `s` cut to `max` chars with an `… (n more chars)` tail when it was longer.
pub(crate) fn clip(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_owned();
    }
    let keep = max.saturating_sub(32);
    let head: String = s.chars().take(keep).collect();
    format!("{head}… ({} more chars)", n - keep)
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
pub(crate) mod tests {
    use std::time::Duration;

    use runtime::TurnId;
    use serde_json::json;

    use super::*;

    pub(crate) const OWNER: &str = "U0WNER";
    const FRIEND: &str = "U0FRIEND";
    const STRANGER: &str = "U5TRANGER";
    pub(crate) const DM: &str = "D0001";
    const LISTED: &str = "C0LISTED";
    pub(crate) const OWNER_DM: &str = "D0WNER";

    #[derive(Debug, PartialEq)]
    enum Call {
        SendInput(Option<SessionId>, String),
        NewRoot(PeerScope),
        Steer(SessionId, String),
        Cancel(SessionId),
        Resolve(ApprovalId, Decision),
        AnswerAsk(AskId, Option<Vec<Answer>>),
    }

    pub(crate) struct Stub {
        pub(crate) session: SessionId,
        calls: Mutex<Vec<Call>>,
    }

    impl Stub {
        fn calls(&self) -> Vec<Call> {
            std::mem::take(&mut *self.calls.lock().unwrap())
        }
    }

    #[async_trait]
    impl Port for Stub {
        async fn send_input(
            &self,
            session: Option<SessionId>,
            text: String,
            _: Vec<PathBuf>,
        ) -> Result<SessionId, String> {
            self.calls
                .lock()
                .unwrap()
                .push(Call::SendInput(session, text));
            Ok(session.unwrap_or(self.session))
        }
        async fn new_root(&self, scope: PeerScope) -> Result<SessionId, String> {
            self.calls.lock().unwrap().push(Call::NewRoot(scope));
            Ok(self.session)
        }
        async fn steer(
            &self,
            session: SessionId,
            _: InputId,
            text: String,
            _: Vec<PathBuf>,
        ) -> Result<(), String> {
            self.calls.lock().unwrap().push(Call::Steer(session, text));
            Ok(())
        }
        async fn cancel(&self, session: SessionId) -> Result<(), String> {
            self.calls.lock().unwrap().push(Call::Cancel(session));
            Ok(())
        }
        async fn list_sessions(&self) -> Result<Vec<SessionSummary>, String> {
            Ok(Vec::new())
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
        async fn list_agents(&self, _: SessionId) -> Result<Vec<AgentInfo>, String> {
            Ok(Vec::new())
        }
    }

    /// Records every call as one line, e.g. `react D0001 1.1 eyes`.
    #[derive(Default)]
    pub(crate) struct FakeApi {
        calls: Mutex<Vec<String>>,
        /// Every `post_blocks` / `update_blocks` payload, in call order.
        blocks: Mutex<Vec<serde_json::Value>>,
    }

    impl FakeApi {
        pub(crate) fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
        /// A stream call's chunks as one line: texts verbatim, task cards as
        /// `[title]`, block chunks as `{n blocks}`.
        fn texts(chunks: &[crate::api::Chunk]) -> String {
            chunks
                .iter()
                .map(|c| match c {
                    crate::api::Chunk::Text(s) => s.clone(),
                    crate::api::Chunk::Task { title, .. } => format!("[{title}]"),
                    crate::api::Chunk::Blocks(b) => {
                        format!("{{{} blocks}}", b.as_array().map_or(0, Vec::len))
                    }
                })
                .collect()
        }
        fn record(&self, line: String) {
            self.calls.lock().unwrap().push(line);
        }
        /// The block payloads sent so far.
        fn blocks(&self) -> Vec<serde_json::Value> {
            self.blocks
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
        fn keep(&self, blocks: serde_json::Value) {
            self.blocks
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(blocks);
        }
    }

    #[async_trait]
    impl SlackApi for FakeApi {
        async fn start_stream(
            &self,
            channel: &str,
            thread_ts: &str,
            _: &str,
            _: &str,
            chunks: Vec<crate::api::Chunk>,
        ) -> Result<String, ApiError> {
            self.record(format!(
                "start_stream {channel} {thread_ts} {}",
                Self::texts(&chunks)
            ));
            Ok("9.9".into())
        }
        async fn append_stream(
            &self,
            channel: &str,
            ts: &str,
            chunks: Vec<crate::api::Chunk>,
        ) -> Result<(), ApiError> {
            self.record(format!(
                "append_stream {channel} {ts} {}",
                Self::texts(&chunks)
            ));
            Ok(())
        }
        async fn stop_stream(
            &self,
            channel: &str,
            ts: &str,
            chunks: Vec<crate::api::Chunk>,
            footer: Option<String>,
            _: Option<&str>,
        ) -> Result<(), ApiError> {
            self.record(format!(
                "stop_stream {channel} {ts} {} | {}",
                Self::texts(&chunks),
                footer.unwrap_or_default()
            ));
            Ok(())
        }
        async fn post_markdown(
            &self,
            channel: &str,
            thread_ts: Option<&str>,
            markdown: &str,
        ) -> Result<String, ApiError> {
            self.record(format!(
                "post_markdown {channel} {} {markdown}",
                thread_ts.unwrap_or("-")
            ));
            Ok("8.8".into())
        }
        async fn update_markdown(&self, channel: &str, ts: &str, _: &str) -> Result<(), ApiError> {
            self.record(format!("update_markdown {channel} {ts}"));
            Ok(())
        }
        async fn post_blocks(
            &self,
            channel: &str,
            thread_ts: Option<&str>,
            blocks: serde_json::Value,
        ) -> Result<String, ApiError> {
            let n = blocks.as_array().map_or(0, Vec::len);
            self.record(format!(
                "post_blocks {channel} {} {n} blocks",
                thread_ts.unwrap_or("-")
            ));
            self.keep(blocks);
            Ok("7.7".into())
        }
        async fn update_blocks(
            &self,
            channel: &str,
            ts: &str,
            blocks: serde_json::Value,
        ) -> Result<(), ApiError> {
            self.record(format!("update_blocks {channel} {ts}"));
            self.keep(blocks);
            Ok(())
        }
        async fn ephemeral(&self, channel: &str, user: &str, text: &str) -> Result<(), ApiError> {
            self.record(format!("ephemeral {channel} {user} {text}"));
            Ok(())
        }
        async fn react(&self, channel: &str, ts: &str, name: &str) -> Result<(), ApiError> {
            self.record(format!("react {channel} {ts} {name}"));
            Ok(())
        }
        async fn set_session_status(
            &self,
            channel: &str,
            thread_ts: &str,
            status: &str,
        ) -> Result<(), ApiError> {
            self.record(format!("set_session_status {channel} {thread_ts} {status}"));
            Ok(())
        }
        async fn rename_session(
            &self,
            channel: &str,
            thread_ts: &str,
            title: &str,
        ) -> Result<(), ApiError> {
            self.record(format!("rename_session {channel} {thread_ts} {title}"));
            Ok(())
        }
        async fn upload_file(
            &self,
            channel: &str,
            thread_ts: Option<&str>,
            files: &[crate::api::UploadFile],
            comment: Option<&str>,
        ) -> Result<String, ApiError> {
            let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
            self.record(format!(
                "upload_file {channel} {} [{}] {}",
                thread_ts.unwrap_or("-"),
                names.join(","),
                comment.unwrap_or("-")
            ));
            Ok("https://files.example/1".into())
        }
        async fn upload_text(
            &self,
            channel: &str,
            _: &str,
            title: &str,
            _: &str,
        ) -> Result<String, ApiError> {
            self.record(format!("upload_text {channel} {title}"));
            Ok("https://files.example/1".into())
        }
        async fn permalink(&self, channel: &str, ts: &str) -> Result<String, ApiError> {
            self.record(format!("permalink {channel} {ts}"));
            Ok(format!(
                "https://example.slack.com/archives/{channel}/p{ts}"
            ))
        }
        async fn download(&self, url: &str) -> Result<Vec<u8>, ApiError> {
            self.record(format!("download {url}"));
            Ok(b"bytes".to_vec())
        }
        async fn respond(&self, url: &str, text: &str) -> Result<(), ApiError> {
            self.record(format!("respond {url} {text}"));
            Ok(())
        }
        async fn respond_blocks(
            &self,
            url: &str,
            _: &str,
            blocks: serde_json::Value,
            replace_original: bool,
        ) -> Result<(), ApiError> {
            let n = blocks.as_array().map_or(0, Vec::len);
            self.record(format!("respond_blocks {url} {replace_original} {n}"));
            Ok(())
        }
        async fn views_open(
            &self,
            trigger_id: &str,
            view: serde_json::Value,
        ) -> Result<(), ApiError> {
            self.record(format!(
                "views_open {trigger_id} {}",
                view["callback_id"].as_str().unwrap_or("-")
            ));
            Ok(())
        }
    }

    pub(crate) struct Fixture {
        pub(crate) api: Arc<FakeApi>,
        pub(crate) stub: Arc<Stub>,
        pub(crate) chan: Arc<Channel>,
        _dir: tempfile::TempDir,
    }

    impl Fixture {
        /// Polls until `pred` holds over the fake's calls, or fails after 5 s.
        async fn until(&self, mut pred: impl FnMut(&[String]) -> bool) {
            tokio::time::timeout(Duration::from_secs(5), async {
                while !pred(&self.api.calls()) {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .unwrap_or_else(|_| panic!("condition not reached; calls: {:?}", self.api.calls()));
        }
    }

    pub(crate) fn fixture() -> Fixture {
        fixture_with(SlackConfig::default())
    }

    fn fixture_with(cfg: SlackConfig) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let api = Arc::new(FakeApi::default());
        let stub = Arc::new(Stub {
            session: SessionId::new(),
            calls: Mutex::new(Vec::new()),
        });
        let cfg = SlackConfig {
            owner: OWNER.into(),
            allowed: vec![FRIEND.into()],
            channels: vec![LISTED.into()],
            ..cfg
        };
        let (tx, _rx) = mpsc::unbounded_channel();
        let chan = Arc::new(Channel::new(
            Arc::clone(&api) as Arc<dyn SlackApi>,
            None,
            cfg,
            rt,
            Arc::clone(&stub) as Arc<dyn Port>,
            tx,
        ));
        let _ = chan.identity.set(Identity {
            bot_user: "UBOT".into(),
            team: "T1".into(),
            owner_dm: Some(OWNER_DM.into()),
        });
        Fixture {
            api,
            stub,
            chan,
            _dir: dir,
        }
    }

    impl Fixture {
        /// Binds `DM:100.1` to the stub's session for `user`.
        pub(crate) fn bind(&self, user: &str) -> ThreadKey {
            let key = thread_key(DM, "100.1");
            self.chan.lock().threads.bind(
                key.clone(),
                Thread::new(self.stub.session, user.into(), "T1".into()),
            );
            key
        }
    }

    fn approval(session: SessionId) -> ApprovalRequested {
        ApprovalRequested {
            id: ApprovalId(uuid::Uuid::new_v4()),
            session,
            tool: "shell".into(),
            args: json!({ "command": "rm -rf target/" }),
            risk: tools::Risk::Destructive,
        }
    }

    fn click(user: &str, id: ApprovalId, action: &str) -> Inbound {
        let ev: SlackInteractionEvent = serde_json::from_value(json!({
            "type": "block_actions",
            "team": { "id": "T1" },
            "user": { "id": user },
            "api_app_id": "A1",
            "container": { "type": "message", "message_ts": "7.7", "channel_id": DM },
            "trigger_id": "tr1",
            "channel": { "id": DM, "name": "dm" },
            "response_url": "https://hooks.slack.com/actions/T1/1/x",
            "actions": [{ "type": "button", "action_id": action, "value": id.0.to_string() }],
        }))
        .unwrap();
        Inbound::Action(Box::new(ev))
    }

    fn ask(session: SessionId) -> AskRequested {
        let choice = |label: &str| tools::Choice {
            label: label.into(),
            description: None,
        };
        AskRequested {
            id: AskId(uuid::Uuid::new_v4()),
            session,
            questions: vec![
                tools::Question {
                    id: "db".into(),
                    question: "Which database?".into(),
                    options: vec![choice("sqlite"), choice("postgres")],
                    recommended: Some(0),
                },
                tools::Question {
                    id: "name".into(),
                    question: "Project name?".into(),
                    options: Vec::new(),
                    recommended: None,
                },
            ],
        }
    }

    /// An ask-card interaction on message `7.7`: a button with `value`, or the
    /// text input (`block_id` = `value`, typed `text`) for [`asks::ACTION_TEXT`].
    fn ask_click(user: &str, action: &str, value: &str, text: Option<&str>) -> Inbound {
        let a = match text {
            Some(t) => {
                json!({ "type": "plain_text_input", "action_id": action, "block_id": value, "value": t })
            }
            None => json!({ "type": "button", "action_id": action, "value": value }),
        };
        let ev: SlackInteractionEvent = serde_json::from_value(json!({
            "type": "block_actions",
            "team": { "id": "T1" },
            "user": { "id": user },
            "api_app_id": "A1",
            "container": { "type": "message", "message_ts": "7.7", "channel_id": DM },
            "trigger_id": "tr1",
            "channel": { "id": DM, "name": "dm" },
            "actions": [a],
        }))
        .unwrap();
        Inbound::Action(Box::new(ev))
    }

    /// A `cmd:*` click: a button when `value` is `None`, else a select/overflow
    /// whose chosen option has that `value`.
    fn cmd_click(user: &str, action_id: &str, value: Option<&str>) -> Inbound {
        let mut a = json!({ "type": "button", "action_id": action_id });
        if let Some(v) = value {
            a["type"] = json!("overflow");
            a["selected_option"] =
                json!({ "text": { "type": "plain_text", "text": "x" }, "value": v });
        }
        let ev: SlackInteractionEvent = serde_json::from_value(json!({
            "type": "block_actions",
            "team": { "id": "T1" },
            "user": { "id": user },
            "api_app_id": "A1",
            "container": { "type": "message", "message_ts": "7.7", "channel_id": DM, "is_ephemeral": true },
            "trigger_id": "tr9",
            "channel": { "id": DM, "name": "dm" },
            "response_url": "https://hooks.slack.com/actions/T1/9/x",
            "actions": [a],
        }))
        .unwrap();
        Inbound::Action(Box::new(ev))
    }

    fn turn_started(session: SessionId, text: &str) -> AgentEvent {
        AgentEvent::TurnStarted {
            session,
            turn: TurnId::new(),
            text: text.into(),
            model: "m".into(),
            turn_seq: 0,
        }
    }

    fn push(event: serde_json::Value) -> SlackPushEventCallback {
        serde_json::from_value(json!({
            "team_id": "T1",
            "api_app_id": "A1",
            "event": event,
            "event_id": "Ev1",
            "event_time": 1_700_000_000
        }))
        .unwrap()
    }

    fn dm(user: &str, ts: &str, thread_ts: Option<&str>, text: &str) -> SlackPushEventCallback {
        let mut ev = json!({
            "type": "message", "channel": DM, "channel_type": "im",
            "user": user, "text": text, "ts": ts,
        });
        if let Some(t) = thread_ts {
            ev["thread_ts"] = json!(t);
        }
        push(ev)
    }

    fn mention(user: &str, channel: &str, ts: &str, text: &str) -> SlackPushEventCallback {
        push(json!({
            "type": "app_mention", "channel": channel, "user": user, "text": text, "ts": ts,
        }))
    }

    #[tokio::test]
    async fn audio_file_without_the_voice_plugin_is_an_attachment_line_and_not_a_voice_turn() {
        let f = fixture();
        let file: SlackFile = serde_json::from_value(json!({
            "id": "F1",
            "name": "clip.m4a",
            "mimetype": "audio/mp4",
            "url_private_download": "https://files.slack.com/clip.m4a",
        }))
        .unwrap();
        let (lines, paths, voice) = f.chan.save_files("1.1", &[file]).await;
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("[attachment: "), "{}", lines[0]);
        assert!(
            lines[0].ends_with("(audio/mp4, 5 B)] (transcription unavailable)"),
            "{}",
            lines[0]
        );
        assert!(paths.is_empty() && !voice);
    }

    #[tokio::test]
    async fn dm_text_is_prefixed_with_the_slack_dm_marker() {
        let f = fixture();
        f.chan
            .handle_inbound(Inbound::Push(Box::new(dm(
                OWNER, "100.1", None, "hi there",
            ))))
            .await;
        assert_eq!(f.api.calls(), vec![format!("react {DM} 100.1 {EYES}")]);
        assert_eq!(
            f.stub.calls(),
            vec![
                Call::NewRoot(PeerScope::Owner),
                Call::SendInput(Some(f.stub.session), "[slack dm]\nhi there".into())
            ]
        );
        let key = thread_key(DM, "100.1");
        let s = f.chan.lock();
        let t = s.threads.get(&key).expect("thread bound");
        assert_eq!(t.session, f.stub.session);
        assert_eq!(t.user, OWNER);
        assert_eq!(t.team, "T1");
        assert!(s.workers.contains_key(&key));
    }

    #[tokio::test]
    async fn reply_in_a_running_thread_steers() {
        let f = fixture();
        let key = thread_key(DM, "100.1");
        {
            let mut s = f.chan.lock();
            s.threads.bind(
                key.clone(),
                Thread::new(f.stub.session, OWNER.into(), "T1".into()),
            );
            s.running.insert(f.stub.session);
        }
        f.chan
            .handle_inbound(Inbound::Push(Box::new(dm(
                OWNER,
                "100.2",
                Some("100.1"),
                "<@UBOT> also this",
            ))))
            .await;
        assert_eq!(
            f.stub.calls(),
            vec![Call::Steer(f.stub.session, "[slack dm]\nalso this".into())]
        );
        assert_eq!(f.api.calls(), vec![format!("react {DM} 100.2 {EYES}")]);
    }

    #[tokio::test]
    async fn stranger_dm_is_dropped_silently() {
        let f = fixture();
        f.chan
            .handle_inbound(Inbound::Push(Box::new(dm(
                STRANGER, "100.1", None, "hello?",
            ))))
            .await;
        assert!(f.api.calls().is_empty());
        assert!(f.stub.calls().is_empty());
        assert!(f.chan.lock().threads.all().is_empty());
    }

    #[tokio::test]
    async fn mentions_honour_the_channel_list_and_the_allowlist() {
        let f = fixture();
        f.chan
            .handle_inbound(Inbound::Push(Box::new(mention(
                OWNER,
                "C0OTHER",
                "200.1",
                "<@UBOT> hi",
            ))))
            .await;
        assert!(f.api.calls().is_empty());
        assert!(f.stub.calls().is_empty());

        f.chan
            .handle_inbound(Inbound::Push(Box::new(mention(
                STRANGER,
                LISTED,
                "200.2",
                "<@UBOT> hi",
            ))))
            .await;
        assert_eq!(
            f.api.calls(),
            vec![format!("ephemeral {LISTED} {STRANGER} not allowed")]
        );
        assert!(f.stub.calls().is_empty());

        f.chan
            .handle_inbound(Inbound::Push(Box::new(mention(
                FRIEND,
                LISTED,
                "200.3",
                "<@UBOT> hi",
            ))))
            .await;
        assert_eq!(
            f.stub.calls(),
            vec![Call::SendInput(None, "[slack channel]\nhi".into())]
        );
        assert!(
            f.chan
                .lock()
                .threads
                .get(&thread_key(LISTED, "200.3"))
                .is_some()
        );
    }

    #[tokio::test]
    async fn agent_names_fall_back_to_theuth_without_the_roster_and_theuth_prefix_is_stripped() {
        let f = fixture();
        f.chan
            .handle_inbound(Inbound::Push(Box::new(dm(OWNER, "100.1", None, "ops: hi"))))
            .await;
        assert_eq!(
            f.stub.calls(),
            vec![
                Call::NewRoot(PeerScope::Owner),
                Call::SendInput(Some(f.stub.session), "[slack dm]\nops: hi".into())
            ]
        );
        let t = f
            .chan
            .lock()
            .threads
            .get(&thread_key(DM, "100.1"))
            .cloned()
            .expect("bound");
        assert_eq!(t.agent, None);
        f.chan
            .handle_inbound(Inbound::Push(Box::new(dm(
                OWNER,
                "100.2",
                Some("100.1"),
                "theuth: also this",
            ))))
            .await;
        assert_eq!(
            f.stub.calls(),
            vec![Call::SendInput(
                Some(f.stub.session),
                "[slack dm]\nalso this".into()
            )]
        );
    }

    #[tokio::test]
    async fn agent_thread_prefixes_each_turn_with_the_title() {
        let f = fixture();
        let key = thread_key(DM, "100.1");
        let mut t = Thread::new(f.stub.session, OWNER.into(), "T1".into());
        t.agent = Some("ops".into());
        t.agent_title = Some("Ops".into());
        f.chan.lock().threads.bind(key, t);
        f.chan
            .route(Event::Agent(turn_started(f.stub.session, "go")))
            .await;
        f.chan
            .route(Event::Agent(AgentEvent::TextDelta {
                session: f.stub.session,
                turn: TurnId::new(),
                text: "hello".into(),
            }))
            .await;
        f.chan
            .route(Event::Agent(AgentEvent::TurnFinished {
                session: f.stub.session,
                turn: TurnId::new(),
                stop_reason: "end_turn".into(),
                usage: agent::Usage::default(),
                context: 0,
            }))
            .await;
        let start = format!("start_stream {DM} 100.1 Ops — ");
        let stop = format!("stop_stream {DM} 9.9 Ops — ");
        f.until(|calls| {
            calls
                .iter()
                .any(|c| c.starts_with(&start) || c.starts_with(&stop))
        })
        .await;
    }

    #[tokio::test]
    async fn cancelled_turn_gets_the_stop_reaction_on_the_mark() {
        let f = fixture();
        f.chan
            .handle_inbound(Inbound::Push(Box::new(dm(OWNER, "100.1", None, "go"))))
            .await;
        f.chan
            .route(Event::Agent(AgentEvent::TurnFinished {
                session: f.stub.session,
                turn: TurnId::new(),
                stop_reason: "cancelled".into(),
                usage: agent::Usage::default(),
                context: 0,
            }))
            .await;
        let want = format!("react {DM} 100.1 {STOPPED}");
        f.until(|calls| calls.contains(&want)).await;
    }

    #[tokio::test]
    async fn stop_button_cancels_the_bound_session() {
        let f = fixture();
        let key = thread_key(DM, "100.1");
        f.chan
            .lock()
            .threads
            .bind(key, Thread::new(f.stub.session, OWNER.into(), "T1".into()));
        f.chan
            .handle_inbound(Inbound::Push(Box::new(push(json!({
                "type": "agent_session_stopped", "channel": DM, "thread_ts": "100.1", "user": OWNER,
            })))))
            .await;
        assert_eq!(f.stub.calls(), vec![Call::Cancel(f.stub.session)]);
    }

    #[test]
    fn recent_keys_evict_the_oldest_past_the_cap() {
        let mut r = RecentKeys::default();
        assert!(!r.seen("k0"));
        assert!(r.seen("k0"));
        for i in 1..RECENT_KEYS {
            assert!(!r.seen(&format!("k{i}")));
        }
        assert!(r.seen("k0"), "still present at exactly the cap");
        assert!(!r.seen("k1024"));
        assert!(r.seen("k1"), "second-oldest survives the 1025th key");
        assert!(
            !r.seen("k0"),
            "evicted by the 1025th key (and re-inserted, evicting k1)"
        );
    }

    #[test]
    fn mention_stripping_and_clipping() {
        assert_eq!(strip_mentions("<@U1> hi <@U2|bob> there"), " hi  there");
        assert_eq!(strip_mentions("plain"), "plain");
        assert_eq!(clip("short", 10), "short");
        let long = "x".repeat(100);
        let c = clip(&long, 50);
        assert!(c.starts_with(&"x".repeat(18)));
        assert!(c.ends_with("… (82 more chars)"));
    }

    #[tokio::test]
    async fn approval_in_a_bound_thread_is_a_card_the_thread_user_can_click() {
        let f = fixture();
        f.bind(FRIEND);
        let req = approval(f.stub.session);
        let id = req.id;
        f.chan.route(Event::Requested(req)).await;
        let card = format!("post_blocks {DM} 100.1 2 blocks");
        f.until(|calls| calls.contains(&card)).await;

        f.chan
            .handle_inbound(click(FRIEND, id, approvals::ACTION_APPROVE))
            .await;
        assert_eq!(f.stub.calls(), vec![Call::Resolve(id, Decision::Allow)]);

        f.chan
            .route(Event::Resolved(ApprovalResolved {
                id,
                decision: Decision::Allow,
                timed_out: false,
            }))
            .await;
        let updated = format!("update_blocks {DM} 7.7");
        f.until(|calls| calls.contains(&updated)).await;
        assert!(f.chan.lock().cards.is_empty());
    }

    fn artifact(session: SessionId, dir: &Path) -> ArtifactShown {
        let path = dir.join("report.md");
        std::fs::write(&path, b"# hi").unwrap();
        ArtifactShown {
            session,
            call_id: "call_1".into(),
            artifact: Artifact {
                id: "a1".into(),
                session,
                call_id: "call_1".into(),
                name: "report.md".into(),
                title: "Weekly report".into(),
                kind: artifact::Kind::Markdown,
                mime: "text/markdown".into(),
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
    async fn artifact_for_a_bound_session_is_uploaded_into_its_thread() {
        let f = fixture();
        f.bind(OWNER);
        f.chan
            .route(Event::Artifact(artifact(f.stub.session, f._dir.path())))
            .await;
        let upload = format!("upload_file {DM} 100.1 [report.md] Weekly report");
        f.until(|calls| calls.contains(&upload)).await;
        assert_eq!(
            f.api
                .calls()
                .iter()
                .filter(|c| c.starts_with("upload_file"))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn artifact_of_an_unbound_child_lands_in_the_bound_parents_thread() {
        let f = fixture();
        f.bind(OWNER);
        let child = SessionId::new();
        f.chan
            .lock()
            .parent_of
            .insert(child, (f.stub.session, "grep-logs".into()));
        f.chan
            .route(Event::Artifact(artifact(child, f._dir.path())))
            .await;
        let upload = format!("upload_file {DM} 100.1 [report.md] Weekly report");
        f.until(|calls| calls.contains(&upload)).await;
    }

    /// 110 §8: an app is one `card` (title, "interactive in theuth"); its
    /// `.openui` source is not uploaded.
    #[tokio::test]
    async fn app_for_a_bound_session_posts_one_card_and_no_upload() {
        let f = fixture();
        f.bind(OWNER);
        let mut ev = artifact(f.stub.session, f._dir.path());
        ev.artifact.kind = artifact::Kind::App;
        ev.artifact.name = "panel.openui".into();
        ev.artifact.title = "Panel".into();
        ev.artifact.path = f._dir.path().join("missing.openui");
        f.chan.route(Event::Artifact(ev)).await;
        let want = format!("post_blocks {DM} 100.1 1 blocks");
        f.until(|calls| calls.contains(&want)).await;
        assert!(
            !f.api.calls().iter().any(|c| c.starts_with("upload_file")),
            "{:?}",
            f.api.calls()
        );
        let blocks = f.api.blocks();
        assert_eq!(
            blocks[0],
            json!([{
                "type": "card",
                "title": { "type": "plain_text", "text": "Panel" },
                "subtitle": { "type": "plain_text", "text": APP_NOTE },
            }])
        );
    }

    /// 110 §8: `doc_create kind: board` that succeeds → a `card` with the
    /// board's path as plain text (no `theuth://` link, §16).
    #[tokio::test(start_paused = true)]
    async fn board_create_posts_a_card_with_the_notes_path_as_text() {
        let f = fixture();
        f.bind(OWNER);
        let s = f.stub.session;
        f.chan.route(Event::Agent(turn_started(s, "go"))).await;
        let started = |call_id: &str, kind: &str| AgentEvent::ToolCallStarted {
            session: s,
            turn: TurnId::new(),
            call_id: call_id.into(),
            name: "doc_create".into(),
            args: json!({ "title": "Auth flow", "kind": kind, "root": "/repo" }),
        };
        let finished = |call_id: &str, output: &str, is_error: bool| AgentEvent::ToolCallFinished {
            session: s,
            turn: TurnId::new(),
            call_id: call_id.into(),
            output: output.into(),
            is_error,
            files: Vec::new(),
        };
        // A note, a failed board and a board that succeeds.
        f.chan.route(Event::Agent(started("c1", "note"))).await;
        f.chan
            .route(Event::Agent(finished(
                "c1",
                "created note n1 (theuth://notes/n1)\n",
                false,
            )))
            .await;
        f.chan.route(Event::Agent(started("c2", "board"))).await;
        f.chan
            .route(Event::Agent(finished(
                "c2",
                "root /repo is not a directory",
                true,
            )))
            .await;
        f.chan.route(Event::Agent(started("c3", "board"))).await;
        f.chan
            .route(Event::Agent(finished(
                "c3",
                "created board b7 (theuth://notes/b7)\n",
                false,
            )))
            .await;
        let want = format!("post_blocks {DM} 100.1 1 blocks");
        f.until(|calls| calls.contains(&want)).await;
        let cards: Vec<_> = f
            .api
            .blocks()
            .into_iter()
            .filter(|b| b[0]["type"] == "card")
            .collect();
        assert_eq!(
            cards,
            vec![json!([{
                "type": "card",
                "title": { "type": "plain_text", "text": "Board · Auth flow" },
                "subtitle": { "type": "plain_text", "text": "Open in theuth: notes/b7" },
            }])]
        );
        assert!(
            !serde_json::to_string(&cards).unwrap().contains("theuth://"),
            "{cards:?}"
        );
    }

    #[tokio::test]
    async fn artifact_for_an_unbound_session_posts_nothing() {
        let f = fixture();
        f.chan
            .route(Event::Artifact(artifact(SessionId::new(), f._dir.path())))
            .await;
        assert!(f.api.calls().is_empty());
        assert!(f.chan.lock().workers.is_empty());
    }

    #[tokio::test]
    async fn stranger_click_is_refused_without_resolving() {
        let f = fixture();
        f.bind(FRIEND);
        let req = approval(f.stub.session);
        let id = req.id;
        f.chan.route(Event::Requested(req)).await;
        f.chan
            .handle_inbound(click(STRANGER, id, approvals::ACTION_DENY))
            .await;
        assert!(f.stub.calls().is_empty());
        f.until(|calls| calls.contains(&format!("ephemeral {DM} {STRANGER} not allowed")))
            .await;
    }

    #[tokio::test]
    async fn unbound_approval_goes_to_the_owner_dm_unless_scoped_to_threads() {
        let f = fixture();
        f.chan
            .route(Event::Requested(approval(SessionId::new())))
            .await;
        assert_eq!(
            f.api.calls(),
            vec![format!("post_blocks {OWNER_DM} - 3 blocks")]
        );
        assert_eq!(f.chan.lock().cards.len(), 1);

        let f = fixture_with(SlackConfig {
            approvals: ApprovalsScope::Threads,
            ..SlackConfig::default()
        });
        f.chan
            .route(Event::Requested(approval(SessionId::new())))
            .await;
        assert!(f.api.calls().is_empty());
        assert!(f.chan.lock().cards.is_empty());
    }

    #[tokio::test]
    async fn asks_in_a_bound_thread_run_one_question_at_a_time() {
        let f = fixture();
        f.bind(FRIEND);
        let req = ask(f.stub.session);
        let id = req.id;
        f.chan.route(Event::Ask(req)).await;
        let card = format!("post_blocks {DM} 100.1 3 blocks");
        f.until(|calls| calls.contains(&card)).await;
        assert_eq!(f.chan.lock().asks[&id].ts, "7.7");

        f.chan
            .handle_inbound(ask_click(
                FRIEND,
                asks::ACTION_PICK,
                &format!("{}:0:1", id.0),
                None,
            ))
            .await;
        let updated = format!("update_blocks {DM} 7.7");
        f.until(|calls| calls.iter().filter(|c| **c == card).count() == 2)
            .await;
        assert!(f.api.calls().contains(&updated));
        assert!(f.stub.calls().is_empty());

        // The old card again: stale.
        f.chan
            .handle_inbound(ask_click(
                FRIEND,
                asks::ACTION_PICK,
                &format!("{}:0:0", id.0),
                None,
            ))
            .await;
        f.until(|calls| calls.contains(&format!("ephemeral {DM} {FRIEND} already answered")))
            .await;

        f.chan
            .handle_inbound(ask_click(
                STRANGER,
                asks::ACTION_TEXT,
                &format!("ask_text:{}:1", id.0),
                Some("nope"),
            ))
            .await;
        f.until(|calls| calls.contains(&format!("ephemeral {DM} {STRANGER} not allowed")))
            .await;
        f.chan
            .handle_inbound(ask_click(
                OWNER,
                asks::ACTION_TEXT,
                &format!("ask_text:{}:1", id.0),
                Some("theuth"),
            ))
            .await;
        assert_eq!(
            f.stub.calls(),
            vec![Call::AnswerAsk(
                id,
                Some(vec![
                    Answer::Choice { index: 1 },
                    Answer::Text {
                        text: "theuth".into()
                    },
                ]),
            )]
        );
        let before = f.api.calls().len();
        f.chan
            .route(Event::AskDone(AskResolved {
                id,
                dismissed: false,
                timed_out: false,
            }))
            .await;
        assert!(f.chan.lock().asks.is_empty());
        assert_eq!(
            f.api.calls().len(),
            before,
            "already finalised; no second update"
        );
    }

    #[tokio::test]
    async fn ask_dismiss_and_timeout_close_the_card() {
        let f = fixture();
        f.bind(FRIEND);
        let req = ask(f.stub.session);
        let id = req.id;
        f.chan.route(Event::Ask(req)).await;
        f.until(|calls| calls.iter().any(|c| c.starts_with("post_blocks")))
            .await;
        f.chan
            .handle_inbound(ask_click(
                FRIEND,
                asks::ACTION_DISMISS,
                &id.0.to_string(),
                None,
            ))
            .await;
        assert!(f.api.calls().contains(&format!("update_blocks {DM} 7.7")));
        assert_eq!(f.stub.calls(), vec![Call::AnswerAsk(id, None)]);

        // Unbound + timeout: the owner-DM card is closed from the bus event.
        let f = fixture();
        let req = ask(SessionId::new());
        let id = req.id;
        f.chan.route(Event::Ask(req)).await;
        assert_eq!(
            f.api.calls(),
            vec![format!("post_blocks {OWNER_DM} - 4 blocks")]
        );
        f.chan
            .route(Event::AskDone(AskResolved {
                id,
                dismissed: false,
                timed_out: true,
            }))
            .await;
        assert_eq!(
            f.api.calls().last().unwrap(),
            &format!("update_blocks {OWNER_DM} 7.7")
        );
        assert!(f.chan.lock().asks.is_empty());
    }

    /// Binds the parent, spawns `grep-logs` under it; the child's thread is `DM:8.8`.
    async fn spawn_child(f: &Fixture) -> SessionId {
        f.bind(OWNER);
        let child = SessionId::new();
        f.chan
            .route(Event::Agent(AgentEvent::SubagentSpawned {
                parent: f.stub.session,
                child,
                name: "grep-logs".into(),
                cap: None,
            }))
            .await;
        child
    }

    fn turn_finished(session: SessionId, stop_reason: &str) -> AgentEvent {
        AgentEvent::TurnFinished {
            session,
            turn: TurnId::new(),
            stop_reason: stop_reason.into(),
            usage: agent::Usage::default(),
            context: 0,
        }
    }

    fn todo_changed(session: SessionId, version: u64, items: &[(&str, &str)]) -> AgentEvent {
        AgentEvent::TodoChanged {
            session,
            snapshot: agent::todo::TodoSnapshot {
                version,
                source: agent::todo::TodoSource::Native,
                items: items
                    .iter()
                    .map(|(text, status)| agent::todo::Todo {
                        text: (*text).to_owned(),
                        status: status.parse().unwrap(),
                        reason: (*status == "blocked").then(|| "needs a token".to_owned()),
                    })
                    .collect(),
            },
        }
    }

    fn peer_delivered(session: SessionId, from: &str, text: &str) -> PeerDelivered {
        PeerDelivered {
            session,
            origin: agent::communication::PeerOrigin {
                id: uuid::Uuid::new_v4(),
                from: from.into(),
                reply_to: None,
                scope: None,
            },
            text: text.into(),
            seq: 1,
            turn: TurnId::new(),
        }
    }

    /// The one alert block in `blocks`, when it is one.
    fn alert_text(blocks: &serde_json::Value) -> Option<&str> {
        let arr = blocks.as_array()?;
        (arr.len() == 1 && arr[0]["type"] == "alert" && arr[0]["level"] == "error")
            .then(|| arr[0]["text"]["text"].as_str())
            .flatten()
    }

    #[tokio::test(start_paused = true)]
    async fn subagent_spawned_posts_the_child_root_and_no_spawned_line() {
        let f = fixture();
        let child = spawn_child(&f).await;
        let root = format!(
            "post_markdown {DM} - ↳ **grep-logs** · child of <https://example.slack.com/archives/{DM}/p100.1|this thread>"
        );
        assert!(f.api.calls().contains(&root), "{:?}", f.api.calls());
        {
            let s = f.chan.lock();
            let (key, t) = s.threads.by_session(child).expect("child bound");
            assert_eq!(key, &thread_key(DM, "8.8"));
            assert_eq!(t.user, OWNER);
            assert_eq!(
                s.parent_of.get(&child).map(|(p, n)| (*p, n.as_str())),
                Some((f.stub.session, "grep-logs"))
            );
        }
        // The board row proves the parent worker consumed `Spawned`; the line never came.
        f.chan
            .route(Event::Agent(turn_started(child, "find the failing test")))
            .await;
        let board = format!("post_markdown {DM} 100.1 **agents**");
        f.until(|calls| {
            calls
                .iter()
                .any(|c| c.starts_with(&board) && c.contains("grep-logs"))
        })
        .await;
        assert!(
            !f.api.calls().iter().any(|c| c.contains("↳ spawned")),
            "{:?}",
            f.api.calls()
        );
    }

    #[tokio::test]
    async fn child_turn_posts_its_task_and_the_parent_board_shows_it_running() {
        let f = fixture();
        let child = spawn_child(&f).await;
        f.chan
            .route(Event::Agent(turn_started(child, "find the failing test")))
            .await;
        let task = format!("post_markdown {DM} 8.8 ▸ **task**\n\nfind the failing test");
        f.until(|calls| calls.contains(&task)).await;
        let board = format!("post_markdown {DM} 100.1 **agents**");
        f.until(|calls| {
            calls
                .iter()
                .any(|c| c.starts_with(&board) && c.contains("grep-logs") && c.contains("running"))
        })
        .await;
    }

    #[tokio::test(start_paused = true)]
    async fn child_error_is_an_alert_block() {
        let f = fixture();
        let child = spawn_child(&f).await;
        f.chan
            .route(Event::Agent(AgentEvent::Error {
                turn: None,
                session: child,
                message: "boom\nstack".into(),
            }))
            .await;
        let parent = format!("post_blocks {DM} 100.1 1 blocks");
        let own = format!("post_blocks {DM} 8.8 1 blocks");
        f.until(|calls| calls.contains(&parent) && calls.contains(&own))
            .await;
        let texts: Vec<String> = f
            .api
            .blocks()
            .iter()
            .filter_map(|b| alert_text(b).map(str::to_owned))
            .collect();
        assert!(
            texts.contains(&"grep-logs · error: boom".to_owned()),
            "{texts:?}"
        );
        assert!(texts.contains(&"boom".to_owned()), "{texts:?}");
        assert!(!f.api.calls().iter().any(|c| c.contains("⚠️")));
        // A cancelled child turn is an alert too.
        f.chan
            .route(Event::Agent(turn_finished(child, "cancelled")))
            .await;
        f.until(|calls| calls.iter().filter(|c| **c == parent).count() == 2)
            .await;
        assert!(
            f.api
                .blocks()
                .iter()
                .any(|b| alert_text(b) == Some("grep-logs · cancelled"))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn error_is_an_alert_block_and_reacts() {
        let f = fixture();
        f.chan
            .handle_inbound(Inbound::Push(Box::new(dm(OWNER, "100.1", None, "go"))))
            .await;
        let long = format!("{}\nstack", "x".repeat(300));
        f.chan
            .route(Event::Agent(AgentEvent::Error {
                turn: None,
                session: f.stub.session,
                message: long,
            }))
            .await;
        let want = format!("react {DM} 100.1 {FAILED}");
        f.until(|calls| calls.contains(&want)).await;
        let blocks = f.api.blocks();
        let text = alert_text(&blocks[0]).expect("one error alert");
        assert_eq!(text.chars().count(), ui::ALERT_MAX);
        assert!(text.ends_with('…') && !text.contains("stack"));
        assert!(!f.api.calls().iter().any(|c| c.starts_with("post_markdown")));
    }

    #[tokio::test(start_paused = true)]
    async fn message_delivered_posts_nothing_and_counts() {
        let f = fixture();
        f.bind(OWNER);
        for from in ["job j3", "reminder r1", "you"] {
            f.chan
                .route(Event::Agent(AgentEvent::MessageDelivered {
                    session: f.stub.session,
                    from: from.into(),
                    text: "<job_result>done</job_result>".into(),
                }))
                .await;
        }
        // A later event proves the worker drained the three deliveries.
        f.chan
            .route(Event::Agent(turn_started(f.stub.session, "go")))
            .await;
        let processing = format!("set_session_status {DM} 100.1 processing");
        f.until(|calls| calls.contains(&processing)).await;
        assert!(
            !f.api.calls().iter().any(|c| c.starts_with("post_")),
            "{:?}",
            f.api.calls()
        );
        assert_eq!(f.chan.status().hidden_deliveries, 3);
    }

    fn advisory(session: SessionId, notes: &[(agent::AdvisorSeverity, &str)]) -> AgentEvent {
        AgentEvent::AdvisoryAdded {
            session,
            seq: 7,
            advisory: agent::Advisory {
                advisor: SessionId::new(),
                step: 3,
                turn: None,
                at_ms: 0,
                notes: notes
                    .iter()
                    .map(|(severity, note)| agent::AdvisoryNote {
                        severity: *severity,
                        note: (*note).into(),
                    })
                    .collect(),
            },
        }
    }

    /// 125 §9: a concern/blocker card is one `context` line (its top note's
    /// first line); a card of nits posts nothing.
    #[tokio::test(start_paused = true)]
    async fn advisory_posts_one_context_line_for_concerns_only() {
        use agent::AdvisorSeverity::{Blocker, Concern, Nit};
        let f = fixture();
        f.bind(OWNER);
        f.chan
            .route(Event::Agent(advisory(
                f.stub.session,
                &[(Nit, "typo in the docstring")],
            )))
            .await;
        f.chan
            .route(Event::Agent(advisory(
                f.stub.session,
                &[
                    (Concern, "claims done\nbut the check never ran"),
                    (Blocker, "the test file was deleted\ndetails"),
                ],
            )))
            .await;
        let want = format!("post_blocks {DM} 100.1 1 blocks");
        f.until(|calls| calls.contains(&want)).await;
        let blocks = f.api.blocks();
        assert_eq!(blocks.len(), 1, "{blocks:?}");
        assert_eq!(blocks[0][0]["type"], "context");
        assert_eq!(
            blocks[0][0]["elements"][0]["text"],
            "advisor · blocker — the test file was deleted"
        );
        assert!(!f.api.calls().iter().any(|c| c.starts_with("post_markdown")));
    }

    fn compacted(session: SessionId, epoch: u32) -> AgentEvent {
        AgentEvent::Compacted {
            session,
            turn: None,
            epoch,
            before: 90_000,
            after: 20_000,
            folded: 40,
            style: agent::CompactStyle::default(),
        }
    }

    fn text_delta(session: SessionId, text: &str) -> AgentEvent {
        AgentEvent::TextDelta {
            session,
            turn: TurnId::new(),
            text: text.into(),
        }
    }

    /// Drives `text` through one turn and returns the `stop_stream` lines so far.
    async fn one_turn(f: &Fixture, text: &str, stops: usize) -> Vec<String> {
        f.chan
            .route(Event::Agent(turn_started(f.stub.session, "go")))
            .await;
        f.chan
            .route(Event::Agent(text_delta(f.stub.session, text)))
            .await;
        f.chan
            .route(Event::Agent(turn_finished(f.stub.session, "end_turn")))
            .await;
        f.until(|calls| {
            calls
                .iter()
                .filter(|c| c.starts_with("stop_stream"))
                .count()
                == stops
        })
        .await;
        f.api
            .calls()
            .into_iter()
            .filter(|c| c.starts_with("stop_stream"))
            .collect()
    }

    /// 110 §6: no message; the epoch rides on the footer of the turn it
    /// lands in, or the next one's when it arrives between turns.
    #[tokio::test(start_paused = true)]
    async fn compacted_posts_nothing_and_lands_in_the_footer() {
        let f = fixture();
        f.bind(OWNER);
        // Between turns: held for the next footer.
        f.chan
            .route(Event::Agent(compacted(f.stub.session, 3)))
            .await;
        let stops = one_turn(&f, "hi", 1).await;
        assert!(stops[0].ends_with(" · ⟲ epoch 3"), "{stops:?}");
        let calls = f.api.calls();
        assert!(!calls.iter().any(|c| c.contains("compacted")), "{calls:?}");
        // Mid-turn: this turn's footer.
        f.chan
            .route(Event::Agent(turn_started(f.stub.session, "again")))
            .await;
        f.chan
            .route(Event::Agent(compacted(f.stub.session, 4)))
            .await;
        f.chan
            .route(Event::Agent(text_delta(f.stub.session, "hi")))
            .await;
        f.chan
            .route(Event::Agent(turn_finished(f.stub.session, "end_turn")))
            .await;
        f.until(|calls| {
            calls
                .iter()
                .filter(|c| c.starts_with("stop_stream"))
                .count()
                == 2
        })
        .await;
        // Neither epoch is carried into a turn that saw no compaction.
        let stops = one_turn(&f, "hi", 3).await;
        assert!(stops[1].ends_with(" · ⟲ epoch 4"), "{stops:?}");
        assert!(!stops[2].contains("epoch"), "{stops:?}");
    }

    /// 110 §4.1: `ReasoningDone.ms` titles the thinking card that text ends.
    #[tokio::test(start_paused = true)]
    async fn reasoning_done_titles_the_thought_card_with_its_seconds() {
        let f = fixture();
        f.bind(OWNER);
        f.chan
            .route(Event::Agent(turn_started(f.stub.session, "go")))
            .await;
        f.chan
            .route(Event::Agent(AgentEvent::ReasoningDelta {
                session: f.stub.session,
                turn: TurnId::new(),
                text: "first idea".into(),
            }))
            .await;
        f.chan
            .route(Event::Agent(AgentEvent::ReasoningDone {
                session: f.stub.session,
                turn: TurnId::new(),
                ms: 3_000,
            }))
            .await;
        f.chan
            .route(Event::Agent(text_delta(f.stub.session, "answer")))
            .await;
        f.chan
            .route(Event::Agent(turn_finished(f.stub.session, "end_turn")))
            .await;
        f.until(|calls| calls.iter().any(|c| c.starts_with("stop_stream")))
            .await;
        let calls = f.api.calls();
        assert!(
            calls.iter().any(|c| c.contains("[Thought 3 s]")),
            "{calls:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn peer_delivered_from_a_child_posts_a_collapsed_container_titled_from_the_board_row() {
        let f = fixture();
        let child = spawn_child(&f).await;
        f.chan
            .route(Event::Peer(peer_delivered(
                f.stub.session,
                &format!("session:{child}"),
                "found it in auth.rs\n\nline 42 panics on an empty token",
            )))
            .await;
        let want = format!("post_blocks {DM} 100.1 1 blocks");
        f.until(|calls| calls.contains(&want)).await;
        // The blocks went through: no markdown fallback for the same body.
        let calls = f.api.calls();
        assert!(
            !calls
                .iter()
                .any(|c| c.starts_with("post_markdown") && c.contains("↳ grep-logs")),
            "{calls:?}"
        );
        let blocks = f.api.blocks();
        let c = &blocks[0][0];
        assert_eq!(c["type"], "container");
        assert_eq!(c["is_collapsible"], true);
        assert_eq!(c["default_collapsed"], true);
        let title = c["title"]["text"].as_str().unwrap();
        assert!(title.starts_with("↳ grep-logs · finished · "), "{title}");
        assert_eq!(c["subtitle"]["text"], "found it in auth.rs");
        // §14.5: no `markdown` child; the body is plain `section`s.
        assert_eq!(c["child_blocks"].as_array().unwrap().len(), 1);
        assert_eq!(c["child_blocks"][0]["type"], "section");
        assert_eq!(c["child_blocks"][0]["text"]["type"], "plain_text");
        assert_eq!(
            c["child_blocks"][0]["text"]["text"],
            "found it in auth.rs\n\nline 42 panics on an empty token"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn peer_delivered_body_is_clipped_with_a_count() {
        let f = fixture();
        f.bind(OWNER);
        let body = "y".repeat(MARKDOWN_TEXT_MAX + 500);
        f.chan
            .route(Event::Peer(peer_delivered(
                f.stub.session,
                "peer:remote-ops",
                &body,
            )))
            .await;
        let want = format!("post_blocks {DM} 100.1 1 blocks");
        f.until(|calls| calls.contains(&want)).await;
        let blocks = f.api.blocks();
        let c = &blocks[0][0];
        assert_eq!(c["title"]["text"], "↳ peer:remote-ops · delivered");
        let kids = c["child_blocks"].as_array().unwrap();
        assert!(kids.iter().all(|k| k["type"] == "section"), "{kids:?}");
        let text: String = kids
            .iter()
            .map(|k| k["text"]["text"].as_str().unwrap())
            .collect();
        assert!(text.chars().count() <= MARKDOWN_TEXT_MAX);
        assert!(
            text.ends_with("… (532 more chars)"),
            "{}",
            &text[text.len() - 40..]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn plan_message_posts_on_the_first_snapshot_and_turn_finished_forces_the_last_edit() {
        let f = fixture();
        f.bind(OWNER);
        f.chan
            .route(Event::Agent(turn_started(f.stub.session, "go")))
            .await;
        f.chan
            .route(Event::Agent(todo_changed(
                f.stub.session,
                1,
                &[("read the code", "in_progress"), ("fix it", "pending")],
            )))
            .await;
        let posted = format!("post_blocks {DM} 100.1 1 blocks");
        f.until(|calls| calls.contains(&posted)).await;
        let first = f.api.blocks()[0][0].clone();
        assert_eq!(first["type"], "plan");
        assert_eq!(first["title"], "read the code");
        // Two more snapshots inside the 2 s gap: coalesced, the last one wins on TurnFinished.
        f.chan
            .route(Event::Agent(todo_changed(
                f.stub.session,
                2,
                &[("read the code", "done"), ("fix it", "in_progress")],
            )))
            .await;
        f.chan
            .route(Event::Agent(todo_changed(
                f.stub.session,
                3,
                &[
                    ("read the code", "done"),
                    ("fix it", "done"),
                    ("old idea", "dropped"),
                    ("ship", "blocked"),
                ],
            )))
            .await;
        f.chan
            .route(Event::Agent(turn_finished(f.stub.session, "end_turn")))
            .await;
        let edited = format!("update_blocks {DM} 7.7");
        f.until(|calls| calls.contains(&edited)).await;
        let blocks = f.api.blocks();
        assert_eq!(
            blocks.iter().filter(|b| b[0]["type"] == "plan").count(),
            2,
            "one post, one edit: {blocks:?}"
        );
        let last = &blocks[blocks.len() - 1][0];
        assert_eq!(last["title"], "Plan · 2 of 3 done");
        let tasks = last["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 3, "dropped omitted");
        assert_eq!(tasks[2]["status"], "error");
        assert_eq!(
            tasks[2]["details"]["elements"][0]["elements"][0]["text"],
            "needs a token"
        );
        // The next turn's snapshot is a fresh message.
        f.chan
            .route(Event::Agent(todo_changed(
                f.stub.session,
                4,
                &[("ship", "in_progress")],
            )))
            .await;
        f.until(|calls| calls.iter().filter(|c| **c == posted).count() == 2)
            .await;
    }

    #[tokio::test(start_paused = true)]
    async fn plan_false_posts_nothing() {
        let f = fixture_with(SlackConfig {
            plan: false,
            ..SlackConfig::default()
        });
        f.bind(OWNER);
        f.chan
            .route(Event::Agent(turn_started(f.stub.session, "go")))
            .await;
        f.chan
            .route(Event::Agent(todo_changed(
                f.stub.session,
                1,
                &[("read the code", "in_progress")],
            )))
            .await;
        f.chan
            .route(Event::Agent(turn_finished(f.stub.session, "end_turn")))
            .await;
        let active = format!("set_session_status {DM} 100.1 active");
        f.until(|calls| calls.contains(&active)).await;
        assert!(f.api.blocks().is_empty(), "{:?}", f.api.calls());
    }

    #[tokio::test]
    async fn mention_text_is_prefixed_with_the_channel_marker_after_agent_routing() {
        let f = fixture();
        f.chan
            .handle_inbound(Inbound::Push(Box::new(mention(
                FRIEND,
                LISTED,
                "200.3",
                "<@UBOT> theuth: hi",
            ))))
            .await;
        assert_eq!(
            f.stub.calls(),
            vec![Call::SendInput(None, "[slack channel]\nhi".into())]
        );
    }

    #[tokio::test]
    async fn attachment_lines_carry_no_slack_marker() {
        let f = fixture();
        let mut ev = dm(OWNER, "100.1", None, "");
        let SlackEventCallbackBody::Message(m) = &mut ev.event else {
            unreachable!()
        };
        let content = m.content.get_or_insert_with(SlackMessageContent::new);
        content.files = Some(vec![
            serde_json::from_value(json!({
                "id": "F1",
                "name": "clip.m4a",
                "mimetype": "audio/mp4",
                "url_private_download": "https://files.slack.com/clip.m4a",
            }))
            .unwrap(),
        ]);
        f.chan.handle_inbound(Inbound::Push(Box::new(ev))).await;
        let calls = f.stub.calls();
        let Some(Call::SendInput(_, text)) = calls.last() else {
            panic!("{calls:?}");
        };
        assert!(text.starts_with("[attachment: "), "{text}");
        assert!(!text.contains("[slack"), "{text}");
    }

    #[tokio::test]
    async fn empty_text_with_no_files_still_posts_nothing() {
        let f = fixture();
        f.chan
            .handle_inbound(Inbound::Push(Box::new(dm(OWNER, "100.1", None, "<@UBOT>"))))
            .await;
        assert!(f.api.calls().is_empty());
        assert!(f.stub.calls().is_empty());
    }

    #[tokio::test]
    async fn theuth_help_answers_in_the_ack_with_blocks() {
        let f = fixture();
        let ev: SlackCommandEvent = serde_json::from_value(json!({
            "team_id": "T1", "channel_id": DM, "user_id": OWNER, "command": "/theuth",
            "text": "help", "response_url": "https://hooks.slack.com/commands/T1/1/x", "trigger_id": "tr1",
        }))
        .unwrap();
        let r = f.chan.command(ev).await;
        assert_eq!(r.response_type, Some(SlackMessageResponseType::Ephemeral));
        let help = ui::help();
        assert_eq!(r.content.text.as_deref(), Some(help.text.as_str()));
        let sent = r.content.blocks.expect("blocks in the ack");
        assert_eq!(sent.len(), help.blocks.unwrap().as_array().unwrap().len());
        assert!(matches!(sent[0], SlackBlock::Header(_)));
        assert!(
            f.api.calls().is_empty(),
            "instant answers do not use response_url"
        );
    }

    #[tokio::test]
    async fn cmd_click_by_an_allowed_user_replaces_the_panel_in_place() {
        let f = fixture();
        // An overflow "Resume here" pick: the command is the option value.
        // Resolving the id asks the agent actor, absent here, so the reply
        // is the error text — the routing, gate and reply path are what this checks.
        f.chan
            .handle_inbound(cmd_click(FRIEND, "cmd:menu:0", Some("cmd:resume:abcd1234")))
            .await;
        f.until(|calls| {
            calls.iter().any(|c| {
                c.starts_with("respond_blocks https://hooks.slack.com/actions/T1/9/x true ")
            })
        })
        .await;
        assert!(f.api.calls().iter().all(|c| !c.starts_with("ephemeral")));
    }

    #[tokio::test]
    async fn resume_outcome_posts_the_root_and_binds_the_thread() {
        let f = fixture();
        let session = SessionId::new();
        let reply = f
            .chan
            .after_command(
                Outcome::Resume {
                    session,
                    title: "t".into(),
                    reply: Reply::text("resumed abcd1234 · t"),
                },
                DM,
                FRIEND,
                "T1",
            )
            .await;
        assert_eq!(reply.text, "resumed abcd1234 · t");
        assert_eq!(
            f.api.calls(),
            [format!("post_markdown {DM} - resumed abcd1234 · t")]
        );
        let s = f.chan.lock();
        let (key, t) = s.threads.by_session(session).expect("bound");
        assert_eq!(
            (key.as_str(), t.user.as_str()),
            (format!("{DM}:8.8").as_str(), FRIEND)
        );
    }

    #[tokio::test]
    async fn cmd_click_by_a_stranger_is_refused_without_effect() {
        let f = fixture();
        f.chan
            .handle_inbound(cmd_click(
                STRANGER,
                "cmd:menu:0",
                Some("cmd:cancel:abcd1234"),
            ))
            .await;
        f.chan
            .handle_inbound(cmd_click(STRANGER, "cmd:skills:add", None))
            .await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            f.api.calls(),
            [
                format!("ephemeral {DM} {STRANGER} not allowed"),
                format!("ephemeral {DM} {STRANGER} not allowed")
            ]
        );
        assert!(f.stub.calls().is_empty());
    }

    #[tokio::test]
    async fn add_button_opens_the_modal_before_anything_else() {
        let f = fixture();
        f.chan
            .handle_inbound(cmd_click(OWNER, "cmd:skills:add", None))
            .await;
        f.chan
            .handle_inbound(cmd_click(OWNER, "cmd:mcp:add", None))
            .await;
        assert_eq!(
            f.api.calls(),
            [
                "views_open tr9 cmd:skills:add",
                "views_open tr9 cmd:mcp:add"
            ]
        );
    }

    #[tokio::test]
    async fn modal_submit_runs_the_add_and_answers_in_its_channel() {
        let f = fixture();
        let ev: SlackInteractionEvent = serde_json::from_value(json!({
            "type": "view_submission",
            "team": { "id": "T1" },
            "user": { "id": OWNER },
            "view": {
                "id": "V1", "team_id": "T1", "hash": "h",
                "type": "modal",
                "callback_id": "cmd:skills:add",
                "private_metadata": DM,
                "title": { "type": "plain_text", "text": "Add skills" },
                "blocks": [],
                "state": { "values": { "url": { "url": { "type": "plain_text_input", "value": "https://x/y.git" } } } },
            },
        }))
        .unwrap();
        f.chan.handle_inbound(Inbound::Action(Box::new(ev))).await;
        // No agent actor: the add fails, and the failure comes back as an ephemeral there.
        f.until(|calls| {
            calls
                .iter()
                .any(|c| c.starts_with(&format!("ephemeral {DM} {OWNER} error: agent actor")))
        })
        .await;
    }
}
