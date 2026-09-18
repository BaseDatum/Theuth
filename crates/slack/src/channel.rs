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
    AGENT_ACTOR, AgentEvent, AgentInfo, Agents, Cancel, ListAgents, ListSessions, SendInput,
    SessionSummary, Steer,
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
use tools::{
    APPROVAL_ACTOR, Answer, Approval, ApprovalId, ApprovalRequested, ApprovalResolved, AskId,
    AskRequested, AskResolved, Decision, Resolve,
};

use crate::api::{Api, ApiError, MARKDOWN_TEXT_MAX, SlackApi};
use crate::bubble::{self, Footer, Target, Turn};
use crate::commands::{self, CommandCx, Outcome, Reply};
use crate::threads::{Thread, ThreadKey, Threads, thread_key};
use crate::ui;
use crate::{
    ApprovalsScope, SlackConfig, SlackEvent, SlackStatus, ToolCards, approvals, asks, board, socket,
};

/// Dedup set size for `event_id` / `trigger_id` (21 §3.1).
pub const RECENT_KEYS: usize = 1024;
/// `agents.sessions.rename` title cap (21 §8.6).
pub const TITLE_MAX: usize = 200;
/// A child's task text cap in its `▸ **task**` line (21 §9.1).
pub const TASK_MAX: usize = 3000;
/// How long an instant `/wth` answer may take before it goes through `response_url` (21 §10.2).
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
        "wth asks for secret `{name}` — {note}. Enter it in the desktop app, the TUI (/secrets add {name}) or `wth --secret-set {name}`; it will not be read from this chat."
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
            })
            .await
            .map_err(|e| e.to_string())
    }

    async fn cancel(&self, session: SessionId) -> Result<(), String> {
        agents()?
            .ask(Cancel { session })
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
    /// Owner-DM cards only: `(ts, body)`.
    dm: Option<(String, String)>,
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
    /// Pending approval cards by id (thread and owner-DM alike).
    cards: HashMap<ApprovalId, Card>,
    /// Pending `ask_user` questions by id.
    asks: HashMap<AskId, AskCard>,
    /// child → (parent, child's name), from `SubagentSpawned` and `list_agents`.
    parent_of: HashMap<SessionId, (SessionId, String)>,
    /// `chat.getPermalink` results by thread key.
    permalinks: HashMap<ThreadKey, String>,
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
            socket::run(Arc::clone(&chan), app_token, shutdown).await;
        }
    }
    inbound.abort();
    router.abort();
    bus.kill();
}

/// Reaction names (21 §8.6).
const EYES: &str = "eyes";
const DONE: &str = "white_check_mark";
const STOPPED: &str = "octagonal_sign";
const FAILED: &str = "x";

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
                cards: HashMap::new(),
                asks: HashMap::new(),
                parent_of: HashMap::new(),
                permalinks: HashMap::new(),
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

    /// `/wth …` inside Slack's 3 s ack window (21 §10.2): instant answers
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
                        Thread {
                            session,
                            user: user.to_owned(),
                            team: team.to_owned(),
                            title: None,
                            updated_at: 0,
                        },
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
            SlackEventCallbackBody::Unknown => {
                self.lock().dropped += 1;
                tracing::debug!("unknown slack event type acked and dropped");
            }
            other => tracing::debug!(kind = ?std::mem::discriminant(&other), "slack event ignored"),
        }
    }

    /// A user message in a DM or a listed channel: files, 👀, ask, bind (21 §7).
    async fn handle_message(self: &Arc<Self>, m: Msg) {
        let mut lines: Vec<String> = Vec::new();
        let text = strip_mentions(&m.text).trim().to_owned();
        if !text.is_empty() {
            lines.push(text);
        }
        let (file_lines, files, voice) = self.save_files(&m.ts, &m.files).await;
        lines.extend(file_lines);
        if lines.is_empty() {
            return;
        }
        let input = lines.join("\n");
        let thread_ts = m.thread_ts.clone().unwrap_or_else(|| m.ts.clone());
        let key = thread_key(&m.channel, &thread_ts);
        self.react(&m.channel, &m.ts, EYES).await;
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
            _ => self.port.send_input(session, input, files).await,
        };
        match result {
            Ok(id) => {
                let thread = Thread {
                    session: id,
                    user: m.user,
                    team: m.team,
                    title: None,
                    updated_at: 0,
                };
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
            Err(e) => {
                tracing::warn!(error = %e, "agent ask failed");
                self.post_markdown(&m.channel, &thread_ts, &format!("⚠️ {e}"))
                    .await;
                self.react(&m.channel, &m.ts, FAILED).await;
            }
        }
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
                    (Some((ts, body)), _) => {
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
                        dm: Some((ts, body)),
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
        let thread = Thread {
            session: child,
            user: pthread.user.clone(),
            team: pthread.team.clone(),
            title: None,
            updated_at: 0,
        };
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
            task_posted: false,
            turn: None,
            started: Instant::now(),
            model: String::new(),
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
    /// User message ts awaiting ✅ / 🛑 / ❌.
    marks: Vec<String>,
    /// Cleared once `agents.sessions.setStatus` says the app is not an agent.
    status_supported: bool,
    title_sent: Option<String>,
    /// Stream ts last written to `threads.json.streams`.
    open_stream: Option<String>,
    /// The subagent's name when this thread renders a child (21 §9.1).
    name: Option<String>,
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
                    Some(Out::Ask(req, i)) => self.ask(*req, i).await,
                    Some(Out::Spawned { child, name, link, depth }) => self.spawned(child, name, link, depth).await,
                    Some(Out::Child(session, ev)) => self.child(session, &ev).await,
                    Some(Out::Artifact(a)) => self.artifact(*a).await,
                    Some(Out::Note(md)) => { self.post_markdown_in_thread(&md).await; }
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

    /// Uploads a `show` artifact into the thread, `title` as the comment.
    async fn artifact(&self, a: Artifact) {
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
        let comment = (!a.title.trim().is_empty()).then_some(a.title.as_str());
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

    /// A descendant got its thread: the `↳ spawned` line for a direct child
    /// and an idle board row.
    async fn spawned(&mut self, child: SessionId, name: String, link: String, depth: u8) {
        if depth == 1 {
            self.post_markdown_in_thread(&format!("↳ spawned **{name}** → <{link}|thread>"))
                .await;
        }
        self.rows.retain(|r| r.session != child);
        self.rows
            .push(board::Row::new(child, name, depth, Some(link)));
    }

    /// A descendant's event: its board row, plus one interjected line for a
    /// direct child's error or cancellation (21 §9.1).
    async fn child(&mut self, session: SessionId, ev: &AgentEvent) {
        let Some(row) = self.rows.iter_mut().find(|r| r.session == session) else {
            return;
        };
        self.rows_dirty |= row.apply(ev);
        if row.depth != 1 {
            return;
        }
        let name = row.name.clone();
        match ev {
            AgentEvent::Error { message, .. } => {
                let line = message.lines().next().unwrap_or_default();
                self.post_markdown_in_thread(&format!("↳ **{name}** error: {line}"))
                    .await;
            }
            AgentEvent::TurnFinished { stop_reason, .. } if stop_reason == "cancelled" => {
                self.post_markdown_in_thread(&format!("↳ **{name}** cancelled"))
                    .await;
            }
            _ => {}
        }
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
                    Arc::clone(&self.chan.api),
                    self.chan.rt.clone(),
                    session,
                ));
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
            }
            AgentEvent::TurnFinished {
                stop_reason, usage, ..
            } => {
                self.speak();
                if let Some(mut t) = self.turn.take() {
                    let streamed = t.open_stream().is_some();
                    let footer = Footer {
                        model: self.model.clone(),
                        usage,
                        elapsed: self.started.elapsed(),
                        tools: t.tool_count(),
                    };
                    t.finish(footer).await;
                    self.sync_stream();
                    if !streamed {
                        // Nothing streamed, so no stopStream carried `session_status`.
                        self.set_status("active").await;
                    }
                }
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
                self.post_markdown_in_thread(&format!("⚠️ {message}")).await;
                self.react_marks(FAILED).await;
            }
            AgentEvent::MessageDelivered { from, text, .. } => {
                let head = format!("⟵ **{from}**\n\n");
                let body = clip(
                    &text,
                    MARKDOWN_TEXT_MAX.saturating_sub(head.chars().count()),
                );
                self.post_markdown_in_thread(&format!("{head}{body}")).await;
            }
            AgentEvent::Compacted { epoch, folded, .. } => {
                self.post_markdown_in_thread(&format!(
                    "⟲ compacted → epoch {epoch} ({folded} messages folded)"
                ))
                .await;
            }
            // Intercepted by the router (`Channel::spawned`), or session
            // bookkeeping with nothing to say in a thread (`ContextChanged`
            // is a projection revision notice, 48 §3.2 — not a reply;
            // `TodoChanged` is plan state for persistent UIs, 47 §5.1 — a
            // thread is not one).
            AgentEvent::SubagentSpawned { .. }
            | AgentEvent::InputQueued { .. }
            | AgentEvent::InputInjected { .. }
            | AgentEvent::InputWithdrawn { .. }
            | AgentEvent::ModelChanged { .. }
            | AgentEvent::EffortChanged { .. }
            | AgentEvent::Compacting { .. }
            | AgentEvent::Harvesting { .. }
            | AgentEvent::Harvested { .. }
            | AgentEvent::SkillsChanged { .. }
            | AgentEvent::TitleChanged { .. }
            | AgentEvent::ToolCallProgress { .. }
            | AgentEvent::SessionDeleted { .. }
            | AgentEvent::ContextChanged { .. }
            | AgentEvent::TodoChanged { .. } => {}
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
    }

    impl FakeApi {
        pub(crate) fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
        fn record(&self, line: String) {
            self.calls.lock().unwrap().push(line);
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
            _: Vec<crate::api::Chunk>,
        ) -> Result<String, ApiError> {
            self.record(format!("start_stream {channel} {thread_ts}"));
            Ok("9.9".into())
        }
        async fn append_stream(
            &self,
            channel: &str,
            ts: &str,
            _: Vec<crate::api::Chunk>,
        ) -> Result<(), ApiError> {
            self.record(format!("append_stream {channel} {ts}"));
            Ok(())
        }
        async fn stop_stream(
            &self,
            channel: &str,
            ts: &str,
            _: Vec<crate::api::Chunk>,
            _: Option<String>,
            _: Option<&str>,
        ) -> Result<(), ApiError> {
            self.record(format!("stop_stream {channel} {ts}"));
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
            Ok("7.7".into())
        }
        async fn update_blocks(
            &self,
            channel: &str,
            ts: &str,
            _: serde_json::Value,
        ) -> Result<(), ApiError> {
            self.record(format!("update_blocks {channel} {ts}"));
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
                Thread {
                    session: self.stub.session,
                    user: user.into(),
                    team: "T1".into(),
                    title: None,
                    updated_at: 0,
                },
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
    async fn top_level_dm_from_owner_starts_a_session_and_binds_the_thread() {
        let f = fixture();
        f.chan
            .handle_inbound(Inbound::Push(Box::new(dm(
                OWNER, "100.1", None, "hi there",
            ))))
            .await;
        assert_eq!(f.api.calls(), vec![format!("react {DM} 100.1 {EYES}")]);
        assert_eq!(
            f.stub.calls(),
            vec![Call::SendInput(None, "hi there".into())]
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
                Thread {
                    session: f.stub.session,
                    user: OWNER.into(),
                    team: "T1".into(),
                    title: None,
                    updated_at: 0,
                },
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
            vec![Call::Steer(f.stub.session, "also this".into())]
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
        assert_eq!(f.stub.calls(), vec![Call::SendInput(None, "hi".into())]);
        assert!(
            f.chan
                .lock()
                .threads
                .get(&thread_key(LISTED, "200.3"))
                .is_some()
        );
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
        f.chan.lock().threads.bind(
            key,
            Thread {
                session: f.stub.session,
                user: OWNER.into(),
                team: "T1".into(),
                title: None,
                updated_at: 0,
            },
        );
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
                Some("wth"),
            ))
            .await;
        assert_eq!(
            f.stub.calls(),
            vec![Call::AnswerAsk(
                id,
                Some(vec![
                    Answer::Choice { index: 1 },
                    Answer::Text { text: "wth".into() },
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
            }))
            .await;
        child
    }

    #[tokio::test]
    async fn subagent_spawned_posts_the_child_root_and_binds_it() {
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
        let line = format!(
            "post_markdown {DM} 100.1 ↳ spawned **grep-logs** → <https://example.slack.com/archives/{DM}/p8.8|thread>"
        );
        f.until(|calls| calls.contains(&line)).await;
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

    #[tokio::test]
    async fn child_error_interjects_in_the_parent_thread() {
        let f = fixture();
        let child = spawn_child(&f).await;
        f.chan
            .route(Event::Agent(AgentEvent::Error {
                turn: None,
                session: child,
                message: "boom\nstack".into(),
            }))
            .await;
        let line = format!("post_markdown {DM} 100.1 ↳ **grep-logs** error: boom");
        f.until(|calls| calls.contains(&line)).await;
        let own = format!("post_markdown {DM} 8.8 ⚠️ boom\nstack");
        f.until(|calls| calls.contains(&own)).await;
    }

    #[tokio::test]
    async fn wth_help_answers_in_the_ack_with_blocks() {
        let f = fixture();
        let ev: SlackCommandEvent = serde_json::from_value(json!({
            "team_id": "T1", "channel_id": DM, "user_id": OWNER, "command": "/wth",
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
