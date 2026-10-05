//! The channel (design doc 98 §3, §5–§7, §9–§11): identity, the
//! hub-lease-gated poller, the bus subscriber, trust, inbound routing to
//! per-thread workers, approvals, asks, subagents, `show`, the owner chat
//! and the control [`Handle`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use agent::AgentEvent;
use agent::communication::PeerScope;
use artifact::ArtifactShown;
use kameo::Actor;
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use parking_lot::{Mutex, MutexGuard};
use runtime::{Runtime, SessionId};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tools::{
    ApprovalId, ApprovalRequested, ApprovalResolved, AskId, AskRequested, AskResolved, Decision,
};

use crate::api::{
    Api, ApiError, Ephemeral, FILE_TOO_BIG, Keyboard, MsgRef, OutFile, Sender, Target,
    is_not_modified, is_topic_gone, is_unreachable,
};
use crate::commands::{self, CommandCx, Expect, Outcome, Reply};
use crate::poll::{
    self, ChatKind, ChatRef, Incoming, IncomingCallback, IncomingMessage, Media, MediaKind,
    PollCtx, RecentKeys, SenderRef, Service,
};
use crate::port::{LivePort, Port};
use crate::threads::{self, Thread, ThreadKey, Threads, Topic};
use crate::worker::{self, RenderConfig, Row, VoiceHandles, WorkerCtx, WorkerMsg, WorkerNote};
use crate::{
    ApprovalsMode, BotIdentity, PAIR_TTL, Pairing, PairingLink, TelegramConfig, TelegramEvent,
    TelegramStatus, approvals, asks, mint_code, setup, ui,
};

/// Bot API download cap (98 §7.1).
const MAX_DOWNLOAD: u64 = 20 * 1024 * 1024;
/// `sendDocument` cap (98 §8.10).
const ARTIFACT_MAX: u64 = 50 * 1024 * 1024;
/// Album messages this close together are one input (98 §7).
const ALBUM_WINDOW: Duration = Duration::from_secs(1);
/// `createForumTopic` name cap.
const TOPIC_NAME_MAX: usize = 128;
/// The child's task line (98 §9.1).
const TASK_MAX: usize = 3000;
/// File caption cap.
const CAPTION_MAX: usize = 1024;
/// Longest detail on a board row (19 §3.1).
const DETAIL_MAX: usize = 60;
/// "seen, working" (98 §7).
const EYES: &str = "👀";
/// "no agent took it" (98 §7).
const FAILED: &str = "😱";
/// Caption on an app's source attachment (46 §8.2): the file, not controls.
const APP_NOTE: &str = "Interactive app — open on desktop/web";

/// `<data_dir>/telegram/threads.json`.
#[must_use]
pub fn threads_path(data_dir: &Path) -> PathBuf {
    data_dir.join("telegram").join("threads.json")
}

/// 37 §7: where to enter a secret the model asked for (never this chat).
#[must_use]
pub fn secret_notice(name: &str, note: &str) -> String {
    format!(
        "theuth asks for secret `{name}` — {note}. Enter it in the desktop app, the TUI (/secrets add {name}) or `t --secret-set {name}`; it will not be read from this chat."
    )
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
}

/// Bus subscriber: forwards into the ordered event task.
#[derive(Actor)]
struct Bus {
    tx: mpsc::UnboundedSender<Event>,
}

macro_rules! forward {
    ($ty:ty => $variant:ident) => {
        impl Message<$ty> for Bus {
            type Reply = ();
            async fn handle(&mut self, m: $ty, _: &mut Context<Self, ()>) {
                let _ = self.tx.send(Event::$variant(m));
            }
        }
    };
}

forward!(AgentEvent => Agent);
forward!(ApprovalRequested => Requested);
forward!(ApprovalResolved => Resolved);
forward!(AskRequested => Ask);
forward!(AskResolved => AskDone);
forward!(ArtifactShown => Artifact);
forward!(secrets::SecretsEvent => Secret);

impl Message<mesh::MeshEvent> for Bus {
    type Reply = ();
    async fn handle(&mut self, m: mesh::MeshEvent, _: &mut Context<Self, ()>) {
        if let mesh::MeshEvent::Lease(l) = m {
            let _ = self.tx.send(Event::Lease(l));
        }
    }
}

/// What the channel remembers about a chat it saw a message in.
#[derive(Clone, Debug, Default)]
struct ChatInfo {
    /// Group title or `@user` / name, for the `message_user` result.
    name: String,
    /// A forum supergroup (topics shape).
    forum: bool,
}

/// A child rendered in its parent's chat (98 §9.2): no thread key of its own.
#[derive(Clone, Debug)]
struct FlatChild {
    to: Target,
    user: u64,
}

/// Live state of one descendant, for the parent boards (19 §3.1 words).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChildKind {
    Idle,
    Running,
    Approval,
    Error,
    Cancelled,
}

impl ChildKind {
    fn glyph(self) -> &'static str {
        match self {
            Self::Idle => "·",
            Self::Running => "⠹",
            Self::Approval => "⚠",
            Self::Error | Self::Cancelled => "✗",
        }
    }

    fn word(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Approval => "approval",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
        }
    }
}

/// One descendant's board row, derived from the events the channel routes.
#[derive(Clone, Debug)]
struct ChildState {
    name: String,
    kind: ChildKind,
    detail: String,
    since: Instant,
}

impl ChildState {
    fn new(name: String) -> Self {
        Self {
            name,
            kind: ChildKind::Idle,
            detail: String::new(),
            since: Instant::now(),
        }
    }

    /// `true` when the row changed (`crates/slack/src/board.rs` `Row::apply`).
    fn apply(&mut self, ev: &AgentEvent) -> bool {
        match ev {
            AgentEvent::TurnStarted { .. } | AgentEvent::ToolCallFinished { .. } => {
                self.set(ChildKind::Running, "streaming".to_owned())
            }
            AgentEvent::ToolCallStarted { name, args, .. } => {
                let arg = first_arg(name, args);
                let detail = if arg.is_empty() {
                    name.clone()
                } else {
                    format!("{name} {}", cut(arg, DETAIL_MAX))
                };
                self.set(ChildKind::Running, detail)
            }
            AgentEvent::TurnFinished { stop_reason, .. } => {
                let kind = match stop_reason.as_str() {
                    "cancelled" => ChildKind::Cancelled,
                    "error" => ChildKind::Error,
                    _ => ChildKind::Idle,
                };
                self.set(kind, String::new())
            }
            AgentEvent::Error { message, .. } => {
                let line = message.lines().next().unwrap_or_default();
                self.set(ChildKind::Error, cut(line, DETAIL_MAX))
            }
            _ => false,
        }
    }

    /// `Some(tool)`: an approval is pending; `None`: answered, running again.
    fn approval(&mut self, tool: Option<&str>) -> bool {
        match tool {
            Some(tool) => self.set(ChildKind::Approval, format!("{tool} …")),
            None => {
                let detail = self.detail.trim_end_matches(" …").to_owned();
                self.set(ChildKind::Running, detail)
            }
        }
    }

    fn set(&mut self, kind: ChildKind, detail: String) -> bool {
        let changed = self.kind != kind || self.detail != detail;
        if self.kind != kind {
            self.since = Instant::now();
        }
        self.kind = kind;
        self.detail = detail;
        changed
    }

    fn row(&self) -> (Row, bool) {
        (
            Row {
                glyph: self.kind.glyph(),
                name: self.name.clone(),
                state: self.kind.word().to_owned(),
                detail: self.detail.clone(),
                age_secs: self.since.elapsed().as_secs(),
            },
            self.kind == ChildKind::Idle,
        )
    }
}

/// Where a session renders: its thread key, target and user.
#[derive(Clone, Debug)]
struct Bound {
    key: ThreadKey,
    to: Target,
    user: u64,
    session: SessionId,
    topic: Option<Topic>,
    child_name: Option<String>,
}

/// Mutable channel state; one lock, never held across an await.
pub(crate) struct State {
    pub(crate) allowed: Vec<u64>,
    pub(crate) groups: Vec<i64>,
    pub(crate) threads: Threads,
    /// One worker per bound thread (and per flat child), spawned on first use.
    pub(crate) workers: HashMap<ThreadKey, mpsc::UnboundedSender<WorkerMsg>>,
    pub(crate) last_event: Option<TelegramEvent>,
    pub(crate) dropped: u64,
    pub(crate) pairing: Option<Pairing>,
    /// 67 §4.4: polling is a hub duty; it follows the lease.
    pub(crate) follower: roster::lease::Follower,
    /// The running poller's token; `None` while stopped.
    pub(crate) poller: Option<CancellationToken>,
    /// Sessions with a turn in flight (`Steer` instead of `SendInput`).
    pub(crate) running: HashSet<SessionId>,
    /// Approval cards awaiting `ApprovalResolved`.
    cards: HashMap<ApprovalId, approvals::Card>,
    /// Ask cards awaiting `AskResolved`.
    asks: HashMap<AskId, asks::Card>,
    /// Child → (parent, name), from `SubagentSpawned`.
    parent_of: HashMap<SessionId, (SessionId, String)>,
    /// Children rendered inline in the parent's chat (98 §9.2).
    flat_children: HashMap<SessionId, FlatChild>,
    /// Board rows of known children.
    children: HashMap<SessionId, ChildState>,
    /// Children whose `▸ **task**` line was posted (98 §9.1).
    task_posted: HashSet<SessionId>,
    /// `↳ spawned` markers → the child a reply steers (98 §9.2).
    markers: HashMap<MsgRef, SessionId>,
    /// `force_reply` prompts → what the reply is for.
    prompts: HashMap<MsgRef, Expect>,
    /// Album messages pending the 1 s flush, by `(chat, media_group_id)`.
    albums: HashMap<(i64, String), Vec<IncomingMessage>>,
    /// Chats where the ephemeral `not allowed` already failed (logged once).
    ephemeral_failed: HashSet<i64>,
    /// What we know about chats we saw.
    chats: HashMap<i64, ChatInfo>,
}

/// Everything shared by the tasks and the [`Handle`].
pub(crate) struct Channel {
    pub(crate) api: Arc<dyn Sender>,
    /// The live façade (poller, `on_event`); `None` under a mock.
    live: Option<Arc<Api>>,
    pub(crate) rt: Runtime,
    pub(crate) identity: OnceLock<BotIdentity>,
    pub(crate) owner: Mutex<Option<u64>>,
    pub(crate) state: Mutex<State>,
    pub(crate) port: Arc<dyn Port>,
    pub(crate) cfg: TelegramConfig,
    /// Process-wide `update_id` dedup, shared with the poller (98 §11).
    pub(crate) recent: Arc<Mutex<RecentKeys>>,
    inbound_tx: mpsc::UnboundedSender<Incoming>,
    /// Worker → channel notes; drained by `run`.
    notes_tx: mpsc::UnboundedSender<WorkerNote>,
}

/// Control handle the `"telegram"` actor uses. Cheap to clone.
#[derive(Clone)]
pub struct Handle {
    pub(crate) chan: Arc<Channel>,
}

impl Handle {
    /// The 98 §13 snapshot.
    #[must_use]
    pub fn status(&self) -> TelegramStatus {
        self.chan.status()
    }

    /// `owner: Some(id)` sets the owner live and returns `Ok(None)` (no link
    /// needed); `None` mints a pairing code and returns the deep link.
    /// `Err` before `getMe` answered.
    pub async fn link(&self, owner: Option<u64>) -> Result<Option<PairingLink>, String> {
        if let Some(id) = owner {
            self.set_owner(id);
            return Ok(None);
        }
        self.chan.mint_link().await.map(Some)
    }

    /// Adds `user` to the live allowlist (the config file is `lib.rs`'s job).
    pub fn allow(&self, user: u64) {
        let mut s = self.chan.lock();
        if !s.allowed.contains(&user) {
            s.allowed.push(user);
        }
    }

    /// Removes `user` from the live allowlist.
    pub fn remove_allowed(&self, user: u64) {
        self.chan.lock().allowed.retain(|u| *u != user);
    }

    /// Adds `chat` to the live group list.
    pub fn allow_group(&self, chat: i64) {
        let mut s = self.chan.lock();
        if !s.groups.contains(&chat) {
            s.groups.push(chat);
        }
    }

    /// Removes `chat` from the live group list.
    pub fn remove_group(&self, chat: i64) {
        self.chan.lock().groups.retain(|c| *c != chat);
    }

    /// Sets the owner live (pairing, `telegram_link { owner }`); the owner
    /// leaves the allowlist.
    pub fn set_owner(&self, user: u64) {
        self.chan.set_owner(user);
    }
}

/// Spawns the channel under `rt.shutdown`; returns immediately. Fails only
/// when the HTTP client cannot be built.
pub fn spawn(cfg: TelegramConfig, token: String, rt: Runtime) -> anyhow::Result<Handle> {
    let api = Arc::new(Api::new(&token, None)?);
    let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
    let (chan, notes_rx) = Channel::new(
        Arc::clone(&api) as Arc<dyn Sender>,
        Some(api),
        cfg,
        rt,
        Arc::new(LivePort),
        inbound_tx,
    );
    let chan = Arc::new(chan);
    tokio::spawn(run(Arc::clone(&chan), inbound_rx, notes_rx));
    Ok(Handle { chan })
}

/// Supervisor (98 §3.1): bus, lease, inbound, event and note tasks; identity
/// with backoff; `deleteWebhook`; command scopes; boot repair; then the
/// poller follows the lease until shutdown.
async fn run(
    chan: Arc<Channel>,
    mut inbound_rx: mpsc::UnboundedReceiver<Incoming>,
    mut notes_rx: mpsc::UnboundedReceiver<WorkerNote>,
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
    if let Some(live) = &chan.live {
        let hook = Arc::clone(&chan);
        live.on_event(Arc::new(move |ev| {
            let chan = Arc::clone(&hook);
            tokio::spawn(async move { chan.publish(ev).await });
        }));
    }
    // 67 §4.7: learn our engine id once the mesh is up; the lease resolves then.
    let me = {
        let chan = Arc::clone(&chan);
        tokio::spawn(async move {
            let Some(mesh) = chan.rt.lookup_when::<mesh::Mesh>(mesh::MESH_ACTOR).await else {
                return;
            };
            if let Ok(st) = mesh.ask(mesh::Status).await {
                chan.lock().follower.set_self(st.self_id.0);
                chan.sync_poller().await;
            }
        })
    };
    let inbound = {
        let chan = Arc::clone(&chan);
        tokio::spawn(async move {
            while let Some(inc) = inbound_rx.recv().await {
                chan.handle_inbound(inc).await;
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
    let notes = {
        let chan = Arc::clone(&chan);
        tokio::spawn(async move {
            while let Some(n) = notes_rx.recv().await {
                chan.handle_note(n).await;
            }
        })
    };
    let shutdown = chan.rt.shutdown.clone();
    let mut attempt: u32 = 0;
    let identity = loop {
        match chan.api.me().await {
            Ok(me) => break Some(me),
            Err(e) => {
                let delay = poll::backoff_delay(attempt);
                attempt = attempt.saturating_add(1);
                tracing::warn!(error = %e, delay_s = delay.as_secs(), "telegram getMe failed; retrying");
                chan.publish(TelegramEvent::Disconnected {
                    reason: format!("getMe: {e}"),
                })
                .await;
                tokio::select! {
                    () = tokio::time::sleep(delay) => {},
                    () = shutdown.cancelled() => break None,
                }
            }
        }
    };
    if let Some(me) = identity {
        tracing::info!(bot = %me.username, id = me.id, topics = me.topics, "telegram identity resolved");
        chan.publish(TelegramEvent::Identity {
            bot: me.username.clone(),
            topics: me.topics,
        })
        .await;
        let _ = chan.identity.set(me);
        if let Err(e) = chan.api.delete_webhook().await {
            tracing::warn!(error = %e, "telegram deleteWebhook failed");
        }
        let owner = *chan.owner.lock();
        chan.set_command_scopes(owner).await;
        chan.repair_open().await;
        chan.sync_poller().await;
        shutdown.cancelled().await;
    }
    me.abort();
    inbound.abort();
    events.abort();
    notes.abort();
    bus.kill();
}

impl Channel {
    fn new(
        api: Arc<dyn Sender>,
        live: Option<Arc<Api>>,
        cfg: TelegramConfig,
        rt: Runtime,
        port: Arc<dyn Port>,
        inbound_tx: mpsc::UnboundedSender<Incoming>,
    ) -> (Self, mpsc::UnboundedReceiver<WorkerNote>) {
        let owner = cfg.owner;
        let allowed = cfg
            .allowed
            .iter()
            .copied()
            .filter(|u| Some(*u) != owner)
            .collect();
        let (notes_tx, notes_rx) = mpsc::unbounded_channel();
        let chan = Self {
            api,
            live,
            identity: OnceLock::new(),
            owner: Mutex::new(owner),
            state: Mutex::new(State {
                allowed,
                groups: cfg.groups.clone(),
                threads: Threads::load(&threads_path(&rt.data_dir)),
                workers: HashMap::new(),
                last_event: None,
                dropped: 0,
                pairing: None,
                follower: roster::lease::Follower::default(),
                poller: None,
                running: HashSet::new(),
                cards: HashMap::new(),
                asks: HashMap::new(),
                parent_of: HashMap::new(),
                flat_children: HashMap::new(),
                children: HashMap::new(),
                task_posted: HashSet::new(),
                markers: HashMap::new(),
                prompts: HashMap::new(),
                albums: HashMap::new(),
                ephemeral_failed: HashSet::new(),
                chats: HashMap::new(),
            }),
            port,
            cfg,
            recent: Arc::new(Mutex::new(RecentKeys::default())),
            inbound_tx,
            notes_tx,
            rt,
        };
        (chan, notes_rx)
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock()
    }

    /// Publishes `ev` on the bus and remembers it for `Status`.
    pub(crate) async fn publish(&self, ev: TelegramEvent) {
        self.lock().last_event = Some(ev.clone());
        self.rt.publish(ev).await;
    }

    /// Sets the owner live; the owner leaves the allowlist.
    pub(crate) fn set_owner(&self, user: u64) {
        *self.owner.lock() = Some(user);
        self.lock().allowed.retain(|u| *u != user);
    }

    fn is_allowed(&self, user: u64) -> bool {
        *self.owner.lock() == Some(user) || self.lock().allowed.contains(&user)
    }

    /// Mints a pairing code, publishes `PairingLink`, returns the link (98 §10.4).
    pub(crate) async fn mint_link(&self) -> Result<PairingLink, String> {
        let me = self
            .identity
            .get()
            .ok_or_else(|| "bot identity unknown yet".to_string())?;
        let code = mint_code();
        let url = setup::deep_link(&me.username, &code);
        let expires_at_ms = u64::try_from(chrono::Utc::now().timestamp_millis())
            .unwrap_or(0)
            .saturating_add(PAIR_TTL.as_millis() as u64);
        self.lock().pairing = Some(Pairing {
            code,
            expires: Instant::now() + PAIR_TTL,
        });
        let link = PairingLink { url, expires_at_ms };
        self.publish(TelegramEvent::PairingLink {
            url: link.url.clone(),
            expires_at_ms,
        })
        .await;
        Ok(link)
    }

    /// 67 §4.4: starts the poller when this engine holds the lease and
    /// identity is up; stops it when the lease went elsewhere. Idempotent.
    async fn sync_poller(self: &Arc<Self>) {
        let Some(live) = &self.live else { return };
        let stopped = {
            let mut s = self.lock();
            let me = self.identity.get().filter(|_| s.follower.holding).cloned();
            match (me, s.poller.take()) {
                (Some(_), Some(tok)) => {
                    s.poller = Some(tok);
                    false
                }
                (Some(me), None) => {
                    let tok = self.rt.shutdown.child_token();
                    let chan = Arc::clone(self);
                    let ctx = PollCtx {
                        api: Arc::clone(live),
                        me,
                        tx: self.inbound_tx.clone(),
                        rt: self.rt.clone(),
                        stop: tok.clone(),
                        on_state: Arc::new(move |ev| {
                            let chan = Arc::clone(&chan);
                            tokio::spawn(async move { chan.publish(ev).await });
                        }),
                        recent: Arc::clone(&self.recent),
                    };
                    tokio::spawn(poll::run(ctx));
                    s.poller = Some(tok);
                    false
                }
                (None, Some(tok)) => {
                    tok.cancel();
                    true
                }
                (None, None) => false,
            }
        };
        if stopped {
            tracing::info!("telegram poller stopped: another engine holds the hub lease");
            self.publish(TelegramEvent::Disconnected {
                reason: "hub lease held elsewhere".into(),
            })
            .await;
        }
    }

    /// 98 §11: edit-mode messages a previous process left mid-edit. Best
    /// effort: the keyboard goes; the text is RENDER's (`OpenMsg` carries no
    /// text to append `· interrupted` to).
    // NEEDS(RENDER): either persist the tail in `OpenMsg` or accept this repair.
    async fn repair_open(&self) {
        let open = self.lock().threads.open_take();
        for m in open {
            let at = MsgRef {
                chat: m.chat,
                id: m.message_id,
            };
            match self.api.edit_markup(&at, None).await {
                Ok(()) => {}
                Err(e) if is_not_modified(&e) => {}
                Err(e) => {
                    tracing::debug!(chat = m.chat, message_id = m.message_id, error = %e, "stale telegram message not repaired")
                }
            }
        }
    }

    /// `setMyCommands` for the three 98 §10.3 scopes.
    async fn set_command_scopes(&self, owner: Option<u64>) {
        for (scope, cmds) in commands::scopes(owner.and_then(|o| i64::try_from(o).ok())) {
            if let Err(e) = self.api.set_commands(scope, &cmds).await {
                tracing::warn!(error = %e, ?scope, "telegram setMyCommands failed");
            }
        }
    }

    /// The 98 §13 snapshot.
    pub(crate) fn status(&self) -> TelegramStatus {
        let owner = *self.owner.lock();
        let s = self.lock();
        TelegramStatus {
            state: s.last_event.clone(),
            bot: self.identity.get().cloned(),
            polling: s.poller.is_some(),
            owner,
            allowed: s.allowed.clone(),
            groups: s.groups.clone(),
            threads: s.threads.list(),
            open: s.threads.open_count(),
            calls: self.api.stats(),
            dropped_updates: s.dropped,
        }
    }

    /// Consumes the live pairing code when `code` matches (98 §10.4).
    pub(crate) fn take_pairing(&self, code: &str) -> bool {
        let mut s = self.lock();
        match &s.pairing {
            Some(p) if p.code.eq_ignore_ascii_case(code) && p.expires > Instant::now() => {
                s.pairing = None;
                true
            }
            _ => false,
        }
    }

    // -- sends with the §4.3 status table ----------------------------------

    /// `send_rich` plus the unreachable / topic-gone bookkeeping (98 §4.3).
    pub(crate) async fn send(
        &self,
        to: &Target,
        md: &str,
        markup: Option<&Keyboard>,
        reply_to: Option<i64>,
        ephemeral: Option<&Ephemeral>,
    ) -> Result<i64, ApiError> {
        let r = self
            .api
            .send_rich(to, md, markup, reply_to, ephemeral)
            .await;
        if let Err(e) = &r {
            self.on_send_error(to, e).await;
        }
        r
    }

    /// `send_file` plus the unreachable / topic-gone bookkeeping.
    pub(crate) async fn send_file(
        &self,
        to: &Target,
        file: &OutFile,
        caption: Option<&str>,
        reply_to: Option<i64>,
    ) -> Result<i64, ApiError> {
        let r = self.api.send_file(to, file, caption, reply_to).await;
        if let Err(e) = &r {
            self.on_send_error(to, e).await;
        }
        r
    }

    /// 98 §4.3 / §6.1: a blocked / deactivated / kicked chat loses its
    /// threads and publishes `Unreachable`; a closed or deleted topic is unbound.
    pub(crate) async fn on_send_error(&self, to: &Target, e: &ApiError) {
        if is_unreachable(e) {
            tracing::warn!(chat = to.chat, error = %e, "telegram chat unreachable; unbinding its threads");
            self.unbind_chat(to.chat);
            self.publish(TelegramEvent::Unreachable {
                chat: to.chat,
                reason: e.to_string(),
            })
            .await;
        } else if is_topic_gone(e) {
            tracing::info!(chat = to.chat, thread = ?to.thread, error = %e, "telegram topic gone; unbinding");
            self.unbind_key(&threads::key(to.chat, to.thread));
        }
    }

    /// Best-effort line into a thread.
    async fn note(&self, to: &Target, md: &str) {
        if let Err(e) = self.send(to, md, None, None, None).await {
            tracing::debug!(chat = to.chat, error = %e, "telegram note failed");
        }
    }

    async fn react(&self, at: MsgRef, emoji: &str) {
        if let Err(e) = self.api.react(&at, Some(emoji)).await {
            tracing::debug!(chat = at.chat, message_id = at.id, emoji, error = %e, "telegram reaction failed");
        }
    }

    async fn answer(&self, id: &str, toast: Option<&str>) {
        if let Err(e) = self.api.answer_callback(id, toast).await {
            tracing::debug!(error = %e, "telegram answerCallbackQuery failed");
        }
    }

    // -- bindings and workers ------------------------------------------------

    /// Drops `key` and its worker.
    fn unbind_key(&self, key: &str) {
        let mut s = self.lock();
        s.threads.unbind(key);
        shutdown_worker(&mut s, key);
    }

    /// Drops every thread, flat child and worker in `chat`.
    fn unbind_chat(&self, chat: i64) {
        let mut s = self.lock();
        s.threads.unbind_chat(chat);
        let prefix = format!("{chat}#");
        let keys: Vec<ThreadKey> = s
            .workers
            .keys()
            .filter(|k| {
                k.starts_with(&prefix) || threads::target(k).is_some_and(|t| t.chat == chat)
            })
            .cloned()
            .collect();
        for k in keys {
            shutdown_worker(&mut s, &k);
        }
        s.flat_children.retain(|_, c| c.to.chat != chat);
        s.markers.retain(|m, _| m.chat != chat);
    }

    /// Binds `key` to `thread`, replacing the worker when the session changed.
    fn rebind(&self, key: &ThreadKey, thread: Thread) {
        let mut s = self.lock();
        if s.threads
            .get(key)
            .is_some_and(|t| t.session != thread.session)
        {
            shutdown_worker(&mut s, key);
        }
        s.threads.bind(key.clone(), thread);
    }

    /// Where `session` renders: its own thread, else its flat-child slot.
    fn bound(&self, session: SessionId) -> Option<Bound> {
        let s = self.lock();
        let child_name = s.parent_of.get(&session).map(|(_, n)| n.clone());
        if let Some((key, t)) = s.threads.by_session(session) {
            return Some(Bound {
                key: key.clone(),
                to: threads::target(key)?,
                user: t.user,
                session,
                topic: t.topic,
                child_name,
            });
        }
        let c = s.flat_children.get(&session)?;
        Some(Bound {
            key: format!("{}#{session}", c.to.chat),
            to: c.to,
            user: c.user,
            session,
            topic: None,
            child_name,
        })
    }

    /// The thread at `key`.
    fn bound_key(&self, key: &str) -> Option<Bound> {
        let s = self.lock();
        let t = s.threads.get(key)?;
        Some(Bound {
            key: key.to_owned(),
            to: threads::target(key)?,
            user: t.user,
            session: t.session,
            topic: t.topic,
            child_name: s.parent_of.get(&t.session).map(|(_, n)| n.clone()),
        })
    }

    /// `session`'s bound ancestors, nearest first (98 §9.3 fan-out).
    fn ancestors(&self, session: SessionId) -> Vec<Bound> {
        let chain: Vec<SessionId> = {
            let s = self.lock();
            let mut out = Vec::new();
            let mut cur = session;
            while let Some((parent, _)) = s.parent_of.get(&cur)
                && out.len() < 16
            {
                out.push(*parent);
                cur = *parent;
            }
            out
        };
        chain.into_iter().filter_map(|p| self.bound(p)).collect()
    }

    /// The worker for `b`, spawned on first use.
    fn worker(self: &Arc<Self>, b: &Bound) -> mpsc::UnboundedSender<WorkerMsg> {
        let mut s = self.lock();
        if let Some(tx) = s.workers.get(&b.key)
            && !tx.is_closed()
        {
            return tx.clone();
        }
        let ctx = WorkerCtx {
            key: b.key.clone(),
            to: b.to,
            user: b.user,
            session: b.session,
            api: Arc::clone(&self.api),
            port: Arc::clone(&self.port),
            rt: self.rt.clone(),
            cfg: RenderConfig {
                tools: self.cfg.tools,
                reasoning: self.cfg.reasoning,
            },
            voice: VoiceHandles {
                voice: ActorRef::<voice::Voice>::lookup(voice::VOICE_ACTOR)
                    .ok()
                    .flatten(),
                stt: ActorRef::<voice::Transcriber>::lookup(voice::TRANSCRIBER_ACTOR)
                    .ok()
                    .flatten(),
            },
            topic: b.topic,
            notes: self.notes_tx.clone(),
            child_name: b.child_name.clone(),
        };
        // NEEDS(RENDER): `WorkerCtx` has no marker message id, so a flat
        // child's messages cannot quote its `↳ spawned` marker (98 §9.2).
        let tx = worker::spawn(ctx);
        s.workers.insert(b.key.clone(), tx.clone());
        tx
    }

    /// Topics or flat shape for `chat` (98 §6): `getMe` for private chats,
    /// `is_forum` as last seen for groups.
    fn topics_shape(&self, chat: i64) -> bool {
        if chat > 0 {
            self.identity.get().is_some_and(|me| me.topics)
        } else {
            self.lock().chats.get(&chat).is_some_and(|c| c.forum)
        }
    }

    fn remember_chat(&self, chat: &ChatRef, from: &SenderRef) {
        let name = match chat.kind {
            ChatKind::Private => from
                .username
                .as_ref()
                .map_or_else(|| from.name.clone(), |u| format!("@{u}")),
            _ => chat
                .title
                .clone()
                .unwrap_or_else(|| format!("chat {}", chat.id)),
        };
        let forum = matches!(chat.kind, ChatKind::Supergroup { forum: true });
        self.lock().chats.insert(chat.id, ChatInfo { name, forum });
    }

    /// The `message_user` result label for `chat`: title or `@user`, else the id.
    pub(crate) fn chat_label(&self, chat: i64) -> String {
        self.lock()
            .chats
            .get(&chat)
            .map_or_else(|| format!("chat {chat}"), |c| c.name.clone())
    }

    /// Where `session` renders, for `message_user` (24 §5).
    pub(crate) fn target_for(&self, session: SessionId) -> Option<Target> {
        self.bound(session).map(|b| b.to)
    }

    // -- inbound (98 §7) -----------------------------------------------------

    async fn handle_inbound(self: &Arc<Self>, inc: Incoming) {
        match inc {
            Incoming::Message(m) => self.handle_message(*m).await,
            Incoming::Callback(cb) => self.handle_callback(cb).await,
            Incoming::Stopped {
                chat,
                thread,
                draft_id,
                ..
            } => {
                if let Some(b) = self.bound_key(&threads::key(chat, thread)) {
                    let _ = self.worker(&b).send(WorkerMsg::Stopped { draft_id });
                }
            }
            Incoming::MyChatMember { chat, left, .. } => {
                if left {
                    tracing::warn!(
                        chat,
                        "telegram bot removed from chat; unbinding its threads"
                    );
                    self.unbind_chat(chat);
                } else {
                    tracing::info!(
                        chat,
                        "telegram bot added to chat (listening needs `/telegram group add`)"
                    );
                }
            }
            Incoming::Ignored { update_id, kind } => {
                self.lock().dropped += 1;
                tracing::debug!(update_id, kind, "telegram update ignored");
            }
        }
    }

    /// 98 §7 in order: bots, channels, pairing, trust, service messages,
    /// lease, `MarkAnswered`, commands, prompt replies, ask replies, marker
    /// replies, albums, then the thread key and the ask.
    async fn handle_message(self: &Arc<Self>, m: IncomingMessage) {
        let Some(from) = m.from.clone() else {
            tracing::debug!(
                chat = m.chat.id,
                "telegram message without a sender dropped"
            );
            return;
        };
        if from.is_bot || m.chat.kind == ChatKind::Channel {
            return;
        }
        let in_group = m.chat.kind != ChatKind::Private;
        self.remember_chat(&m.chat, &from);
        // Pairing: the one message from a stranger the channel acts on (98 §5).
        if let Some(cmd) = &m.command
            && cmd.name == "start"
            && !cmd.for_other_bot
            && !cmd.arg.is_empty()
            && self.take_pairing(cmd.arg.trim())
        {
            self.pair(&m, &from).await;
            return;
        }
        let allowed = self.is_allowed(from.id);
        if in_group {
            if !self.lock().groups.contains(&m.chat.id) {
                tracing::debug!(
                    chat = m.chat.id,
                    "telegram message in an unlisted group ignored"
                );
                return;
            }
            if !allowed {
                if m.mentions_bot || m.command.as_ref().is_some_and(|c| !c.for_other_bot) {
                    self.not_allowed(&m, from.id).await;
                }
                return;
            }
        } else if !allowed {
            tracing::warn!(
                user = from.id,
                "dropping telegram message from a user not in the allowlist"
            );
            return;
        }
        if let Some(service) = &m.service {
            let key = threads::key(m.chat.id, m.thread);
            match service {
                Service::TopicEdited { .. } => {
                    self.lock().threads.set_named_by_user(&key, true);
                    if let Some(b) = self.bound_key(&key) {
                        let _ = self.worker(&b).send(WorkerMsg::TopicNamedByUser(true));
                    }
                }
                Service::TopicClosed => self.unbind_key(&key),
                _ => tracing::debug!(?service, "telegram service message ignored"),
            }
            return;
        }
        // 67 decision 22: only the lease holder answers.
        if !self.lock().follower.holding {
            tracing::debug!(
                chat = m.chat.id,
                update_id = m.update_id,
                "not the hub; recorded, silent"
            );
            return;
        }
        // 67 §4.7 answered markers: the first hub to mark it answers. Tasks
        // down (`None` / `Err`) must not mute Telegram, so only `Ok(false)` stops.
        if let Some(tasks) = tasks::actor()
            && let Ok(false) = tasks
                .ask(tasks::MarkAnswered {
                    sender: format!("{}:{}", m.chat.id, from.id),
                    timestamp: u64::try_from(m.update_id).unwrap_or(0),
                })
                .await
        {
            tracing::debug!(
                chat = m.chat.id,
                update_id = m.update_id,
                "already answered by another hub"
            );
            return;
        }
        let owner = *self.owner.lock();
        if let Some(cmd) = &m.command
            && !cmd.for_other_bot
        {
            if cmd.name == "start" {
                // A dead code, or `/start` from a non-owner: silence (98 §10.4).
                if cmd.arg.is_empty() && owner == Some(from.id) {
                    let to = Target {
                        chat: m.chat.id,
                        thread: m.thread,
                    };
                    self.reply(&to, from.id, in_group, &commands::help(), None)
                        .await;
                }
                return;
            }
            self.command(&m, &from, in_group).await;
            return;
        }
        if let Some(r) = &m.reply_to {
            let at = MsgRef {
                chat: m.chat.id,
                id: r.message_id,
            };
            let expect = self.lock().prompts.remove(&at);
            if let Some(expect) = expect {
                let text = m.text.clone().unwrap_or_default();
                let out = {
                    let cx = self.command_cx(from.id, m.chat.id, m.thread, in_group);
                    commands::continue_prompt(cx, expect, text.trim()).await
                };
                let to = Target {
                    chat: m.chat.id,
                    thread: m.thread,
                };
                self.apply_outcome(out, &to, &from, None).await;
                return;
            }
            let ask = self
                .lock()
                .asks
                .iter()
                .find(|(_, c)| c.at == Some(at))
                .map(|(id, _)| *id);
            if let Some(id) = ask {
                self.ask_text(id, &m, &from).await;
                return;
            }
        }
        if m.media_group_id.is_some() {
            self.album(m).await;
            return;
        }
        self.route_message(m, from, owner).await;
    }

    /// 98 §7: an album's messages within 1 s become one input.
    async fn album(self: &Arc<Self>, m: IncomingMessage) {
        let Some(gid) = m.media_group_id.clone() else {
            return;
        };
        let key = (m.chat.id, gid);
        let first = {
            let mut s = self.lock();
            let pending = s.albums.entry(key.clone()).or_default();
            pending.push(m);
            pending.len() == 1
        };
        if first {
            let chan = Arc::clone(self);
            tokio::spawn(async move {
                tokio::time::sleep(ALBUM_WINDOW).await;
                chan.flush_album(&key).await;
            });
        }
    }

    /// Merges the pending album into its first message and routes it.
    async fn flush_album(self: &Arc<Self>, key: &(i64, String)) {
        let msgs = self.lock().albums.remove(key).unwrap_or_default();
        let mut iter = msgs.into_iter();
        let Some(mut head) = iter.next() else { return };
        for m in iter {
            if head.text.as_deref().is_none_or(str::is_empty) {
                head.text = m.text;
            }
            head.media.extend(m.media);
        }
        head.media_group_id = None;
        let Some(from) = head.from.clone() else {
            return;
        };
        let owner = *self.owner.lock();
        self.route_message(head, from, owner).await;
    }

    /// The thread key (98 §6), the text (98 §7, §7.1), 👀, the ask, the bind.
    async fn route_message(
        self: &Arc<Self>,
        m: IncomingMessage,
        from: SenderRef,
        owner: Option<u64>,
    ) {
        let in_group = m.chat.kind != ChatKind::Private;
        let chat = m.chat.id;
        let addressed = m.mentions_bot
            || m.command.is_some()
            || m.reply_to.as_ref().is_some_and(|r| r.from_bot);
        // A reply to a child's marker steers that child (98 §9.2).
        let marker = m.reply_to.as_ref().and_then(|r| {
            self.lock()
                .markers
                .get(&MsgRef {
                    chat,
                    id: r.message_id,
                })
                .copied()
        });
        let topics = self.topics_shape(chat);
        let (key, general) = match (topics, m.thread) {
            (true, Some(t)) => (Some(threads::key(chat, Some(t))), false),
            (true, None) => (None, true),
            (false, _) => (Some(threads::key(chat, None)), false),
        };
        let bound = key
            .as_deref()
            .and_then(|k| self.lock().threads.get(k).cloned());
        if marker.is_none() && in_group && (!topics || bound.is_none()) && !addressed {
            tracing::debug!(chat, "unaddressed telegram group message ignored");
            return;
        }
        let (input, files, voice) = self.assemble(&m).await;
        if input.is_empty() {
            return;
        }
        let mark = MsgRef {
            chat,
            id: m.message_id,
        };
        let here = Target {
            chat,
            thread: m.thread,
        };
        self.react(mark, EYES).await;
        if let Some(child) = marker {
            let running = self.lock().running.contains(&child);
            let r = if running {
                self.port.steer(child, input, files).await
            } else {
                self.port
                    .send_input(Some(child), input, files, PeerScope::Isolated)
                    .await
                    .map(|_| ())
            };
            match (r, self.bound(child)) {
                (Ok(()), Some(b)) => {
                    let _ = self.worker(&b).send(WorkerMsg::Mark {
                        message_id: m.message_id,
                        voice,
                    });
                }
                (Ok(()), None) => {}
                (Err(e), _) => self.failed(&here, mark, &e).await,
            }
            return;
        }
        let (session, running) = {
            let s = self.lock();
            let session = bound.as_ref().map(|t| t.session);
            (session, session.is_some_and(|id| s.running.contains(&id)))
        };
        let scope = if owner == Some(from.id) {
            PeerScope::Owner
        } else {
            PeerScope::Isolated
        };
        let result = match session {
            Some(id) if running => self.port.steer(id, input.clone(), files).await.map(|()| id),
            Some(id) => {
                self.port
                    .send_input(Some(id), input.clone(), files, scope)
                    .await
            }
            None => {
                self.port
                    .send_input(None, input.clone(), files, scope)
                    .await
            }
        };
        let id = match result {
            Ok(id) => id,
            Err(e) => return self.failed(&here, mark, &e).await,
        };
        let (key, topic) = match key {
            Some(k) => {
                let topic = bound.as_ref().and_then(|t| t.topic).or_else(|| {
                    m.thread.filter(|_| topics).map(|id| Topic {
                        id,
                        named_by_user: true,
                    })
                });
                (k, topic)
            }
            None => {
                debug_assert!(general);
                let name = topic_name(&input);
                match self.api.create_topic(chat, &name).await {
                    Ok(t) => (
                        threads::key(chat, Some(t)),
                        Some(Topic {
                            id: t,
                            named_by_user: false,
                        }),
                    ),
                    Err(e) => {
                        tracing::warn!(chat, error = %e, "createForumTopic failed; falling back to the flat shape");
                        (threads::key(chat, None), None)
                    }
                }
            }
        };
        if session == Some(id) {
            self.lock().threads.touch(&key);
        } else {
            self.rebind(&key, Thread::new(id, from.id, topic));
        }
        if let Some(b) = self.bound_key(&key) {
            let _ = self.worker(&b).send(WorkerMsg::Mark {
                message_id: m.message_id,
                voice,
            });
        }
    }

    /// The ⚠️ line and 😱 for a message no agent took.
    async fn failed(&self, to: &Target, mark: MsgRef, e: &str) {
        tracing::warn!(error = %e, "agent ask failed");
        self.note(to, &format!("⚠️ {e}")).await;
        self.react(mark, FAILED).await;
    }

    /// 98 §7 text: `(forwarded from …)`, `(replying to: "…")`, the text with
    /// the `@bot` mention stripped, one line per attachment (§7.1).
    async fn assemble(&self, m: &IncomingMessage) -> (String, Vec<PathBuf>, bool) {
        let mut lines: Vec<String> = Vec::new();
        if let Some(name) = m
            .forward_from
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
        {
            lines.push(format!("(forwarded from {name})"));
        }
        if let Some(q) = m
            .reply_to
            .as_ref()
            .and_then(|r| r.text.as_deref())
            .map(str::trim)
            .filter(|q| !q.is_empty())
        {
            lines.push(format!("(replying to: \"{q}\")"));
        }
        let username = self.identity.get().map_or("", |me| me.username.as_str());
        let text = strip_mention(m.text.as_deref().unwrap_or(""), username);
        if !text.is_empty() {
            lines.push(text);
        }
        let (file_lines, files, voice) = self.save_media(m).await;
        lines.extend(file_lines);
        (lines.join("\n"), files, voice)
    }

    /// 98 §7.1: downloads to `telegram/inbox/<ts>-<name>`, one
    /// `[attachment: …]` line each plus the paths of images and PDFs; voice,
    /// video notes and audio are transcribed (45 §4.5.1); stickers, locations
    /// and contacts are one descriptive line.
    async fn save_media(&self, m: &IncomingMessage) -> (Vec<String>, Vec<PathBuf>, bool) {
        let mut lines = Vec::with_capacity(m.media.len());
        let mut paths = Vec::new();
        let mut voice = false;
        if let Some((lat, lon)) = m.location {
            lines.push(format!("[location: {lat}, {lon}]"));
        }
        if let Some(c) = m
            .contact
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
        {
            lines.push(format!("[contact: {c}]"));
        }
        if m.media.is_empty() {
            return (lines, paths, voice);
        }
        let dir = self.rt.data_dir.join("telegram").join("inbox");
        if let Err(e) = tokio::fs::create_dir_all(&dir).await {
            tracing::warn!(error = %e, "failed to create telegram inbox dir");
            return (lines, paths, voice);
        }
        for (n, a) in m.media.iter().enumerate() {
            if a.kind == MediaKind::Sticker {
                lines.push(format!(
                    "[sticker: {}]",
                    a.emoji.as_deref().unwrap_or("sticker")
                ));
                continue;
            }
            let label = a
                .file_name
                .clone()
                .unwrap_or_else(|| kind_name(a.kind).to_owned());
            let (bytes, remote) = match self.api.download(&a.file_id, MAX_DOWNLOAD).await {
                Ok(d) => d,
                Err(ApiError::TooBig) => {
                    lines.push(format!(
                        "[attachment: {label} — too large for the Bot API (20 MB); not downloaded]"
                    ));
                    continue;
                }
                Err(e) if e.is(FILE_TOO_BIG) => {
                    lines.push(format!(
                        "[attachment: {label} — too large for the Bot API (20 MB); not downloaded]"
                    ));
                    continue;
                }
                Err(e) => {
                    lines.push(format!("[attachment: {label} — download failed: {e}]"));
                    continue;
                }
            };
            let hint = a.file_name.as_deref().or(remote.as_deref()).unwrap_or("");
            let mime = effective_mime(a.mime.as_deref().unwrap_or(""), &bytes, hint);
            let name = file_name(a, remote.as_deref(), &mime);
            let ts = if n == 0 {
                format!("{}.{}", m.date, m.message_id)
            } else {
                format!("{}.{}.{n}", m.date, m.message_id)
            };
            let path = dir.join(format!("{ts}-{name}"));
            if let Err(e) = tokio::fs::write(&path, &bytes).await {
                tracing::warn!(error = %e, path = %path.display(), "failed to save attachment");
                lines.push(format!("[attachment: {label} — save failed: {e}]"));
                continue;
            }
            let mut detail = format!("{mime}, {}", fmt_size(bytes.len()));
            let spoken = matches!(
                a.kind,
                MediaKind::Voice | MediaKind::VideoNote | MediaKind::Audio
            );
            if spoken {
                detail.push_str(", voice note");
                if let Some(ms) = a.duration_ms {
                    detail.push_str(&format!(", {} s", ms.div_ceil(1000)));
                }
            }
            let mut suffix = "";
            if spoken {
                match transcribe(&path).await {
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
            if mime.starts_with("image/") || mime == "application/pdf" {
                paths.push(path);
            }
        }
        (lines, paths, voice)
    }

    /// One ephemeral `not allowed` for a non-allowed member who addressed the
    /// bot (98 §5); when the API refuses, drop silently and log once per chat.
    async fn not_allowed(&self, m: &IncomingMessage, user: u64) {
        let to = Target {
            chat: m.chat.id,
            thread: m.thread,
        };
        let eph = Ephemeral {
            user,
            callback_query_id: None,
        };
        if let Err(e) = self
            .api
            .send_rich(&to, "not allowed", None, Some(m.message_id), Some(&eph))
            .await
            && self.lock().ephemeral_failed.insert(m.chat.id)
        {
            tracing::debug!(chat = m.chat.id, error = %e, "ephemeral `not allowed` refused; dropping silently from now on");
        }
    }

    // -- pairing (98 §10.4) --------------------------------------------------

    /// `/start <live code>`: owner live, `SetOwner` to the actor for the
    /// config write, `Paired`, welcome + help, the owner command scope.
    async fn pair(&self, m: &IncomingMessage, from: &SenderRef) {
        self.set_owner(from.id);
        match crate::telegram_actor() {
            Some(actor) => {
                if let Err(e) = actor
                    .ask(crate::SetOwner(from.id))
                    .await
                    .map_err(|e| e.to_string())
                {
                    tracing::warn!(error = %e, "telegram owner not written to config");
                }
            }
            None => tracing::warn!("telegram actor not running; owner set live only"),
        }
        tracing::info!(user = from.id, name = %from.name, "telegram paired");
        self.publish(TelegramEvent::Paired {
            user: from.id,
            name: from.name.clone(),
        })
        .await;
        let help = commands::help();
        let md = format!(
            "👋 paired: **{}** is the owner now.\n\n{}",
            from.name, help.markdown
        );
        let to = Target {
            chat: m.chat.id,
            thread: m.thread,
        };
        if let Err(e) = self
            .send(&to, &md, help.keyboard.as_ref(), None, None)
            .await
        {
            tracing::warn!(error = %e, "telegram pairing welcome failed");
        }
        self.set_command_scopes(Some(from.id)).await;
    }

    // -- commands (98 §10.3) -------------------------------------------------

    /// The session a command runs against: the thread's, else the invoker's
    /// latest in the chat (98 §10.3).
    fn command_cx(
        &self,
        user: u64,
        chat: i64,
        thread: Option<i64>,
        in_group: bool,
    ) -> CommandCx<'_> {
        let session = {
            let s = self.lock();
            s.threads
                .get(&threads::key(chat, thread))
                .map(|t| t.session)
                .or_else(|| s.threads.latest_in_chat(chat, user).map(|(_, t)| t.session))
        };
        CommandCx {
            user,
            owner: *self.owner.lock(),
            chat,
            thread,
            in_group,
            session,
            status: self.status(),
            port: self.port.as_ref(),
            page: 0,
        }
    }

    async fn command(self: &Arc<Self>, m: &IncomingMessage, from: &SenderRef, in_group: bool) {
        let Some(cmd) = &m.command else { return };
        let out = {
            let cx = self.command_cx(from.id, m.chat.id, m.thread, in_group);
            commands::handle(cx, &cmd.name, &cmd.arg).await
        };
        let to = Target {
            chat: m.chat.id,
            thread: m.thread,
        };
        self.apply_outcome(out, &to, from, None).await;
    }

    /// Side effects of a command outcome and its reply; `edit` = the panel a
    /// `cmd:` press re-renders in place.
    async fn apply_outcome(
        self: &Arc<Self>,
        out: Outcome,
        to: &Target,
        from: &SenderRef,
        edit: Option<MsgRef>,
    ) {
        let in_group = to.chat < 0;
        let user = from.id;
        match out {
            Outcome::Reply(r) | Outcome::Cancelled { reply: r, .. } => {
                self.reply(to, user, in_group, &r, edit).await;
            }
            Outcome::NewThread(r) => {
                self.unbind_key(&threads::key(to.chat, to.thread));
                self.reply(to, user, in_group, &r, edit).await;
            }
            Outcome::Resume {
                session,
                title,
                reply,
            } => {
                let (key, at) = if self.topics_shape(to.chat) && to.thread.is_none() {
                    match self
                        .api
                        .create_topic(to.chat, &clip(title.trim(), TOPIC_NAME_MAX))
                        .await
                    {
                        Ok(t) => {
                            let key = threads::key(to.chat, Some(t));
                            self.rebind(
                                &key,
                                Thread::new(
                                    session,
                                    user,
                                    Some(Topic {
                                        id: t,
                                        named_by_user: false,
                                    }),
                                ),
                            );
                            (
                                key,
                                Target {
                                    chat: to.chat,
                                    thread: Some(t),
                                },
                            )
                        }
                        Err(e) => {
                            tracing::warn!(chat = to.chat, error = %e, "createForumTopic for /resume failed; binding the chat");
                            (threads::key(to.chat, None), *to)
                        }
                    }
                } else {
                    (threads::key(to.chat, to.thread), *to)
                };
                if self
                    .lock()
                    .threads
                    .get(&key)
                    .is_none_or(|t| t.session != session)
                {
                    let topic = {
                        let s = self.lock();
                        s.threads.get(&key).and_then(|t| t.topic).or_else(|| {
                            at.thread.map(|id| Topic {
                                id,
                                named_by_user: true,
                            })
                        })
                    };
                    self.rebind(&key, Thread::new(session, user, topic));
                }
                if let Some(b) = self.bound_key(&key) {
                    let _ = self.worker(&b);
                }
                self.reply(&at, user, in_group, &reply, edit.filter(|_| at == *to))
                    .await;
            }
            Outcome::Allow(_)
            | Outcome::RemoveAllowed(_)
            | Outcome::AllowGroup(_)
            | Outcome::RemoveGroup(_) => {
                let md = self.admin(out).await;
                let r = Reply {
                    markdown: md,
                    keyboard: None,
                };
                self.reply(to, user, in_group, &r, edit).await;
            }
            Outcome::Link => {
                let md = match self.mint_link().await {
                    Ok(link) => format!(
                        "open this from the owner's Telegram:\n{}\n```\n{}\n```\n(expires in 10 min)",
                        link.url,
                        link::qr(&link.url)
                    ),
                    Err(e) => format!("⚠️ {e}"),
                };
                let r = Reply {
                    markdown: md,
                    keyboard: None,
                };
                self.reply(to, user, in_group, &r, None).await;
            }
            Outcome::Prompt { reply, expect } => {
                match self
                    .send(to, &reply.markdown, reply.keyboard.as_ref(), None, None)
                    .await
                {
                    Ok(id) => {
                        self.lock()
                            .prompts
                            .insert(MsgRef { chat: to.chat, id }, expect);
                    }
                    Err(e) => tracing::warn!(error = %e, "telegram prompt failed"),
                }
            }
        }
    }

    /// `Allow` / `RemoveAllowed` / `AllowGroup` / `RemoveGroup` through the
    /// `"telegram"` actor, which owns the config write (98 §10.3).
    async fn admin(&self, out: Outcome) -> String {
        let Some(actor) = crate::telegram_actor() else {
            return "⚠️ telegram actor is not running".to_owned();
        };
        let (r, ok) = match out {
            Outcome::Allow(id) => (
                actor.ask(crate::Allow(id)).await.map_err(|e| e.to_string()),
                format!("allowed {id}"),
            ),
            Outcome::RemoveAllowed(id) => (
                actor
                    .ask(crate::RemoveAllowed(id))
                    .await
                    .map_err(|e| e.to_string()),
                format!("removed {id}"),
            ),
            Outcome::AllowGroup(id) => (
                actor
                    .ask(crate::AllowGroup(id))
                    .await
                    .map_err(|e| e.to_string()),
                format!("group {id} added"),
            ),
            Outcome::RemoveGroup(id) => (
                actor
                    .ask(crate::RemoveGroup(id))
                    .await
                    .map_err(|e| e.to_string()),
                format!("group {id} removed"),
            ),
            _ => return String::new(),
        };
        match r {
            Ok(()) => ok,
            Err(e) => format!("⚠️ {e}"),
        }
    }

    /// Sends `r`: edits `edit` in place when given, else ephemeral to `user`
    /// in groups (falling back to a normal send), else a normal send.
    async fn reply(&self, to: &Target, user: u64, in_group: bool, r: &Reply, edit: Option<MsgRef>) {
        let kb = r.keyboard.as_ref();
        if let Some(at) = edit {
            match self.api.edit_rich(&at, &r.markdown, kb).await {
                Ok(()) => return,
                Err(e) if is_not_modified(&e) => return,
                Err(e) => tracing::debug!(error = %e, "panel edit failed; sending a new message"),
            }
        }
        if in_group {
            let eph = Ephemeral {
                user,
                callback_query_id: None,
            };
            if self
                .send(to, &r.markdown, kb, None, Some(&eph))
                .await
                .is_ok()
            {
                return;
            }
        }
        if let Err(e) = self.send(to, &r.markdown, kb, None, None).await {
            tracing::warn!(chat = to.chat, error = %e, "telegram command reply failed");
        }
    }

    // -- callbacks (98 §7) ---------------------------------------------------

    /// `answerCallbackQuery` is the first call on every press; the prefix
    /// routes `ap:` / `ask:` / `cmd:` / `out:`.
    async fn handle_callback(self: &Arc<Self>, cb: IncomingCallback) {
        if cb.from.is_bot {
            return self.answer(&cb.id, None).await;
        }
        let data = cb.data.as_str();
        if let Some((id, decision)) = approvals::parse(data) {
            self.press_approval(&cb, id, decision).await;
        } else if let Some(p) = asks::parse(data) {
            self.press_ask(&cb, p).await;
        } else if let Some(rest) = data.strip_prefix("cmd:") {
            self.press_cmd(&cb, rest).await;
        } else if let Some(call_id) = data.strip_prefix("out:") {
            self.press_out(&cb, call_id).await;
        } else {
            tracing::debug!(data, "telegram callback with unknown data");
            self.answer(&cb.id, None).await;
        }
    }

    /// Owner or the card's thread user may answer (98 §10.1).
    async fn press_approval(&self, cb: &IncomingCallback, id: ApprovalId, decision: Decision) {
        let user = cb.from.id;
        let permitted = *self.owner.lock() == Some(user)
            || self
                .lock()
                .cards
                .get(&id)
                .is_some_and(|c| c.thread_user == Some(user));
        if !permitted {
            tracing::warn!(
                user,
                "telegram approval press from a user who may not answer it"
            );
            return self.answer(&cb.id, Some("not allowed")).await;
        }
        match self.port.resolve(id, decision).await {
            Ok(true) => {
                if let Some(c) = self.lock().cards.get_mut(&id) {
                    c.by = Some(cb.from.name.clone());
                }
                let toast = match decision {
                    Decision::Allow => "allowed",
                    Decision::Deny => "denied",
                };
                self.answer(&cb.id, Some(toast)).await;
            }
            Ok(false) => self.answer(&cb.id, Some("already resolved")).await,
            Err(e) => {
                tracing::warn!(error = %e, "resolve failed");
                self.answer(&cb.id, Some(&format!("⚠️ {e}"))).await;
            }
        }
    }

    /// A question-card press: the card is finalised at once; the next
    /// question is posted or the answers handed to the agent (98 §10.2).
    async fn press_ask(self: &Arc<Self>, cb: &IncomingCallback, p: asks::Press) {
        let user = cb.from.id;
        let step = {
            let owner = *self.owner.lock();
            let mut s = self.lock();
            let card = s.asks.get_mut(&p.id);
            let permitted =
                owner == Some(user) || card.as_deref().is_some_and(|c| c.thread_user == Some(user));
            if !permitted {
                Err("not allowed")
            } else {
                match card {
                    None => Err("already answered"),
                    Some(c) => c
                        .step(p.question, &p.kind)
                        .map(|(text, next)| (text, next, c.to)),
                }
            }
        };
        let (text, next, to) = match step {
            Ok(step) => step,
            Err(msg) => {
                tracing::debug!(user, msg, "telegram ask press refused");
                return self.answer(&cb.id, Some(msg)).await;
            }
        };
        self.answer(&cb.id, None).await;
        let at = MsgRef {
            chat: cb.chat,
            id: cb.message_id,
        };
        self.edit(at, &text, None).await;
        self.ask_next(p.id, next, &to).await;
    }

    /// A reply to the live card of `id` is `Answer::Text`.
    async fn ask_text(self: &Arc<Self>, id: AskId, m: &IncomingMessage, from: &SenderRef) {
        let text = m.text.clone().unwrap_or_default();
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let step = {
            let owner = *self.owner.lock();
            let mut s = self.lock();
            let Some(c) = s.asks.get_mut(&id) else { return };
            if owner != Some(from.id) && c.thread_user != Some(from.id) {
                tracing::debug!(
                    user = from.id,
                    "telegram ask reply from a user who may not answer it"
                );
                return;
            }
            let at = c.at;
            c.step(Some(c.current), &asks::Kind::Text(text.to_owned()))
                .map(|(t, next)| (t, next, c.to, at))
        };
        let Ok((card_text, next, to, at)) = step else {
            return;
        };
        if let Some(at) = at {
            self.edit(at, &card_text, None).await;
        }
        self.react(
            MsgRef {
                chat: m.chat.id,
                id: m.message_id,
            },
            EYES,
        )
        .await;
        self.ask_next(id, next, &to).await;
    }

    async fn ask_next(self: &Arc<Self>, id: AskId, next: asks::Next, to: &Target) {
        let answers = match next {
            asks::Next::Post(i) => return self.post_question(id, i).await,
            asks::Next::Answer(a) => Some(a),
            asks::Next::Dismiss => None,
        };
        match self.port.answer_ask(id, answers).await {
            Ok(true) => {}
            Ok(false) => tracing::debug!(id = %id.0, "ask already answered elsewhere"),
            Err(e) => {
                tracing::warn!(error = %e, "answer_ask failed");
                self.note(to, &format!("⚠️ {e}")).await;
            }
        }
    }

    /// Posts question `i` of `id` and records where it is.
    async fn post_question(&self, id: AskId, i: usize) {
        let card = self
            .lock()
            .asks
            .get(&id)
            .map(|c| (c.req.clone(), c.to, c.context.clone()));
        let Some((req, to, context)) = card else {
            return;
        };
        let (text, kb) = asks::question(&req, i, context.as_deref());
        match self.send(&to, &text, Some(&kb), None, None).await {
            Ok(mid) => {
                if let Some(c) = self.lock().asks.get_mut(&id) {
                    c.at = Some(MsgRef {
                        chat: to.chat,
                        id: mid,
                    });
                }
            }
            Err(e) => tracing::warn!(error = %e, "ask card post failed"),
        }
    }

    /// `cmd:` presses re-render the panel in place (98 §10.3).
    async fn press_cmd(self: &Arc<Self>, cb: &IncomingCallback, rest: &str) {
        let user = cb.from.id;
        let b = self.bound_key(&threads::key(cb.chat, cb.thread));
        if !(self.is_allowed(user) || b.as_ref().is_some_and(|b| b.user == user)) {
            return self.answer(&cb.id, Some("not allowed")).await;
        }
        self.answer(&cb.id, None).await;
        let out = {
            let cx = self.command_cx(user, cb.chat, cb.thread, cb.chat < 0);
            ui::action(cx, rest).await
        };
        let Some(out) = out else { return };
        let to = Target {
            chat: cb.chat,
            thread: cb.thread,
        };
        let at = MsgRef {
            chat: cb.chat,
            id: cb.message_id,
        };
        self.apply_outcome(out, &to, &cb.from, Some(at)).await;
    }

    /// `out:<call_id>`: the full tool output as a text file (98 §8.4).
    async fn press_out(self: &Arc<Self>, cb: &IncomingCallback, call_id: &str) {
        let user = cb.from.id;
        let b = self.bound_key(&threads::key(cb.chat, cb.thread));
        if !(self.is_allowed(user) || b.as_ref().is_some_and(|b| b.user == user)) {
            return self.answer(&cb.id, Some("not allowed")).await;
        }
        let Some(b) = b else {
            return self.answer(&cb.id, Some("output expired")).await;
        };
        let (tx, rx) = oneshot::channel();
        let _ = self.worker(&b).send(WorkerMsg::Output {
            call_id: call_id.to_owned(),
            reply: tx,
        });
        let Ok(Some(text)) = rx.await else {
            return self.answer(&cb.id, Some("output expired")).await;
        };
        self.answer(&cb.id, None).await;
        let file = OutFile {
            name: format!("{call_id}-output.txt"),
            mime: "text/plain".into(),
            bytes: text.into_bytes(),
        };
        if let Err(e) = self
            .send_file(&b.to, &file, None, Some(cb.message_id))
            .await
        {
            tracing::warn!(error = %e, "tool output file failed");
        }
    }

    async fn edit(&self, at: MsgRef, md: &str, kb: Option<&Keyboard>) {
        match self.api.edit_rich(&at, md, kb).await {
            Ok(()) => {}
            Err(e) if is_not_modified(&e) => {}
            Err(e) => {
                tracing::debug!(chat = at.chat, message_id = at.id, error = %e, "telegram card edit failed")
            }
        }
    }

    // -- events (98 §8–§10) --------------------------------------------------

    async fn handle_event(self: &Arc<Self>, ev: Event) {
        match ev {
            Event::Lease(lease) => {
                self.lock().follower.observe(&lease);
                self.sync_poller().await;
            }
            Event::Agent(AgentEvent::SubagentSpawned {
                parent,
                child,
                name,
                ..
            }) => self.spawned(parent, child, name).await,
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
                if let AgentEvent::TurnStarted { text, .. } = &ev {
                    self.child_task(session, text).await;
                }
                self.child_row(session, |c| c.apply(&ev)).await;
                if let Some(b) = self.bound(session) {
                    let _ = self.worker(&b).send(WorkerMsg::Agent(ev));
                }
            }
            Event::Requested(req) => {
                self.child_row(req.session, |c| c.approval(Some(&req.tool)))
                    .await;
                let Some((to, thread_user, context)) = self.card_target(req.session).await else {
                    return;
                };
                let text = approvals::card(&req, context.as_deref());
                let kb = approvals::keyboard(req.id);
                self.lock().cards.insert(
                    req.id,
                    approvals::Card {
                        session: req.session,
                        thread_user,
                        at: None,
                        body: approvals::body(&req),
                        by: None,
                    },
                );
                match self.send(&to, &text, Some(&kb), None, None).await {
                    Ok(mid) => {
                        if let Some(c) = self.lock().cards.get_mut(&req.id) {
                            c.at = Some(MsgRef {
                                chat: to.chat,
                                id: mid,
                            });
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "approval card post failed"),
                }
            }
            Event::Resolved(res) => {
                let card = self.lock().cards.remove(&res.id);
                let Some(card) = card else { return };
                self.child_row(card.session, |c| c.approval(None)).await;
                if let Some(at) = card.at {
                    let (text, kb) = approvals::resolved(
                        &card.body,
                        res.decision,
                        res.timed_out,
                        card.by.as_deref(),
                    );
                    self.edit(at, &text, Some(&kb)).await;
                }
            }
            Event::Ask(req) => {
                let Some((to, thread_user, context)) = self.card_target(req.session).await else {
                    return;
                };
                let id = req.id;
                self.lock()
                    .asks
                    .insert(id, asks::Card::new(req, thread_user, to, context));
                self.post_question(id, 0).await;
            }
            Event::AskDone(res) => {
                let card = self.lock().asks.remove(&res.id);
                let Some(card) = card else { return };
                if card.done {
                    return;
                }
                let Some(at) = card.at else { return };
                let text = asks::closed(&card.req, card.current, res.dismissed, res.timed_out);
                self.edit(at, &text, None).await;
            }
            Event::Artifact(m) => self.artifact(m).await,
            // 37 §7: the model asked the user for a secret; say where to enter
            // it (never here). The owner's chat only.
            Event::Secret(secrets::SecretsEvent::Requested { name, note, .. }) => {
                let Some(owner) = *self.owner.lock() else {
                    return;
                };
                let to = Target {
                    chat: i64::try_from(owner).unwrap_or(0),
                    thread: None,
                };
                self.note(&to, &secret_notice(&name, &note)).await;
            }
            Event::Secret(_) => {}
        }
    }

    /// Where a card for `session` goes: its thread (owner or the thread's
    /// user may press), else the owner's chat with a context line when
    /// `approvals: all`, else nowhere (98 §10.1).
    async fn card_target(
        &self,
        session: SessionId,
    ) -> Option<(Target, Option<u64>, Option<String>)> {
        if let Some(b) = self.bound(session) {
            return Some((b.to, Some(b.user), None));
        }
        if self.cfg.approvals != ApprovalsMode::All {
            return None;
        }
        let owner = (*self.owner.lock())?;
        let context = self.session_context(session).await;
        Some((
            Target {
                chat: i64::try_from(owner).ok()?,
                thread: None,
            },
            None,
            Some(context),
        ))
    }

    /// `session <last 8> · <title>` for owner-chat cards; `Unknown` when the
    /// title ask fails.
    async fn session_context(&self, session: SessionId) -> String {
        let title = match self.port.session_title(session).await {
            Ok(Some(t)) if !t.trim().is_empty() => t,
            Ok(_) => "Unknown".to_owned(),
            Err(e) => {
                tracing::debug!(error = %e, "session_title for the card context failed");
                "Unknown".to_owned()
            }
        };
        let id = session.to_string();
        format!("session {} · {title}", &id[id.len().saturating_sub(8)..])
    }

    /// 98 §9: a child of a bound parent gets its own topic (topics shape) or
    /// renders inline behind a `↳ spawned` marker (flat shape).
    async fn spawned(self: &Arc<Self>, parent: SessionId, child: SessionId, name: String) {
        let Some(pb) = self.bound(parent) else {
            return;
        };
        {
            let mut s = self.lock();
            s.parent_of.insert(child, (parent, name.clone()));
            s.children.insert(child, ChildState::new(name.clone()));
        }
        let mut flat = true;
        if self.topics_shape(pb.to.chat) {
            match self
                .api
                .create_topic(pb.to.chat, &format!("↳ {name}"))
                .await
            {
                Ok(t) => {
                    let key = threads::key(pb.to.chat, Some(t));
                    self.rebind(
                        &key,
                        Thread::new(
                            child,
                            pb.user,
                            Some(Topic {
                                id: t,
                                named_by_user: true,
                            }),
                        ),
                    );
                    flat = false;
                }
                Err(e) => {
                    tracing::warn!(error = %e, %name, "child topic failed; rendering inline");
                }
            }
        }
        if flat {
            self.lock().flat_children.insert(
                child,
                FlatChild {
                    to: pb.to,
                    user: pb.user,
                },
            );
        }
        match self
            .send(&pb.to, &format!("↳ spawned **{name}**"), None, None, None)
            .await
        {
            Ok(mid) if flat => {
                self.lock().markers.insert(
                    MsgRef {
                        chat: pb.to.chat,
                        id: mid,
                    },
                    child,
                );
            }
            Ok(_) => {}
            Err(e) => tracing::debug!(error = %e, "spawned line failed"),
        }
        if let Some(cb) = self.bound(child) {
            let _ = self.worker(&cb);
        }
        self.child_row(child, |_| true).await;
    }

    /// 98 §9.1: the child's first `TurnStarted.text` goes into its topic as the task.
    async fn child_task(&self, session: SessionId, text: &str) {
        let post = {
            let mut s = self.lock();
            s.parent_of.contains_key(&session)
                && s.threads.by_session(session).is_some()
                && s.task_posted.insert(session)
        };
        if !post {
            return;
        }
        let Some(b) = self.bound(session) else { return };
        self.note(
            &b.to,
            &format!("▸ **task**\n{}", clip(text.trim(), TASK_MAX)),
        )
        .await;
    }

    /// Applies `f` to `session`'s row and, when it changed, fans the row out
    /// to every bound ancestor's worker (98 §9.3).
    async fn child_row(
        self: &Arc<Self>,
        session: SessionId,
        f: impl FnOnce(&mut ChildState) -> bool,
    ) {
        let row = {
            let mut s = self.lock();
            let Some(c) = s.children.get_mut(&session) else {
                return;
            };
            if !f(c) {
                return;
            }
            c.row()
        };
        let (row, idle) = row;
        for a in self.ancestors(session) {
            let _ = self.worker(&a).send(WorkerMsg::ChildRow {
                child: session,
                row: row.clone(),
                idle,
            });
        }
    }

    /// 98 §8.10: the session's thread, else the nearest bound ancestor's; the
    /// file by kind, captioned with the title (apps with the 46 §8.2 note).
    async fn artifact(&self, m: ArtifactShown) {
        let chain = match self.port.ancestors(m.session).await {
            Ok(chain) => chain,
            Err(e) => {
                tracing::debug!(error = %e, "ancestors for the artifact failed; using the session alone");
                vec![m.session]
            }
        };
        let Some(b) = chain.iter().find_map(|id| self.bound(*id)) else {
            tracing::debug!(session = %m.session, name = %m.artifact.name, "artifact for an unbound session; not sent");
            return;
        };
        let a = m.artifact;
        if a.bytes > ARTIFACT_MAX {
            self.note(
                &b.to,
                &format!(
                    "artifact too large for Telegram (50 MB): {}",
                    a.path.display()
                ),
            )
            .await;
            return;
        }
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
        let caption = caption.map(|c| clip(&c, CAPTION_MAX));
        let file = OutFile {
            name: a.name.clone(),
            mime: a.mime.clone(),
            bytes,
        };
        if let Err(e) = self.send_file(&b.to, &file, caption.as_deref(), None).await {
            tracing::warn!(error = %e, name = %a.name, "artifact send failed");
        }
    }

    /// Worker → channel notes (98 contract §2.6).
    pub(crate) async fn handle_note(&self, n: WorkerNote) {
        match n {
            WorkerNote::OpenMsg { msg, open } => {
                let mut s = self.lock();
                if open {
                    s.threads.open_add(msg);
                } else {
                    s.threads.open_remove(&msg);
                }
            }
            WorkerNote::Renamed { key, title } => self.lock().threads.set_title(&key, title),
            WorkerNote::Fallback { session, reason } => {
                self.publish(TelegramEvent::StreamFallback { session, reason })
                    .await;
            }
            WorkerNote::Unreachable { chat, reason } => {
                tracing::warn!(chat, %reason, "telegram chat unreachable (worker); unbinding its threads");
                self.unbind_chat(chat);
                self.publish(TelegramEvent::Unreachable { chat, reason })
                    .await;
            }
        }
    }
}

/// Removes `key`'s worker, telling it to finish.
fn shutdown_worker(s: &mut State, key: &str) {
    if let Some(tx) = s.workers.remove(key) {
        let _ = tx.send(WorkerMsg::Shutdown);
    }
}

/// `Transcribe` on the `voice` plugin's actor; `Err` when it is not running (45 §4.5.1).
async fn transcribe(path: &Path) -> Result<voice::Transcript, String> {
    let stt = ActorRef::<voice::Transcriber>::lookup(voice::TRANSCRIBER_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(|| "voice plugin is not running".to_owned())?;
    stt.ask(voice::Transcribe {
        path: path.to_path_buf(),
    })
    .await
    .map_err(|e| e.to_string())
}

/// The first line of `input`, ≤ [`TOPIC_NAME_MAX`] chars, for `createForumTopic`.
fn topic_name(input: &str) -> String {
    let line = input
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("new session");
    clip(line, TOPIC_NAME_MAX)
}

/// Removes `@username` (case-insensitive) and trims.
fn strip_mention(text: &str, username: &str) -> String {
    if username.is_empty() {
        return text.trim().to_owned();
    }
    let needle = format!("@{}", username.to_ascii_lowercase());
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(pos) = lower[i..].find(&needle) {
        out.push_str(&text[i..i + pos]);
        i += pos + needle.len();
    }
    out.push_str(&text[i..]);
    out.trim().to_owned()
}

/// `s` cut to `max` chars, the last one an ellipsis when cut.
fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let end = s
        .char_indices()
        .nth(max.saturating_sub(1))
        .map_or(s.len(), |(i, _)| i);
    format!("{}…", &s[..end])
}

/// `s` cut to `max` chars with an `… (n more chars)` tail when it was longer.
pub(crate) fn clip(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_owned();
    }
    let tail = format!("… ({} more chars)", n - max);
    let keep = max.saturating_sub(tail.chars().count());
    let end = s.char_indices().nth(keep).map_or(s.len(), |(i, _)| i);
    format!("{}{tail}", &s[..end])
}

/// The one argument worth a board column: `shell`'s command, else the first
/// string argument; first line only.
fn first_arg<'a>(name: &str, args: &'a serde_json::Value) -> &'a str {
    let s = if name == "shell" {
        args.get("command").and_then(|c| c.as_str())
    } else {
        args.as_object()
            .and_then(|o| o.values().find_map(|v| v.as_str()))
    };
    s.and_then(|s| s.lines().next()).unwrap_or_default().trim()
}

fn kind_name(k: MediaKind) -> &'static str {
    match k {
        MediaKind::Photo => "photo",
        MediaKind::Document => "document",
        MediaKind::Voice => "voice",
        MediaKind::VideoNote => "video_note",
        MediaKind::Video => "video",
        MediaKind::Audio => "audio",
        MediaKind::Animation => "animation",
        MediaKind::Sticker => "sticker",
    }
}

/// The inbox file name: Telegram's `file_name`, else `<kind>.<ext>` from the
/// remote path's extension or the MIME; slashes replaced.
fn file_name(a: &Media, remote: Option<&str>, mime: &str) -> String {
    let name = match &a.file_name {
        Some(n) if !n.trim().is_empty() => n.trim().to_owned(),
        _ => {
            let ext = remote
                .and_then(|r| Path::new(r).extension())
                .and_then(|e| e.to_str())
                .filter(|e| !e.is_empty())
                .map_or_else(|| extension_for(mime), str::to_ascii_lowercase);
            format!("{}.{ext}", kind_name(a.kind))
        }
    };
    name.chars()
        .map(|c| if c == '/' || c == '\\' { '_' } else { c })
        .collect()
}

/// The declared MIME unless it is generic, then the magic-byte guess, then
/// the extension of `hint` (09 §7.4).
fn effective_mime(declared: &str, bytes: &[u8], hint: &str) -> String {
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
    if let Some(m) = sniff(bytes) {
        return m.to_owned();
    }
    if let Some(m) = mime_guess::from_path(hint).first_raw() {
        return m.to_owned();
    }
    if base.is_empty() {
        "application/octet-stream".to_owned()
    } else {
        base
    }
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
    use std::sync::atomic::{AtomicI64, Ordering};

    use agent::{AgentInfo, Loaded, SessionSummary};
    use async_trait::async_trait;
    use runtime::TurnId;
    use tools::{Answer, AskId, Choice, Question, Risk};

    use super::*;
    use crate::api::mock::{Call, MockSender};
    use crate::api::{BOT_BLOCKED, Button, CommandScope};
    use crate::poll::{Command, ReplyRef};

    pub(crate) const OWNER: u64 = 1001;
    const FRIEND: u64 = 1002;
    const STRANGER: u64 = 1003;
    const GROUP: i64 = -100_200;
    const OTHER_GROUP: i64 = -100_999;
    const BOT: &str = "theuth_bot";

    static NEXT_ID: AtomicI64 = AtomicI64::new(1);

    #[derive(Debug, PartialEq)]
    pub(crate) enum PortCall {
        SendInput(Option<SessionId>, String, usize),
        Steer(SessionId, String),
        Resolve(ApprovalId, Decision),
        AnswerAsk(AskId, Option<Vec<Answer>>),
    }

    pub(crate) struct Stub {
        pub(crate) session: SessionId,
        calls: Mutex<Vec<PortCall>>,
        parents: Mutex<HashMap<SessionId, SessionId>>,
    }

    impl Stub {
        pub(crate) fn calls(&self) -> Vec<PortCall> {
            std::mem::take(&mut *self.calls.lock())
        }
    }

    #[async_trait]
    impl Port for Stub {
        async fn send_input(
            &self,
            session: Option<SessionId>,
            text: String,
            files: Vec<PathBuf>,
            _: PeerScope,
        ) -> Result<SessionId, String> {
            self.calls
                .lock()
                .push(PortCall::SendInput(session, text, files.len()));
            Ok(session.unwrap_or(self.session))
        }
        async fn new_root(&self, _: PeerScope) -> Result<SessionId, String> {
            Ok(self.session)
        }
        async fn steer(
            &self,
            session: SessionId,
            text: String,
            _: Vec<PathBuf>,
        ) -> Result<(), String> {
            self.calls.lock().push(PortCall::Steer(session, text));
            Ok(())
        }
        async fn cancel(&self, _: SessionId) -> Result<(), String> {
            Ok(())
        }
        async fn list_sessions(&self) -> Result<Vec<SessionSummary>, String> {
            Ok(Vec::new())
        }
        async fn load_session(&self, _: SessionId) -> Result<Loaded, String> {
            Err("stub".into())
        }
        async fn resolve(&self, id: ApprovalId, decision: Decision) -> Result<bool, String> {
            self.calls.lock().push(PortCall::Resolve(id, decision));
            Ok(true)
        }
        async fn answer_ask(
            &self,
            id: AskId,
            answers: Option<Vec<Answer>>,
        ) -> Result<bool, String> {
            self.calls.lock().push(PortCall::AnswerAsk(id, answers));
            Ok(true)
        }
        async fn list_agents(&self, _: SessionId) -> Result<Vec<AgentInfo>, String> {
            Ok(Vec::new())
        }
        async fn ancestors(&self, session: SessionId) -> Result<Vec<SessionId>, String> {
            let parents = self.parents.lock();
            let mut out = vec![session];
            let mut cur = session;
            while let Some(p) = parents.get(&cur) {
                out.push(*p);
                cur = *p;
            }
            Ok(out)
        }
        async fn session_title(&self, _: SessionId) -> Result<Option<String>, String> {
            Ok(Some("Title".into()))
        }
    }

    pub(crate) struct Fixture {
        pub(crate) api: Arc<MockSender>,
        pub(crate) stub: Arc<Stub>,
        pub(crate) chan: Arc<Channel>,
        pub(crate) rt: Runtime,
        _dir: tempfile::TempDir,
    }

    pub(crate) fn fixture(topics: bool) -> Fixture {
        fixture_with(TelegramConfig::default(), topics)
    }

    fn fixture_with(cfg: TelegramConfig, topics: bool) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let api = MockSender::new();
        let stub = Arc::new(Stub {
            session: SessionId::new(),
            calls: Mutex::new(Vec::new()),
            parents: Mutex::new(HashMap::new()),
        });
        let cfg = TelegramConfig {
            owner: Some(OWNER),
            allowed: vec![FRIEND],
            groups: vec![GROUP],
            ..cfg
        };
        let (tx, _rx) = mpsc::unbounded_channel();
        let (chan, _notes) = Channel::new(
            Arc::clone(&api) as Arc<dyn Sender>,
            None,
            cfg,
            rt.clone(),
            Arc::clone(&stub) as Arc<dyn Port>,
            tx,
        );
        let chan = Arc::new(chan);
        let _ = chan.identity.set(BotIdentity {
            id: 42,
            username: BOT.into(),
            topics,
            users_may_create_topics: true,
        });
        Fixture {
            api,
            stub,
            chan,
            rt,
            _dir: dir,
        }
    }

    impl Fixture {
        /// Binds `key` to the stub's session for `user`.
        fn bind(&self, key: &str, user: u64) -> SessionId {
            let topic = threads::target(key).and_then(|t| t.thread).map(|id| Topic {
                id,
                named_by_user: false,
            });
            self.chan
                .lock()
                .threads
                .bind(key.to_owned(), Thread::new(self.stub.session, user, topic));
            self.stub.session
        }

        /// Replaces `key`'s worker mailbox with one the test reads.
        fn tap_worker(&self, key: &str) -> mpsc::UnboundedReceiver<WorkerMsg> {
            let (tx, rx) = mpsc::unbounded_channel();
            self.chan.lock().workers.insert(key.to_owned(), tx);
            rx
        }

        fn sends(&self) -> Vec<Call> {
            self.api
                .calls()
                .into_iter()
                .filter(|c| matches!(c, Call::SendRich { .. }))
                .collect()
        }

        async fn inbound(&self, m: IncomingMessage) {
            self.chan
                .handle_inbound(Incoming::Message(Box::new(m)))
                .await;
        }

        async fn press(
            &self,
            user: u64,
            chat: i64,
            thread: Option<i64>,
            message_id: i64,
            data: &str,
        ) {
            self.chan
                .handle_inbound(Incoming::Callback(IncomingCallback {
                    update_id: NEXT_ID.fetch_add(1, Ordering::SeqCst),
                    id: format!("cb{message_id}"),
                    from: sender(user),
                    chat,
                    message_id,
                    thread,
                    data: data.to_owned(),
                }))
                .await;
        }
    }

    fn sender(id: u64) -> SenderRef {
        SenderRef {
            id,
            is_bot: false,
            name: format!("user{id}"),
            username: Some(format!("u{id}")),
        }
    }

    fn private(user: u64) -> ChatRef {
        ChatRef {
            id: i64::try_from(user).unwrap(),
            kind: ChatKind::Private,
            title: None,
        }
    }

    fn group(id: i64) -> ChatRef {
        ChatRef {
            id,
            kind: ChatKind::Supergroup { forum: false },
            title: Some("ops".into()),
        }
    }

    fn msg(chat: ChatRef, from: u64, thread: Option<i64>, text: &str) -> IncomingMessage {
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        let command = text.strip_prefix('/').map(|rest| {
            let (name, arg) = rest.split_once(' ').unwrap_or((rest, ""));
            Command {
                name: name.to_owned(),
                for_other_bot: false,
                arg: arg.trim().to_owned(),
            }
        });
        IncomingMessage {
            update_id: id,
            chat,
            thread,
            message_id: id,
            date: 1_700_000_000,
            from: Some(sender(from)),
            text: Some(text.to_owned()),
            command,
            mentions_bot: text.contains(&format!("@{BOT}")),
            reply_to: None,
            forward_from: None,
            media: Vec::new(),
            media_group_id: None,
            location: None,
            contact: None,
            service: None,
        }
    }

    fn photo(file_id: &str) -> Media {
        Media {
            kind: MediaKind::Photo,
            file_id: file_id.to_owned(),
            file_name: None,
            mime: None,
            size: Some(8),
            duration_ms: None,
            emoji: None,
        }
    }

    fn approval(session: SessionId) -> ApprovalRequested {
        ApprovalRequested {
            id: ApprovalId(uuid::Uuid::new_v4()),
            session,
            tool: "shell".into(),
            args: serde_json::json!({ "command": "ls" }),
            risk: Risk::Mutating,
        }
    }

    fn ask(session: SessionId) -> AskRequested {
        let choice = |label: &str| Choice {
            label: label.into(),
            description: None,
        };
        AskRequested {
            id: AskId(uuid::Uuid::new_v4()),
            session,
            questions: vec![
                Question {
                    id: "db".into(),
                    question: "Which database?".into(),
                    options: vec![choice("Postgres"), choice("SQLite")],
                    recommended: Some(0),
                },
                Question {
                    id: "more".into(),
                    question: "Anything else?".into(),
                    options: vec![],
                    recommended: None,
                },
            ],
        }
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

    fn artifact(
        session: SessionId,
        dir: &Path,
        name: &str,
        kind: artifact::Kind,
        mime: &str,
    ) -> ArtifactShown {
        let path = dir.join(name);
        std::fs::write(&path, b"\x89PNG\r\n\x1a\nbytes").unwrap();
        ArtifactShown {
            session,
            call_id: "call_1".into(),
            artifact: artifact::Artifact {
                id: "abc".into(),
                session,
                call_id: "call_1".into(),
                name: name.into(),
                title: "Report".into(),
                kind,
                mime: mime.into(),
                bytes: 13,
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

    const OWNER_CHAT: i64 = OWNER as i64;

    #[tokio::test]
    async fn general_message_in_a_topics_chat_creates_the_session_then_the_topic_and_binds_it() {
        let f = fixture(true);
        let m = msg(private(OWNER), OWNER, None, "hello world\nsecond line");
        let mark = m.message_id;
        f.inbound(m).await;
        assert_eq!(
            f.stub.calls(),
            vec![PortCall::SendInput(
                None,
                "hello world\nsecond line".into(),
                0
            )]
        );
        let calls = f.api.calls();
        assert!(calls.contains(&Call::React {
            at: MsgRef {
                chat: OWNER_CHAT,
                id: mark
            },
            emoji: Some(EYES.into())
        }));
        assert!(calls.contains(&Call::CreateTopic {
            chat: OWNER_CHAT,
            name: "hello world".into()
        }));
        let key = threads::key(OWNER_CHAT, Some(500));
        let t = f.chan.lock().threads.get(&key).cloned().unwrap();
        assert_eq!(t.session, f.stub.session);
        assert_eq!(
            t.topic,
            Some(Topic {
                id: 500,
                named_by_user: false
            })
        );
        assert!(f.chan.lock().workers.contains_key(&key));
    }

    #[tokio::test]
    async fn message_in_a_bound_topic_steers_while_a_turn_runs() {
        let f = fixture(true);
        let key = threads::key(OWNER_CHAT, Some(7));
        let session = f.bind(&key, OWNER);
        f.chan.lock().running.insert(session);
        f.inbound(msg(private(OWNER), OWNER, Some(7), "more")).await;
        assert_eq!(
            f.stub.calls(),
            vec![PortCall::Steer(session, "more".into())]
        );
        assert!(
            !f.api
                .calls()
                .iter()
                .any(|c| matches!(c, Call::CreateTopic { .. }))
        );
        f.chan.lock().running.remove(&session);
        f.inbound(msg(private(OWNER), OWNER, Some(7), "again"))
            .await;
        assert_eq!(
            f.stub.calls(),
            vec![PortCall::SendInput(Some(session), "again".into(), 0)]
        );
    }

    #[tokio::test]
    async fn flat_private_chat_binds_the_chat_key() {
        let f = fixture(false);
        f.inbound(msg(private(FRIEND), FRIEND, None, "hi")).await;
        assert_eq!(
            f.stub.calls(),
            vec![PortCall::SendInput(None, "hi".into(), 0)]
        );
        assert!(
            !f.api
                .calls()
                .iter()
                .any(|c| matches!(c, Call::CreateTopic { .. }))
        );
        let t = f
            .chan
            .lock()
            .threads
            .get(&threads::key(FRIEND as i64, None))
            .cloned()
            .unwrap();
        assert_eq!((t.session, t.user, t.topic), (f.stub.session, FRIEND, None));
    }

    #[tokio::test]
    async fn plain_group_needs_a_mention_and_strips_it() {
        let f = fixture(false);
        f.inbound(msg(group(GROUP), FRIEND, None, "just chatting"))
            .await;
        assert!(f.stub.calls().is_empty());
        assert!(f.api.calls().is_empty());
        f.inbound(msg(group(GROUP), FRIEND, None, &format!("@{BOT} do it")))
            .await;
        assert_eq!(
            f.stub.calls(),
            vec![PortCall::SendInput(None, "do it".into(), 0)]
        );
        assert!(
            f.chan
                .lock()
                .threads
                .get(&threads::key(GROUP, None))
                .is_some()
        );
        // An unlisted group is ignored even when addressed.
        f.inbound(msg(group(OTHER_GROUP), OWNER, None, &format!("@{BOT} hi")))
            .await;
        assert!(f.stub.calls().is_empty());
    }

    #[tokio::test]
    async fn stranger_dm_is_dropped_silently() {
        let f = fixture(true);
        f.inbound(msg(private(STRANGER), STRANGER, None, "let me in"))
            .await;
        assert!(f.stub.calls().is_empty());
        assert!(f.api.calls().is_empty());
    }

    #[tokio::test]
    async fn non_allowed_group_member_gets_one_ephemeral_attempt_and_nothing_else() {
        let f = fixture(false);
        f.api.fail_next(
            "send_rich",
            ApiError::Api {
                code: 400,
                description: "Bad Request: not enough rights".into(),
            },
        );
        f.inbound(msg(group(GROUP), STRANGER, None, &format!("@{BOT} hi")))
            .await;
        let calls = f.api.take_calls();
        assert_eq!(calls.len(), 1);
        let Call::SendRich { md, ephemeral, .. } = &calls[0] else {
            panic!("{calls:?}")
        };
        assert_eq!(md, "not allowed");
        assert_eq!(ephemeral.as_ref().map(|e| e.user), Some(STRANGER));
        assert!(f.stub.calls().is_empty());
        // Unaddressed chatter from the same member: nothing at all.
        f.inbound(msg(group(GROUP), STRANGER, None, "hmm")).await;
        assert!(f.api.calls().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn an_album_of_three_photos_is_one_input_with_three_lines() {
        let f = fixture(false);
        for id in ["f1", "f2", "f3"] {
            f.api.set_download(
                id,
                b"\x89PNG\r\n\x1a\nxx".to_vec(),
                Some("photos/file_1.jpg"),
            );
        }
        for (i, id) in ["f1", "f2", "f3"].into_iter().enumerate() {
            let mut m = msg(
                private(OWNER),
                OWNER,
                None,
                if i == 0 { "look" } else { "" },
            );
            m.media = vec![photo(id)];
            m.media_group_id = Some("g1".into());
            f.inbound(m).await;
        }
        assert!(f.stub.calls().is_empty());
        tokio::time::sleep(ALBUM_WINDOW + Duration::from_millis(50)).await;
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        let calls = f.stub.calls();
        assert_eq!(calls.len(), 1, "{calls:?}");
        let PortCall::SendInput(None, text, files) = &calls[0] else {
            panic!("{calls:?}")
        };
        assert_eq!(*files, 3);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "look");
        assert_eq!(lines.len(), 4);
        assert!(
            lines[1..]
                .iter()
                .all(|l| l.starts_with("[attachment: ") && l.contains("(image/png, 10 B)]")),
            "{text}"
        );
        assert_eq!(
            f.api
                .calls()
                .iter()
                .filter(|c| matches!(c, Call::Download(_)))
                .count(),
            3
        );
    }

    #[tokio::test]
    async fn subagent_of_a_bound_parent_gets_a_topic_in_the_topics_shape() {
        let f = fixture(true);
        let parent = f.bind(&threads::key(OWNER_CHAT, Some(7)), OWNER);
        let child = SessionId::new();
        f.chan
            .handle_event(Event::Agent(AgentEvent::SubagentSpawned {
                parent,
                child,
                name: "grep-logs".into(),
                cap: None,
            }))
            .await;
        let calls = f.api.calls();
        assert!(calls.contains(&Call::CreateTopic {
            chat: OWNER_CHAT,
            name: "↳ grep-logs".into()
        }));
        assert!(calls.iter().any(|c| matches!(c, Call::SendRich { to, md, .. } if *to == Target { chat: OWNER_CHAT, thread: Some(7) } && md == "↳ spawned **grep-logs**")));
        let (key, t) = f
            .chan
            .lock()
            .threads
            .by_session(child)
            .map(|(k, t)| (k.clone(), t.clone()))
            .unwrap();
        assert_eq!(key, threads::key(OWNER_CHAT, Some(500)));
        assert_eq!(t.user, OWNER);
        // The child's first turn posts its task into the child topic.
        f.chan
            .handle_event(Event::Agent(turn_started(child, "find the slow query")))
            .await;
        assert!(f.api.calls().iter().any(|c| matches!(c, Call::SendRich { to, md, .. } if *to == Target { chat: OWNER_CHAT, thread: Some(500) } && md == "▸ **task**\nfind the slow query")));
    }

    /// Binds the flat parent chat and spawns `grep-logs`; returns the child and its marker id.
    async fn spawn_flat_child(f: &Fixture) -> (SessionId, i64) {
        let parent = f.bind(&threads::key(OWNER_CHAT, None), OWNER);
        let child = SessionId::new();
        f.stub.parents.lock().insert(child, parent);
        f.chan
            .handle_event(Event::Agent(AgentEvent::SubagentSpawned {
                parent,
                child,
                name: "grep-logs".into(),
                cap: None,
            }))
            .await;
        let marker = f
            .chan
            .lock()
            .markers
            .iter()
            .find(|(_, s)| **s == child)
            .map(|(m, _)| m.id)
            .expect("marker remembered");
        (child, marker)
    }

    #[tokio::test]
    async fn subagent_in_the_flat_shape_is_a_marker_and_a_reply_to_it_steers_the_child() {
        let f = fixture(false);
        let (child, marker) = spawn_flat_child(&f).await;
        assert!(
            !f.api
                .calls()
                .iter()
                .any(|c| matches!(c, Call::CreateTopic { .. }))
        );
        assert!(f.chan.lock().threads.by_session(child).is_none());
        assert_eq!(
            f.chan.target_for(child),
            Some(Target {
                chat: OWNER_CHAT,
                thread: None
            })
        );
        f.chan.lock().running.insert(child);
        let mut m = msg(private(OWNER), OWNER, None, "stop that");
        m.reply_to = Some(ReplyRef {
            message_id: marker,
            from_bot: true,
            text: Some("↳ spawned **grep-logs**".into()),
        });
        f.inbound(m).await;
        assert_eq!(
            f.stub.calls(),
            vec![PortCall::Steer(
                child,
                "(replying to: \"↳ spawned **grep-logs**\")\nstop that".into()
            )]
        );
    }

    #[tokio::test]
    async fn child_events_reach_the_parent_worker_as_rows() {
        let f = fixture(false);
        let mut rx = f.tap_worker(&threads::key(OWNER_CHAT, None));
        let (child, _) = spawn_flat_child(&f).await;
        f.chan
            .handle_event(Event::Agent(turn_started(child, "go")))
            .await;
        let mut seen_running = false;
        while let Ok(m) = rx.try_recv() {
            if let WorkerMsg::ChildRow {
                child: c,
                row,
                idle,
            } = m
            {
                assert_eq!(c, child);
                assert_eq!(row.name, "grep-logs");
                if row.state == "running" {
                    assert!(!idle);
                    assert_eq!(row.glyph, "⠹");
                    seen_running = true;
                }
            }
        }
        assert!(seen_running);
    }

    #[tokio::test]
    async fn approvals_go_to_the_child_topic_or_the_owner_chat_or_nowhere() {
        let f = fixture(true);
        let parent = f.bind(&threads::key(OWNER_CHAT, Some(7)), OWNER);
        let child = SessionId::new();
        f.chan
            .handle_event(Event::Agent(AgentEvent::SubagentSpawned {
                parent,
                child,
                name: "x".into(),
                cap: None,
            }))
            .await;
        f.api.take_calls();
        f.chan.handle_event(Event::Requested(approval(child))).await;
        let sends = f.sends();
        assert_eq!(sends.len(), 1);
        let Call::SendRich { to, md, markup, .. } = &sends[0] else {
            panic!()
        };
        assert_eq!(
            *to,
            Target {
                chat: OWNER_CHAT,
                thread: Some(500)
            }
        );
        assert!(md.starts_with("⚠️ **shell** wants to run\n```sh\nls\n```"));
        assert_eq!(markup.as_ref().unwrap().rows[0].len(), 2);

        f.api.take_calls();
        let unbound = SessionId::new();
        f.chan
            .handle_event(Event::Requested(approval(unbound)))
            .await;
        let sends = f.sends();
        assert_eq!(sends.len(), 1);
        let Call::SendRich { to, md, .. } = &sends[0] else {
            panic!()
        };
        assert_eq!(
            *to,
            Target {
                chat: OWNER_CHAT,
                thread: None
            }
        );
        assert!(
            md.ends_with(&format!("session {} · Title", &unbound.to_string()[28..])),
            "{md}"
        );

        let g = fixture_with(
            TelegramConfig {
                approvals: ApprovalsMode::Threads,
                ..TelegramConfig::default()
            },
            true,
        );
        g.chan
            .handle_event(Event::Requested(approval(SessionId::new())))
            .await;
        assert!(g.api.calls().is_empty());
    }

    #[tokio::test]
    async fn approval_press_by_the_thread_user_resolves_and_a_stranger_is_refused() {
        let f = fixture(false);
        let session = f.bind(&threads::key(FRIEND as i64, None), FRIEND);
        let req = approval(session);
        f.chan.handle_event(Event::Requested(req.clone())).await;
        let card = f.chan.lock().cards.get(&req.id).and_then(|c| c.at).unwrap();
        f.api.take_calls();
        f.press(
            STRANGER,
            card.chat,
            None,
            card.id,
            &format!("ap:{}:a", req.id.0),
        )
        .await;
        assert_eq!(
            f.api.take_calls(),
            vec![Call::AnswerCallback {
                id: format!("cb{}", card.id),
                toast: Some("not allowed".into())
            }]
        );
        assert!(f.stub.calls().is_empty());
        f.press(
            FRIEND,
            card.chat,
            None,
            card.id,
            &format!("ap:{}:a", req.id.0),
        )
        .await;
        assert_eq!(
            f.stub.calls(),
            vec![PortCall::Resolve(req.id, Decision::Allow)]
        );
        assert_eq!(
            f.api.take_calls(),
            vec![Call::AnswerCallback {
                id: format!("cb{}", card.id),
                toast: Some("allowed".into())
            }]
        );
        f.chan
            .handle_event(Event::Resolved(ApprovalResolved {
                id: req.id,
                decision: Decision::Allow,
                timed_out: false,
            }))
            .await;
        let calls = f.api.take_calls();
        assert_eq!(calls.len(), 1);
        let Call::EditRich { at, md, markup } = &calls[0] else {
            panic!("{calls:?}")
        };
        assert_eq!(*at, card);
        assert!(
            md.starts_with(&format!(
                "✓ **shell** allowed by user{FRIEND}\n```sh\nls\n```"
            )),
            "{md}"
        );
        assert_eq!(
            markup.as_ref().unwrap().rows,
            vec![vec![Button::Disabled {
                text: "✓ allowed".into()
            }]]
        );
        assert!(f.chan.lock().cards.is_empty());
    }

    #[tokio::test]
    async fn asks_post_one_question_at_a_time_and_a_reply_answers_the_card() {
        let f = fixture(false);
        let session = f.bind(&threads::key(OWNER_CHAT, None), OWNER);
        let req = ask(session);
        f.chan.handle_event(Event::Ask(req.clone())).await;
        let sends = f.sends();
        assert_eq!(sends.len(), 1);
        let Call::SendRich { md, markup, .. } = &sends[0] else {
            panic!()
        };
        assert!(md.starts_with("❓ **Question 1/2** — Which database?\n1. **Postgres** _(recommended)_\n2. **SQLite**"), "{md}");
        let kb = markup.as_ref().unwrap();
        assert!(kb.force_reply);
        assert_eq!(kb.rows.len(), 2);
        let card = f.chan.lock().asks.get(&req.id).and_then(|c| c.at).unwrap();
        f.api.take_calls();
        let mut m = msg(private(OWNER), OWNER, None, "Postgres please");
        m.reply_to = Some(ReplyRef {
            message_id: card.id,
            from_bot: true,
            text: None,
        });
        f.inbound(m).await;
        assert!(
            f.stub.calls().is_empty(),
            "nothing goes to the model before the set is answered"
        );
        let calls = f.api.take_calls();
        assert!(calls.iter().any(|c| matches!(c, Call::EditRich { at, md, markup: None } if *at == card && md.ends_with("→ Postgres please"))), "{calls:?}");
        let second = calls.iter().find_map(|c| match c {
            Call::SendRich { md, .. } if md.starts_with("❓ **Question 2/2**") => Some(()),
            _ => None,
        });
        assert!(second.is_some(), "{calls:?}");
        let card2 = f.chan.lock().asks.get(&req.id).and_then(|c| c.at).unwrap();
        assert_ne!(card2, card);
        f.press(
            OWNER,
            card2.chat,
            None,
            card2.id,
            &format!("ask:{}:1:s", req.id.0),
        )
        .await;
        assert_eq!(
            f.stub.calls(),
            vec![PortCall::AnswerAsk(
                req.id,
                Some(vec![
                    Answer::Text {
                        text: "Postgres please".into()
                    },
                    Answer::Skipped
                ])
            )]
        );
        // A set resolved elsewhere closes its live card.
        let other = ask(session);
        f.chan.handle_event(Event::Ask(other.clone())).await;
        let card3 = f
            .chan
            .lock()
            .asks
            .get(&other.id)
            .and_then(|c| c.at)
            .unwrap();
        f.api.take_calls();
        f.chan
            .handle_event(Event::AskDone(AskResolved {
                id: other.id,
                dismissed: false,
                timed_out: true,
            }))
            .await;
        assert_eq!(
            f.api.take_calls(),
            vec![Call::EditRich {
                at: card3,
                md: "⌛ **Question 1/2** — timed out".into(),
                markup: None
            }]
        );
    }

    #[tokio::test]
    async fn start_with_a_live_code_pairs_and_a_dead_code_is_silent() {
        let f = fixture(true);
        let actor = crate::Telegram::spawn(crate::Telegram {
            handle: Some(Handle {
                chan: Arc::clone(&f.chan),
            }),
            cfg: f.chan.cfg.clone(),
            rt: f.rt.clone(),
            configured: true,
        });
        let _ = actor.register(crate::TELEGRAM_ACTOR);
        f.inbound(msg(private(STRANGER), STRANGER, None, "/start nope"))
            .await;
        assert!(f.api.calls().is_empty());
        assert_eq!(*f.chan.owner.lock(), Some(OWNER));
        f.chan.lock().pairing = Some(Pairing {
            code: "abcdefgh23456789".into(),
            expires: Instant::now() + PAIR_TTL,
        });
        f.inbound(msg(
            private(STRANGER),
            STRANGER,
            None,
            "/start ABCDEFGH23456789",
        ))
        .await;
        assert_eq!(*f.chan.owner.lock(), Some(STRANGER));
        assert!(f.chan.lock().pairing.is_none());
        assert!(
            matches!(f.chan.lock().last_event, Some(TelegramEvent::Paired { user, .. }) if user == STRANGER)
        );
        let calls = f.api.calls();
        assert!(calls.iter().any(|c| matches!(c, Call::SendRich { to, md, .. } if to.chat == STRANGER as i64 && md.starts_with("👋 paired"))));
        assert!(calls.iter().any(|c| matches!(c, Call::SetCommands { scope: CommandScope::Chat(id), .. } if *id == STRANGER as i64)));
        let config = std::fs::read_to_string(f.rt.data_dir.join("config.yaml")).unwrap();
        assert!(config.contains(&format!("owner: {STRANGER}")), "{config}");
        actor.kill();
    }

    #[tokio::test]
    async fn bot_blocked_on_a_send_unbinds_the_chat_and_publishes_unreachable() {
        let f = fixture(false);
        f.bind(&threads::key(OWNER_CHAT, None), OWNER);
        f.api.fail_next(
            "send_rich",
            ApiError::Api {
                code: 403,
                description: BOT_BLOCKED.into(),
            },
        );
        f.chan
            .handle_event(Event::Secret(secrets::SecretsEvent::Requested {
                id: secrets::RequestId(uuid::Uuid::new_v4()),
                session: SessionId::new(),
                name: "OPENAI_KEY".into(),
                note: "for the api".into(),
                current: None,
                title: None,
                fields: Vec::new(),
            }))
            .await;
        assert!(
            f.chan
                .lock()
                .threads
                .get(&threads::key(OWNER_CHAT, None))
                .is_none()
        );
        assert!(
            matches!(f.chan.lock().last_event, Some(TelegramEvent::Unreachable { chat, .. }) if chat == OWNER_CHAT)
        );
    }

    #[tokio::test]
    async fn secret_request_is_a_notice_to_the_owner_only() {
        let f = fixture(false);
        f.bind(&threads::key(FRIEND as i64, None), FRIEND);
        f.chan
            .handle_event(Event::Secret(secrets::SecretsEvent::Requested {
                id: secrets::RequestId(uuid::Uuid::new_v4()),
                session: f.stub.session,
                name: "OPENAI_KEY".into(),
                note: "for the api".into(),
                current: None,
                title: None,
                fields: Vec::new(),
            }))
            .await;
        assert_eq!(
            f.api.calls(),
            vec![Call::SendRich {
                to: Target {
                    chat: OWNER_CHAT,
                    thread: None
                },
                md: secret_notice("OPENAI_KEY", "for the api"),
                markup: None,
                reply_to: None,
                ephemeral: None,
            }]
        );
    }

    #[tokio::test]
    async fn artifact_of_a_child_lands_in_the_nearest_bound_ancestor_by_kind() {
        let f = fixture(false);
        let (child, _) = spawn_flat_child(&f).await;
        let grandchild = SessionId::new();
        f.stub.parents.lock().insert(grandchild, child);
        f.api.take_calls();
        f.chan
            .handle_event(Event::Artifact(artifact(
                grandchild,
                f._dir.path(),
                "plot.png",
                artifact::Kind::Image,
                "image/png",
            )))
            .await;
        f.chan
            .handle_event(Event::Artifact(artifact(
                grandchild,
                f._dir.path(),
                "report.pdf",
                artifact::Kind::Pdf,
                "application/pdf",
            )))
            .await;
        let files: Vec<(Target, String, String, Option<String>)> = f
            .api
            .calls()
            .into_iter()
            .filter_map(|c| match c {
                Call::SendFile {
                    to,
                    name,
                    mime,
                    caption,
                    ..
                } => Some((to, name, mime, caption)),
                _ => None,
            })
            .collect();
        let to = Target {
            chat: OWNER_CHAT,
            thread: None,
        };
        assert_eq!(
            files,
            vec![
                (
                    to,
                    "plot.png".into(),
                    "image/png".into(),
                    Some("Report".into())
                ),
                (
                    to,
                    "report.pdf".into(),
                    "application/pdf".into(),
                    Some("Report".into())
                ),
            ]
        );
        // An unbound chain sends nothing.
        f.api.take_calls();
        f.chan
            .handle_event(Event::Artifact(artifact(
                SessionId::new(),
                f._dir.path(),
                "x.png",
                artifact::Kind::Image,
                "image/png",
            )))
            .await;
        assert!(f.api.calls().is_empty());
    }

    #[tokio::test]
    async fn not_holding_the_lease_drops_inbound_after_recording() {
        let f = fixture(false);
        f.chan.lock().follower.holding = false;
        f.inbound(msg(private(OWNER), OWNER, None, "hello")).await;
        assert!(f.stub.calls().is_empty());
        assert!(f.api.calls().is_empty());
        // The chat was still recorded for `message_user` labels.
        assert_eq!(f.chan.chat_label(OWNER_CHAT), format!("@u{OWNER}"));
    }

    #[tokio::test]
    async fn new_thread_outcome_unbinds_and_topic_closed_unbinds() {
        let f = fixture(true);
        let key = threads::key(OWNER_CHAT, Some(7));
        f.bind(&key, OWNER);
        let mut m = msg(private(OWNER), OWNER, Some(7), "");
        m.text = None;
        m.service = Some(Service::TopicClosed);
        f.inbound(m).await;
        assert!(f.chan.lock().threads.get(&key).is_none());
        f.bind(&key, OWNER);
        let mut m = msg(private(OWNER), OWNER, Some(7), "");
        m.text = None;
        m.service = Some(Service::TopicEdited {
            name: Some("mine".into()),
        });
        f.inbound(m).await;
        assert_eq!(
            f.chan
                .lock()
                .threads
                .get(&key)
                .and_then(|t| t.topic)
                .map(|t| t.named_by_user),
            Some(true)
        );
    }

    #[test]
    fn mention_stripping_and_mime_sniffing() {
        assert_eq!(
            strip_mention("@Theuth_bot hi there", "theuth_bot"),
            "hi there"
        );
        assert_eq!(strip_mention("hi @theuth_bot", "theuth_bot"), "hi");
        assert_eq!(strip_mention("plain", ""), "plain");
        assert_eq!(effective_mime("image/jpeg", b"", "x.bin"), "image/jpeg");
        assert_eq!(effective_mime("", b"%PDF-1.4", "x"), "application/pdf");
        assert_eq!(
            effective_mime("application/octet-stream", b"zzz", "notes.txt"),
            "text/plain"
        );
        assert_eq!(effective_mime("", b"zzz", ""), "application/octet-stream");
        assert_eq!(topic_name("\n\n  first line  \nsecond"), "first line");
        assert_eq!(clip("abc", 3), "abc");
        assert!(clip(&"x".repeat(200), 128).chars().count() <= 128);
    }
}
