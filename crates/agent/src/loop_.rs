//! One session's model loop: frame, stream, dispatch tool calls, steer,
//! compact, cancel (design docs 07, 08).
//!
//! The [`Session`] actor owns the [`SessionState`] between turns; a running
//! turn owns it inside a spawned task and hands it back via [`TurnDone`], so
//! the actor's mailbox never blocks on network I/O.

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::future::join_all;
use futures::{Stream, StreamExt};
use genai::Client;
use genai::chat::{
    ChatMessage, ChatStreamEvent, MessageContent, StreamEnd, Tool, ToolCall, ToolResponse,
};
use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use models::Resolved;
use monty_pool::Pool;
use runtime::{Runtime, SessionId, TurnId};
use tokio_util::sync::CancellationToken;
use tools::{Invoke, ListTools, REGISTRY_ACTOR, ToolOutput, ToolRegistry, ToolSpec};

use crate::frame::{self, Frame, Transport};
use crate::peers::SpawnSubagentTool;
use crate::repl::{self, Repl, RunCx};
use crate::store::Store;
use crate::supervisor::{Auth, PeerContext, Roster};
use crate::{AGENT_ACTOR, AgentConfig, AgentEvent, Agents, Usage, compact, spill, todo};

/// Characters of tool output shown in [`AgentEvent::ToolCallFinished`].
const DISPLAY_OUTPUT_CHARS: usize = 2000;
/// A cache hit is only expected within the provider's TTL (07 §7).
const CACHE_TTL: Duration = Duration::from_secs(5 * 60);
/// Below this prompt size `cached == 0` is legitimate (07 §7).
const CACHE_FLOOR: u32 = 4096;

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
pub struct CompactNow;

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
    store: Store,
    client: Client,
    auth: Auth,
    rt: Runtime,
    steer: Arc<Mutex<Vec<String>>>,
    catalog: Arc<models::Catalog>,
    /// Python runtime pool; `None` when `monty` is absent (08 §9).
    pool: Option<Arc<Pool>>,
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
        store: Store,
        client: Client,
        auth: Auth,
        rt: Runtime,
        catalog: Arc<models::Catalog>,
        pool: Option<Arc<Pool>>,
    ) -> Self {
        let titled = !state.history.is_empty() || state.head.is_some();
        Self {
            shared: Shared {
                id,
                depth,
                model: Arc::new(Mutex::new(model)),
                cfg,
                store,
                client,
                auth,
                spill_dir: rt.data_dir.join("spill"),
                rt,
                steer: Arc::default(),
                catalog,
                pool,
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
    async fn handle(&mut self, _: CompactNow, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
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
        tokio::spawn(async move {
            let result = turn.compact_now().await;
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
        }
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
    async fn rebase(&mut self, registry: &ActorRef<ToolRegistry>) {
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
        for text in inputs {
            self.persist(ChatMessage::user(text)).await;
        }
        let registry = ActorRef::<ToolRegistry>::lookup(REGISTRY_ACTOR)
            .ok()
            .flatten();
        let stop_reason = match registry {
            None => {
                self.error("tool registry not running".into()).await;
                "error".to_string()
            }
            Some(registry) => {
                self.rebase(&registry).await;
                self.state.todo_touched_round = 0;
                let mut rounds = 0u32;
                loop {
                    match self.round(&registry, turn, rounds).await {
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
                if let Err(e) = self.compact(&model, &tools, transport, projected).await {
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
                    match self.compact(&model, &tools, transport, projected).await {
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
            return ToolOutput::err("python runtime is off (monty worker not found)");
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
                    cwd: std::env::current_dir().unwrap_or_default(),
                };
                Repl::open(pool, self.shared.id, &self.shared.rt.data_dir, cx).await
            }
        };
        self.repl.insert(repl).run(&call.call_id, &code).await
    }

    /// `/compact` between turns: same baseline/tools/transport as a turn, then [`Turn::compact`].
    async fn compact_now(&mut self) -> Result<u32, String> {
        let (model, error) = self.model();
        if let Some(e) = error {
            return Err(format!("model: {e}"));
        }
        let registry = ActorRef::<ToolRegistry>::lookup(REGISTRY_ACTOR)
            .ok()
            .flatten()
            .ok_or("tool registry not running")?;
        self.rebase(&registry).await;
        let tools = self.tools(&registry).await;
        let transport = self.transport(&model).await?;
        let projected = self.state.used;
        self.compact(&model, &tools, transport, projected).await
    }

    /// Compaction (07 §8.2–§8.4): cut, summarise as a cache-hot continuation
    /// of the live frame, open a new epoch. `projected` is reported in `Compacted.before`.
    async fn compact(
        &mut self,
        model: &Resolved,
        tools: &[Tool],
        transport: Transport,
        projected: u32,
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
        let session = self.shared.id;
        self.publish(AgentEvent::Compacting {
            session,
            turn: self.turn,
        })
        .await;
        full.push(ChatMessage::user(compact::COMPACT_INSTRUCTION));
        let summary = self
            .summarise(model, tools, transport, &mut full, keep_from)
            .await?;

        // 07 §8.4: harness truth the summariser cannot drift — roster, todos, runtime inventory.
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
        let mut text = format!("<context_summary epoch=\"{n}\">\n{}", summary.trim());
        for part in [
            Some(roster).filter(|r| !r.is_empty()),
            todos,
            runtime.map(|r| format!("Runtime: {r}")),
        ]
        .into_iter()
        .flatten()
        {
            text.push('\n');
            text.push_str(&part);
        }
        text.push_str("\n</context_summary>");
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
        self.reset_this_round = true;
        self.since = self.state.history.len();
        tracing::info!(session = %session, epoch = n, folded = keep_from, before = projected, "compaction");
        self.publish(AgentEvent::Compacted {
            session,
            turn: self.turn,
            epoch: n,
            before: projected,
            folded: keep_from as u32,
        })
        .await;
        Ok(n)
    }

    /// The summariser call (07 §8.3, §8.5): `full` is the history plus the
    /// instruction as its last message. Retries once on a text-less reply
    /// (`Text only.` prefix) and once on overflow (kept tail removed).
    async fn summarise(
        &self,
        model: &Resolved,
        tools: &[Tool],
        transport: Transport,
        full: &mut Vec<ChatMessage>,
        keep_from: usize,
    ) -> Result<String, String> {
        let mut text_only = false;
        let mut trimmed = false;
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
                        return Err("summariser returned no text twice".into());
                    }
                    text_only = true;
                    tracing::info!(session = %self.shared.id, "summariser replied without text; retrying");
                }
                Err(Abort::Cancelled) => return Err("cancelled".into()),
                Err(Abort::Model(e)) if compact::is_overflow(&e) && !trimmed => {
                    trimmed = true;
                    tracing::info!(session = %self.shared.id, "summariser overflowed; retrying without the kept tail");
                    full.truncate(keep_from);
                    full.push(ChatMessage::user(compact::COMPACT_INSTRUCTION));
                }
                Err(Abort::Model(e)) => return Err(format!("summariser: {e}")),
            }
            if text_only && let Some(last) = full.last_mut() {
                *last = ChatMessage::user(format!("Text only. {}", compact::COMPACT_INSTRUCTION));
            }
        }
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
