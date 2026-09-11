//! One session's model loop: frame, stream, dispatch tool calls, steer,
//! compact, cancel, undo (design docs 07, 08).
//!
//! The [`Session`] actor owns the [`SessionState`] between turns; a running
//! turn owns it inside a spawned task and hands it back via [`TurnDone`], so
//! the actor's mailbox never blocks on network I/O.
//!
//! Input arriving mid-turn queues in `Shared::steer` and is injected after the
//! tool batch; a caller-minted [`InputId`] rides along so the UI can follow it
//! (`InputQueued` → `InputInjected` | `InputWithdrawn`). Harness inputs
//! (`<subagent_result>`, `<message>`) carry no id and publish nothing.
//! [`UndoTurn`]/[`RedoTurn`] move the store's cut between turns; the first
//! write afterwards (`Session::claim`) commits it.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures::StreamExt;
use futures::future::join_all;
use genai::Client;
use genai::chat::{
    ChatMessage, ChatRole, ChatStreamEvent, ContentPart, MessageContent, ReasoningEffort,
    StreamEnd, Tool, ToolCall, ToolResponse,
};
use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use knowledge::{HarvestResult, KNOWLEDGE_ACTOR, Knowledge, KnowledgeActor, Put, Source};
use models::Resolved;
use monty_pool::Pool;
use runtime::{InputId, Runtime, SessionId, TurnId};
use tokio_util::sync::CancellationToken;
use tools::{Invoke, ListPrompts, ListTools, REGISTRY_ACTOR, ToolOutput, ToolRegistry, ToolSpec};

use crate::backend::Backend;
use crate::frame::{self, Frame, Transport};
use crate::peers::SpawnSubagentTool;
use crate::repl::{self, Repl, RunCx};
use crate::snap;
use crate::store::Store;
use crate::supervisor::{Auth, PeerContext, Roster};
use crate::{
    AGENT_ACTOR, AgentConfig, AgentEvent, Agents, CompactStyle, QueuedInput, Usage, compact, spill,
    todo,
};

/// Characters of tool output shown in [`AgentEvent::ToolCallFinished`].
const DISPLAY_OUTPUT_CHARS: usize = 2000;
/// A cache hit is only expected within the provider's TTL (07 §7).
const CACHE_TTL: Duration = Duration::from_secs(5 * 60);
/// Below this prompt size `cached == 0` is legitimate (07 §7).
const CACHE_FLOOR: u32 = 4096;

/// `tell`: user text. Starts a turn when idle, otherwise queued and injected
/// after the current tool batch (steering). `id` is the caller's
/// [`InputId`]; `None` for harness inputs, which publish no `Input*` events.
pub struct UserInput {
    /// Caller-minted id, echoed by the `Input*` events.
    pub id: Option<InputId>,
    /// User text.
    pub text: String,
}

/// One queued input: the caller's id (`None` for harness inputs) and its text.
type Queued = (Option<InputId>, String);

/// `ask` → `Result<String, String>`: remove a queued input; `Ok(text)`, or
/// `Err("not queued")` when it already entered the context.
pub struct WithdrawInput(pub InputId);

/// `ask` → `Vec<QueuedInput>`: queued inputs that carry an id.
pub struct QueuedInputs;

/// `ask` → `Result<String, String>`: hide the last turn; `Ok(first line)`.
/// `Err("turn in flight" | "nothing to undo")`.
pub struct UndoTurn;

/// `ask` → `Result<String, String>`: restore the last hidden turn; `Ok(first line)`.
/// `Err("turn in flight" | "nothing to redo")`.
pub struct RedoTurn;

/// `tell`: abort the in-flight turn, if any.
pub struct CancelTurn;

/// `ask` → whether a turn is in flight.
pub struct IsRunning;

/// `tell`: switch the model; takes effect on the next model call, mid-turn included.
pub struct SwitchModel(pub Resolved);

/// `tell`: reasoning effort for the session's next turn (`None` = provider
/// default). Not applied mid-turn: Anthropic rejects thinking changes inside a
/// tool-use turn.
pub struct SetPendingEffort(pub Option<ReasoningEffort>);

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
    /// Effort set while a turn ran (or between turns); applied when the next turn starts.
    pending_effort: Option<Option<ReasoningEffort>>,
}

/// Turn-invariant state; cheap to clone.
#[derive(Clone)]
struct Shared {
    id: SessionId,
    depth: u8,
    /// What the session streams with; shared with the running turn so
    /// [`SwitchModel`] applies mid-turn.
    model: Arc<Mutex<Current>>,
    cfg: Arc<AgentConfig>,
    /// Live default style; shared with the supervisor so [`crate::SetCompactStyle`] is in-process.
    compact_style: Arc<RwLock<CompactStyle>>,
    store: Store,
    client: Client,
    auth: Auth,
    rt: Runtime,
    /// Inputs waiting for the tool batch to end.
    steer: Arc<Mutex<Vec<Queued>>>,
    catalog: Arc<models::Catalog>,
    /// Python runtime pool; `None` when the pool failed to start (08 §9).
    pool: Option<Arc<Pool>>,
    /// Knowledge store, looked up per turn in [`Turn::rebase`]; `None` when the
    /// plugin is disabled or not started (12 §9.2).
    knowledge: Option<Arc<Knowledge>>,
    /// `<data_dir>/spill` (07 §6).
    spill_dir: PathBuf,
}

/// The model, what streams it (12 §6) and the resolution error of the
/// configured id, if any; an error makes the next round stop instead of
/// calling the model. Set by [`Session::new`] and [`SwitchModel`].
#[derive(Clone)]
struct Current {
    model: Resolved,
    backend: Backend,
    error: Option<String>,
    /// Reasoning effort sent with every call; `None` = provider default.
    effort: Option<ReasoningEffort>,
}

impl Current {
    fn new(model: Resolved, error: Option<String>, client: &Client, auth: &Auth) -> Self {
        Self {
            backend: Backend::for_model(&model, client, auth),
            model,
            error,
            effort: None,
        }
    }
}

impl Session {
    /// Build a session around `state` (`SessionState::resume(0, None, vec![])`
    /// for a new one). `depth` ≥ `subagents.max_depth` hides `spawn_subagent`.
    /// `model` is the resolved id and, when resolution failed, the message to surface.
    /// `effort` is the reasoning effort the first turn runs with.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        id: SessionId,
        depth: u8,
        model: (Resolved, Option<String>),
        effort: Option<ReasoningEffort>,
        state: SessionState,
        cfg: Arc<AgentConfig>,
        compact_style: Arc<RwLock<CompactStyle>>,
        store: Store,
        client: Client,
        auth: Auth,
        rt: Runtime,
        catalog: Arc<models::Catalog>,
        pool: Option<Arc<Pool>>,
    ) -> Self {
        let titled = !state.history.is_empty() || state.head.is_some();
        let (model, error) = model;
        let mut current = Current::new(model, error, &client, &auth);
        current.effort = effort;
        Self {
            shared: Shared {
                id,
                depth,
                model: Arc::new(Mutex::new(current)),
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
                knowledge: None,
            },
            state: Some(state),
            inflight: None,
            titled,
            pending_effort: None,
        }
    }

    /// The idle state, with any pending undo cut committed first: this is the
    /// first write after an undo, so the hidden tail is deleted for real.
    /// `None` while a turn (or compaction/harvest) owns the state.
    async fn claim(&mut self) -> Option<SessionState> {
        let state = self.state.take()?;
        if let Err(e) = self.shared.store.commit_cut(self.shared.id).await {
            tracing::warn!(session = %self.shared.id, "commit_cut: {e:#}");
        }
        Some(state)
    }

    /// [`UndoTurn`]/[`RedoTurn`]: move the store's cut, reload the frame from
    /// the visible rows, and return the first line of the moved turn's user text.
    async fn cut(&mut self, undo: bool) -> Result<String, String> {
        if self.inflight.is_some() || self.state.is_none() {
            return Err("turn in flight".into());
        }
        let id = self.shared.id;
        let store = &self.shared.store;
        let (moved, empty) = if undo {
            (store.undo(id).await, "nothing to undo")
        } else {
            (store.redo(id).await, "nothing to redo")
        };
        let text = moved.map_err(|e| e.to_string())?.ok_or(empty)?;
        let (epoch, head, history) = store.frame(id).await.map_err(|e| e.to_string())?;
        self.state = Some(SessionState::resume(epoch, head, history));
        Ok(text.lines().next().unwrap_or("").chars().take(80).collect())
    }

    async fn start_turn(&mut self, inputs: Vec<Queued>, me: &ActorRef<Self>) {
        let Some(state) = self.claim().await else {
            // Only reachable if a turn is already running; the caller checked `inflight`.
            return;
        };
        let turn = TurnId::new();
        let cancel = self.shared.rt.shutdown.child_token();
        self.inflight = Some(cancel.clone());
        if !self.titled {
            self.titled = true;
            if let Some((_, first)) = inputs.first()
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
        let model = {
            let mut current = self.shared.model.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(e) = self.pending_effort.take() {
                current.effort = e;
            }
            current.model.id.clone()
        };
        // Identified inputs resolve before `TurnStarted` so a UI already
        // showing them skips the echo.
        for id in inputs.iter().filter_map(|(id, _)| *id) {
            self.shared
                .rt
                .publish(AgentEvent::InputInjected {
                    session: self.shared.id,
                    id,
                    turn,
                })
                .await;
        }
        let inputs: Vec<String> = inputs.into_iter().map(|(_, text)| text).collect();
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

    async fn handle(
        &mut self,
        UserInput { id, text }: UserInput,
        ctx: &mut Context<Self, Self::Reply>,
    ) {
        if self.inflight.is_none() {
            let me = ctx.actor_ref().clone();
            return self.start_turn(vec![(id, text)], &me).await;
        }
        let event = id.map(|id| AgentEvent::InputQueued {
            session: self.shared.id,
            id,
            text: text.clone(),
        });
        self.shared
            .steer
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push((id, text));
        if let Some(event) = event {
            self.shared.rt.publish(event).await;
        }
    }
}

impl Message<WithdrawInput> for Session {
    type Reply = Result<String, String>;

    async fn handle(
        &mut self,
        WithdrawInput(id): WithdrawInput,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<String, String> {
        let removed = {
            let mut queue = self.shared.steer.lock().unwrap_or_else(|p| p.into_inner());
            let at = queue.iter().position(|(q, _)| *q == Some(id));
            at.map(|i| queue.remove(i).1)
        };
        let Some(text) = removed else {
            return Err("not queued".into());
        };
        self.shared
            .rt
            .publish(AgentEvent::InputWithdrawn {
                session: self.shared.id,
                id,
            })
            .await;
        Ok(text)
    }
}

impl Message<QueuedInputs> for Session {
    type Reply = Vec<QueuedInput>;

    async fn handle(&mut self, _: QueuedInputs, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.shared
            .steer
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .filter_map(|(id, text)| {
                Some(QueuedInput {
                    id: (*id)?,
                    text: text.clone(),
                })
            })
            .collect()
    }
}

impl Message<UndoTurn> for Session {
    type Reply = Result<String, String>;

    async fn handle(&mut self, _: UndoTurn, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.cut(true).await
    }
}

impl Message<RedoTurn> for Session {
    type Reply = Result<String, String>;

    async fn handle(&mut self, _: RedoTurn, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.cut(false).await
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
        let mut current = self.shared.model.lock().unwrap_or_else(|p| p.into_inner());
        let effort = current.effort.take();
        *current = Current::new(model, None, &self.shared.client, &self.shared.auth);
        current.effort = effort;
    }
}

impl Message<SetPendingEffort> for Session {
    type Reply = ();

    async fn handle(
        &mut self,
        SetPendingEffort(effort): SetPendingEffort,
        _: &mut Context<Self, Self::Reply>,
    ) {
        self.pending_effort = Some(effort);
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
        let Some(state) = self.claim().await.filter(|_| self.inflight.is_none()) else {
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
        let Some(state) = self.claim().await.filter(|_| self.inflight.is_none()) else {
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
    /// This turn's direct tools, fetched once in [`Turn::rebase`] (07 §5, 12 §4).
    tools: Vec<Tool>,
    /// A rebase or compaction happened since the last call (07 §7).
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
pub(crate) fn ns(genai: &str) -> &str {
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
            tools: Vec::new(),
            reset_this_round: false,
            overflowed: false,
            window_logged: false,
            repl: None,
            input_text: String::new(),
            calls: 0,
            cwd: std::env::current_dir().unwrap_or_default(),
        }
    }

    /// The store when the knowledge plugin is up (12 §9.2).
    fn knowledge(&self) -> Option<Arc<Knowledge>> {
        self.shared.knowledge.clone()
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

    fn model(&self) -> Current {
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

    /// Turn start: render the system prompt and log a rebase when it changed
    /// (07 §4); fetch the tool list once for the whole turn (07 §5, 12 §4).
    /// Also looks up the knowledge store (12 §9.2) and resyncs its index with
    /// the bundle's files (10 §3.2).
    async fn rebase(&mut self, registry: &ActorRef<ToolRegistry>) {
        self.shared.knowledge = match ActorRef::<KnowledgeActor>::lookup(KNOWLEDGE_ACTOR) {
            Ok(Some(a)) => a.ask(knowledge::Handle).await.ok(),
            _ => None,
        };
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
        let specs = match registry.ask(ListTools).await {
            Ok(specs) => specs,
            Err(e) => {
                self.error(format!("list tools: {e}")).await;
                Vec::new()
            }
        };
        let blocks = registry
            .ask(ListPrompts)
            .await
            .unwrap_or_default()
            .join("\n\n");
        let genai = self.model().model.genai;
        let claude_code = ns(&genai) == "anthropic" && self.shared.auth.login("anthropic");
        let mut baseline =
            frame::baseline(&self.shared.rt.data_dir, &peer_line, &blocks, claude_code);
        if self.shared.pool.is_some() {
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
        self.tools = self.direct_tools(specs);
    }

    /// Cache marks for `current`'s provider; resolved once per model per turn (07 §3).
    async fn transport(&mut self, current: &Current) -> Result<Transport, String> {
        if let Some((id, t)) = &self.transport
            && *id == current.model.genai
        {
            return Ok(*t);
        }
        let t = current.backend.transport(&current.model).await?;
        self.transport = Some((current.model.genai.clone(), t));
        Ok(t)
    }

    /// This turn's direct tools (07 §5, 08 §0.4, 12 §4), from one `ListTools`
    /// per turn: registry tools (only `direct` ones when the runtime is on,
    /// `spawn_subagent` gated by depth) plus the loop-local `todo` and `python`.
    /// A tool registered mid-turn is visible on the next turn.
    fn direct_tools(&self, specs: Vec<ToolSpec>) -> Vec<Tool> {
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
        specs
            .into_iter()
            .map(|s| {
                Tool::new(s.name)
                    .with_description(s.description)
                    .with_schema(s.schema)
            })
            .collect()
    }

    /// Open the stream for `frame` and drain it to its `End`. With `live`,
    /// deltas and pending tool calls are published; the summariser passes `None`.
    async fn call(
        &self,
        current: &Current,
        frame: Frame<'_>,
        transport: Transport,
        live: Option<TurnId>,
    ) -> Result<StreamEnd, Abort> {
        let (req, opts) = frame.request(transport, self.shared.id, current.effort.clone());
        let open = current
            .backend
            .stream(&current.model, req, &opts, self.shared.id);
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
                            for (id, text) in queued {
                                self.persist(ChatMessage::user(text)).await;
                                if let Some(id) = id {
                                    self.publish(AgentEvent::InputInjected {
                                        session: self.shared.id,
                                        id,
                                        turn,
                                    })
                                    .await;
                                }
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
        if harvestable && registry.is_some() && self.knowledge().is_some() {
            self.state.turns_since_harvest += 1;
            if every > 0 && self.state.turns_since_harvest >= every {
                self.state.turns_since_harvest = 0;
                self.publish(AgentEvent::Harvesting {
                    session: self.shared.id,
                })
                .await;
                if let Err(e) = self.harvest_with().await {
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
        let current = self.model();
        if let Some(e) = &current.error {
            self.error(format!("model: {e}")).await;
            return Round::Stop("error".into());
        }
        let transport = match self.transport(&current).await {
            Ok(t) => t,
            Err(e) => {
                self.error(format!("model: {e}")).await;
                return Round::Stop("error".into());
            }
        };

        // 07 §8.1 trigger, §8.7 pressure note, §9 todo reminder — all before the call.
        let window = self.window(&current.model);
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
                        &current,
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
            tools: &self.tools,
            system: &self.state.baseline,
            head: self.state.head.as_ref(),
            history: &msgs,
        };
        let end = match self.call(&current, frame, transport, Some(turn)).await {
            Ok(end) => end,
            Err(Abort::Cancelled) => return Round::Stop("cancelled".into()),
            Err(Abort::Model(e)) => {
                // 07 §8.5: one forced compaction per turn, then retry the round.
                if compact::is_overflow(&e) && !self.overflowed {
                    self.overflowed = true;
                    match self
                        .compact(
                            &current,
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
                    "prefix cache miss (no rebase/compaction since the previous call)"
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
                .filter(|c| c.fn_name == knowledge::tools::GET)
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
        self.between_turns().await?;
        let (current, transport) = self.resolve().await?;
        let projected = self.state.used;
        self.compact(
            &current,
            transport,
            projected,
            style,
            compact::CompactReason::Manual,
        )
        .await
    }

    /// `/knowledge harvest` between turns (10 §5.2): rebase, then [`Turn::harvest`].
    async fn harvest_now(&mut self) -> Result<HarvestResult, String> {
        self.between_turns().await?;
        self.harvest_with().await
    }

    /// The mini-turn preamble shared by `/compact` and `/knowledge harvest`:
    /// a running registry, a fresh baseline and this turn's tools.
    async fn between_turns(&mut self) -> Result<(), String> {
        let registry = ActorRef::<ToolRegistry>::lookup(REGISTRY_ACTOR)
            .ok()
            .flatten()
            .ok_or("tool registry not running")?;
        self.rebase(&registry).await;
        Ok(())
    }

    /// What a model call needs besides the frame: the current model and
    /// backend (or the model's resolution error) and the provider's transport.
    async fn resolve(&mut self) -> Result<(Current, Transport), String> {
        let current = self.model();
        if let Some(e) = &current.error {
            return Err(format!("model: {e}"));
        }
        let transport = self.transport(&current).await?;
        Ok((current, transport))
    }

    /// [`Turn::resolve`] then [`Turn::harvest`].
    async fn harvest_with(&mut self) -> Result<HarvestResult, String> {
        let (current, transport) = self.resolve().await?;
        self.harvest(&current, transport).await
    }

    /// Compaction (07 §8.2–§8.4, 11): cut, build the head, open a new epoch.
    /// `projected` is reported in `Compacted.before`. Styles only change the head.
    async fn compact(
        &mut self,
        current: &Current,
        transport: Transport,
        projected: u32,
        r#override: Option<CompactStyle>,
        reason: compact::CompactReason,
    ) -> Result<u32, String> {
        let window = self.window(&current.model);
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
        // 13 §0.5: snap needs a model that takes images; the Cursor backend
        // serialises text parts only, so it is text-only whatever the model.
        let vision = matches!(current.backend, Backend::Genai { .. })
            && current.model.model.as_ref().is_some_and(|m| m.image);
        let mut style = compact::resolve(r#override, cfg_style, reason, vision)?;
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
            if let Err(e) = self.harvest(current, transport).await {
                tracing::warn!(session = %session, "harvest before compaction: {e}");
            }
        }
        let k = keep_from + usize::from(self.state.head.is_some());
        let cut_body = || format!("folded {k} messages; no summary. Recent turns follow verbatim.");
        // Snap: the body is the caption and `pngs` the frames it describes (13 §2.4).
        let mut pngs: Vec<Vec<u8>> = Vec::new();
        let body = match style {
            CompactStyle::Snap => {
                let adapter = current.model.provider.as_deref().unwrap_or_default();
                let shape = snap::shape(models::Catalog::adapter(adapter));
                let lines = snap::serialise(
                    self.state.head.as_ref(),
                    &full[..keep_from],
                    snap::cols(&shape),
                );
                let (lines, dropped) = snap::budget_lines(lines, limit / 8, &shape);
                pngs = snap::rasterize(&lines, &shape);
                if pngs.is_empty() {
                    // 13 §2.2: nothing printable → degrade to cut.
                    style = CompactStyle::Cut;
                    cut_body()
                } else {
                    let f = pngs.len();
                    let dropped = if dropped > 0 {
                        format!("; the earliest {dropped} lines were dropped")
                    } else {
                        String::new()
                    };
                    format!(
                        "Archive of {k} folded messages as {f} image(s), most recent last{dropped}. \
                         Read them if you need earlier facts. Recent turns follow verbatim."
                    )
                }
            }
            CompactStyle::Cut => cut_body(),
            CompactStyle::Summary => {
                full.push(ChatMessage::user(compact::COMPACT_INSTRUCTION));
                match self
                    .summarise(current, transport, &mut full, keep_from)
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
        let harness = [
            roster,
            todos.unwrap_or_default(),
            runtime.map(|r| format!("Runtime: {r}")).unwrap_or_default(),
            read.unwrap_or_default(),
        ];
        let frames = pngs.len();
        let head = if pngs.is_empty() {
            ChatMessage::user(compact::wrap_head(n, style, body.trim(), harness))
        } else {
            // 13 §2.4: open tag + caption, one PNG part per frame, closing tag.
            let mut parts = Vec::with_capacity(frames + 2);
            parts.push(ContentPart::Text(compact::wrap_head_open(
                n,
                style,
                body.trim(),
                harness,
            )));
            parts.extend(pngs.into_iter().enumerate().map(|(i, png)| {
                ContentPart::from_binary_base64(
                    "image/png",
                    BASE64.encode(png),
                    Some(format!("frame-{i}.png")),
                )
            }));
            parts.push(ContentPart::Text(compact::HEAD_CLOSE.into()));
            ChatMessage::user(MessageContent::from_parts(parts))
        };
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
        tracing::info!(session = %session, epoch = n, folded = keep_from, frames, before = projected, style = %style, "compaction");
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
        current: &Current,
        transport: Transport,
        full: &mut Vec<ChatMessage>,
        keep_from: usize,
    ) -> Result<String, String> {
        let mut trimmed = false;
        loop {
            match self
                .continuation(current, transport, full, compact::COMPACT_INSTRUCTION)
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
        current: &Current,
        transport: Transport,
        full: &mut Vec<ChatMessage>,
        instruction: &str,
    ) -> Result<String, String> {
        let mut text_only = false;
        loop {
            let frame = Frame {
                tools: &self.tools,
                system: &self.state.baseline,
                head: self.state.head.as_ref(),
                history: full.as_slice(),
            };
            match self.call(current, frame, transport, None).await {
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
        current: &Current,
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
            .continuation(current, transport, &mut full, &instruction)
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
                    .and_then(|v| v.as_display_string())
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
        let bare = frame::baseline(&data_dir, "", "", false);
        assert!(bare.starts_with("You are a fast"));
        assert!(!bare.contains("## Skills"));

        std::fs::write(data_dir.join("prompt.md"), "Standing rule.\n\n").unwrap();
        std::fs::write(
            skills_dir.join("rules.md"),
            "---\nalways: true\n---\nAlways rule.\n",
        )
        .unwrap();
        std::fs::write(skills_dir.join("later.md"), "Listed skill\n").unwrap();
        let full = frame::baseline(&data_dir, "## Peers\npeer text", "Plugin block.", false);
        std::fs::remove_dir_all(&data_dir).unwrap();

        let pos = |needle: &str| {
            full.find(needle)
                .unwrap_or_else(|| panic!("{needle:?} in {full}"))
        };
        assert!(pos("Date: ") < pos("\n\nStanding rule.\n\nAlways rule.\n\n## Skills\n"));
        assert!(
            pos("- later: Listed skill\n\n## Peers\npeer text\n\nPlugin block.\n\n## Guidance")
                > pos("## Skills")
        );
        assert!(!full.contains("rules"));
    }
}
