//! One session's model loop: frame, stream, dispatch tool calls, steer,
//! compact, cancel (design docs 07, 08).
//!
//! The [`Session`] actor owns the [`SessionState`] between turns; a running
//! turn owns it inside a spawned task and hands it back via [`TurnDone`], so
//! the actor's mailbox never blocks on network I/O.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use futures::future::join_all;
use futures::{Stream, StreamExt};
use genai::Client;
use genai::adapter::AdapterKind;
use genai::chat::{
    ChatMessage, ChatRole, ChatStreamEvent, MessageContent, StreamEnd, Tool, ToolCall, ToolResponse,
};
use genai::resolver::{AuthData, Endpoint};
use genai::{ModelIden, ServiceTarget};
use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use models::Resolved;
use monty_pool::Pool;
use runtime::{Runtime, SessionId, TurnId};
use tokio_util::sync::CancellationToken;
use tools::{Invoke, ListTools, REGISTRY_ACTOR, ToolOutput, ToolRegistry, ToolSpec};

use crate::frame::{self, Frame, Transport};
use crate::knowledge::{self, HarvestResult, Knowledge, Put, Source};
use crate::peers::SpawnSubagentTool;
use crate::repl::{self, Repl, RunCx};
use crate::store::Store;
use crate::supervisor::{Auth, PeerContext, Roster};
use crate::{
    AGENT_ACTOR, AgentConfig, AgentEvent, Agents, CompactStyle, Usage, compact, spill, todo,
};

/// Characters of tool output shown in [`AgentEvent::ToolCallFinished`].
const DISPLAY_OUTPUT_CHARS: usize = 2000;
/// A cache hit is only expected within the provider's TTL (07 §7).
const CACHE_TTL: Duration = Duration::from_secs(5 * 60);
/// Below this prompt size `cached == 0` is legitimate (07 §7).
const CACHE_FLOOR: u32 = 4096;
/// The tool whose `id` argument feeds the compaction head's `Knowledge:` line (10 §7).
const KNOWLEDGE_GET: &str = "knowledge_get";

/// One model call's events, from genai or the Cursor backend.
type EventStream = Pin<Box<dyn Stream<Item = Result<ChatStreamEvent, String>> + Send>>;

/// `tell`: user text. Starts a turn when idle, otherwise queued and injected
/// after the current tool batch (steering).
pub struct UserInput(pub String);

/// `tell`: abort the in-flight turn, if any.
pub struct CancelTurn;

/// `ask` → whether a turn is in flight.
pub struct IsRunning;

/// `tell`: switch the model; takes effect on the next model call, mid-turn included.
pub struct SwitchModel(pub Resolved);

/// `ask` → `Result<u32 /* new epoch */, String>`: compact now, threshold
/// ignored (07 §8.6). `Err("turn in flight")` while a turn runs;
/// `Err("nothing to compact")` when nothing can be folded.
pub struct CompactNow {
    /// One-shot style; `None` = configured default.
    pub style: Option<CompactStyle>,
}

/// `ask` → `Result<HarvestResult, String>`: harvest the finished frame now
/// (10 §5.2). `Err("turn in flight")` while a turn runs.
pub struct HarvestNow;

/// `tell` (from the turn task to its own session): the turn ended.
pub struct TurnDone(SessionState);

/// Per-session state that crosses turns (07 §11). Moved into the running
/// turn and handed back in [`TurnDone`].
pub struct SessionState {
    /// Current epoch; 0 before any compaction.
    pub epoch: u32,
    /// The epoch's `<context_summary>` head; `None` in epoch 0.
    pub head: Option<ChatMessage>,
    /// Store seq + message, every message since the epoch began.
    pub history: Vec<(i64, ChatMessage)>,
    /// Prompt + completion of the last call; 0 after compaction/resume.
    pub used: u32,
    /// Last rendered system prompt; `""` on a fresh actor.
    pub baseline: String,
    /// The `<context_pressure>` note was sent this epoch.
    pub pressure_sent: bool,
    /// When the last call returned and its prompt size, for miss detection (07 §7).
    pub prev_call: Option<(Instant, u32)>,
    /// Run-local todo list (07 §9).
    pub todos: Vec<todo::Todo>,
    /// Round of this turn in which `todo` was last called or reminded.
    pub todo_touched_round: u32,
    /// Concept ids hinted this epoch (10 §4.2); cleared by compaction, not persisted.
    pub hinted: BTreeSet<String>,
    /// Round of this turn in which the last `<knowledge>` check ran.
    pub knowledge_hinted_round: u32,
    /// Ids passed to `knowledge_get` this epoch, for the compaction head (10 §7).
    pub knowledge_read: BTreeSet<String>,
    /// Tool-using root turns since the last harvest (10 §5.2); in memory only.
    pub turns_since_harvest: u32,
}

impl SessionState {
    /// From `Store::frame`.
    pub fn resume(epoch: u32, head: Option<ChatMessage>, history: Vec<(i64, ChatMessage)>) -> Self {
        Self {
            epoch,
            head,
            history,
            used: 0,
            baseline: String::new(),
            pressure_sent: false,
            prev_call: None,
            todos: Vec::new(),
            todo_touched_round: 0,
            hinted: BTreeSet::new(),
            knowledge_hinted_round: 0,
            knowledge_read: BTreeSet::new(),
            turns_since_harvest: 0,
        }
    }
}

/// One conversation. Spawn with `Session::spawn(session)`.
#[derive(kameo::Actor)]
pub struct Session {
    /// Everything a turn task needs, cloned per turn.
    shared: Shared,
    /// `None` while a turn owns it.
    state: Option<SessionState>,
    /// Cancels the in-flight turn; `None` when idle.
    inflight: Option<CancellationToken>,
    /// Whether `sessions.title` is set.
    titled: bool,
}

/// Turn-invariant state; cheap to clone.
#[derive(Clone)]
struct Shared {
    id: SessionId,
    depth: u8,
    /// Current model plus the resolution error of the configured id, if any;
    /// an error makes the next round stop instead of calling genai. Shared
    /// with the running turn so [`SwitchModel`] applies mid-turn.
    model: Arc<Mutex<(Resolved, Option<String>)>>,
    cfg: Arc<AgentConfig>,
    /// Live default style; shared with the supervisor so [`crate::SetCompactStyle`] is in-process.
    compact_style: Arc<RwLock<CompactStyle>>,
    store: Store,
    client: Client,
    auth: Auth,
    rt: Runtime,
    steer: Arc<Mutex<Vec<String>>>,
    catalog: Arc<models::Catalog>,
    /// Python runtime pool; `None` when the pool failed to start (08 §9).
    pool: Option<Arc<Pool>>,
    /// Knowledge bundle; `None` when `agent.knowledge.enabled` is false (10).
    knowledge: Option<Arc<Knowledge>>,
    /// `<data_dir>/spill` (07 §6).
    spill_dir: PathBuf,
}

impl Session {
    /// Build a session around `state` (`SessionState::resume(0, None, vec![])`
    /// for a new one). `depth` ≥ `subagents.max_depth` hides `spawn_subagent`.
    /// `model` is the resolved id and, when resolution failed, the message to surface.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        id: SessionId,
        depth: u8,
        model: (Resolved, Option<String>),
        state: SessionState,
        cfg: Arc<AgentConfig>,
        compact_style: Arc<RwLock<CompactStyle>>,
        store: Store,
        client: Client,
        auth: Auth,
        rt: Runtime,
        catalog: Arc<models::Catalog>,
        pool: Option<Arc<Pool>>,
        knowledge: Option<Arc<Knowledge>>,
    ) -> Self {
        let titled = !state.history.is_empty() || state.head.is_some();
        Self {
            shared: Shared {
                id,
                depth,
                model: Arc::new(Mutex::new(model)),
                cfg,
                compact_style,
                store,
                client,
                auth,
                spill_dir: rt.data_dir.join("spill"),
                rt,
                steer: Arc::default(),
                catalog,
                pool,
                knowledge,
            },
            state: Some(state),
            inflight: None,
            titled,
        }
    }

    async fn start_turn(&mut self, inputs: Vec<String>, me: &ActorRef<Self>) {
        let Some(state) = self.state.take() else {
            // Only reachable if a turn is already running; the caller checked `inflight`.
            return;
        };
        let turn = TurnId::new();
        let cancel = self.shared.rt.shutdown.child_token();
        self.inflight = Some(cancel.clone());
        if !self.titled {
            self.titled = true;
            if let Some(first) = inputs.first()
                && let Err(e) = self
                    .shared
                    .store
                    .set_title(
                        self.shared.id,
                        &first.trim().chars().take(60).collect::<String>(),
                    )
                    .await
            {
                tracing::warn!(session = %self.shared.id, "set_title: {e:#}");
            }
        }
        let model = self
            .shared
            .model
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .0
            .id
            .clone();
        self.shared
            .rt
            .publish(AgentEvent::TurnStarted {
                session: self.shared.id,
                turn,
                text: inputs.join("\n"),
                model,
            })
            .await;
        let task = Turn::new(self.shared.clone(), Some(turn), cancel, state);
        let me = me.clone();
        tokio::spawn(async move {
            let state = task.run(turn, inputs).await;
            let _ = me.tell(TurnDone(state)).await;
        });
    }
}

impl Message<UserInput> for Session {
    type Reply = ();

    async fn handle(&mut self, UserInput(text): UserInput, ctx: &mut Context<Self, Self::Reply>) {
        if self.inflight.is_some() {
            self.shared
                .steer
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(text);
        } else {
            let me = ctx.actor_ref().clone();
            self.start_turn(vec![text], &me).await;
        }
    }
}

impl Message<CancelTurn> for Session {
    type Reply = ();

    async fn handle(&mut self, _: CancelTurn, _: &mut Context<Self, Self::Reply>) {
        if let Some(c) = &self.inflight {
            c.cancel();
        }
    }
}

impl Message<IsRunning> for Session {
    type Reply = bool;

    async fn handle(&mut self, _: IsRunning, _: &mut Context<Self, Self::Reply>) -> bool {
        self.inflight.is_some()
    }
}

impl Message<SwitchModel> for Session {
    type Reply = ();

    async fn handle(
        &mut self,
        SwitchModel(model): SwitchModel,
        _: &mut Context<Self, Self::Reply>,
    ) {
        *self.shared.model.lock().unwrap_or_else(|p| p.into_inner()) = (model, None);
    }
}

impl Message<CompactNow> for Session {
    type Reply = kameo::reply::DelegatedReply<Result<u32, String>>;

    /// A mini-turn (07 §8.6): refused while a turn is in flight, otherwise the
    /// compaction runs in its own task with `inflight` set — input arriving
    /// meanwhile queues as steering, `IsRunning` is true, and the actor's
    /// mailbox stays free (the summariser asks the supervisor for the roster).
    async fn handle(&mut self, m: CompactNow, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        if self.inflight.is_some() {
            tx.send(Err("turn in flight".into()));
            return reply;
        }
        let Some(state) = self.state.take() else {
            tx.send(Err("turn in flight".into()));
            return reply;
        };
        let cancel = self.shared.rt.shutdown.child_token();
        self.inflight = Some(cancel.clone());
        let mut turn = Turn::new(self.shared.clone(), None, cancel, state);
        let me = ctx.actor_ref().clone();
        let style = m.style;
        tokio::spawn(async move {
            let result = turn.compact_now(style).await;
            let _ = me.tell(TurnDone(turn.state)).await;
            tx.send(result);
        });
        reply
    }
}

impl Message<HarvestNow> for Session {
    type Reply = kameo::reply::DelegatedReply<Result<HarvestResult, String>>;

    /// `/knowledge harvest` (10 §5.2): the [`CompactNow`] shape — refused
    /// while a turn is in flight, otherwise a `Turn`-less task owns the state
    /// and `inflight` until the harvest returns.
    async fn handle(&mut self, _: HarvestNow, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let Some(state) = self.state.take().filter(|_| self.inflight.is_none()) else {
            tx.send(Err("turn in flight".into()));
            return reply;
        };
        let cancel = self.shared.rt.shutdown.child_token();
        self.inflight = Some(cancel.clone());
        let mut turn = Turn::new(self.shared.clone(), None, cancel, state);
        let me = ctx.actor_ref().clone();
        tokio::spawn(async move {
            let result = turn.harvest_now().await;
            let _ = me.tell(TurnDone(turn.state)).await;
            tx.send(result);
        });
        reply
    }
}

impl Message<TurnDone> for Session {
    type Reply = ();

    async fn handle(&mut self, TurnDone(state): TurnDone, ctx: &mut Context<Self, Self::Reply>) {
        self.state = Some(state);
        self.inflight = None;
        let queued =
            std::mem::take(&mut *self.shared.steer.lock().unwrap_or_else(|p| p.into_inner()));
        if !queued.is_empty() {
            let me = ctx.actor_ref().clone();
            self.start_turn(queued, &me).await;
        }
    }
}

/// One turn: model call, tool batch, steer injection, repeat. Also the
/// `Turn`-less host of a `/compact` (`turn: None`).
struct Turn {
    shared: Shared,
    /// `None` for a manual compaction between turns.
    turn: Option<TurnId>,
    cancel: CancellationToken,
    state: SessionState,
    usage: Usage,
    /// Cached `(model.genai, transport)`; re-resolved when the model changes mid-turn.
    transport: Option<(String, Transport)>,
    /// Hash + names of the last round's tool list (07 §5).
    tools_seen: Option<(u64, Vec<String>)>,
    /// A rebase, compaction or tool change happened since the last call (07 §7).
    reset_this_round: bool,
    /// The overflow retry was used this turn (07 §8.5).
    overflowed: bool,
    /// `history[since..]` was appended since the last call (07 §8.1).
    since: usize,
    /// Logged `window == 0` this turn.
    window_logged: bool,
    /// Python runtime, opened on the first `python` call (08 §4).
    repl: Option<Repl>,
    /// The turn's inputs joined, the round-0 hint query (10 §4.2).
    input_text: String,
    /// `history[hint_since..]` was appended since the last `<knowledge>` check.
    hint_since: usize,
    /// Tool calls dispatched this turn, the harvest trigger (10 §5.2).
    calls: u32,
    /// Process working directory, resolved once per turn.
    cwd: PathBuf,
}

/// Why a model call did not return a [`StreamEnd`].
enum Abort {
    Cancelled,
    Model(String),
}

/// Outcome of one model call.
enum Round {
    /// Tool calls ran; call the model again.
    Tools,
    /// Overflow compaction succeeded; call again without counting a round.
    Retry,
    /// The turn is over with this stop reason.
    Stop(String),
}

/// genai namespace of a `adapter::model` id.
fn ns(genai: &str) -> &str {
    genai.split_once("::").map_or("", |(n, _)| n)
}

/// Text of `history` worth matching against the bundle (10 §4.2): user
/// steering, assistant texts and tool-call arguments. Not tool results (mostly
/// already read, and large) nor harness wrappers (`<note>`, `<message>`, …).
fn hint_query(history: &[(i64, ChatMessage)]) -> String {
    let mut out = String::new();
    for (_, m) in history {
        match m.role {
            ChatRole::User => {
                let text = text_of(&m.content);
                if !text.trim_start().starts_with('<') {
                    out.push_str(&text);
                    out.push('\n');
                }
            }
            ChatRole::Assistant => {
                for t in m.content.texts() {
                    out.push_str(t);
                    out.push('\n');
                }
                for c in m.content.tool_calls() {
                    out.push_str(&c.fn_arguments.to_string());
                    out.push('\n');
                }
            }
            ChatRole::System | ChatRole::Tool => {}
        }
    }
    out
}

/// The messages of `history`, cloned for a frame.
// ponytail: one clone per round (and one more inside `Frame::request`); keep a
// parallel `Vec<ChatMessage>` in `SessionState` if a profile shows it.
fn messages(history: &[(i64, ChatMessage)]) -> Vec<ChatMessage> {
    history.iter().map(|(_, m)| m.clone()).collect()
}

impl Turn {
    fn new(
        shared: Shared,
        turn: Option<TurnId>,
        cancel: CancellationToken,
        state: SessionState,
    ) -> Self {
        Self {
            since: state.history.len(),
            hint_since: state.history.len(),
            shared,
            turn,
            cancel,
            state,
            usage: Usage::default(),
            transport: None,
            tools_seen: None,
            reset_this_round: false,
            overflowed: false,
            window_logged: false,
            repl: None,
            input_text: String::new(),
            calls: 0,
            cwd: std::env::current_dir().unwrap_or_default(),
        }
    }

    /// The bundle when knowledge is on (10 §8 `enabled`).
    fn knowledge(&self) -> Option<Arc<Knowledge>> {
        self.shared
            .cfg
            .knowledge
            .enabled
            .then(|| self.shared.knowledge.clone())
            .flatten()
    }

    async fn publish(&self, event: AgentEvent) {
        self.shared.rt.publish(event).await;
    }

    async fn error(&self, message: String) {
        tracing::warn!(session = %self.shared.id, turn = ?self.turn, "{message}");
        self.publish(AgentEvent::Error {
            session: self.shared.id,
            turn: self.turn,
            message,
        })
        .await;
    }

    fn model(&self) -> (Resolved, Option<String>) {
        self.shared
            .model
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Context window of `model`; 0 when the catalog does not know it (07 §8.1).
    fn window(&mut self, model: &Resolved) -> u32 {
        let window = self
            .shared
            .catalog
            .closest(&model.id)
            .map_or(0, |m| m.limit.context);
        if window == 0 && !self.window_logged {
            self.window_logged = true;
            tracing::info!(session = %self.shared.id, model = %model.id, "context window unknown; auto-compaction off");
        }
        window
    }

    /// Append to the store and the history (07 §2.2.1: the only way in).
    async fn persist(&mut self, msg: ChatMessage) {
        let seq = match self.shared.store.append(self.shared.id, &msg).await {
            Ok(seq) => seq,
            Err(e) => {
                self.error(format!("store: {e:#}")).await;
                self.state.history.last().map_or(0, |(s, _)| s + 1)
            }
        };
        self.state.history.push((seq, msg));
    }

    /// A harness note: user-role tail message (07 §8.7).
    async fn note(&mut self, kind: &str, text: &str) {
        self.persist(ChatMessage::user(format!("<{kind}>\n{text}\n</{kind}>")))
            .await;
    }

    /// Render the system prompt for this turn; log a rebase when it changed (07 §4).
    /// Also resyncs the knowledge index with the bundle's files (10 §3.2).
    async fn rebase(&mut self, registry: &ActorRef<ToolRegistry>) {
        if let Some(knowledge) = self.knowledge() {
            match knowledge.sync().await {
                Ok(s) if !s.is_empty() => tracing::info!(
                    session = %self.shared.id,
                    added = s.added,
                    updated = s.updated,
                    removed = s.removed,
                    "knowledge sync"
                ),
                Ok(_) => {}
                Err(e) => tracing::warn!(session = %self.shared.id, "knowledge sync: {e:#}"),
            }
        }
        let peer_line = match ActorRef::<Agents>::lookup(AGENT_ACTOR) {
            Ok(Some(agents)) => agents
                .ask(PeerContext {
                    session: self.shared.id,
                })
                .await
                .unwrap_or_default(),
            _ => String::new(),
        };
        let genai = self.model().0.genai;
        let claude_code = ns(&genai) == "anthropic" && self.shared.auth.login("anthropic");
        let mut baseline = frame::baseline(&self.shared.rt.data_dir, &peer_line, claude_code);
        if self.shared.pool.is_some() {
            let specs = registry.ask(ListTools).await.unwrap_or_default();
            baseline.push_str("\n\n");
            baseline.push_str(&repl::prompt_block(&specs));
        }
        if baseline != self.state.baseline {
            if !self.state.baseline.is_empty() {
                tracing::info!(
                    session = %self.shared.id,
                    "rebase: system prompt changed ({} → {} bytes)",
                    self.state.baseline.len(),
                    baseline.len()
                );
                self.reset_this_round = true;
            }
            self.state.baseline = baseline;
        }
    }

    /// Cache marks for `model`'s provider; resolved once per model per turn (07 §3).
    async fn transport(&mut self, model: &Resolved) -> Result<Transport, String> {
        if let Some((id, t)) = &self.transport
            && *id == model.genai
        {
            return Ok(*t);
        }
        let t = if ns(&model.genai) == "cursor" {
            Transport::Plain
        } else if model
            .provider
            .as_deref()
            .is_some_and(|p| self.shared.auth.gateway(p).is_some())
        {
            Transport::OpenAiLike
        } else {
            let target = self
                .shared
                .client
                .resolve_service_target(model.genai.as_str())
                .await
                .map_err(|e| e.to_string())?;
            Transport::of(target.model.adapter_kind)
        };
        self.transport = Some((model.genai.clone(), t));
        Ok(t)
    }

    /// This round's direct tools (07 §5, 08 §0.4): registry tools (only
    /// `direct` ones when the runtime is on, `spawn_subagent` gated by depth)
    /// plus the loop-local `todo` and `python`.
    async fn tools(&mut self, registry: &ActorRef<ToolRegistry>) -> Vec<Tool> {
        let specs = match registry.ask(ListTools).await {
            Ok(specs) => specs,
            Err(e) => {
                self.error(format!("list tools: {e}")).await;
                Vec::new()
            }
        };
        let runtime = self.shared.pool.is_some();
        let mut specs: Vec<ToolSpec> = specs
            .into_iter()
            .filter(|s| {
                (!runtime || s.direct)
                    && (s.name != <SpawnSubagentTool as tools::Tool>::NAME
                        || self.shared.depth < self.shared.cfg.subagents.max_depth)
            })
            .collect();
        specs.push(todo::spec());
        if runtime {
            specs.push(repl::spec());
        }
        specs.sort_by(|a, b| a.name.cmp(&b.name));
        let names: Vec<String> = specs.iter().map(|s| s.name.clone()).collect();
        let tools: Vec<Tool> = specs
            .into_iter()
            .map(|s| {
                Tool::new(s.name)
                    .with_description(s.description)
                    .with_schema(s.schema)
            })
            .collect();
        let hash = frame::tools_hash(&tools);
        if let Some((prev, seen)) = &self.tools_seen
            && *prev != hash
        {
            let diff = |a: &[String], b: &[String]| {
                a.iter()
                    .filter(|n| !b.contains(n))
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            tracing::warn!(
                session = %self.shared.id,
                "tools changed mid-turn: +[{}] -[{}] (prefix cache reset)",
                diff(&names, seen),
                diff(seen, &names)
            );
            self.reset_this_round = true;
        }
        self.tools_seen = Some((hash, names));
        tools
    }

    /// Open the stream for `frame` and drain it to its `End`. With `live`,
    /// deltas and pending tool calls are published; the summariser passes `None`.
    async fn call(
        &self,
        model: &Resolved,
        frame: Frame<'_>,
        transport: Transport,
        live: Option<TurnId>,
    ) -> Result<StreamEnd, Abort> {
        let (req, opts) = frame.request(transport, self.shared.id);
        let open = async {
            if ns(&model.genai) == "cursor" {
                let t = self.shared.auth.oauth.access(&oauth::CURSOR).await?;
                let slug = model
                    .id
                    .split_once('/')
                    .map_or(model.id.as_str(), |(_, m)| m);
                cursor::chat_stream(&t.access_token, slug, req, &self.shared.id.to_string()).await
            } else if let Some(gw) = model
                .provider
                .as_deref()
                .and_then(|p| self.shared.auth.gateway(p))
            {
                let slug = model
                    .id
                    .split_once('/')
                    .map_or(model.id.as_str(), |(_, m)| m);
                let target = ServiceTarget {
                    endpoint: Endpoint::from_owned(gw.base_url),
                    auth: AuthData::Key(gw.key),
                    model: ModelIden::new(AdapterKind::OpenAI, slug.to_owned()),
                };
                self.shared
                    .client
                    .exec_chat_stream(target, req, Some(&opts))
                    .await
                    .map(|r| -> EventStream {
                        Box::pin(r.stream.map(|ev| ev.map_err(|e| e.to_string())))
                    })
                    .map_err(|e| e.to_string())
            } else {
                self.shared
                    .client
                    .exec_chat_stream(&model.genai, req, Some(&opts))
                    .await
                    .map(|r| -> EventStream {
                        Box::pin(r.stream.map(|ev| ev.map_err(|e| e.to_string())))
                    })
                    .map_err(|e| e.to_string())
            }
        };
        let mut stream = tokio::select! {
            _ = self.cancel.cancelled() => return Err(Abort::Cancelled),
            r = open => r.map_err(Abort::Model)?,
        };
        let session = self.shared.id;
        let mut pending: Vec<String> = Vec::new();
        loop {
            let ev = tokio::select! {
                _ = self.cancel.cancelled() => return Err(Abort::Cancelled),
                ev = stream.next() => ev,
            };
            match ev {
                None => return Err(Abort::Model("stream ended without End event".into())),
                Some(Err(e)) => return Err(Abort::Model(e)),
                Some(Ok(ChatStreamEvent::End(end))) => return Ok(end),
                Some(Ok(ChatStreamEvent::Chunk(c))) => {
                    if let Some(turn) = live {
                        self.publish(AgentEvent::TextDelta {
                            session,
                            turn,
                            text: c.content.into(),
                        })
                        .await
                    }
                }
                Some(Ok(ChatStreamEvent::ReasoningChunk(c))) => {
                    if let Some(turn) = live {
                        self.publish(AgentEvent::ReasoningDelta {
                            session,
                            turn,
                            text: c.content.into(),
                        })
                        .await
                    }
                }
                // Arguments arrive whole only at End (ToolCallStarted is
                // published at dispatch); the first chunk names the tool.
                Some(Ok(ChatStreamEvent::ToolCallChunk(c))) => {
                    let tc = c.tool_call;
                    if let Some(turn) = live
                        && !tc.fn_name.is_empty()
                        && !pending.contains(&tc.call_id)
                    {
                        pending.push(tc.call_id.clone());
                        self.publish(AgentEvent::ToolCallPending {
                            session,
                            turn,
                            call_id: tc.call_id,
                            name: tc.fn_name,
                        })
                        .await;
                    }
                }
                Some(Ok(
                    ChatStreamEvent::Start
                    | ChatStreamEvent::Heartbeat
                    | ChatStreamEvent::ThoughtSignatureChunk(_),
                )) => {}
            }
        }
    }

    /// Runs to completion (or cancel/error) and returns the state to hand back.
    async fn run(mut self, turn: TurnId, inputs: Vec<String>) -> SessionState {
        self.input_text = inputs.join("\n");
        for text in inputs {
            self.persist(ChatMessage::user(text)).await;
        }
        let registry = ActorRef::<ToolRegistry>::lookup(REGISTRY_ACTOR)
            .ok()
            .flatten();
        let stop_reason = match &registry {
            None => {
                self.error("tool registry not running".into()).await;
                "error".to_string()
            }
            Some(registry) => {
                self.rebase(registry).await;
                self.state.todo_touched_round = 0;
                self.state.knowledge_hinted_round = 0;
                let mut rounds = 0u32;
                loop {
                    match self.round(registry, turn, rounds).await {
                        Round::Tools => {
                            rounds += 1;
                            if rounds >= self.shared.cfg.max_tool_rounds {
                                self.error(format!("stopped after {rounds} tool rounds"))
                                    .await;
                                break "max_tool_rounds".to_string();
                            }
                            let queued = std::mem::take(
                                &mut *self.shared.steer.lock().unwrap_or_else(|p| p.into_inner()),
                            );
                            for text in queued {
                                self.persist(ChatMessage::user(text)).await;
                            }
                        }
                        Round::Retry => {}
                        Round::Stop(reason) => break reason,
                    }
                }
            }
        };
        if let Some(repl) = self.repl.take() {
            if stop_reason == "cancelled" {
                // 08 §5: a cancelled cell's worker is discarded; the next turn restores the last dump.
                drop(repl);
            } else {
                repl.close().await;
            }
        }
        // 10 §5.2 trigger (a), before `TurnFinished`: one-shot clients (`wth -p`)
        // exit on that event, so a harvest published after it is always cancelled.
        // Inline rather than concurrent with the next turn: the state is still
        // owned here, so input arriving meanwhile queues as steering.
        let harvestable = self.calls > 0 && stop_reason != "cancelled" && self.shared.depth == 0;
        let every = self.shared.cfg.knowledge.harvest_every;
        if harvestable
            && let Some(registry) = &registry
            && self.knowledge().is_some()
        {
            self.state.turns_since_harvest += 1;
            if every > 0 && self.state.turns_since_harvest >= every {
                self.state.turns_since_harvest = 0;
                self.publish(AgentEvent::Harvesting {
                    session: self.shared.id,
                })
                .await;
                if let Err(e) = self.harvest_with(registry).await {
                    tracing::warn!(session = %self.shared.id, "harvest: {e}");
                }
            }
        }
        self.publish(AgentEvent::TurnFinished {
            session: self.shared.id,
            turn,
            stop_reason,
            usage: self.usage,
            context: self.state.used,
        })
        .await;
        self.state
    }

    /// One model call plus its tool batch. `round` counts calls made so far this turn.
    async fn round(
        &mut self,
        registry: &ActorRef<ToolRegistry>,
        turn: TurnId,
        round: u32,
    ) -> Round {
        let (model, error) = self.model();
        if let Some(e) = error {
            self.error(format!("model: {e}")).await;
            return Round::Stop("error".into());
        }
        let tools = self.tools(registry).await;
        let transport = match self.transport(&model).await {
            Ok(t) => t,
            Err(e) => {
                self.error(format!("model: {e}")).await;
                return Round::Stop("error".into());
            }
        };

        // 07 §8.1 trigger, §8.7 pressure note, §9 todo reminder — all before the call.
        let window = self.window(&model);
        let appended: u32 = self.state.history[self.since..]
            .iter()
            .map(|(_, m)| compact::est(m))
            .sum();
        let projected = self.state.used + appended;
        let compact_at = self.shared.cfg.compact_at;
        tracing::debug!(session = %self.shared.id, round, window, used = self.state.used, appended, projected, "trigger check");
        if window > 0 && compact_at > 0.0 {
            let fill = projected as f32 / window as f32;
            if fill >= compact_at {
                if let Err(e) = self
                    .compact(
                        &model,
                        &tools,
                        transport,
                        projected,
                        None,
                        compact::CompactReason::Auto,
                    )
                    .await
                {
                    self.error(format!("compaction: {e}")).await;
                }
            } else if !self.state.pressure_sent && compact_at > 0.1 && fill >= compact_at - 0.1 {
                let pct = (fill * 100.0) as u32;
                self.note(
                    "context_pressure",
                    &format!("The context window is {pct}% full; compaction is near. Finish the current step and state results and next steps explicitly in your reply so they survive."),
                )
                .await;
                self.state.pressure_sent = true;
            }
        }
        let open_items = self
            .state
            .todos
            .iter()
            .any(|t| matches!(t.status, todo::Status::Pending | todo::Status::InProgress));
        if open_items && round - self.state.todo_touched_round >= todo::TODO_REMIND_ROUNDS {
            let list = todo::render(&self.state.todos);
            self.note("todo", &list).await;
            self.state.todo_touched_round = round;
        }
        // 10 §4.2: hints at turn start against the input, then every `hint_rounds`
        // rounds against what was appended since; each id once per epoch.
        if let Some(knowledge) = self.knowledge() {
            let cfg = &self.shared.cfg.knowledge;
            let query = if round == 0 {
                Some(std::mem::take(&mut self.input_text))
            } else if round - self.state.knowledge_hinted_round >= cfg.hint_rounds {
                Some(hint_query(&self.state.history[self.hint_since..]))
            } else {
                None
            };
            if query.is_some() {
                self.state.knowledge_hinted_round = round;
                self.hint_since = self.state.history.len();
            }
            // A retried round 0 or a text-less span has nothing to match.
            if let Some(query) = query.filter(|q| !q.trim().is_empty()) {
                match knowledge
                    .hint(&query, &self.cwd, &self.state.hinted, cfg.hint_limit)
                    .await
                {
                    Ok(metas) if !metas.is_empty() => {
                        self.note("knowledge", &knowledge::harvest::render_hints(&metas))
                            .await;
                        self.state.hinted.extend(metas.into_iter().map(|m| m.id));
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!(session = %self.shared.id, "knowledge hint: {e:#}"),
                }
            }
        }

        let msgs = messages(&self.state.history);
        let frame = Frame {
            tools: &tools,
            system: &self.state.baseline,
            head: self.state.head.as_ref(),
            history: &msgs,
        };
        let end = match self.call(&model, frame, transport, Some(turn)).await {
            Ok(end) => end,
            Err(Abort::Cancelled) => return Round::Stop("cancelled".into()),
            Err(Abort::Model(e)) => {
                // 07 §8.5: one forced compaction per turn, then retry the round.
                if compact::is_overflow(&e) && !self.overflowed {
                    self.overflowed = true;
                    match self
                        .compact(
                            &model,
                            &tools,
                            transport,
                            projected,
                            None,
                            compact::CompactReason::Overflow,
                        )
                        .await
                    {
                        Ok(_) => return Round::Retry,
                        Err(ce) => {
                            tracing::warn!(session = %self.shared.id, "overflow compaction failed: {ce}")
                        }
                    }
                }
                self.error(format!("model: {e}")).await;
                return Round::Stop("error".into());
            }
        };

        if let Some(u) = &end.captured_usage {
            let prompt = u.prompt_tokens.unwrap_or(0).max(0) as u32;
            let completion = u.completion_tokens.unwrap_or(0).max(0) as u32;
            let cached = u
                .prompt_tokens_details
                .as_ref()
                .and_then(|d| d.cached_tokens)
                .unwrap_or(0)
                .max(0) as u32;
            self.usage.prompt += prompt;
            self.usage.completion += completion;
            self.usage.cached += cached;
            self.state.used = prompt + completion;
            // 07 §7 miss detection: the design checking itself in production.
            if transport != Transport::Plain
                && let Some((at, prev_prompt)) = self.state.prev_call
                && at.elapsed() < CACHE_TTL
                && !self.reset_this_round
                && prev_prompt >= CACHE_FLOOR
                && cached < prev_prompt * 9 / 10
            {
                tracing::warn!(
                    session = %self.shared.id,
                    round,
                    cached,
                    prev_prompt,
                    "prefix cache miss (no rebase/compaction/tool change since the previous call)"
                );
            }
            self.state.prev_call = Some((Instant::now(), prompt));
        }
        self.reset_this_round = false;
        self.since = self.state.history.len();

        let stop = end
            .captured_stop_reason
            .as_ref()
            .map(|s| s.raw().to_string())
            .unwrap_or_else(|| "end_turn".into());
        let calls: Vec<ToolCall> = end
            .captured_tool_calls()
            .map(|v| v.into_iter().cloned().collect())
            .unwrap_or_default();
        if calls.is_empty() {
            if let Some(content) = end.captured_content.filter(|c| !c.is_empty()) {
                self.persist(ChatMessage::assistant(content)).await;
            }
            return Round::Stop(stop);
        }
        let assistant = end
            .into_assistant_message_for_tool_use()
            .unwrap_or_else(|| ChatMessage::from(calls.clone()));
        self.persist(assistant).await;

        let session = self.shared.id;
        for call in &calls {
            self.publish(AgentEvent::ToolCallStarted {
                session,
                turn,
                call_id: call.call_id.clone(),
                name: call.fn_name.clone(),
                args: call.fn_arguments.clone(),
            })
            .await;
        }
        // Registry batch first (parallel), then the loop-local calls in order.
        let local = |name: &str| name == todo::NAME || name == repl::NAME;
        let mut outputs: Vec<Option<ToolOutput>> = calls.iter().map(|_| None).collect();
        let remote: Vec<usize> = (0..calls.len())
            .filter(|&i| !local(&calls[i].fn_name))
            .collect();
        let cancel = self.cancel.clone();
        let mut cancelled = false;
        let outs = tokio::select! {
            _ = cancel.cancelled() => { cancelled = true; Vec::new() }
            outs = join_all(remote.iter().map(|&i| {
                let c = &calls[i];
                registry.ask(Invoke(tools::ToolCall {
                    id: c.call_id.clone(),
                    name: c.fn_name.clone(),
                    args: c.fn_arguments.clone(),
                    session,
                })).send()
            })) => outs,
        };
        for (i, out) in remote.into_iter().zip(outs) {
            outputs[i] =
                Some(out.unwrap_or_else(|e| ToolOutput::err(format!("tool registry: {e}"))));
        }
        for (i, call) in calls.iter().enumerate() {
            if cancelled || outputs[i].is_some() {
                continue;
            }
            let out = if call.fn_name == todo::NAME {
                self.todo(call.fn_arguments.clone(), round)
            } else {
                tokio::select! {
                    _ = cancel.cancelled() => { cancelled = true; continue }
                    out = self.python(registry, turn, call) => out,
                }
            };
            outputs[i] = Some(out);
        }

        let mut responses = Vec::with_capacity(calls.len());
        for (call, out) in calls.iter().zip(outputs) {
            let out = out.unwrap_or_else(|| ToolOutput::err("cancelled by user"));
            self.publish(AgentEvent::ToolCallFinished {
                session,
                turn,
                call_id: call.call_id.clone(),
                output: frame::truncate(&out.content, DISPLAY_OUTPUT_CHARS),
                is_error: out.is_error,
            })
            .await;
            // 07 §6: bounded once, here, with the full text spilled to disk.
            let content = spill::cap(
                session,
                &call.call_id,
                &out.content,
                self.shared.cfg.max_tool_output_chars,
                &self.shared.spill_dir,
            );
            let content = if out.is_error {
                format!("ERROR: {content}")
            } else {
                content
            };
            responses.push(ToolResponse::new(call.call_id.clone(), content));
        }
        // One tool message carrying every response (genai: `ChatMessage::from(Vec<ToolResponse>)`).
        self.persist(ChatMessage::from(responses)).await;
        self.calls += calls.len() as u32;
        // 10 §7: the ids the model held verbatim this epoch, for the compaction head.
        self.state.knowledge_read.extend(
            calls
                .iter()
                .filter(|c| c.fn_name == KNOWLEDGE_GET)
                .filter_map(|c| c.fn_arguments.get("id")?.as_str().map(str::to_owned)),
        );
        if cancelled {
            return Round::Stop("cancelled".into());
        }
        Round::Tools
    }

    /// The loop-local `todo` tool (07 §9): the rendered list is the output.
    fn todo(&mut self, args: serde_json::Value, round: u32) -> ToolOutput {
        self.state.todo_touched_round = round;
        let applied = serde_json::from_value::<todo::TodoArgs>(args)
            .map_err(|e| format!("bad arguments: {e}"))
            .and_then(|a| todo::apply(&mut self.state.todos, a));
        match applied {
            Ok(()) => ToolOutput::ok(todo::render(&self.state.todos)),
            Err(e) => ToolOutput::err(e),
        }
    }

    /// The loop-local `python` tool (08 §3.1): one [`Repl`] per turn, opened on first use.
    async fn python(
        &mut self,
        registry: &ActorRef<ToolRegistry>,
        turn: TurnId,
        call: &ToolCall,
    ) -> ToolOutput {
        let Some(pool) = &self.shared.pool else {
            return ToolOutput::err("python runtime is off (worker pool failed to start)");
        };
        let code = match serde_json::from_value::<repl::PythonArgs>(call.fn_arguments.clone()) {
            Ok(a) => a.code,
            Err(e) => return ToolOutput::err(format!("bad arguments: {e}")),
        };
        let repl = match self.repl.take() {
            Some(r) => r,
            None => {
                let cx = RunCx {
                    session: self.shared.id,
                    turn,
                    rt: self.shared.rt.clone(),
                    registry: registry.clone(),
                    spill_dir: self.shared.spill_dir.clone(),
                    cwd: self.cwd.clone(),
                };
                Repl::open(pool, self.shared.id, &self.shared.rt.data_dir, cx).await
            }
        };
        self.repl.insert(repl).run(&call.call_id, &code).await
    }

    /// `/compact` between turns: same baseline/tools/transport as a turn, then [`Turn::compact`].
    async fn compact_now(&mut self, style: Option<CompactStyle>) -> Result<u32, String> {
        let registry = self.between_turns().await?;
        let (model, tools, transport) = self.resolve(&registry).await?;
        let projected = self.state.used;
        self.compact(
            &model,
            &tools,
            transport,
            projected,
            style,
            compact::CompactReason::Manual,
        )
        .await
    }

    /// `/knowledge harvest` between turns (10 §5.2): rebase, then [`Turn::harvest`].
    async fn harvest_now(&mut self) -> Result<HarvestResult, String> {
        let registry = self.between_turns().await?;
        self.harvest_with(&registry).await
    }

    /// The mini-turn preamble shared by `/compact` and `/knowledge harvest`:
    /// a running registry and a fresh baseline.
    async fn between_turns(&mut self) -> Result<ActorRef<ToolRegistry>, String> {
        let registry = ActorRef::<ToolRegistry>::lookup(REGISTRY_ACTOR)
            .ok()
            .flatten()
            .ok_or("tool registry not running")?;
        self.rebase(&registry).await;
        Ok(registry)
    }

    /// What a model call needs: the current model (or its resolution error),
    /// this round's tools and the provider's transport.
    async fn resolve(
        &mut self,
        registry: &ActorRef<ToolRegistry>,
    ) -> Result<(Resolved, Vec<Tool>, Transport), String> {
        let (model, error) = self.model();
        if let Some(e) = error {
            return Err(format!("model: {e}"));
        }
        let tools = self.tools(registry).await;
        let transport = self.transport(&model).await?;
        Ok((model, tools, transport))
    }

    /// [`Turn::resolve`] then [`Turn::harvest`].
    async fn harvest_with(
        &mut self,
        registry: &ActorRef<ToolRegistry>,
    ) -> Result<HarvestResult, String> {
        let (model, tools, transport) = self.resolve(registry).await?;
        self.harvest(&model, &tools, transport).await
    }

    /// Compaction (07 §8.2–§8.4, 11): cut, build the head, open a new epoch.
    /// `projected` is reported in `Compacted.before`. Styles only change the head.
    async fn compact(
        &mut self,
        model: &Resolved,
        tools: &[Tool],
        transport: Transport,
        projected: u32,
        r#override: Option<CompactStyle>,
        reason: compact::CompactReason,
    ) -> Result<u32, String> {
        let window = self.window(model);
        // 07 §8.2: the kept tail is 1/8 of the *threshold* — a tail sized to the
        // raw window would leave a small-threshold session re-compacting every round.
        let compact_at = self.shared.cfg.compact_at;
        let limit = if compact_at > 0.0 {
            (compact_at * window as f32) as u32
        } else {
            window
        };
        let n_msgs = self.state.history.len();
        let mut full = Vec::with_capacity(n_msgs + 1);
        full.extend(self.state.history.iter().map(|(_, m)| m.clone()));
        let keep_from = compact::keep_from(&full, limit);
        if keep_from == 0 {
            return Err("nothing to compact".into());
        }
        let cfg_style = *self
            .shared
            .compact_style
            .read()
            .unwrap_or_else(|p| p.into_inner());
        let mut style = compact::resolve(r#override, cfg_style, reason)?;
        let session = self.shared.id;
        self.publish(AgentEvent::Compacting {
            session,
            turn: self.turn,
        })
        .await;
        // 10 §7: harvest → summarise → cut; the frame's detail is cheapest to read now.
        // Counts this turn's own tool calls too: a session's first long turn compacts
        // with `turns_since_harvest == 0`. `calls` restarts so the turn end only
        // recounts work done after the cut.
        if self.shared.cfg.knowledge.harvest_every > 0
            && self.shared.depth == 0
            && (self.state.turns_since_harvest > 0 || self.calls > 0)
            && self.knowledge().is_some()
        {
            self.state.turns_since_harvest = 0;
            self.calls = 0;
            if let Err(e) = self.harvest(model, tools, transport).await {
                tracing::warn!(session = %session, "harvest before compaction: {e}");
            }
        }
        let cut_body = || {
            let k = keep_from + usize::from(self.state.head.is_some());
            format!("folded {k} messages; no summary. Recent turns follow verbatim.")
        };
        let body = match style {
            CompactStyle::Snap => return Err("snap not built".into()),
            CompactStyle::Cut => cut_body(),
            CompactStyle::Summary => {
                full.push(ChatMessage::user(compact::COMPACT_INSTRUCTION));
                match self
                    .summarise(model, tools, transport, &mut full, keep_from)
                    .await
                {
                    Ok(text) => text,
                    Err(e) if e.contains("cancelled") => return Err(e),
                    Err(e)
                        if compact::is_overflow(&e)
                            || e.contains("no text twice")
                            || reason == compact::CompactReason::Overflow =>
                    {
                        style = CompactStyle::Cut;
                        cut_body()
                    }
                    Err(e) => return Err(e),
                }
            }
        };

        // 07 §8.4, 10 §7: harness truth the summariser cannot drift — roster, todos,
        // runtime inventory, concepts read.
        let n = self.state.epoch + 1;
        let roster = match ActorRef::<Agents>::lookup(AGENT_ACTOR) {
            Ok(Some(agents)) => agents.ask(Roster { session }).await.unwrap_or_default(),
            _ => String::new(),
        };
        let todos = (!self.state.todos.is_empty()).then(|| todo::render(&self.state.todos));
        let runtime = match &self.repl {
            Some(r) => r.render_inventory(),
            None => repl::saved_inventory(&self.shared.rt.data_dir, session),
        };
        let read = (!self.state.knowledge_read.is_empty()).then(|| {
            let ids: Vec<&str> = self
                .state
                .knowledge_read
                .iter()
                .map(String::as_str)
                .collect();
            format!("Knowledge: read {}", ids.join(", "))
        });
        let text = compact::wrap_head(
            n,
            style,
            body.trim(),
            [
                roster,
                todos.unwrap_or_default(),
                runtime.map(|r| format!("Runtime: {r}")).unwrap_or_default(),
                read.unwrap_or_default(),
            ],
        );
        let head = ChatMessage::user(text);
        let at_seq = self.state.history[n_msgs - 1].0;
        let tail_from = self.state.history[keep_from].0;
        self.shared
            .store
            .compact(session, n, at_seq, tail_from, &head)
            .await
            .map_err(|e| format!("store: {e:#}"))?;
        // 07 §2.2.1: the one non-append mutation of history.
        self.state.history.drain(..keep_from);
        self.state.head = Some(head);
        self.state.used = 0;
        self.state.epoch = n;
        self.state.pressure_sent = false;
        self.state.hinted.clear();
        self.state.knowledge_read.clear();
        self.reset_this_round = true;
        self.since = self.state.history.len();
        self.hint_since = self.hint_since.saturating_sub(keep_from);
        tracing::info!(session = %session, epoch = n, folded = keep_from, before = projected, style = %style, "compaction");
        self.publish(AgentEvent::Compacted {
            session,
            turn: self.turn,
            epoch: n,
            before: projected,
            folded: keep_from as u32,
            style,
        })
        .await;
        Ok(n)
    }

    /// The summariser call (07 §8.3, §8.5): `full` is the history plus the
    /// instruction as its last message. A [`Turn::continuation`] that retries
    /// once on overflow with the kept tail removed.
    async fn summarise(
        &self,
        model: &Resolved,
        tools: &[Tool],
        transport: Transport,
        full: &mut Vec<ChatMessage>,
        keep_from: usize,
    ) -> Result<String, String> {
        let mut trimmed = false;
        loop {
            match self
                .continuation(model, tools, transport, full, compact::COMPACT_INSTRUCTION)
                .await
            {
                Ok(text) => return Ok(text),
                Err(e) if compact::is_overflow(&e) && !trimmed => {
                    trimmed = true;
                    tracing::info!(session = %self.shared.id, "summariser overflowed; retrying without the kept tail");
                    full.truncate(keep_from);
                    full.push(ChatMessage::user(compact::COMPACT_INSTRUCTION));
                }
                Err(e) => return Err(format!("summariser: {e}")),
            }
        }
    }

    /// A cache-hot continuation of the live frame (07 §8.3, 10 §5.4): `full`
    /// is the history plus `instruction` as its last message. Retries once
    /// with a `Text only.` prefix when the reply carries only tool calls.
    /// `Err` is the raw model error (callers test `compact::is_overflow`).
    async fn continuation(
        &self,
        model: &Resolved,
        tools: &[Tool],
        transport: Transport,
        full: &mut Vec<ChatMessage>,
        instruction: &str,
    ) -> Result<String, String> {
        let mut text_only = false;
        loop {
            let frame = Frame {
                tools,
                system: &self.state.baseline,
                head: self.state.head.as_ref(),
                history: full.as_slice(),
            };
            match self.call(model, frame, transport, None).await {
                Ok(end) => {
                    let has_calls = end.captured_tool_calls().is_some_and(|c| !c.is_empty());
                    let text = end.captured_texts().map(|t| t.concat()).unwrap_or_default();
                    if !has_calls && !text.trim().is_empty() {
                        return Ok(text);
                    }
                    if text_only {
                        return Err("no text twice".into());
                    }
                    text_only = true;
                    tracing::info!(session = %self.shared.id, "continuation replied without text; retrying");
                    if let Some(last) = full.last_mut() {
                        *last = ChatMessage::user(format!("Text only. {instruction}"));
                    }
                }
                Err(Abort::Cancelled) => return Err("cancelled".into()),
                Err(Abort::Model(e)) => return Err(e),
            }
        }
    }

    /// Harvest (10 §5.4): ask the finished frame for durable knowledge as OKF
    /// documents and write them. Nothing is persisted to the session; an
    /// overflow is an `Err` (the frame is about to be compacted anyway).
    async fn harvest(
        &mut self,
        model: &Resolved,
        tools: &[Tool],
        transport: Transport,
    ) -> Result<HarvestResult, String> {
        let knowledge = self.knowledge().ok_or("knowledge is off")?;
        if self.state.history.is_empty() {
            return Err("nothing to harvest".into());
        }
        let session = self.shared.id;
        let related = knowledge
            .hint(
                &hint_query(&self.state.history),
                &self.cwd,
                &BTreeSet::new(),
                10,
            )
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(session = %session, "harvest related: {e:#}");
                Vec::new()
            });
        let cwd = self.cwd.to_string_lossy();
        let instruction = knowledge::harvest::instruction(&related, &cwd);
        let mut full = messages(&self.state.history);
        full.push(ChatMessage::user(&instruction));
        let text = self
            .continuation(model, tools, transport, &mut full, &instruction)
            .await?;

        let mut result = HarvestResult::default();
        let max = self.shared.cfg.knowledge.harvest_max as usize;
        for (id, doc) in knowledge::harvest::parse_harvest(&text)
            .into_iter()
            .take(max)
        {
            let fm = &doc.frontmatter;
            let put = Put {
                type_: fm.type_().unwrap_or_default(),
                title: fm.title().unwrap_or_else(|| id.clone()),
                description: fm.description(),
                tags: fm.tags(),
                scope: fm
                    .get("scope")
                    .and_then(okf::Value::as_display_string)
                    .map(PathBuf::from),
                body: doc.body,
                id,
            };
            match knowledge.put(put, Source::Harvest).await {
                Ok((_, true)) => result.added += 1,
                Ok((_, false)) => result.updated += 1,
                Err(e) => {
                    result.skipped += 1;
                    tracing::warn!(session = %session, "harvest put: {e:#}");
                }
            }
        }
        tracing::info!(
            session = %session,
            epoch = self.state.epoch,
            added = result.added,
            updated = result.updated,
            skipped = result.skipped,
            "harvest"
        );
        self.publish(AgentEvent::Harvested {
            session,
            added: result.added,
            updated: result.updated,
        })
        .await;
        Ok(result)
    }
}

/// Text parts of a message joined, for history rendering.
pub(crate) fn text_of(content: &MessageContent) -> String {
    content.texts().join("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills;

    #[test]
    fn hint_query_keeps_steering_and_tool_args_only() {
        let call = ToolCall {
            call_id: "c1".into(),
            fn_name: "read_file".into(),
            fn_arguments: serde_json::json!({ "path": "crates/signal/src/channel.rs" }),
            thought_signatures: None,
        };
        let history = vec![
            (1, ChatMessage::user("fix the signal channel")),
            (2, ChatMessage::user("<todo>\n- [ ] item\n</todo>")),
            (
                3,
                ChatMessage::user("  <message from=\"peer\">hello</message>"),
            ),
            (4, ChatMessage::assistant("Reading the channel now.")),
            (5, ChatMessage::from(vec![call])),
            (
                6,
                ChatMessage::from(vec![ToolResponse::new("c1", "fn secret_body() {}")]),
            ),
        ];
        let q = hint_query(&history);
        assert!(q.contains("fix the signal channel"));
        assert!(q.contains("Reading the channel now."));
        assert!(q.contains("crates/signal/src/channel.rs"));
        assert!(!q.contains("item"), "wrapped note leaked: {q}");
        assert!(!q.contains("hello"), "wrapped message leaked: {q}");
        assert!(!q.contains("secret_body"), "tool result leaked: {q}");
    }

    #[test]
    fn prompt_parts_in_order_with_blank_lines() {
        let data_dir = std::env::temp_dir().join(format!("wth-prompt-{}", uuid::Uuid::new_v4()));
        let skills_dir = skills::dir(&data_dir);
        std::fs::create_dir_all(&skills_dir).unwrap();
        let bare = frame::baseline(&data_dir, "", false);
        assert!(bare.starts_with("You are a fast"));
        assert!(!bare.contains("## Skills"));

        std::fs::write(data_dir.join("prompt.md"), "Standing rule.\n\n").unwrap();
        std::fs::write(
            skills_dir.join("rules.md"),
            "---\nalways: true\n---\nAlways rule.\n",
        )
        .unwrap();
        std::fs::write(skills_dir.join("later.md"), "Listed skill\n").unwrap();
        let full = frame::baseline(&data_dir, "## Peers\npeer text", false);
        std::fs::remove_dir_all(&data_dir).unwrap();

        let pos = |needle: &str| {
            full.find(needle)
                .unwrap_or_else(|| panic!("{needle:?} in {full}"))
        };
        assert!(pos("Date: ") < pos("\n\nStanding rule.\n\nAlways rule.\n\n## Skills\n"));
        assert!(
            pos("- later: Listed skill\n\n## Peers\npeer text\n\n## Guidance") > pos("## Skills")
        );
        assert!(!full.contains("rules"));
    }
}
