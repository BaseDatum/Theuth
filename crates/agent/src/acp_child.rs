//! [`AcpChild`]: one external harness — a process from `agents.yaml` or a
//! dialled node (32 §6.5) — bound to one theuth session (design doc 06 §3.2–§3.5,
//! §8.7). It answers the same [`UserInput`], [`CancelTurn`] and [`IsRunning`]
//! messages as a native `Session`, so the supervisor, roster, approvals and
//! `<subagent_result>` delivery are unchanged; every `session/update` becomes
//! an [`AgentEvent`] on the theuth session id (§3.3), every
//! `session/request_permission` goes through the registry's one approval gate
//! (§3.4, §8.8). Turn claims, input consumption and returns go to the
//! supervisor's continuation ledger (47 §4.3); the harness's plan is the
//! session's persistent todo snapshot (47 §5.3).

use std::collections::VecDeque;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use acp::{Connect, Harness, Permission, Transport, Update};
use agent_client_protocol::schema::v1::{
    AvailableCommand, AvailableCommandInput, PermissionOptionKind, Plan, PlanEntryStatus,
    RequestPermissionOutcome, SelectedPermissionOutcome, SessionUpdate, ToolCallStatus,
    Usage as AcpUsage, UsageUpdate,
};
use genai::chat::{ChatMessage, MessageContent, ToolCall, ToolResponse};
use kameo::Actor;
use kameo::actor::{ActorRef, WeakActorRef};
use kameo::error::ActorStopReason;
use kameo::message::{Context, Message};
use runtime::{InputId, Runtime, SessionId, TurnId};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tokio_util::sync::CancellationToken;
use tools::{Decision, ExternalApproval, REGISTRY_ACTOR, ToolRegistry};
use uuid::Uuid;

use crate::communication::{PeerConsumed, PeerTurnFinished};
use crate::continuation::{
    self, InputConsumed, InputDropped, InputTicket, SessionUnknown, TurnEntered, TurnReturned,
};
use crate::cost::{MeterUpdate, Money, TokenTotals};
use crate::durability::handoff;
use crate::loop_::{
    CancelPeerScope, CancelTurn, DISPLAY_OUTPUT_CHARS, IsRunning, PEER_INTAKE, PeerInput, PeerMeta,
    SuppressPeerInputs, UserInput, peer_wrapper, refuse_peer,
};
use crate::store::Store;
use crate::store::communication::CommError;
use crate::supervisor::Deliver;
use crate::todo::{Status, Todo, TodoSource};
use crate::{AGENT_ACTOR, AgentEvent, Agents, HarnessCommand, Usage, attach};

/// Grace after `session/cancel` before the process is killed (06 §3.5).
const CANCEL_GRACE: Duration = Duration::from_secs(5);
/// Longest a harness may take to connect — `initialize` + `session/new` or
/// `session/load` — before the child errors (R2.1); a hung `omp acp` must not
/// hold its permit forever.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(120);
/// Longest a busy-refused prompt keeps retrying (R2.5), whatever the request
/// deadline: after this the busy refusal is the turn's error.
const BUSY_RETRY: Duration = Duration::from_secs(600);
/// Stored result of a tool call the harness never reported finished (06 §3.6).
const NOT_FINISHED: &str =
    "ERROR: not finished: the turn ended before the harness reported this call";
/// Backoff between `session/prompt` attempts a busy harness refused (06 §3.5,
/// 2026-09-24): the last step repeats until the request deadline.
const RETRY: [Duration; 4] = [
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(20),
    Duration::from_secs(30),
];
/// A continuation turn (harness work after its prompt returned, 06 §3.5)
/// closes once no `session/update` arrived for this long.
pub(crate) const CONTINUATION_QUIET: Duration = Duration::from_secs(120);
/// `TurnFinished.stop_reason` of a continuation turn: the supervisor hands
/// nothing to the parent for it.
pub const HARNESS_CONTINUED: &str = "harness_continued";
/// `TurnStarted.text` of a continuation turn: a harness wrapper, so no UI
/// renders it as a user message.
const CONTINUED_NOTE: &str = "<runtime>harness still working after its turn ended</runtime>";

/// The actor. Spawn with `AcpChild::spawn(AcpChild::new(..))`; the process
/// starts in `on_start` and dies with the actor.
pub struct AcpChild {
    id: SessionId,
    /// `agents.yaml` key or node name; shown as the turn's model.
    name: String,
    store: Store,
    rt: Runtime,
    /// How to start the process; taken by `on_start`.
    connect: Option<Connect>,
    /// `Some` once `initialize` + `session/new|load` succeeded.
    harness: Option<Harness>,
    /// Why the connection failed; the next input is failed with it.
    dead: Option<String>,
    /// A dialled node rather than a child process (32 §6.5): losing the
    /// connection does not prove the remote stopped (47 §4.2 rule 6).
    remote: bool,
    /// A prompt was ever sent over this connection: on a remote, work that
    /// may outlive the prompt and this connection, so a disconnect is an
    /// unknown outcome even when idle. A never-prompted remote is clean.
    worked: bool,
    /// Inputs waiting for the connection or for the prompt in flight.
    queue: VecDeque<Queued>,
    /// The `session/prompt` in flight, or a continuation turn (06 §3.5).
    turn: Option<Turn>,
    /// `Agents::request_deadline`: how long a busy-refused prompt keeps retrying,
    /// capped by `busy` ([`BUSY_RETRY`]).
    deadline: Duration,
    /// [`RETRY`], [`CONTINUATION_QUIET`], [`BUSY_RETRY`], [`CONNECT_TIMEOUT`]
    /// and [`CANCEL_GRACE`]; tests shorten them.
    retry: [Duration; 4],
    quiet: Duration,
    busy: Duration,
    connect_timeout: Duration,
    grace: Duration,
    /// Cancelled when the harness connection ends: permission asks outside a
    /// live turn wait on this, not on a turn.
    alive: CancellationToken,
    /// The row's title was checked/set.
    titled: bool,
    /// An unusable `usage_update.cost` was already logged (once per connection).
    meter_warned: bool,
    /// `agents.yaml` `model:` — shown when the harness reports no model (06 §8.2).
    fallback_model: Option<String>,
    /// The model last persisted and published for this session (06 §8.2):
    /// the harness's `configOptions` model select, else [`Self::fallback_model`].
    harness_model: Option<String>,
    /// The manifest last persisted and published (75 §3).
    harness_commands: Vec<HarnessCommand>,
    /// Last `usage_update.used` (75 §5): a session fact, arriving with or
    /// without a turn open; `TurnFinished.context`.
    context: u32,
    /// Last `usage_update.size` persisted and published; `None` until reported.
    context_size: Option<u32>,
    /// The last turn opened: what a turnless `ContextUsed` is attributed to.
    last_turn: Option<TurnId>,
    /// Skills written into the cwd for this child (75 §4); removed with the
    /// connection, whatever stopped it.
    skills: Option<crate::skills::Materialised>,
    /// `(key, value)` option changes asked for mid-turn (75 §8): sent at
    /// `PromptDone`, before the next queued input's prompt.
    pending_options: Vec<(String, String)>,
    /// Runtime notes for the harness, each sent as `<runtime>…</runtime>`
    /// ahead of the first prompt's text and never persisted (75 §10, §4).
    notes: Vec<String>,
    /// A pending [`Respawn`] (its token budget), acted on at `Connected`.
    respawn: Option<usize>,
}

/// One input waiting its turn: the caller's [`InputId`], the supervisor's
/// admission ticket (47 §4.3; consumed when the turn persists the text), the
/// text and, for a peer exchange, its durable identity (50 §7).
struct Queued {
    id: Option<InputId>,
    ticket: Option<InputTicket>,
    text: String,
    /// Owning A2A request scope, if any (50 §6.3).
    scope: Option<Uuid>,
    peer: Option<PeerMeta>,
    /// When this actor accepted the input: the busy-retry deadline counts from here.
    admitted: Instant,
}

/// Per-turn state the translation accumulates into (06 §3.3). Text and tool
/// calls are queued on `messages` as ordinary chat messages in arrival order
/// (06 §3.6); the actor persists the queue before publishing the events.
pub(crate) struct Turn {
    /// theuth session.
    session: SessionId,
    /// theuth turn.
    id: TurnId,
    /// Accumulated `agent_message_chunk` text: the turn's result for the parent.
    text: String,
    /// Byte length of `text` already queued as assistant messages.
    flushed: usize,
    /// Translated but not yet persisted messages, in order.
    messages: Vec<ChatMessage>,
    /// Calls started and not yet finished, in start order.
    open: Vec<String>,
    /// The last queued message is a tool response: the turn must still close
    /// with an assistant row so the next user message opens a turn.
    tool_last: bool,
    /// Last `tool_call_update` output: what the parent gets when the harness
    /// ends the turn without text (opencode after a rejected permission).
    last_tool: Option<String>,
    /// `PromptResponse.usage` of this turn's prompt (75 §7), mapped by
    /// [`tokens_of`]; `None` when the harness sent none or the turn never
    /// prompted (continuation, error).
    tokens: Option<TokenTotals>,
    /// Cancelled with the turn: pending permission asks answer `cancelled`.
    cancel: CancellationToken,
    /// Kill timer armed by [`CancelTurn`].
    kill: Option<AbortHandle>,
    /// Owning A2A request scope of the input that opened the turn (50 §6.3):
    /// the turn is that task's work and reports as such.
    scope: Option<Uuid>,
    /// The prompt text and its admission time, for a busy retry; `None` for
    /// a continuation turn.
    input: Option<(String, Instant)>,
    /// Busy refusals so far; indexes the retry schedule.
    retries: usize,
    /// The pending busy retry.
    retry: Option<AbortHandle>,
    /// Opened by harness updates after its prompt returned (06 §3.5): no
    /// parent hand-off, closed by [`CONTINUATION_QUIET`], the next prompt or
    /// the harness closing.
    continuation: bool,
    /// The quiet timer of a continuation turn.
    quiet: Option<AbortHandle>,
    /// 91: when the first `agent_thought_chunk` of the current thinking
    /// arrived; taken by the first message/tool update after it.
    thinking: Option<Instant>,
}

impl Turn {
    pub(crate) fn new(session: SessionId, id: TurnId, scope: Option<Uuid>) -> Self {
        Self {
            session,
            id,
            text: String::new(),
            flushed: 0,
            messages: Vec::new(),
            open: Vec::new(),
            tool_last: false,
            last_tool: None,
            tokens: None,
            cancel: CancellationToken::new(),
            kill: None,
            scope,
            input: None,
            retries: 0,
            retry: None,
            thinking: None,
            continuation: false,
            quiet: None,
        }
    }

    /// Abort every timer armed on this turn.
    fn disarm(&mut self) {
        for t in [self.kill.take(), self.retry.take(), self.quiet.take()]
            .into_iter()
            .flatten()
        {
            t.abort();
        }
    }

    fn assistant(&mut self, text: &str) {
        self.messages
            .push(ChatMessage::assistant(MessageContent::from_text(text)));
        self.tool_last = false;
    }

    /// Queue the text since the last flush as one assistant message.
    fn flush_text(&mut self) {
        if self.flushed < self.text.len() {
            let tail = self.text[self.flushed..].to_owned();
            self.assistant(&tail);
            self.flushed = self.text.len();
        }
    }

    /// A call started: the text before it keeps its place, then the call.
    fn call_started(&mut self, call_id: &str, name: &str, args: serde_json::Value) {
        self.flush_text();
        self.messages.push(ChatMessage::from(vec![ToolCall {
            call_id: call_id.to_owned(),
            fn_name: name.to_owned(),
            fn_arguments: args,
            thought_signatures: None,
        }]));
        self.tool_last = false;
        self.open.push(call_id.to_owned());
    }

    /// An open call finished: its response, capped like the live output and
    /// `ERROR: `-prefixed on failure so `history()` marks it. A finish for a
    /// call never started (or already finished) is not stored: it would be a
    /// dangling card.
    fn call_finished(&mut self, call_id: &str, output: &str, is_error: bool) {
        let Some(i) = self.open.iter().position(|c| c == call_id) else {
            return;
        };
        self.open.remove(i);
        let text = if is_error && !output.starts_with("ERROR: ") {
            format!("ERROR: {output}")
        } else {
            output.to_owned()
        };
        self.messages.push(ChatMessage::from(vec![ToolResponse::new(
            call_id,
            crate::frame::truncate(&text, DISPLAY_OUTPUT_CHARS),
        )]));
        self.tool_last = true;
    }

    /// The turn's result for the parent: its text, else `error`, else the
    /// last tool result, else `"(no text)"`.
    fn result(&self, error: Option<String>) -> String {
        if !self.text.is_empty() {
            return self.text.clone();
        }
        error
            .or_else(|| {
                self.last_tool
                    .as_ref()
                    .map(|t| format!("[no reply; last tool result]\n{t}"))
            })
            .unwrap_or_else(|| "(no text)".into())
    }

    /// End of turn: every open call is answered [`NOT_FINISHED`] (pair
    /// invariant), the trailing text is flushed (or `result` stored when no
    /// text arrived at all), and the turn ends on an assistant row.
    fn close(&mut self, result: &str) {
        while let Some(id) = self.open.first().cloned() {
            self.call_finished(&id, NOT_FINISHED, true);
        }
        if self.text.is_empty() {
            self.assistant(result);
        } else {
            self.flush_text();
        }
        if self.tool_last {
            self.assistant("");
        }
    }
}

/// `tell` (internal): `Harness::connect` returned.
struct Connected(Result<Harness, String>);

/// `tell` (internal): `session/prompt` returned with its stop reason and the
/// response's token usage, or failed.
struct PromptDone(Result<(String, Option<AcpUsage>), String>);

/// `tell` (internal): the cancel grace for turn `.0` elapsed.
struct KillTimer(TurnId);

/// `tell` (internal): the busy backoff for turn `.0` elapsed; prompt again.
struct Retry(TurnId);

/// `tell` (internal): continuation turn `.0` saw no update for [`CONTINUATION_QUIET`].
struct Quiet(TurnId);

/// `ask` → `Result<acp::SetOptionResult, String>` (75 §8): set the harness's
/// config option `config_id` (an id, or the alias `model` / `effort`) to
/// `value`. Idle → sent now; a turn in flight → queued and sent at its end
/// (`pending: true`). A missing option or an unlisted value is refused with
/// the list; nothing is applied silently.
#[derive(Clone, Debug)]
pub struct SetOption {
    /// Option id or alias.
    pub config_id: String,
    /// One of the option's `options[].value`.
    pub value: String,
}

/// `ask` → `Vec<acp::ConfigOption>`: the harness's config options as they stand.
#[derive(Clone, Copy, Debug, Default)]
pub struct HarnessOptions;

/// `tell` (75 §10): end the connection — killing a local harness process,
/// whether idle or still working on its own after a cancel (06 §8.3) — finish
/// an open turn `cancelled`, fail queued inputs and stop the actor. The
/// supervisor then respawns from the row with `session/load`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Shutdown;

/// `tell`: a runtime note for the harness — sent inside the first
/// `session/prompt` after it, before the user's text. Used after a resume
/// (75 §10): [`CANCELLED_NOTE`] when the replayed turn ended `cancelled`, and
/// one per stored skill that no longer exists (75 §4).
#[derive(Clone, Debug)]
pub struct Note(pub String);

/// The [`Note`] after a resume of a cancelled turn: `session/load` replays
/// the unfinished work and the harness would otherwise pick it up.
pub const CANCELLED_NOTE: &str = "your previous turn was cancelled by the user before it finished; do not continue it — answer only the message below";

/// `tell` (109 §7.3): this actor is the boot respawn of a child whose
/// parent's open turn waits on its result. Once connected, the child is
/// prompted with a first input the supervisor never typed: the short
/// [`handoff::restored_note`] when `session/load` restored the harness
/// context, else the delta handoff ([`handoff::compose_handoff`]) under
/// `.0` tokens. Its turn ends through the ordinary path, so the parent's
/// wait is answered exactly as a live child's result would be — as an
/// error result when the harness cannot even be connected.
#[derive(Clone, Copy, Debug)]
pub struct Respawn(pub usize);

/// Persisted user text of a respawn whose harness never connected (109
/// §7.3): the error result the parent gets needs a turn to ride on.
const RESPAWN_FAILED_INPUT: &str = "<runtime>respawn after a theuth restart</runtime>";

/// A `session/set_config_option` round trip is bounded so a hung harness
/// cannot hold the actor's mailbox.
// ponytail: the option request is awaited on the mailbox (one short round
// trip on an idle harness); move to a delegated reply if a harness is seen
// stalling it.
const OPTION_TIMEOUT: Duration = Duration::from_secs(10);

impl AcpChild {
    /// Bind session `id` to harness `name` reached over `transport`, prompting
    /// in `cwd`; `resume` is the stored ACP session id to `session/load`;
    /// `fallback_model` is the entry's `model:` label (06 §8.2); `meta` is
    /// what a remote theuth reads from `session/new` `_meta` and `model` /
    /// `effort` the picks a local harness switches to through its options
    /// (67 §8, 75 §8); `mcp_servers` goes into `session/new.mcpServers`
    /// (75 §9); `deadline` is how long a prompt the harness refuses as busy
    /// is retried.
    #[allow(clippy::too_many_arguments)] // one call site (the supervisor); a builder would only rename the fields
    pub fn new(
        id: SessionId,
        name: String,
        transport: Transport,
        cwd: PathBuf,
        resume: Option<String>,
        fallback_model: Option<String>,
        meta: Option<acp::SessionMeta>,
        model: Option<String>,
        effort: Option<String>,
        deadline: Duration,
        store: Store,
        rt: Runtime,
        skills: crate::skills::Materialised,
        mcp_servers: Vec<agent_client_protocol::schema::v1::McpServer>,
    ) -> Self {
        let remote = matches!(transport, Transport::Dial(_));
        let connect = Connect {
            transport,
            cwd,
            resume,
            stderr_log: rt.data_dir.join("logs").join(format!("acp-{name}.log")),
            meta,
            model,
            effort,
            mcp_servers,
        };
        Self {
            id,
            name,
            store,
            rt,
            connect: Some(connect),
            harness: None,
            dead: None,
            remote,
            worked: false,
            queue: VecDeque::new(),
            turn: None,
            deadline,
            retry: RETRY,
            quiet: CONTINUATION_QUIET,
            busy: BUSY_RETRY,
            connect_timeout: CONNECT_TIMEOUT,
            grace: CANCEL_GRACE,
            alive: CancellationToken::new(),
            titled: false,
            meter_warned: false,
            fallback_model,
            harness_model: None,
            harness_commands: Vec::new(),
            context: 0,
            context_size: None,
            last_turn: None,
            skills: Some(skills),
            pending_options: Vec::new(),
            notes: Vec::new(),
            respawn: None,
        }
    }

    async fn publish(&self, ev: AgentEvent) {
        self.rt.publish(ev).await;
    }

    /// 06 §8.2: `model` is what the harness runs on now. Persist and publish
    /// it once per distinct value; a failed write advertises nothing.
    async fn set_harness_model(&mut self, model: String) {
        if self.harness_model.as_deref() == Some(model.as_str()) {
            return;
        }
        if let Err(e) = self.store.set_harness_model(self.id, &model).await {
            tracing::warn!(session = %self.id, "set_harness_model: {e:#}");
            return;
        }
        self.publish(AgentEvent::HarnessModelChanged {
            session: self.id,
            model: model.clone(),
        })
        .await;
        self.harness_model = Some(model);
    }

    /// 75 §3: the harness's `available_commands_update`, the whole manifest
    /// each time. Persist and publish once per distinct value (omp re-sends
    /// on every metadata change, opencode once per prompt).
    async fn set_harness_commands(&mut self, commands: Vec<AvailableCommand>) {
        let commands: Vec<HarnessCommand> = commands
            .into_iter()
            .map(|c| HarnessCommand {
                name: c.name,
                description: c.description,
                hint: c.input.map(|i| match i {
                    AvailableCommandInput::Unstructured(u) => u.hint,
                    _ => String::new(),
                }),
            })
            .collect();
        if self.harness_commands == commands {
            return;
        }
        if let Err(e) = self.store.set_harness_commands(self.id, &commands).await {
            tracing::warn!(session = %self.id, "set_harness_commands: {e:#}");
            return;
        }
        self.publish(AgentEvent::HarnessCommandsChanged {
            session: self.id,
            commands: commands.clone(),
        })
        .await;
        self.harness_commands = commands;
    }

    /// 75 §5: one `usage_update` — session facts, so handled with or without
    /// a turn open (opencode's may land after the prompt response). `size`
    /// is persisted once per distinct value; the gauge event is attributed
    /// to the open turn or the last one; `cost` goes to the ledger (51 §6.3).
    async fn usage(&mut self, u: UsageUpdate) {
        self.context = u32::try_from(u.used).unwrap_or(u32::MAX);
        let size = u32::try_from(u.size).ok().filter(|s| *s > 0);
        if size.is_some() && size != self.context_size {
            match self
                .store
                .set_context_size(self.id, size.unwrap_or(0))
                .await
            {
                Ok(()) => self.context_size = size,
                Err(e) => tracing::warn!(session = %self.id, "set_context_size: {e:#}"),
            }
        }
        if let Some(turn) = self.turn.as_ref().map(|t| t.id).or(self.last_turn) {
            self.publish(AgentEvent::ContextUsed {
                session: self.id,
                turn,
                context: self.context,
                size: self.context_size,
            })
            .await;
        }
        if let Some(c) = u.cost {
            self.meter(c.amount, &c.currency).await;
        }
    }

    /// 75 §8: one `set_config_option` on the live harness, with its model
    /// select re-read into [`Self::set_harness_model`]. `Err` is prefixed
    /// with the harness name, as the user and the model see it.
    async fn apply_option(
        &mut self,
        key: &str,
        value: &str,
    ) -> Result<Vec<acp::ConfigOption>, String> {
        let h = self
            .harness
            .clone()
            .ok_or_else(|| format!("{}: harness process not running", self.name))?;
        let options = tokio::time::timeout(OPTION_TIMEOUT, h.set_config_option(key, value))
            .await
            .map_err(|_| "set_config_option timed out".to_owned())
            .and_then(|r| r)
            .map_err(|e| format!("{}: {e}", self.name))?;
        if let Some(model) = h.model() {
            self.set_harness_model(model).await;
        }
        Ok(options)
    }

    /// Send every option change queued during the turn (75 §8), in order; a
    /// failure is published as this session's error, not swallowed.
    async fn apply_pending_options(&mut self) {
        for (key, value) in std::mem::take(&mut self.pending_options) {
            if let Err(message) = self.apply_option(&key, &value).await {
                self.publish(AgentEvent::Error {
                    session: self.id,
                    turn: None,
                    message,
                })
                .await;
            }
        }
    }

    /// 67 §8: the remote child's `send_message{to: "parent"}` arrives as a
    /// `_theuth/peer_message` notification; it enters the parent through the
    /// ordinary peer route ([`Deliver`] from this session to `parent`), so the
    /// communication switch, redaction, the size bound and reply matching
    /// apply exactly as for a local child. The child's own id is the
    /// idempotency key: a re-sent notification is not delivered twice. A
    /// message for a session other than ours on this conn is dropped.
    async fn uplink(&self, m: acp::PeerMessage) {
        if self.harness.as_ref().map(Harness::session_id) != Some(m.session_id.as_str()) {
            tracing::warn!(session = %self.id, "peer_message for another session dropped");
            return;
        }
        let Ok(Some(agents)) = ActorRef::<Agents>::lookup(AGENT_ACTOR) else {
            return;
        };
        let r = agents
            .ask(Deliver {
                from: self.id,
                to: "parent".to_owned(),
                text: m.text,
                await_reply: false,
                reply_to: m.reply_to,
                request_id: Some(format!("uplink:{}", m.id)),
                admitted: CancellationToken::new(),
            })
            .await;
        if let Err(e) = r {
            tracing::info!(session = %self.id, "peer_message not delivered: {e}");
        }
    }

    /// 51 §6.3: one `usage_update.cost` observation → the ledger. An
    /// unusable amount/currency is logged once, without its content.
    async fn meter(&mut self, amount: f64, currency: &str) {
        let Some(h) = &self.harness else { return };
        match meter_update(&self.name, h.session_id(), amount, currency, !h.resumed()) {
            Ok(update) => self.apply_meter(update).await,
            Err(why) => {
                if !self.meter_warned {
                    self.meter_warned = true;
                    tracing::warn!(session = %self.id, "usage_update.cost ignored: {why}");
                }
            }
        }
    }

    /// Persist `update` on this session's owner and publish every bumped revision.
    async fn apply_meter(&self, update: MeterUpdate) {
        match self.store.apply_meter(self.id, update).await {
            Ok(bumped) => {
                for (session, revision) in bumped {
                    self.publish(AgentEvent::ModelUsageChanged { session, revision })
                        .await;
                }
            }
            Err(e) => tracing::warn!(session = %self.id, "apply_meter: {e:#}"),
        }
    }

    /// Persist `m`; its seq, or 0 when the write failed (logged).
    async fn append(&self, m: ChatMessage) -> i64 {
        match self.store.append(self.id, &m).await {
            Ok(seq) => seq,
            Err(e) => {
                tracing::error!(session = %self.id, "append: {e:#}");
                0
            }
        }
    }

    /// First prompt of a fresh row names the session, like a native first
    /// turn: its first line after any attachment markers (66 §8).
    async fn title(&mut self, text: &str) {
        if self.titled {
            return;
        }
        self.titled = true;
        if let Ok(Some(row)) = self.store.session(self.id).await
            && row.title.is_empty()
            && let Some(line) = attach::first_line(text)
        {
            let title = attach::seed_title(&line);
            match self.store.set_title(self.id, &title).await {
                Ok(()) => {
                    self.publish(AgentEvent::TitleChanged {
                        session: self.id,
                        title,
                        tags: Vec::new(),
                    })
                    .await;
                }
                Err(e) => tracing::warn!(session = %self.id, "set_title: {e:#}"),
            }
        }
    }

    /// Persist `q` as the turn's user message and acknowledge its ticket
    /// (47 §4.3): a plain append, or for a peer exchange the durable
    /// consumption of 50 §7 — generation rechecked, row linked once, refused
    /// exchanges suppressed/failed and their tickets released through
    /// [`crate::communication::PeerIntakeFailed`]. `Ok(seq)`; `Err(Some(why))` when the store
    /// refused a write; `Err(None)` when a peer exchange was refused (already
    /// reported; nothing to run).
    async fn persist_input(&self, q: &Queued, turn: TurnId) -> Result<i64, Option<String>> {
        let msg = ChatMessage::user(&q.text);
        let Some(p) = &q.peer else {
            let seq = self
                .store
                .append(self.id, &msg)
                .await
                .map_err(|e| Some(format!("{e:#}")))?;
            if let Some(t) = q.ticket {
                continuation::notify(InputConsumed {
                    session: self.id,
                    id: t.id,
                    turn,
                })
                .await;
            }
            return Ok(seq);
        };
        // 50 §8 barrier: live check and store transaction as one admission.
        let _admitted = PEER_INTAKE.read().await;
        if !p.generation.live() {
            refuse_peer(
                &self.store,
                self.id,
                p.id,
                q.ticket,
                "communication_disabled",
                true,
            )
            .await;
            return Err(None);
        }
        let generation = i64::try_from(p.generation.revision()).unwrap_or(i64::MAX);
        match self
            .store
            .consume_exchange(p.id, self.id, generation, msg)
            .await
        {
            Ok(Some((seq, _))) => {
                if let Some(t) = q.ticket {
                    continuation::notify(InputConsumed {
                        session: self.id,
                        id: t.id,
                        turn,
                    })
                    .await;
                }
                continuation::notify(PeerConsumed {
                    session: self.id,
                    id: p.id,
                    turn,
                    seq,
                })
                .await;
                Ok(seq)
            }
            Ok(None) => {
                refuse_peer(
                    &self.store,
                    self.id,
                    p.id,
                    q.ticket,
                    "already settled",
                    false,
                )
                .await;
                Err(None)
            }
            Err(e) if matches!(e.downcast_ref::<CommError>(), Some(CommError::Revoked)) => {
                refuse_peer(
                    &self.store,
                    self.id,
                    p.id,
                    q.ticket,
                    "communication_disabled",
                    true,
                )
                .await;
                Err(None)
            }
            Err(e) => {
                let why = format!("{e:#}");
                if let Err(e) = self.store.fail_exchange(p.id, &why).await {
                    tracing::debug!(session = %self.id, id = %p.id, "fail_exchange: {e:#}");
                }
                refuse_peer(&self.store, self.id, p.id, q.ticket, &why, false).await;
                Err(None)
            }
        }
    }

    /// Claim the turn with the supervisor, persist `text` as the user message
    /// and acknowledge its ticket as consumed, then publish `TurnStarted`; the
    /// row exists when it is delivered (design doc 29 §4.2, 47 §4.3).
    /// `Err` when the row could not be written: the turn is already open and
    /// finished as an error, and the ticket stays pending — failed
    /// persistence never certifies a delivered input. A refused peer exchange
    /// (50 §7) opens no turn at all: its ticket is already released.
    async fn open(&mut self, q: &Queued) -> Result<TurnId, ()> {
        let turn = TurnId::new();
        // The durable link precedes the claim so a refusal costs no turn.
        let persisted = match self.persist_input(q, turn).await {
            Ok(seq) => Ok(seq),
            Err(None) => return Err(()),
            Err(Some(why)) => Err(why),
        };
        continuation::notify(TurnEntered {
            session: self.id,
            turn: Some(turn),
        })
        .await;
        if let Some(id) = q.id {
            self.publish(AgentEvent::InputInjected {
                session: self.id,
                id,
                turn,
            })
            .await;
        }
        self.title(&q.text).await;
        self.publish(AgentEvent::TurnStarted {
            session: self.id,
            turn,
            text: q.text.clone(),
            model: self.name.clone(),
            turn_seq: *persisted.as_ref().unwrap_or(&0),
        })
        .await;
        let mut t = Turn::new(self.id, turn, q.scope);
        t.input = Some((q.text.clone(), q.admitted));
        self.turn = Some(t);
        match persisted {
            Ok(_) => Ok(turn),
            Err(e) => {
                tracing::error!(session = %self.id, "append: {e}");
                self.finish("error", Some(format!("persist input: {e}")))
                    .await;
                Err(())
            }
        }
    }

    /// Send one `session/prompt`; its result arrives as [`PromptDone`]. Pending
    /// [`Note`]s go ahead of the text on the wire only — the persisted user
    /// message is the text alone — and stay with the turn for a busy retry.
    async fn start(&mut self, q: Queued, me: &ActorRef<Self>) {
        if self.harness.is_none() {
            return;
        }
        if self.open(&q).await.is_err() {
            return;
        }
        let mut text = q.text;
        if !self.notes.is_empty() {
            let notes: String = self
                .notes
                .drain(..)
                .map(|n| format!("<runtime>{n}</runtime>\n"))
                .collect();
            text.insert_str(0, &notes);
            if let Some(t) = &mut self.turn {
                t.input = Some((text.clone(), q.admitted));
            }
        }
        self.prompt(&text, me);
    }

    /// Send `text` as `session/prompt`; its result arrives as [`PromptDone`].
    fn prompt(&mut self, text: &str, me: &ActorRef<Self>) {
        let Some(harness) = &self.harness else { return };
        let fut = harness.prompt(text.to_owned());
        self.worked = true;
        let me = me.clone();
        tokio::spawn(async move {
            let _ = me.tell(PromptDone(fut.await)).await;
        });
    }

    /// `me.tell(msg)` after `after`; the handle to abort it.
    fn timer<M: Send + 'static>(me: &ActorRef<Self>, after: Duration, msg: M) -> AbortHandle
    where
        Self: Message<M, Reply = ()>,
    {
        let me = me.clone();
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            let _ = me.tell(msg).await;
        })
        .abort_handle()
    }

    /// 06 §3.5 (2026-09-24): the harness refused the prompt as busy. Keep the
    /// turn open and prompt again after the next [`RETRY`] step, unless the
    /// request deadline has passed since the input was admitted: then the
    /// refusal is the turn's error. `false` when the turn was not retried.
    async fn retry_busy(&mut self, error: &str, me: &ActorRef<Self>) -> bool {
        let Some(turn) = &mut self.turn else {
            return false;
        };
        let Some((_, admitted)) = &turn.input else {
            return false;
        };
        if turn.cancel.is_cancelled() || admitted.elapsed() >= self.deadline {
            return false;
        }
        let after = self.retry[turn.retries.min(self.retry.len() - 1)];
        turn.retries += 1;
        turn.retry = Some(Self::timer(me, after, Retry(turn.id)));
        let (id, secs) = (turn.id, after.as_secs());
        tracing::info!(session = %self.id, "harness busy, retrying in {secs}s: {error}");
        self.publish(AgentEvent::Error {
            session: self.id,
            turn: Some(id),
            message: format!(
                "{}: harness is still busy with its previous task; retrying in {secs}s",
                self.name
            ),
        })
        .await;
        true
    }

    /// `update` is harness work worth a continuation turn (06 §3.5): a tool
    /// call or text, not usage/plan/mode bookkeeping.
    fn continues(update: &SessionUpdate) -> bool {
        matches!(
            update,
            SessionUpdate::ToolCall(_)
                | SessionUpdate::ToolCallUpdate(_)
                | SessionUpdate::AgentMessageChunk(_)
                | SessionUpdate::AgentThoughtChunk(_)
        )
    }

    /// Open a continuation turn (06 §3.5, 2026-09-24): the harness kept
    /// working after its prompt returned. Claimed like any turn so the
    /// ledger and board show the child busy; its `TurnStarted` text is a
    /// harness wrapper no UI echoes; it never reports to the parent.
    async fn continue_turn(&mut self) {
        let turn = TurnId::new();
        continuation::notify(TurnEntered {
            session: self.id,
            turn: Some(turn),
        })
        .await;
        self.publish(AgentEvent::TurnStarted {
            session: self.id,
            turn,
            text: CONTINUED_NOTE.into(),
            model: self.name.clone(),
            turn_seq: 0,
        })
        .await;
        let mut t = Turn::new(self.id, turn, None);
        t.continuation = true;
        self.turn = Some(t);
    }

    /// Close an open continuation turn: the next prompt takes over.
    async fn end_continuation(&mut self) {
        if self.turn.as_ref().is_some_and(|t| t.continuation) {
            self.finish(HARNESS_CONTINUED, None).await;
        }
    }

    /// Persist `messages` in order (06 §3.6).
    async fn persist_all(&self, messages: Vec<ChatMessage>) {
        for m in messages {
            self.append(m).await;
        }
    }

    /// Close the turn in flight: answer its unfinished tool calls, persist
    /// the trailing assistant text (or `error`, the last tool result, or
    /// `"(no text)"` when none arrived), publish `Error` when given, report
    /// the turn's full text by turn ([`PeerTurnFinished`], 50 §6.3), then
    /// `TurnFinished`, then hand the state back to the supervisor
    /// ([`TurnReturned`]; it keeps this child counted until the parent holds
    /// the result). A continuation turn (06 §3.5) persists and closes the
    /// same way but always stops as [`HARNESS_CONTINUED`] and reports no
    /// result: nothing reaches the parent.
    async fn finish(&mut self, stop_reason: &str, error: Option<String>) {
        let Some(mut turn) = self.turn.take() else {
            return;
        };
        turn.cancel.cancel();
        turn.disarm();
        if let Some(e) = &error {
            self.publish(AgentEvent::Error {
                session: self.id,
                turn: Some(turn.id),
                message: e.clone(),
            })
            .await;
        }
        let stop_reason = if turn.continuation {
            HARNESS_CONTINUED
        } else {
            stop_reason
        };
        let text = if turn.continuation {
            String::new()
        } else {
            turn.result(error)
        };
        turn.close(&text);
        self.persist_all(std::mem::take(&mut turn.messages)).await;
        if !turn.continuation {
            continuation::notify(PeerTurnFinished {
                session: self.id,
                turn: turn.id,
                scope: turn.scope,
                text,
                stop_reason: stop_reason.to_owned(),
            })
            .await;
        }
        // 75 §7 / 06 §8.4: the prompt response's tokens go on the ledger
        // under the harness model of this turn, flagged a lower bound when
        // the harness reports its last request only (opencode 1.x), then
        // out with `TurnFinished`. No usage → no row (nothing to correct
        // later, nothing to sum).
        let lower_bound = self
            .harness
            .as_ref()
            .is_some_and(Harness::usage_is_lower_bound);
        let usage = match &turn.tokens {
            Some(t) => {
                match self
                    .store
                    .record_remote_turn(
                        self.id,
                        turn.id,
                        &self.name,
                        self.harness_model.as_deref(),
                        t,
                        lower_bound,
                    )
                    .await
                {
                    Ok(bumped) => {
                        for (session, revision) in bumped {
                            self.publish(AgentEvent::ModelUsageChanged { session, revision })
                                .await;
                        }
                    }
                    Err(e) => tracing::warn!(session = %self.id, "record_remote_turn: {e:#}"),
                }
                Usage {
                    prompt: u32::try_from(t.input).unwrap_or(u32::MAX),
                    completion: u32::try_from(t.output).unwrap_or(u32::MAX),
                    cached: u32::try_from(t.cache_read).unwrap_or(u32::MAX),
                }
            }
            None => Usage::default(),
        };
        self.publish(AgentEvent::TurnFinished {
            session: self.id,
            turn: turn.id,
            stop_reason: stop_reason.to_owned(),
            usage,
            context: self.context,
        })
        .await;
        self.last_turn = Some(turn.id);
        continuation::notify(TurnReturned {
            session: self.id,
            turn: Some(turn.id),
        })
        .await;
    }

    /// Remove every queued peer input matching `pick` and release it (50 §8):
    /// its ticket through [`crate::communication::PeerIntakeFailed`], its row suppressed only when
    /// `suppress`.
    async fn drop_queued(&mut self, pick: impl Fn(&Queued) -> bool, reason: &str, suppress: bool) {
        let mut i = 0;
        while i < self.queue.len() {
            if pick(&self.queue[i]) {
                let q = self.queue.remove(i).expect("index checked");
                match (q.peer, q.ticket) {
                    (Some(p), _) => {
                        refuse_peer(&self.store, self.id, p.id, q.ticket, reason, suppress).await;
                    }
                    (None, Some(ticket)) => {
                        continuation::notify(InputDropped {
                            session: self.id,
                            ticket,
                        })
                        .await;
                    }
                    (None, None) => {}
                }
            } else {
                i += 1;
            }
        }
    }

    /// A turn that never reached the harness (06 §8.7): the error is the
    /// assistant message, so `<subagent_result stop="error">` names the cause.
    async fn fail(&mut self, q: Queued, why: &str) {
        if self.open(&q).await.is_ok() {
            self.finish("error", Some(why.to_owned())).await;
        }
    }

    /// Fail every queued input with `why`: each was admitted by the
    /// supervisor and answered, so its ticket is consumed and its parent gets
    /// an error result instead of a silently vanished message.
    async fn drain(&mut self, why: &str) {
        while let Some(q) = self.queue.pop_front() {
            self.fail(q, why).await;
        }
    }

    /// Start the next queued input, if idle — after any option change queued
    /// during the turn just ended (75 §8), so it precedes the prompt on the wire.
    async fn next(&mut self, me: &ActorRef<Self>) {
        if self.turn.is_none() && self.harness.is_some() {
            self.apply_pending_options().await;
            if let Some(q) = self.queue.pop_front() {
                self.start(q, me).await;
            }
        }
    }

    /// An input theuth itself puts to the harness (109 §7.3): no caller id,
    /// no admission ticket, no peer exchange; persisted and reported like
    /// any other.
    fn synthetic(text: &str) -> Queued {
        Queued {
            id: None,
            ticket: None,
            text: text.to_owned(),
            scope: None,
            peer: None,
            admitted: Instant::now(),
        }
    }

    /// 109 §7.3, the `resumed()` branch: the respawned child's first input
    /// goes to the front of the queue — the restored note when
    /// `session/load` brought the harness context back, else the delta
    /// handoff under `tokens`. The two `git` reads in it run here, on the
    /// mailbox, 5 s capped each (boot only).
    async fn queue_respawn(&mut self, tokens: usize) {
        let restored = self.harness.as_ref().is_some_and(Harness::resumed);
        let text = if restored {
            handoff::restored_note(chrono::Utc::now())
        } else {
            handoff::compose_handoff(&self.store, self.id, tokens).await
        };
        tracing::info!(session = %self.id, restored, "109: ACP child respawned; prompting");
        self.queue.push_front(Self::synthetic(&text));
    }

    /// The connection is gone. A child process that exited is known
    /// stopped: end the turn in flight as an error, fail the queue, stop. A
    /// dialled remote is not, once it was ever prompted (work it started can
    /// outlive the prompt and this connection): that work is recorded
    /// unknown first ([`Self::lose`], 47 §4.2 rule 6). The next
    /// `send_message` respawns the child from the store row either way (06 §3.2).
    async fn closed(&mut self, why: String, ctx: &mut Context<Self, ()>) {
        self.close_harness();
        if self.remote && self.worked {
            self.lose(why, ctx).await;
            return;
        }
        self.finish("error", Some(why.clone())).await;
        self.drain(&why).await;
        ctx.stop();
    }

    /// End the connection (kills a child process), release every permission
    /// ask waiting outside a live turn and take the materialised skills
    /// back out of the cwd (75 §4).
    fn close_harness(&mut self) {
        if let Some(h) = self.harness.take() {
            h.close();
        }
        self.alive.cancel();
        if let Some(m) = self.skills.take() {
            m.remove();
        }
    }

    /// Record the remote's unverifiable work as unknown under this session,
    /// then end the turn in flight (if any) with `why`, fail the queue and
    /// stop.
    async fn lose(&mut self, why: String, ctx: &mut Context<Self, ()>) {
        let why = self.unknown(why).await;
        self.finish("error", Some(why.clone())).await;
        self.drain(&why).await;
        ctx.stop();
    }

    /// Put `why` on the supervisor's ledger as this session's unknown outcome
    /// (47 §4.2 rule 6); the reason the turn is then ended with.
    /// [`SessionUnknown`] and [`TurnReturned`] share the supervisor's mailbox,
    /// so the blocker exists before the return can be read as a clean
    /// handback; the supervisor publishes the visible error.
    async fn unknown(&self, why: String) -> String {
        let why = format!("{why}; remote harness not confirmed stopped");
        continuation::notify(SessionUnknown {
            session: self.id,
            failure: format!("{}: {why}", self.name),
        })
        .await;
        why
    }

    /// 47 §5.3: the harness's plan is this session's canonical todo list.
    /// Persist at the current history boundary, publish after commit; a
    /// failed write advertises nothing.
    async fn plan(&self, plan: &Plan) {
        let items = plan_items(plan);
        match self
            .store
            .save_todos(self.id, TodoSource::Acp, &items)
            .await
        {
            Ok(snapshot) => {
                self.publish(AgentEvent::TodoChanged {
                    session: self.id,
                    snapshot,
                })
                .await;
            }
            Err(e) => tracing::error!(session = %self.id, "save_todos: {e:#}"),
        }
    }

    /// Route a `session/request_permission` through the registry's gate
    /// (06 §3.4) off the mailbox. An ask of the prompt in flight is answered
    /// `cancelled` with that turn (the spec's MUST on `session/cancel`); one
    /// arriving with no turn, during a continuation turn, or after the turn
    /// was already cancelled (2026-09-24: omp keeps working past its cancel)
    /// is still asked, and released only when the harness closes.
    fn permission(&self, p: Permission) {
        let kind = p.tool_call.fields.kind.unwrap_or_default();
        let ask = ExternalApproval {
            session: self.id,
            tool: p
                .tool_call
                .fields
                .title
                .clone()
                .unwrap_or_else(|| "tool".into()),
            args: with_kind(p.tool_call.fields.raw_input.clone(), kind),
            risk: acp::risk(kind),
        };
        let cancel = match &self.turn {
            Some(t) if !t.continuation && !t.cancel.is_cancelled() => t.cancel.clone(),
            _ => self.alive.clone(),
        };
        tokio::spawn(async move {
            let decide = async {
                let reg = ActorRef::<ToolRegistry>::lookup(REGISTRY_ACTOR)
                    .ok()
                    .flatten()?;
                reg.ask(ask).await.ok()
            };
            let decision = tokio::select! {
                () = cancel.cancelled() => None,
                d = decide => Some(d.unwrap_or(Decision::Deny)),
            };
            let o = outcome(&p.options, decision);
            p.respond(o);
        });
    }
}

/// A `session/prompt` error that means "the harness is still working on
/// its previous prompt" (06 §3.5, 2026-09-24). The transport renders the
/// JSON-RPC error as `message: data`, so omp's `{"reason":"session_busy"}`
/// is in the text; `already processing` in the message is the
/// case-insensitive fallback for other harnesses.
pub(crate) fn is_busy(error: &str) -> bool {
    let e = error.to_ascii_lowercase();
    e.contains("busy") || e.contains("already processing")
}

/// `raw_input ∪ {"kind": kind}` (06 §3.3); a non-object input is kept under `input`.
fn with_kind(
    raw: Option<serde_json::Value>,
    kind: agent_client_protocol::schema::v1::ToolKind,
) -> serde_json::Value {
    let kind = serde_json::to_value(kind).unwrap_or_default();
    let mut m = match raw {
        Some(serde_json::Value::Object(m)) => m,
        None => serde_json::Map::new(),
        Some(v) => serde_json::Map::from_iter([("input".to_owned(), v)]),
    };
    m.insert("kind".into(), kind);
    serde_json::Value::Object(m)
}

/// The option for `decision` (06 §3.4): `allow_once` else `allow_always`;
/// `reject_once` else `reject_always`; none offered / no decision → `cancelled`.
fn outcome(
    options: &[agent_client_protocol::schema::v1::PermissionOption],
    decision: Option<Decision>,
) -> RequestPermissionOutcome {
    let kinds: [PermissionOptionKind; 2] = match decision {
        Some(Decision::Allow) => [
            PermissionOptionKind::AllowOnce,
            PermissionOptionKind::AllowAlways,
        ],
        Some(Decision::Deny) => [
            PermissionOptionKind::RejectOnce,
            PermissionOptionKind::RejectAlways,
        ],
        None => return RequestPermissionOutcome::Cancelled,
    };
    kinds
        .iter()
        .find_map(|k| options.iter().find(|o| o.kind == *k))
        .map_or(RequestPermissionOutcome::Cancelled, |o| {
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(o.option_id.clone()))
        })
}

/// 51 §6.3: the harness `name`'s cumulative `amount` on external session
/// `external` as a ledger update; `new_session` when this wrapper created that
/// session (baseline known zero). `Err` is content-free.
pub(crate) fn meter_update(
    name: &str,
    external: &str,
    amount: f64,
    currency: &str,
    new_session: bool,
) -> Result<MeterUpdate, String> {
    Ok(MeterUpdate {
        origin: name.to_owned(),
        external_session: external.to_owned(),
        cumulative: Money::from_f64(amount, currency)?,
        confirmed_new_session: new_session,
        report_id: None,
    })
}

/// 75 §7: `PromptResponse.usage` as ledger tokens, in `model_calls` /
/// [`Usage`] terms: `input` = fresh + cache read + cache write (ACP's
/// `inputTokens` is the uncached remainder — omp 18.3.1 live: `input 4,
/// cachedRead 12833, cachedWrite 172, total 13013`; genai's `prompt_tokens`
/// includes the cache), `output` = `outputTokens + thoughtTokens`, the cache
/// split kept. Otherwise raw as the harness sent it: omp and opencode ≥ 2 the
/// turn's sum over its steps, opencode 1.x its last request only (06 §8.4).
pub(crate) fn tokens_of(u: &AcpUsage) -> TokenTotals {
    let cache_read = u.cached_read_tokens.unwrap_or(0);
    let cache_write = u.cached_write_tokens.unwrap_or(0);
    TokenTotals {
        input: u
            .input_tokens
            .saturating_add(cache_read)
            .saturating_add(cache_write),
        output: u
            .output_tokens
            .saturating_add(u.thought_tokens.unwrap_or(0)),
        cache_read,
        cache_write,
        unknown_calls: 0,
    }
}

/// 06 §3.3: one `session/update` → the [`AgentEvent`]s to publish, with the
/// text and context window accumulating on `turn` and the messages to
/// persist queued on it (06 §3.6).
pub(crate) fn translate(update: SessionUpdate, turn: &mut Turn) -> Vec<AgentEvent> {
    let (session, id) = (turn.session, turn.id);
    // 91: the first message or tool update after thought chunks ends the
    // thinking; `ReasoningDone` goes ahead of that update's own event.
    let done = match &update {
        SessionUpdate::AgentThoughtChunk(_) => {
            turn.thinking.get_or_insert_with(Instant::now);
            None
        }
        SessionUpdate::AgentMessageChunk(_)
        | SessionUpdate::ToolCall(_)
        | SessionUpdate::ToolCallUpdate(_) => {
            turn.thinking.take().map(|start| AgentEvent::ReasoningDone {
                session,
                turn: id,
                ms: start.elapsed().as_millis() as u64,
            })
        }
        _ => None,
    };
    let mut out = match update {
        SessionUpdate::AgentMessageChunk(c) => {
            let text = acp::render::content_block_text(&c.content);
            turn.text.push_str(&text);
            vec![AgentEvent::TextDelta {
                session,
                turn: id,
                text: Arc::from(text),
            }]
        }
        SessionUpdate::AgentThoughtChunk(c) => vec![AgentEvent::ReasoningDelta {
            session,
            turn: id,
            text: Arc::from(acp::render::content_block_text(&c.content)),
        }],
        SessionUpdate::ToolCall(tc) => {
            let call_id = tc.tool_call_id.0.to_string();
            let args = with_kind(tc.raw_input, tc.kind);
            turn.call_started(&call_id, &tc.title, args.clone());
            let mut out = vec![AgentEvent::ToolCallStarted {
                session,
                turn: id,
                call_id: call_id.clone(),
                name: tc.title,
                args,
            }];
            // Some harnesses report a finished call in one message.
            if let Some(is_error) = finished(tc.status) {
                let output = output(&tc.content, tc.raw_output.as_ref());
                turn.last_tool = Some(output.clone());
                turn.call_finished(&call_id, &output, is_error);
                out.push(AgentEvent::ToolCallFinished {
                    session,
                    turn: id,
                    call_id,
                    output,
                    is_error,
                    files: Vec::new(),
                });
            }
            out
        }
        SessionUpdate::ToolCallUpdate(u) => match u.fields.status.and_then(finished) {
            Some(is_error) => {
                let call_id = u.tool_call_id.0.to_string();
                let output = output(
                    u.fields.content.as_deref().unwrap_or_default(),
                    u.fields.raw_output.as_ref(),
                );
                turn.last_tool = Some(output.clone());
                turn.call_finished(&call_id, &output, is_error);
                vec![AgentEvent::ToolCallFinished {
                    session,
                    turn: id,
                    call_id,
                    output,
                    is_error,
                    files: Vec::new(),
                }]
            }
            // Pending / in-progress / partial content: the card already shows the call.
            None => Vec::new(),
        },
        // Plans are canonical state, not transcript text: `AcpChild::plan`.
        SessionUpdate::Plan(_) => Vec::new(),
        // usage_update: session facts, handled in the mailbox (75 §5).
        // user_message_chunk (load replay, 06 §8.11), commands, modes, config, info.
        _ => Vec::new(),
    };
    if let Some(done) = done {
        out.insert(0, done);
    }
    out
}

/// 47 §5.3: an ACP plan as theuth todo items, upstream order and content kept.
/// Pending/in-progress/completed map directly; a status this build does not
/// know is shown as pending rather than claimed done.
fn plan_items(plan: &Plan) -> Vec<Todo> {
    plan.entries
        .iter()
        .map(|e| Todo {
            text: e.content.clone(),
            status: match e.status {
                PlanEntryStatus::InProgress => Status::InProgress,
                PlanEntryStatus::Completed => Status::Done,
                _ => Status::Pending,
            },
            reason: None,
        })
        .collect()
}

/// `Some(is_error)` for a terminal status.
fn finished(status: ToolCallStatus) -> Option<bool> {
    match status {
        ToolCallStatus::Completed => Some(false),
        ToolCallStatus::Failed => Some(true),
        _ => None,
    }
}

/// Rendered tool output: the content blocks, else the raw output, else `""`.
fn output(
    content: &[agent_client_protocol::schema::v1::ToolCallContent],
    raw: Option<&serde_json::Value>,
) -> String {
    let text = acp::render_content(content);
    if !text.is_empty() {
        return text;
    }
    match raw {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(v) => serde_json::to_string_pretty(v).unwrap_or_default(),
        None => String::new(),
    }
}

impl Actor for AcpChild {
    type Args = Self;
    type Error = Infallible;

    async fn on_start(mut child: Self, me: ActorRef<Self>) -> Result<Self, Infallible> {
        if let Some(c) = child.connect.take() {
            let (tx, mut rx) = mpsc::unbounded_channel::<Update>();
            let pump = me.clone();
            tokio::spawn(async move {
                while let Some(u) = rx.recv().await {
                    if pump.tell(u).await.is_err() {
                        break;
                    }
                }
            });
            let connect_timeout = child.connect_timeout;
            tokio::spawn(async move {
                // R2.1: a harness that never answers `initialize` /
                // `session/new` errors out like any failed connect, which
                // drains the queue and frees the child's permit.
                let r = match tokio::time::timeout(connect_timeout, Harness::connect(c, tx)).await {
                    Ok(r) => r,
                    Err(_) => Err(format!(
                        "harness did not connect within {}s",
                        connect_timeout.as_secs()
                    )),
                };
                let _ = me.tell(Connected(r)).await;
            });
        }
        Ok(child)
    }

    async fn on_stop(
        &mut self,
        _: WeakActorRef<Self>,
        _: ActorStopReason,
    ) -> Result<(), Infallible> {
        self.close_harness();
        Ok(())
    }
}

impl Message<Connected> for AcpChild {
    type Reply = ();

    async fn handle(&mut self, Connected(r): Connected, ctx: &mut Context<Self, Self::Reply>) {
        match r {
            Ok(h) => {
                // A stored id that was not restored means the harness starts
                // without its context; the parent learns it from the UI/transcript.
                if !h.resumed()
                    && let Ok(Some(prev)) = self.store.acp_session(self.id).await
                    && prev != h.session_id()
                {
                    let message = format!(
                        "{}: previous harness session not restored; starting fresh",
                        self.name
                    );
                    if let Err(e) = self.store.record_error(self.id, &message).await {
                        tracing::warn!(session = %self.id, "record_error: {e:#}");
                    }
                    self.publish(AgentEvent::Error {
                        session: self.id,
                        turn: None,
                        message,
                    })
                    .await;
                }
                if let Err(e) = self.store.set_acp_session(self.id, h.session_id()).await {
                    tracing::warn!(session = %self.id, "set_acp_session: {e:#}");
                }
                // 06 §8.2: the model select of the session response, else the
                // entry's `model:`; nothing when neither says.
                let model = h.model().or_else(|| self.fallback_model.clone());
                if let Some(model) = model {
                    self.set_harness_model(model).await;
                }
                // 51 §6.3: a session this wrapper created has a known-zero
                // baseline; persist that before any prompt so the first
                // cumulative total is attributed in full. Nothing has been
                // observed yet, so the row's currency is a placeholder:
                // `apply_meter` rewrites it in place from the first report
                // when that comes in another currency (75 §6).
                if !h.resumed()
                    && let Ok(zero) = meter_update(&self.name, h.session_id(), 0.0, "USD", true)
                {
                    self.apply_meter(zero).await;
                }
                self.harness = Some(h);
                if let Some(tokens) = self.respawn.take() {
                    self.queue_respawn(tokens).await;
                }
                let me = ctx.actor_ref().clone();
                self.next(&me).await;
            }
            Err(e) => {
                // 109 §7.3: a boot respawn that cannot connect still answers
                // the waiting parent — with the connect error as its result.
                if self.respawn.take().is_some() {
                    self.queue.push_front(Self::synthetic(RESPAWN_FAILED_INPUT));
                }
                if self.queue.is_empty() {
                    if let Err(err) = self.store.record_error(self.id, &e).await {
                        tracing::warn!(session = %self.id, "record_error: {err:#}");
                    }
                    self.publish(AgentEvent::Error {
                        session: self.id,
                        turn: None,
                        message: e.clone(),
                    })
                    .await;
                    self.dead = Some(e);
                } else {
                    self.drain(&e).await;
                    ctx.stop();
                }
            }
        }
    }
}

// `files` is dropped: harness inputs come from text tools, so there is no
// image to hand over (17 §5).
impl Message<UserInput> for AcpChild {
    type Reply = ();

    async fn handle(
        &mut self,
        UserInput {
            id,
            text,
            files: _,
            // No mid-turn injection on the ACP prompt path: every queued
            // input waits for the turn already, so `defer` changes nothing (62 §3.1).
            defer: _,
            ticket,
            scope,
        }: UserInput,
        ctx: &mut Context<Self, Self::Reply>,
    ) {
        let q = Queued {
            id,
            ticket,
            text,
            scope,
            peer: None,
            admitted: Instant::now(),
        };
        if let Some(why) = self.dead.clone() {
            self.fail(q, &why).await;
            ctx.stop();
            return;
        }
        self.end_continuation().await;
        if self.harness.is_none() || self.turn.is_some() {
            if let Some(id) = q.id {
                self.publish(AgentEvent::InputQueued {
                    session: self.id,
                    id,
                    text: q.text.clone(),
                    defer: false,
                })
                .await;
            }
            self.queue.push_back(q);
            return;
        }
        let me = ctx.actor_ref().clone();
        self.start(q, &me).await;
    }
}

/// 50 §6.4: an ACP child receives enabled peer messages like any session,
/// queued behind the prompt in flight, and consumes them durably. It is not
/// an exportable A2A backend — native request ownership and cancellation
/// cannot be imposed on an opaque harness loop — so a task input
/// (`isolated`) is refused at the ask and never accepted into its queue; the
/// supervisor fails the exchange visibly. An external harness has no reverse
/// peer tools; a remote theuth child answers through [`Self::uplink`] (67 §8).
impl Message<PeerInput> for AcpChild {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: PeerInput, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if m.record.session != self.id {
            return Err(format!(
                "exchange {} is addressed to {}, not this session",
                m.record.receipt.id, m.record.session
            ));
        }
        if m.isolated {
            return Err("ACP child is not an exportable A2A backend (50 §6.3)".into());
        }
        let q = Queued {
            id: None,
            ticket: Some(m.ticket),
            text: peer_wrapper(&m.record, m.reply_expected, chrono::Local::now()),
            scope: m.request_scope,
            peer: Some(PeerMeta {
                id: m.record.receipt.id,
                generation: m.generation,
                isolated: false,
            }),
            admitted: Instant::now(),
        };
        if let Some(why) = self.dead.clone() {
            self.fail(q, &why).await;
            ctx.stop();
            return Ok(());
        }
        self.end_continuation().await;
        if self.harness.is_none() || self.turn.is_some() {
            self.queue.push_back(q);
            return Ok(());
        }
        let me = ctx.actor_ref().clone();
        self.start(q, &me).await;
        Ok(())
    }
}

impl Message<CancelPeerScope> for AcpChild {
    type Reply = ();

    async fn handle(&mut self, m: CancelPeerScope, _: &mut Context<Self, Self::Reply>) {
        // No task ever opens an ACP turn (isolated input is refused), so only
        // queued inputs of that scope can exist.
        self.drop_queued(|q| q.scope == Some(m.scope), "canceled", false)
            .await;
    }
}

impl Message<SuppressPeerInputs> for AcpChild {
    type Reply = ();

    async fn handle(&mut self, _: SuppressPeerInputs, _: &mut Context<Self, Self::Reply>) {
        self.drop_queued(|q| q.peer.is_some(), "communication_disabled", true)
            .await;
    }
}

impl Message<Update> for AcpChild {
    type Reply = ();

    async fn handle(&mut self, u: Update, ctx: &mut Context<Self, Self::Reply>) {
        match u {
            Update::Session(SessionUpdate::Plan(p)) => self.plan(&p).await,
            // 06 §8.2: a mid-session switch (`/switch` in omp); an update
            // without a model select keeps the last known value.
            Update::Session(SessionUpdate::ConfigOptionUpdate(u)) => {
                if let Some(model) = acp::current_model(&u.config_options) {
                    self.set_harness_model(model).await;
                }
            }
            // 75 §3/§5: session-level facts that arrive with no turn open
            // (omp sends the manifest 50 ms after `session/new`; opencode's
            // usage may follow the prompt response). Handled before the turn
            // check; 06 §8.11's replay rule gets these two exceptions.
            Update::Session(SessionUpdate::AvailableCommandsUpdate(u)) => {
                self.set_harness_commands(u.available_commands).await;
            }
            Update::Session(SessionUpdate::UsageUpdate(u)) => self.usage(u).await,
            // 06 §8.4: opencode ≥ 2 marks a provider retry or a compaction
            // in `session_info_update._meta`; no event slot shows these yet.
            Update::Session(SessionUpdate::SessionInfoUpdate(u)) => {
                if let Some(meta) = &u.meta
                    && meta.keys().any(|k| k.starts_with("opencode/"))
                {
                    tracing::debug!(session = %self.id, "session_info_update meta: {}", serde_json::Value::Object(meta.clone()));
                }
            }
            Update::Session(update) => {
                // 06 §3.5 (2026-09-24): work after the prompt returned opens
                // a continuation turn. Before any prompt on this connection
                // the updates are `session/load` replay and stay dropped (§8 rule 11).
                if self.turn.is_none() {
                    if !(self.worked && self.harness.is_some() && Self::continues(&update)) {
                        return;
                    }
                    self.continue_turn().await;
                }
                let me = ctx.actor_ref().clone();
                let Some(turn) = &mut self.turn else { return };
                if turn.continuation {
                    if let Some(q) = turn.quiet.take() {
                        q.abort();
                    }
                    turn.quiet = Some(Self::timer(&me, self.quiet, Quiet(turn.id)));
                }
                let events = translate(update, turn);
                // Rows exist before the events that show them (29 §4.2).
                let messages = std::mem::take(&mut turn.messages);
                self.persist_all(messages).await;
                for ev in events {
                    self.publish(ev).await;
                }
            }
            Update::Permission(p) => self.permission(p),
            Update::Peer(m) => self.uplink(m).await,
            Update::Closed(why) => self.closed(why, ctx).await,
        }
    }
}

impl Message<PromptDone> for AcpChild {
    type Reply = ();

    async fn handle(&mut self, PromptDone(r): PromptDone, ctx: &mut Context<Self, Self::Reply>) {
        let me = ctx.actor_ref().clone();
        match r {
            Ok((stop, usage)) => {
                if let Some(t) = &mut self.turn {
                    t.tokens = usage.as_ref().map(tokens_of);
                }
                self.finish(&stop, None).await;
            }
            Err(e) => {
                if is_busy(&e) && self.retry_busy(&e, &me).await {
                    return;
                }
                // A remote prompt that failed is an unknown outcome (the
                // transport may be gone with work still running): the blocker
                // goes on the ledger before this turn returns. The connection
                // itself is kept; a real disconnect follows as `Closed`.
                let why = if self.remote {
                    self.unknown(e).await
                } else {
                    e
                };
                self.finish("error", Some(why)).await;
            }
        }
        self.next(&me).await;
    }
}

impl Message<CancelTurn> for AcpChild {
    type Reply = ();

    // `keep` (76 §7) is ignored: the harness owns the stream and its history;
    // theuth cannot persist a prefix into a transcript it does not write.
    async fn handle(&mut self, _: CancelTurn, ctx: &mut Context<Self, Self::Reply>) {
        let Some(turn) = &mut self.turn else { return };
        let Some(h) = &self.harness else { return };
        if turn.kill.is_some() {
            return;
        }
        // Waiting out a busy refusal: nothing of ours is in flight to
        // cancel, and the harness's own work is not this turn's to kill.
        if turn.retry.is_some() {
            self.finish("cancelled", None).await;
            let me = ctx.actor_ref().clone();
            self.next(&me).await;
            return;
        }
        turn.cancel.cancel();
        if let Err(e) = h.cancel() {
            self.closed(e, ctx).await;
            return;
        }
        let me = ctx.actor_ref().clone();
        turn.kill = Some(Self::timer(&me, self.grace, KillTimer(turn.id)));
    }
}

impl Message<KillTimer> for AcpChild {
    type Reply = ();

    async fn handle(&mut self, KillTimer(id): KillTimer, ctx: &mut Context<Self, Self::Reply>) {
        if !self.turn.as_ref().is_some_and(|t| t.id == id) {
            return;
        }
        self.close_harness();
        // Closing our end kills a child process; a remote may still be working.
        if self.remote {
            self.lose("cancel not acknowledged within grace".into(), ctx)
                .await;
            return;
        }
        self.finish("cancelled", None).await;
        // As `closed()`: the inputs queued behind the killed turn get their
        // error result and release their tickets (R2.2), not a silent drop.
        self.drain("cancelled").await;
        ctx.stop();
    }
}

impl Message<Shutdown> for AcpChild {
    type Reply = ();

    async fn handle(&mut self, _: Shutdown, ctx: &mut Context<Self, Self::Reply>) {
        self.close_harness();
        self.finish("cancelled", None).await;
        self.drain("harness replaced by resume_child").await;
        ctx.stop();
    }
}

impl Message<Note> for AcpChild {
    type Reply = ();

    async fn handle(&mut self, Note(n): Note, _: &mut Context<Self, Self::Reply>) {
        self.notes.push(n);
    }
}

/// 109 §7.3: remembered until the connection is up; acted on at once when
/// it already is (or already failed).
impl Message<Respawn> for AcpChild {
    type Reply = ();

    async fn handle(&mut self, Respawn(tokens): Respawn, ctx: &mut Context<Self, Self::Reply>) {
        if let Some(why) = self.dead.clone() {
            self.fail(Self::synthetic(RESPAWN_FAILED_INPUT), &why).await;
            ctx.stop();
            return;
        }
        if self.harness.is_none() {
            self.respawn = Some(tokens);
            return;
        }
        self.queue_respawn(tokens).await;
        let me = ctx.actor_ref().clone();
        self.next(&me).await;
    }
}

impl Message<Retry> for AcpChild {
    type Reply = ();

    async fn handle(&mut self, Retry(id): Retry, ctx: &mut Context<Self, Self::Reply>) {
        let Some(turn) = &mut self.turn else { return };
        if turn.id != id {
            return;
        }
        turn.retry = None;
        let Some((text, _)) = turn.input.clone() else {
            return;
        };
        let me = ctx.actor_ref().clone();
        self.prompt(&text, &me);
    }
}

impl Message<Quiet> for AcpChild {
    type Reply = ();

    async fn handle(&mut self, Quiet(id): Quiet, ctx: &mut Context<Self, Self::Reply>) {
        if self
            .turn
            .as_ref()
            .is_some_and(|t| t.id == id && t.continuation)
        {
            self.finish(HARNESS_CONTINUED, None).await;
            let me = ctx.actor_ref().clone();
            self.next(&me).await;
        }
    }
}

/// A prompt in flight (or waiting out a busy refusal), a continuation turn,
/// or an input waiting for the connection/prompt: the row is busy either way
/// (the ledger, not this projection, is authority).
impl Message<IsRunning> for AcpChild {
    type Reply = bool;

    async fn handle(&mut self, _: IsRunning, _: &mut Context<Self, Self::Reply>) -> bool {
        self.turn.is_some() || !self.queue.is_empty()
    }
}

impl Message<HarnessOptions> for AcpChild {
    type Reply = Result<Vec<acp::ConfigOption>, String>;

    async fn handle(
        &mut self,
        _: HarnessOptions,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.harness
            .as_ref()
            .map(Harness::options)
            .ok_or_else(|| format!("{}: harness process not running", self.name))
    }
}

/// 75 §8: idle → set now; a turn in flight (prompt or continuation) → queue
/// it for the turn's end and say so. The queued change is validated
/// against the option list now, so a refusal is immediate either way.
impl Message<SetOption> for AcpChild {
    type Reply = Result<acp::SetOptionResult, String>;

    async fn handle(&mut self, m: SetOption, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let Some(h) = &self.harness else {
            return Err(format!("{}: harness process not running", self.name));
        };
        if self.turn.is_some() {
            let options = h.options();
            h.check_option(&m.config_id, &m.value)
                .map_err(|e| format!("{}: {e}", self.name))?;
            self.pending_options.push((m.config_id, m.value));
            return Ok(acp::SetOptionResult {
                options,
                pending: true,
            });
        }
        let options = self.apply_option(&m.config_id, &m.value).await?;
        Ok(acp::SetOptionResult {
            options,
            pending: false,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use agent_client_protocol::schema::v1::{
        ContentBlock, ContentChunk, Cost, PlanEntry, PlanEntryPriority, ToolCall, ToolCallContent,
        ToolCallUpdate, ToolCallUpdateFields, ToolKind, UsageUpdate,
    };
    use genai::chat::ChatRole;
    use serde_json::json;

    use super::*;
    use crate::HistoryEntry;

    fn turn() -> Turn {
        Turn::new(SessionId::new(), TurnId::new(), None)
    }

    #[test]
    fn message_chunks_accumulate_text() {
        let mut t = turn();
        let ev = translate(
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("Hel"))),
            &mut t,
        );
        assert!(
            matches!(&ev[..], [AgentEvent::TextDelta { text, turn, .. }] if &**text == "Hel" && *turn == t.id)
        );
        translate(
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("lo"))),
            &mut t,
        );
        assert_eq!(t.text, "Hello");
        let ev = translate(
            SessionUpdate::AgentThoughtChunk(ContentChunk::new(ContentBlock::from("hmm"))),
            &mut t,
        );
        assert!(matches!(&ev[..], [AgentEvent::ReasoningDelta { text, .. }] if &**text == "hmm"));
        assert_eq!(t.text, "Hello", "thoughts are not assistant text");
        // 91: the next message chunk ends the thinking, `ReasoningDone` first; once.
        let ev = translate(
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("!"))),
            &mut t,
        );
        assert!(
            matches!(
                &ev[..],
                [AgentEvent::ReasoningDone { .. }, AgentEvent::TextDelta { text, .. }] if &**text == "!"
            ),
            "{ev:?}"
        );
        let ev = translate(
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("?"))),
            &mut t,
        );
        assert!(matches!(&ev[..], [AgentEvent::TextDelta { .. }]), "{ev:?}");
    }

    #[test]
    fn tool_call_carries_kind_and_finishes_on_terminal_status() {
        let mut t = turn();
        let tc = ToolCall::new("c1", "Read foo.rs")
            .kind(ToolKind::Read)
            .raw_input(json!({"path": "foo.rs"}));
        let ev = translate(SessionUpdate::ToolCall(tc), &mut t);
        assert_eq!(ev.len(), 1);
        let AgentEvent::ToolCallStarted {
            call_id,
            name,
            args,
            ..
        } = &ev[0]
        else {
            panic!("{ev:?}");
        };
        assert_eq!(call_id, "c1");
        assert_eq!(name, "Read foo.rs");
        assert_eq!(*args, json!({"path": "foo.rs", "kind": "read"}));

        // In progress: nothing.
        let ev = translate(
            SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                "c1",
                ToolCallUpdateFields::new().status(ToolCallStatus::InProgress),
            )),
            &mut t,
        );
        assert!(ev.is_empty());

        let ev = translate(
            SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                "c1",
                ToolCallUpdateFields::new()
                    .status(ToolCallStatus::Completed)
                    .content(vec![ToolCallContent::from("fn main() {}")]),
            )),
            &mut t,
        );
        let [
            AgentEvent::ToolCallFinished {
                call_id,
                output,
                is_error,
                ..
            },
        ] = &ev[..]
        else {
            panic!("{ev:?}");
        };
        assert_eq!(
            (call_id.as_str(), output.as_str(), *is_error),
            ("c1", "fn main() {}", false)
        );

        let ev = translate(
            SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                "c2",
                ToolCallUpdateFields::new()
                    .status(ToolCallStatus::Failed)
                    .raw_output(json!("boom")),
            )),
            &mut t,
        );
        let [
            AgentEvent::ToolCallFinished {
                output, is_error, ..
            },
        ] = &ev[..]
        else {
            panic!("{ev:?}");
        };
        assert_eq!((output.as_str(), *is_error), ("boom", true));

        // A call reported already completed yields both events.
        let tc = ToolCall::new("c3", "ls").status(ToolCallStatus::Completed);
        let ev = translate(SessionUpdate::ToolCall(tc), &mut t);
        assert!(matches!(
            &ev[..],
            [
                AgentEvent::ToolCallStarted { .. },
                AgentEvent::ToolCallFinished {
                    is_error: false,
                    ..
                }
            ]
        ));
    }

    /// Persist `t`'s queued messages after a user row; the rows and their
    /// `history()` rendering.
    async fn stored(t: &mut Turn) -> (Vec<(i64, ChatMessage)>, Vec<HistoryEntry>) {
        let store = Store::open_memory().await.unwrap();
        let id = SessionId::new();
        store
            .create_session(id, "omp", None, None, None)
            .await
            .unwrap();
        store.append(id, &ChatMessage::user("go")).await.unwrap();
        for m in std::mem::take(&mut t.messages) {
            store.append(id, &m).await.unwrap();
        }
        let rows = store.messages(id).await.unwrap();
        let history = crate::supervisor::render(&rows, &Default::default(), &Default::default())
            .into_iter()
            .map(|(_, e)| e)
            .collect();
        (rows, history)
    }

    fn chunk(text: &str) -> SessionUpdate {
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from(text)))
    }

    fn update(id: &'static str, fields: ToolCallUpdateFields) -> SessionUpdate {
        SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(id, fields))
    }

    /// 06 §3.6: text and tool calls are stored as ordinary chat messages in
    /// arrival order — text before a call flushed first, the call as an
    /// assistant `tool_use`, its result as a tool row — so `history()` pairs
    /// every card in place, and the parent still gets the whole text.
    #[tokio::test]
    async fn tool_calls_persist_in_order_with_text() {
        let mut t = turn();
        translate(chunk("A"), &mut t);
        let x = ToolCall::new("x", "Read foo.rs")
            .kind(ToolKind::Read)
            .raw_input(json!({"path": "foo.rs"}))
            .status(ToolCallStatus::InProgress);
        translate(SessionUpdate::ToolCall(x), &mut t);
        translate(
            update(
                "x",
                ToolCallUpdateFields::new().status(ToolCallStatus::InProgress),
            ),
            &mut t,
        );
        translate(
            update(
                "x",
                ToolCallUpdateFields::new()
                    .status(ToolCallStatus::Completed)
                    .content(vec![ToolCallContent::from("fn main() {}")]),
            ),
            &mut t,
        );
        translate(chunk("B"), &mut t);
        let y = ToolCall::new("y", "ls")
            .status(ToolCallStatus::Failed)
            .raw_output(json!("boom"));
        translate(SessionUpdate::ToolCall(y), &mut t);
        // A finish for a call never started is not a dangling card.
        translate(
            update(
                "ghost",
                ToolCallUpdateFields::new().status(ToolCallStatus::Completed),
            ),
            &mut t,
        );
        // A repeated finish is not a second row.
        translate(
            update(
                "x",
                ToolCallUpdateFields::new().status(ToolCallStatus::Completed),
            ),
            &mut t,
        );
        let result = t.result(None);
        assert_eq!(result, "AB", "the parent gets the whole text");
        t.close(&result);

        let (rows, history) = stored(&mut t).await;
        let shape: Vec<(ChatRole, String, Vec<&str>, Vec<&str>)> = rows
            .iter()
            .map(|(_, m)| {
                (
                    m.role.clone(),
                    crate::loop_::text_of(&m.content),
                    m.content
                        .tool_calls()
                        .iter()
                        .map(|c| c.call_id.as_str())
                        .collect(),
                    m.content
                        .tool_responses()
                        .iter()
                        .map(|r| r.call_id.as_str())
                        .collect(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            [
                (ChatRole::User, "go".into(), vec![], vec![]),
                (ChatRole::Assistant, "A".into(), vec![], vec![]),
                (ChatRole::Assistant, String::new(), vec!["x"], vec![]),
                (ChatRole::Tool, "fn main() {}".into(), vec![], vec!["x"]),
                (ChatRole::Assistant, "B".into(), vec![], vec![]),
                (ChatRole::Assistant, String::new(), vec!["y"], vec![]),
                (ChatRole::Tool, "ERROR: boom".into(), vec![], vec!["y"]),
                // Closer: the turn ends on an assistant row (invisible in history).
                (ChatRole::Assistant, String::new(), vec![], vec![]),
            ]
        );
        let call = rows[2].1.content.tool_calls()[0];
        assert_eq!(call.fn_name, "Read foo.rs");
        assert_eq!(call.fn_arguments, json!({"path": "foo.rs", "kind": "read"}));

        // (role, text or tool name, result text, is_error); "-" = no result.
        let flat: Vec<(&str, &str, &str, bool)> = history
            .iter()
            .map(|e| match e {
                HistoryEntry::User { text, .. } => ("user", text.as_str(), "-", false),
                HistoryEntry::Assistant { text, .. } => ("assistant", text.as_str(), "-", false),
                HistoryEntry::Tool {
                    name,
                    result: Some((out, is_error)),
                    ..
                } => ("tool", name.as_str(), out.as_str(), *is_error),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            flat,
            [
                ("user", "go", "-", false),
                ("assistant", "A", "-", false),
                ("tool", "Read foo.rs", "fn main() {}", false),
                ("assistant", "B", "-", false),
                ("tool", "ls", "ERROR: boom", true),
            ]
        );
    }

    /// A call the harness never finished is closed at the end of the turn
    /// with an `ERROR: not finished` result, so the card reloads with an
    /// error instead of staying pending; the stored output is capped.
    #[tokio::test]
    async fn unfinished_call_is_closed_with_an_error_at_turn_end() {
        let mut t = turn();
        translate(chunk("working"), &mut t);
        translate(SessionUpdate::ToolCall(ToolCall::new("x", "shell")), &mut t);
        let long = "z".repeat(DISPLAY_OUTPUT_CHARS + 50);
        translate(
            SessionUpdate::ToolCall(
                ToolCall::new("big", "cat")
                    .status(ToolCallStatus::Completed)
                    .raw_output(json!(long)),
            ),
            &mut t,
        );
        let result = t.result(None);
        assert_eq!(result, "working");
        t.close(&result);
        let (rows, history) = stored(&mut t).await;
        assert_eq!(rows.len(), 7, "{rows:?}");
        assert!(t.open.is_empty());

        let cards: Vec<(&str, &str, bool)> = history
            .iter()
            .filter_map(|e| match e {
                HistoryEntry::Tool {
                    name,
                    result: Some((out, is_error)),
                    ..
                } => Some((name.as_str(), out.as_str(), *is_error)),
                HistoryEntry::Tool { result: None, .. } => panic!("pending card: {e:?}"),
                _ => None,
            })
            .collect();
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].0, "cat");
        assert!(
            cards[0].1.contains("…[truncated 50 chars]"),
            "{}",
            cards[0].1
        );
        assert!(!cards[0].2);
        assert_eq!(cards[1], ("shell", NOT_FINISHED, true));

        // No text at all: the fallback is the assistant row, once.
        let mut t = turn();
        translate(
            SessionUpdate::ToolCall(
                ToolCall::new("x", "shell")
                    .status(ToolCallStatus::Completed)
                    .raw_output(json!("out")),
            ),
            &mut t,
        );
        let result = t.result(None);
        assert_eq!(result, "[no reply; last tool result]\nout");
        t.close(&result);
        let (rows, _) = stored(&mut t).await;
        assert_eq!(rows.len(), 4);
        assert_eq!(crate::loop_::text_of(&rows[3].1.content), result);
        assert_eq!(rows[3].1.role, ChatRole::Assistant);
    }

    #[test]
    fn plan_is_structured_state_not_transcript_text() {
        let mut t = turn();
        let plan = Plan::new(vec![
            PlanEntry::new("read", PlanEntryPriority::High, PlanEntryStatus::Completed),
            PlanEntry::new(
                "edit",
                PlanEntryPriority::Medium,
                PlanEntryStatus::InProgress,
            ),
            PlanEntry::new("test", PlanEntryPriority::Low, PlanEntryStatus::Pending),
        ]);
        let items = plan_items(&plan);
        assert_eq!(
            items
                .iter()
                .map(|i| (i.text.as_str(), i.status, i.reason.as_deref()))
                .collect::<Vec<_>>(),
            [
                ("read", Status::Done, None),
                ("edit", Status::InProgress, None),
                ("test", Status::Pending, None),
            ],
            "upstream order and status, nothing renamed or reordered"
        );
        // ACP owns its plan: no theuth cap, no duplicate transcript rendering.
        let long = Plan::new(
            (0..40)
                .map(|i| {
                    PlanEntry::new(
                        format!("step {i}"),
                        PlanEntryPriority::Medium,
                        PlanEntryStatus::Pending,
                    )
                })
                .collect(),
        );
        assert_eq!(plan_items(&long).len(), 40);
        assert!(translate(SessionUpdate::Plan(plan), &mut t).is_empty());
        assert!(t.text.is_empty());
    }

    /// 51 §6.3: the wrapper's own new session has a known-zero baseline
    /// (persisted at `Connected` as a zero total), so replayed and rising
    /// cumulative totals attribute exactly the high water once, under the
    /// inclusive remote scope. An unusable cost never reaches the ledger.
    #[tokio::test]
    async fn cumulative_meter_attributes_high_water_once() {
        let store = Store::open_memory().await.unwrap();
        let child = SessionId::new();
        store.ensure_owner(child, None).await.unwrap();

        assert!(meter_update("omp", "ext", 0.2, "usd", true).is_err());
        assert!(meter_update("omp", "ext", f64::NAN, "USD", true).is_err());

        let zero = meter_update("omp", "ext", 0.0, "USD", true).unwrap();
        assert!(!store.apply_meter(child, zero).await.unwrap().is_empty());
        for amount in [0.20, 0.20, 0.35] {
            let u = meter_update("omp", "ext", amount, "USD", true).unwrap();
            store.apply_meter(child, u).await.unwrap();
        }
        let cost = store.session_cost(child).await.unwrap().unwrap();
        assert_eq!(
            cost.direct.returned,
            vec![Money::from_f64(0.35, "USD").unwrap()]
        );
        assert_eq!(cost.scope, crate::cost::CostScope::OpaqueRemoteInclusive);
        assert_eq!(cost.remote_meter_gaps, 0);
    }

    /// 47 §5.3: the harness owns its plan. The stored snapshot carries the
    /// `acp` source and upstream statuses verbatim; a re-sent plan that
    /// reopens a completed step is stored as sent (no native sticky-done
    /// rule), an unchanged plan keeps its version, and an empty plan is an
    /// authoritative empty list, not a missing one.
    #[tokio::test]
    async fn stored_acp_plan_keeps_source_status_and_reopens() {
        let dir = std::env::temp_dir().join(format!("theuth-acp-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir.join("sessions.sqlite"), None)
            .await
            .unwrap();
        let id = SessionId::new();
        store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        let entry = |c: &str, s| PlanEntry::new(c, PlanEntryPriority::Medium, s);

        let plan = Plan::new(vec![
            entry("read", PlanEntryStatus::Completed),
            entry("edit", PlanEntryStatus::InProgress),
        ]);
        let snap = store
            .save_todos(id, TodoSource::Acp, &plan_items(&plan))
            .await
            .unwrap();
        assert_eq!((snap.version, snap.source), (1, TodoSource::Acp));
        assert_eq!(
            snap.items.iter().map(|i| i.status).collect::<Vec<_>>(),
            [Status::Done, Status::InProgress]
        );
        assert_eq!(store.todos(id).await.unwrap(), snap);

        let same = store
            .save_todos(id, TodoSource::Acp, &plan_items(&plan))
            .await
            .unwrap();
        assert_eq!(same.version, 1, "unchanged plan is not a new version");

        let reopened = Plan::new(vec![
            entry("read", PlanEntryStatus::InProgress),
            entry("edit", PlanEntryStatus::Pending),
        ]);
        let snap = store
            .save_todos(id, TodoSource::Acp, &plan_items(&reopened))
            .await
            .unwrap();
        assert_eq!(snap.version, 2);
        assert_eq!(
            snap.items.iter().map(|i| i.status).collect::<Vec<_>>(),
            [Status::InProgress, Status::Pending],
            "upstream reopened a done step; stored as sent"
        );

        let snap = store.save_todos(id, TodoSource::Acp, &[]).await.unwrap();
        assert_eq!((snap.version, snap.source), (3, TodoSource::Acp));
        assert!(snap.items.is_empty());
        assert_eq!(store.todos(id).await.unwrap(), snap);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn permission_outcome_prefers_once_and_cancels_without_decision() {
        use agent_client_protocol::schema::v1::PermissionOption;
        let opts = vec![
            PermissionOption::new("always", "Always", PermissionOptionKind::AllowAlways),
            PermissionOption::new("once", "Once", PermissionOptionKind::AllowOnce),
            PermissionOption::new("no", "No", PermissionOptionKind::RejectOnce),
        ];
        let id = |o: RequestPermissionOutcome| match o {
            RequestPermissionOutcome::Selected(s) => Some(s.option_id.0.to_string()),
            _ => None,
        };
        assert_eq!(
            id(outcome(&opts, Some(Decision::Allow))).as_deref(),
            Some("once")
        );
        assert_eq!(
            id(outcome(&opts, Some(Decision::Deny))).as_deref(),
            Some("no")
        );
        assert_eq!(id(outcome(&opts, None)), None);
        assert_eq!(
            id(outcome(&opts[..1], Some(Decision::Allow))).as_deref(),
            Some("always")
        );
        assert_eq!(id(outcome(&opts[..1], Some(Decision::Deny))), None);
    }

    /// omp's exact refusal (message plus the `data` the transport appends,
    /// pretty-printed) is busy; so is a bare "already processing"; a
    /// transport failure or an unrelated error is not.
    #[test]
    fn busy_errors_are_recognised() {
        assert!(is_busy(OMP_BUSY));
        assert!(is_busy("Agent Is Already Processing"));
        assert!(is_busy("-32000: {\"reason\": \"session_busy\"}"));
        assert!(!is_busy("process exited: signal 9"));
        assert!(!is_busy("Internal error: prompt too long"));
    }

    /// The text `Harness::prompt` yields for omp's `session_busy` refusal.
    const OMP_BUSY: &str = "Agent is already processing. Use steer() or followUp() to queue messages, or wait for completion.: {\n  \"reason\": \"session_busy\",\n  \"hint\": \"steer|followUp|wait\"\n}";

    // ---- actor tests against an in-process fake harness ---------------

    use agent_client_protocol::schema::ProtocolVersion;
    use agent_client_protocol::schema::v1::{
        InitializeResponse, LoadSessionResponse, NewSessionResponse, PermissionOption,
        PromptResponse, RequestPermissionRequest, SessionConfigOption, SessionNotification,
        SetSessionConfigOptionResponse, StopReason,
    };
    use kameo::actor::Spawn;
    use parking_lot::Mutex;
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
    use tools::ApprovalRequested;

    /// The agent side of a [`LineDuplex`]: the test reads what the client
    /// sent (parsed) and writes raw JSON-RPC lines back.
    pub(crate) struct Fake {
        from_client: UnboundedReceiver<serde_json::Value>,
        to_client: UnboundedSender<String>,
    }

    /// Hands its one duplex to the first `dial`.
    pub(crate) struct FakeDialer(
        Mutex<
            Option<(
                UnboundedSender<serde_json::Value>,
                UnboundedReceiver<String>,
            )>,
        >,
    );

    impl std::fmt::Debug for FakeDialer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("FakeDialer")
        }
    }

    #[async_trait::async_trait]
    impl acp::Dialer for FakeDialer {
        async fn dial(&self) -> Result<acp::LineDuplex, String> {
            let (tx, rx) = self.0.lock().take().ok_or("dialled twice")?;
            let outgoing = futures::sink::unfold(tx, |tx, line: String| async move {
                let v: serde_json::Value = serde_json::from_str(&line).expect("client sent JSON");
                let _ = tx.send(v);
                Ok::<_, std::io::Error>(tx)
            });
            let incoming = futures::stream::unfold(rx, |mut rx| async move {
                rx.recv().await.map(|s| (Ok(s), rx))
            });
            Ok(acp::LineDuplex {
                outgoing: Box::pin(outgoing),
                incoming: Box::pin(incoming),
            })
        }
    }

    impl Fake {
        pub(crate) fn new() -> (Self, Arc<FakeDialer>) {
            let (ctx, crx) = unbounded_channel();
            let (atx, arx) = unbounded_channel();
            (
                Self {
                    from_client: crx,
                    to_client: atx,
                },
                Arc::new(FakeDialer(Mutex::new(Some((ctx, arx))))),
            )
        }

        fn send(&self, v: serde_json::Value) {
            self.to_client.send(v.to_string()).unwrap();
        }

        /// The client's next message, within 10 s (virtual under a paused clock).
        pub(crate) async fn next(&mut self) -> serde_json::Value {
            tokio::time::timeout(Duration::from_secs(10), self.from_client.recv())
                .await
                .expect("client message")
                .expect("client connected")
        }

        fn respond(&self, id: &serde_json::Value, result: impl serde::Serialize) {
            self.send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
        }

        /// `initialize` → `session/new` as omp answers them, announcing
        /// `options`; `agent` is the `agentInfo` to claim (06 §8.4). Checks
        /// the client capabilities theuth must and must not advertise.
        pub(crate) async fn handshake(
            &mut self,
            options: Vec<SessionConfigOption>,
            agent: Option<agent_client_protocol::schema::v1::Implementation>,
        ) {
            let m = self.next().await;
            assert_eq!(m["method"], "initialize", "{m}");
            let caps = &m["params"]["clientCapabilities"];
            assert_eq!(caps["_meta"]["opencode/child-session-updates"], true, "{m}");
            assert!(
                caps.get("elicitation")
                    .is_none_or(serde_json::Value::is_null),
                "{m}"
            );
            let mut init = InitializeResponse::new(ProtocolVersion::V1);
            init.agent_info = agent;
            self.respond(&m["id"], init);
            let m = self.next().await;
            assert_eq!(m["method"], "session/new", "{m}");
            let mut r =
                NewSessionResponse::new(agent_client_protocol::schema::v1::SessionId::new("s1"));
            r.config_options = Some(options);
            self.respond(&m["id"], r);
        }

        /// The next `session/set_config_option`: `(id, configId, value)`.
        async fn set_option(&mut self) -> (serde_json::Value, String, String) {
            let m = self.next().await;
            assert_eq!(m["method"], "session/set_config_option", "{m}");
            let p = &m["params"];
            (
                m["id"].clone(),
                p["configId"].as_str().unwrap().to_owned(),
                p["value"].as_str().unwrap().to_owned(),
            )
        }

        /// The next `session/prompt`: `(id, text)`.
        pub(crate) async fn prompt(&mut self) -> (serde_json::Value, String) {
            let m = self.next().await;
            assert_eq!(m["method"], "session/prompt", "{m}");
            let text = m["params"]["prompt"][0]["text"]
                .as_str()
                .unwrap()
                .to_owned();
            (m["id"].clone(), text)
        }

        /// omp 18.2.9's refusal of a prompt while its loop runs.
        fn busy(&self, id: &serde_json::Value) {
            self.send(json!({"jsonrpc": "2.0", "id": id, "error": {
                "code": -32000,
                "message": "Agent is already processing. Use steer() or followUp() to queue messages, or wait for completion.",
                "data": {"reason": "session_busy", "hint": "steer|followUp|wait"},
            }}));
        }

        pub(crate) fn end_turn(&self, id: &serde_json::Value) {
            self.respond(id, PromptResponse::new(StopReason::EndTurn));
        }

        fn update(&self, u: SessionUpdate) {
            self.send(json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": SessionNotification::new(agent_client_protocol::schema::v1::SessionId::new("s1"), u),
            }));
        }

        fn permission(&self, id: &str, tool_call: ToolCallUpdate, options: Vec<PermissionOption>) {
            self.send(json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "session/request_permission",
                "params": RequestPermissionRequest::new(agent_client_protocol::schema::v1::SessionId::new("s1"), tool_call, options),
            }));
        }
    }

    /// Every `AgentEvent` / `ApprovalRequested` published on the runtime, in order.
    #[derive(kameo::Actor)]
    struct Sink {
        events: UnboundedSender<AgentEvent>,
        approvals: UnboundedSender<ApprovalRequested>,
    }

    impl Message<AgentEvent> for Sink {
        type Reply = ();
        async fn handle(&mut self, e: AgentEvent, _: &mut Context<Self, ()>) {
            let _ = self.events.send(e);
        }
    }

    impl Message<ApprovalRequested> for Sink {
        type Reply = ();
        async fn handle(&mut self, e: ApprovalRequested, _: &mut Context<Self, ()>) {
            let _ = self.approvals.send(e);
        }
    }

    const FAST_RETRY: [Duration; 4] = [
        Duration::from_millis(50),
        Duration::from_millis(100),
        Duration::from_millis(200),
        Duration::from_millis(300),
    ];
    const FAST_QUIET: Duration = Duration::from_millis(300);

    /// One spawned `AcpChild` on a fresh runtime and store, connected to a
    /// [`Fake`] whose handshake is done.
    struct Rig {
        actor: ActorRef<AcpChild>,
        fake: Fake,
        store: Store,
        session: SessionId,
        rt: Runtime,
        events: UnboundedReceiver<AgentEvent>,
        approvals: UnboundedReceiver<ApprovalRequested>,
    }

    impl Rig {
        async fn new(deadline: Duration) -> Self {
            Self::with_options(deadline, Vec::new()).await
        }

        /// A rig whose `session/new` announces `options` (75 §8).
        async fn with_options(deadline: Duration, options: Vec<SessionConfigOption>) -> Self {
            Self::with_agent(deadline, options, None).await
        }

        /// A rig whose harness claims `agent` in `initialize.agentInfo` (06 §8.4).
        async fn with_agent(
            deadline: Duration,
            options: Vec<SessionConfigOption>,
            agent: Option<agent_client_protocol::schema::v1::Implementation>,
        ) -> Self {
            Self::build(deadline, options, agent, |_| {}, true).await
        }

        /// A rig with `tune` applied to the child before it spawns (R2
        /// knobs); `handshake: false` leaves the harness silent.
        async fn tuned(
            deadline: Duration,
            tune: impl FnOnce(&mut AcpChild),
            handshake: bool,
        ) -> Self {
            Self::build(deadline, Vec::new(), None, tune, handshake).await
        }

        async fn build(
            deadline: Duration,
            options: Vec<SessionConfigOption>,
            agent: Option<agent_client_protocol::schema::v1::Implementation>,
            tune: impl FnOnce(&mut AcpChild),
            handshake: bool,
        ) -> Self {
            let dir = std::env::temp_dir().join(format!("theuth-acp-rig-{}", Uuid::now_v7()));
            std::fs::create_dir_all(&dir).unwrap();
            let rt = Runtime::new(figment::Figment::new(), dir.clone());
            let (etx, events) = unbounded_channel();
            let (atx, approvals) = unbounded_channel();
            let sink = Sink::spawn_with_mailbox(
                Sink {
                    events: etx,
                    approvals: atx,
                },
                runtime::subscriber_mailbox(),
            );
            rt.subscribe::<Sink, AgentEvent>(&sink).await;
            rt.subscribe::<Sink, ApprovalRequested>(&sink).await;
            let store = Store::open_memory().await.unwrap();
            let session = SessionId::new();
            store
                .create_session(session, "omp", None, None, None)
                .await
                .unwrap();
            let (mut fake, dialer) = Fake::new();
            let mut child = AcpChild::new(
                session,
                "omp".into(),
                Transport::Dial(dialer),
                dir,
                None,
                None,
                None,
                None,
                None,
                deadline,
                store.clone(),
                rt.clone(),
                Default::default(),
                Vec::new(),
            );
            // The fake stands in for a child process, not a dialled node.
            child.remote = false;
            // Real time, scaled: 5/10/20/30 s → 50/100/200/300 ms, 120 s → 300 ms.
            child.retry = FAST_RETRY;
            child.quiet = FAST_QUIET;
            tune(&mut child);
            let actor = AcpChild::spawn(child);
            if handshake {
                fake.handshake(options, agent).await;
            }
            Self {
                actor,
                fake,
                store,
                session,
                rt,
                events,
                approvals,
            }
        }

        async fn input(&self, text: &str) {
            self.actor
                .tell(UserInput {
                    id: None,
                    text: text.into(),
                    files: Vec::new(),
                    ticket: None,
                    scope: None,
                    defer: false,
                })
                .await
                .unwrap();
        }

        /// The next `HarnessModelChanged` (the connect-time one proves the
        /// actor holds its harness); other events on the way are dropped.
        async fn model_changed(&mut self) -> String {
            loop {
                let e = tokio::time::timeout(Duration::from_secs(10), self.events.recv())
                    .await
                    .expect("an event")
                    .expect("bus alive");
                if let AgentEvent::HarnessModelChanged { model, .. } = e {
                    return model;
                }
            }
        }

        /// Events up to and including the next `TurnFinished`.
        async fn until_finished(&mut self) -> Vec<AgentEvent> {
            let mut out = Vec::new();
            loop {
                let e = tokio::time::timeout(Duration::from_secs(600), self.events.recv())
                    .await
                    .expect("an event")
                    .expect("bus alive");
                let done = matches!(e, AgentEvent::TurnFinished { .. });
                out.push(e);
                if done {
                    return out;
                }
            }
        }

        /// `(role, text)` of every stored row.
        async fn rows(&self) -> Vec<(ChatRole, String)> {
            self.store
                .messages(self.session)
                .await
                .unwrap()
                .into_iter()
                .map(|(_, m)| (m.role, crate::loop_::text_of(&m.content)))
                .collect()
        }
    }

    fn stop_reasons(events: &[AgentEvent]) -> Vec<&str> {
        events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::TurnFinished { stop_reason, .. } => Some(stop_reason.as_str()),
                _ => None,
            })
            .collect()
    }

    fn errors(events: &[AgentEvent]) -> Vec<&str> {
        events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::Error { message, .. } => Some(message.as_str()),
                _ => None,
            })
            .collect()
    }

    /// 75 §10: a child idle after a cancelled turn — the harness may still be
    /// working (06 §8.3) — is stopped by [`Shutdown`], which closes its
    /// connection; a second actor for the same row opens the stored ACP
    /// session with `session/load` and the next prompt goes to the new
    /// process, without a "starting fresh" note.
    #[tokio::test]
    async fn shutdown_then_resume_loads_stored_session() {
        let mut rig = Rig::new(Duration::from_secs(60)).await;
        rig.input("task").await;
        let (id, _) = rig.fake.prompt().await;
        rig.actor.tell(CancelTurn { keep: None }).await.unwrap();
        let m = rig.fake.next().await;
        assert_eq!(m["method"], "session/cancel", "{m}");
        rig.fake
            .respond(&id, PromptResponse::new(StopReason::Cancelled));
        let events = rig.until_finished().await;
        assert_eq!(stop_reasons(&events), ["cancelled"], "{events:?}");
        assert_eq!(
            rig.store.acp_session(rig.session).await.unwrap().as_deref(),
            Some("s1")
        );
        assert!(rig.actor.is_alive(), "idle after cancel, process kept");

        rig.actor.tell(Shutdown).await.unwrap();
        rig.actor.wait_for_shutdown().await;
        assert!(
            tokio::time::timeout(Duration::from_secs(10), rig.fake.from_client.recv())
                .await
                .expect("connection closes")
                .is_none(),
            "the old harness connection is gone"
        );

        let (mut fake, dialer) = Fake::new();
        let mut child = AcpChild::new(
            rig.session,
            "omp".into(),
            Transport::Dial(dialer),
            rig.rt.data_dir.clone(),
            Some("s1".into()),
            None,
            None,
            None,
            None,
            Duration::from_secs(60),
            rig.store.clone(),
            rig.rt.clone(),
            Default::default(),
            Vec::new(),
        );
        child.remote = false;
        let actor = AcpChild::spawn(child);
        // What the supervisor tells a resumed child whose turn was cancelled.
        actor.tell(Note(CANCELLED_NOTE.to_owned())).await.unwrap();
        let m = fake.next().await;
        assert_eq!(m["method"], "initialize", "{m}");
        let mut init = InitializeResponse::new(ProtocolVersion::V1);
        init.agent_capabilities.load_session = true;
        fake.respond(&m["id"], init);
        let m = fake.next().await;
        assert_eq!(m["method"], "session/load", "{m}");
        assert_eq!(m["params"]["sessionId"], "s1", "{m}");
        fake.respond(&m["id"], LoadSessionResponse::new());

        actor
            .tell(UserInput {
                id: None,
                text: "again".into(),
                files: Vec::new(),
                ticket: None,
                scope: None,
                defer: false,
            })
            .await
            .unwrap();
        let (id, text) = fake.prompt().await;
        assert_eq!(
            text,
            format!("<runtime>{CANCELLED_NOTE}</runtime>\nagain"),
            "the note precedes the next prompt on the wire"
        );
        fake.end_turn(&id);
        let events = rig.until_finished().await;
        assert_eq!(stop_reasons(&events), ["end_turn"], "{events:?}");
        assert!(
            errors(&events).is_empty(),
            "no 'starting fresh' note: {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::TurnStarted { text, .. } if text == "again")),
            "the persisted turn text is the input alone: {events:?}"
        );
        // The note is spent: the following prompt is bare.
        actor
            .tell(UserInput {
                id: None,
                text: "more".into(),
                files: Vec::new(),
                ticket: None,
                scope: None,
                defer: false,
            })
            .await
            .unwrap();
        let (id, text) = fake.prompt().await;
        assert_eq!(text, "more");
        fake.end_turn(&id);
        assert_eq!(stop_reasons(&rig.until_finished().await), ["end_turn"]);
    }

    /// 109 §7.3: a second actor for `session` on `rig`'s store and runtime,
    /// told [`Respawn`] before its handshake (the boot trigger's order).
    async fn respawned(
        rig: &Rig,
        session: SessionId,
        resume: Option<&str>,
        transport: Transport,
    ) -> ActorRef<AcpChild> {
        let mut child = AcpChild::new(
            session,
            "omp".into(),
            transport,
            rig.rt.data_dir.clone(),
            resume.map(str::to_owned),
            None,
            None,
            None,
            None,
            Duration::from_secs(60),
            rig.store.clone(),
            rig.rt.clone(),
            Default::default(),
            Vec::new(),
        );
        child.remote = false;
        let actor = AcpChild::spawn(child);
        actor.tell(Respawn(4000)).await.unwrap();
        actor
    }

    /// `initialize` advertising `loadSession`, then the `session/load` request.
    async fn load_request(fake: &mut Fake) -> serde_json::Value {
        let m = fake.next().await;
        assert_eq!(m["method"], "initialize", "{m}");
        let mut init = InitializeResponse::new(ProtocolVersion::V1);
        init.agent_capabilities.load_session = true;
        fake.respond(&m["id"], init);
        let m = fake.next().await;
        assert_eq!(m["method"], "session/load", "{m}");
        assert_eq!(m["params"]["sessionId"], "s1", "{m}");
        m
    }

    fn turn_texts(events: &[AgentEvent]) -> Vec<&str> {
        events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::TurnStarted { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    /// 109 §7.3, `acp_session` present × load ok: the respawned child's
    /// first prompt is the restored note — no delta, no "starting fresh"
    /// error — persisted as the turn's user text, and its result closes
    /// the turn through the ordinary path.
    #[tokio::test]
    async fn respawn_after_successful_load_prompts_restored_note() {
        let mut rig = Rig::new(Duration::from_secs(60)).await;
        rig.actor.tell(Shutdown).await.unwrap();
        rig.actor.wait_for_shutdown().await;
        let (mut fake, dialer) = Fake::new();
        let actor = respawned(&rig, rig.session, Some("s1"), Transport::Dial(dialer)).await;
        let m = load_request(&mut fake).await;
        fake.respond(&m["id"], LoadSessionResponse::new());
        let (id, text) = fake.prompt().await;
        assert!(text.starts_with("<resume_handoff>\n"), "{text}");
        assert!(text.contains("has been restored"), "{text}");
        assert!(!text.contains("could not be restored"), "{text}");
        assert!(!text.contains("session_read"), "no delta: {text}");
        fake.update(chunk("done"));
        fake.end_turn(&id);
        let events = rig.until_finished().await;
        assert_eq!(stop_reasons(&events), ["end_turn"], "{events:?}");
        assert!(errors(&events).is_empty(), "{events:?}");
        assert_eq!(turn_texts(&events), [text.as_str()], "{events:?}");
        let rows = rig.rows().await;
        assert_eq!(
            rows,
            [
                (ChatRole::User, text.clone()),
                (ChatRole::Assistant, "done".into())
            ]
        );
        assert!(actor.is_alive(), "idle after its turn, like any child");
    }

    /// 109 §7.3, `acp_session` present × load fails: the harness falls back
    /// to `session/new`, the existing "not restored" error is published, and
    /// the first prompt is the delta handoff — the child's last report, the
    /// `session_read` pointer, no git sections (the cwd is no repo).
    #[tokio::test]
    async fn respawn_after_failed_load_prompts_delta_handoff() {
        let mut rig = Rig::new(Duration::from_secs(60)).await;
        rig.input("task").await;
        let (id, _) = rig.fake.prompt().await;
        rig.fake.update(chunk("Edited foo.rs so far"));
        rig.fake.end_turn(&id);
        assert_eq!(stop_reasons(&rig.until_finished().await), ["end_turn"]);
        rig.actor.tell(Shutdown).await.unwrap();
        rig.actor.wait_for_shutdown().await;

        let (mut fake, dialer) = Fake::new();
        let actor = respawned(&rig, rig.session, Some("s1"), Transport::Dial(dialer)).await;
        let m = load_request(&mut fake).await;
        fake.send(json!({"jsonrpc": "2.0", "id": m["id"], "error": {
            "code": -32603, "message": "session not found",
        }}));
        let m = fake.next().await;
        assert_eq!(m["method"], "session/new", "falls back: {m}");
        fake.respond(
            &m["id"],
            NewSessionResponse::new(agent_client_protocol::schema::v1::SessionId::new("s2")),
        );
        let (id, text) = fake.prompt().await;
        assert!(text.starts_with("<resume_handoff>\n"), "{text}");
        assert!(text.contains("could not be restored"), "{text}");
        assert!(text.contains("Keep the uncommitted diff"), "{text}");
        assert!(
            text.contains("What you had reported last: \"Edited foo.rs so far\""),
            "{text}"
        );
        assert!(
            text.contains(&format!("session_read(\"{}\")", rig.session)),
            "{text}"
        );
        assert!(!text.contains("Uncommitted work"), "no repo in cwd: {text}");
        assert!(text.len() <= 16_000, "{}", text.len());
        fake.end_turn(&id);
        let events = rig.until_finished().await;
        assert_eq!(stop_reasons(&events), ["end_turn"], "{events:?}");
        assert_eq!(
            errors(&events),
            ["omp: previous harness session not restored; starting fresh"],
            "{events:?}"
        );
        assert_eq!(turn_texts(&events), [text.as_str()], "{events:?}");
        assert_eq!(
            rig.store.acp_session(rig.session).await.unwrap().as_deref(),
            Some("s2")
        );
        assert!(actor.is_alive());
    }

    /// 109 §7.3, `acp_session` absent: nothing to load, so the fresh child
    /// gets the handoff without any "not restored" error.
    #[tokio::test]
    async fn respawn_without_stored_session_prompts_delta_handoff() {
        let mut rig = Rig::new(Duration::from_secs(60)).await;
        let session = SessionId::new();
        rig.store
            .create_session(session, "omp", None, None, None)
            .await
            .unwrap();
        let (mut fake, dialer) = Fake::new();
        let actor = respawned(&rig, session, None, Transport::Dial(dialer)).await;
        fake.handshake(Vec::new(), None).await;
        let (id, text) = fake.prompt().await;
        assert!(text.contains("could not be restored"), "{text}");
        assert!(
            !text.contains("What you had reported"),
            "no transcript: {text}"
        );
        fake.end_turn(&id);
        let events = rig.until_finished().await;
        assert_eq!(stop_reasons(&events), ["end_turn"], "{events:?}");
        assert!(errors(&events).is_empty(), "{events:?}");
        assert!(actor.is_alive());
    }

    #[derive(Debug)]
    struct Refuse;

    #[async_trait::async_trait]
    impl acp::Dialer for Refuse {
        async fn dial(&self) -> Result<acp::LineDuplex, String> {
            Err("harness binary missing".to_owned())
        }
    }

    /// 109 §7.3: a respawn whose harness cannot be connected still runs one
    /// turn — the connect error as its result — so the parent's wait is
    /// answered instead of hanging; the actor then stops.
    #[tokio::test]
    async fn respawn_connect_failure_is_the_turns_error() {
        let mut rig = Rig::new(Duration::from_secs(60)).await;
        rig.actor.tell(Shutdown).await.unwrap();
        rig.actor.wait_for_shutdown().await;
        let actor = respawned(
            &rig,
            rig.session,
            Some("s1"),
            Transport::Dial(Arc::new(Refuse)),
        )
        .await;
        let events = rig.until_finished().await;
        assert_eq!(stop_reasons(&events), ["error"], "{events:?}");
        assert_eq!(turn_texts(&events), [RESPAWN_FAILED_INPUT], "{events:?}");
        assert!(
            errors(&events)
                .iter()
                .any(|e| e.contains("harness binary missing")),
            "{events:?}"
        );
        actor.wait_for_shutdown().await;
        let rows = rig.rows().await;
        assert_eq!(rows[0], (ChatRole::User, RESPAWN_FAILED_INPUT.into()));
        assert!(
            rows[1].1.contains("harness binary missing"),
            "the error is the assistant row: {rows:?}"
        );
    }

    /// 06 §8.3: a `session_busy` refusal keeps the turn open — no
    /// `TurnFinished`, a visible retry note, the session still running —
    /// and the same text is prompted again after the first backoff step;
    /// the accepted attempt runs a normal turn.
    #[tokio::test]
    async fn busy_prompt_is_retried_until_accepted() {
        let mut rig = Rig::new(Duration::from_secs(3600)).await;
        rig.input("go").await;
        let (id, text) = rig.fake.prompt().await;
        assert_eq!(text, "go");
        let refused_at = Instant::now();
        rig.fake.busy(&id);

        let (id, text) = rig.fake.prompt().await;
        assert_eq!(text, "go", "the same input is prompted again");
        assert!(refused_at.elapsed() >= FAST_RETRY[0]);
        assert!(
            rig.actor.ask(IsRunning).await.unwrap(),
            "busy while waiting"
        );
        rig.fake.update(chunk("ok"));
        rig.fake.end_turn(&id);

        let events = rig.until_finished().await;
        assert_eq!(stop_reasons(&events), ["end_turn"], "{events:?}");
        let notes = errors(&events);
        assert_eq!(notes.len(), 1, "{events:?}");
        assert!(
            notes[0].contains("still busy") && notes[0].contains("retrying in"),
            "{}",
            notes[0]
        );
        let started = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::TurnStarted { .. }))
            .count();
        assert_eq!(started, 1, "one turn, not one per attempt");
        assert_eq!(
            rig.rows().await,
            [
                (ChatRole::User, "go".into()),
                (ChatRole::Assistant, "ok".into())
            ]
        );
    }

    /// 06 §8.3: attempts at 0, 5, 15, 35 and 65 s (scaled ×0.01 here); the
    /// refusal at 65 s is past a 40 s deadline and becomes the turn's
    /// error, verbatim.
    #[tokio::test]
    async fn busy_retries_stop_at_the_deadline() {
        let mut rig = Rig::new(Duration::from_millis(500)).await;
        let t0 = Instant::now();
        rig.input("go").await;
        let mut at = Vec::new();
        let mut events = Vec::new();
        loop {
            tokio::select! {
                m = rig.fake.next() => {
                    assert_eq!(m["method"], "session/prompt", "{m}");
                    at.push(t0.elapsed());
                    rig.fake.busy(&m["id"]);
                }
                e = rig.events.recv() => {
                    let e = e.unwrap();
                    let done = matches!(e, AgentEvent::TurnFinished { .. });
                    events.push(e);
                    if done {
                        break;
                    }
                }
            }
        }
        assert_eq!(at.len(), 5, "{at:?}");
        assert!(
            at[3] < Duration::from_millis(500) && at[4] >= Duration::from_millis(500),
            "{at:?}"
        );
        assert_eq!(stop_reasons(&events), ["error"], "{events:?}");
        let notes = errors(&events);
        assert_eq!(
            notes.len(),
            5,
            "four retry notes, then the error: {notes:?}"
        );
        assert_eq!(notes[4], OMP_BUSY);
        let rows = rig.rows().await;
        assert_eq!(rows[0], (ChatRole::User, "go".into()));
        assert_eq!(rows[1], (ChatRole::Assistant, OMP_BUSY.into()));
    }

    /// R2.1: a harness that never answers `initialize` is a failed connect
    /// after `connect_timeout`: the queued input ends as an `error` turn
    /// naming the timeout and the actor stops.
    #[tokio::test]
    async fn silent_harness_times_out_on_connect() {
        let mut rig = Rig::tuned(
            Duration::from_secs(60),
            |c| c.connect_timeout = Duration::from_millis(300),
            false,
        )
        .await;
        rig.input("go").await;
        // `initialize` arrives and is never answered.
        let m = rig.fake.next().await;
        assert_eq!(m["method"], "initialize", "{m}");
        let events = rig.until_finished().await;
        assert_eq!(stop_reasons(&events), ["error"], "{events:?}");
        assert!(
            errors(&events)
                .iter()
                .any(|e| e.contains("did not connect within 0s")),
            "{events:?}"
        );
        rig.actor.wait_for_shutdown().await;
    }

    /// 06 §8.3: updates after the prompt returned open a continuation turn
    /// on a fresh turn id, its cards and text are persisted like a prompted
    /// turn's, the session counts as running, and it closes after the quiet
    /// period as `harness_continued`.
    #[tokio::test]
    async fn updates_after_the_turn_open_a_continuation_turn() {
        let mut rig = Rig::new(Duration::from_secs(3600)).await;
        rig.input("go").await;
        let (id, _) = rig.fake.prompt().await;
        rig.fake.update(chunk("done"));
        rig.fake.end_turn(&id);
        let events = rig.until_finished().await;
        assert_eq!(stop_reasons(&events), ["end_turn"]);
        let AgentEvent::TurnFinished { turn: first, .. } = events.last().unwrap() else {
            unreachable!()
        };
        assert!(!rig.actor.ask(IsRunning).await.unwrap());

        // omp keeps working: a tool call, its result, some text.
        rig.fake.update(SessionUpdate::ToolCall(
            ToolCall::new("c1", "git commit")
                .kind(ToolKind::Execute)
                .status(ToolCallStatus::InProgress),
        ));
        rig.fake.update(update(
            "c1",
            ToolCallUpdateFields::new()
                .status(ToolCallStatus::Completed)
                .content(vec![ToolCallContent::from("[main abc] fix")]),
        ));
        rig.fake.update(chunk("committed"));
        let quiet_from = Instant::now();
        let events = rig.until_finished().await;
        assert!(
            quiet_from.elapsed() >= FAST_QUIET,
            "closed only after the quiet period"
        );
        let kinds: Vec<String> = events
            .iter()
            .map(|e| match e {
                AgentEvent::TurnStarted { turn, text, .. } => {
                    assert_ne!(turn, first, "a fresh turn id");
                    assert_eq!(text, CONTINUED_NOTE);
                    "started".into()
                }
                AgentEvent::ToolCallStarted { call_id, name, .. } => {
                    format!("call {call_id} {name}")
                }
                AgentEvent::ToolCallFinished {
                    call_id, output, ..
                } => format!("done {call_id} {output}"),
                AgentEvent::TextDelta { text, .. } => format!("text {text}"),
                AgentEvent::TurnFinished {
                    turn, stop_reason, ..
                } => {
                    assert_ne!(turn, first);
                    format!("finished {stop_reason}")
                }
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "started",
                "call c1 git commit",
                "done c1 [main abc] fix",
                "text committed",
                "finished harness_continued",
            ]
        );
        assert!(!rig.actor.ask(IsRunning).await.unwrap());

        let rows = rig.store.messages(rig.session).await.unwrap();
        let history: Vec<(String, String)> =
            crate::supervisor::render(&rows, &Default::default(), &Default::default())
                .into_iter()
                .map(|(_, e)| match e {
                    HistoryEntry::User { text, .. } => ("user".into(), text),
                    HistoryEntry::Assistant { text, .. } => ("assistant".into(), text),
                    HistoryEntry::Tool {
                        name,
                        result: Some((out, false)),
                        ..
                    } => (name, out),
                    other => panic!("{other:?}"),
                })
                .collect();
        assert_eq!(
            history,
            [
                ("user".into(), "go".into()),
                ("assistant".into(), "done".into()),
                ("git commit".into(), "[main abc] fix".into()),
                ("assistant".into(), "committed".into()),
            ],
            "load_session keeps the continuation's card and text"
        );
    }

    /// 75 §3/§5: the manifest and usage arrive with no turn open and are
    /// applied — persisted, then published once per distinct value — instead
    /// of dropped as replay. `size` rides on `ContextUsed` and the row; a
    /// turnless update after a turn is attributed to that turn.
    #[tokio::test]
    async fn manifest_and_usage_apply_without_a_turn() {
        use agent_client_protocol::schema::v1::{
            AvailableCommand, AvailableCommandInput, AvailableCommandsUpdate,
            UnstructuredCommandInput,
        };
        let mut rig = Rig::new(Duration::from_secs(3600)).await;
        let manifest = || {
            SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(vec![
                AvailableCommand::new("skill:ponytail", "Lazy senior developer"),
                AvailableCommand::new("/ponytail", "Set the level").input(
                    AvailableCommandInput::Unstructured(UnstructuredCommandInput::new("lite|full")),
                ),
            ]))
        };
        rig.fake.update(manifest());
        rig.fake.update(manifest());
        rig.fake
            .update(SessionUpdate::UsageUpdate(UsageUpdate::new(1_000, 200_000)));
        // The three ride the transport; the prompt is a direct tell. Wait for
        // the size to land so the updates are provably turnless.
        let landed = async {
            loop {
                let row = rig.store.session(rig.session).await.unwrap().unwrap();
                if row.context_size == Some(200_000) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        tokio::time::timeout(Duration::from_secs(10), landed)
            .await
            .expect("usage persisted");
        rig.input("go").await;
        let (id, _) = rig.fake.prompt().await;
        rig.fake.update(SessionUpdate::UsageUpdate(
            UsageUpdate::new(5_000, 200_000).cost(Cost::new(0.25, "USD")),
        ));
        rig.fake.end_turn(&id);
        let events = rig.until_finished().await;

        let manifests: Vec<&Vec<HarnessCommand>> = events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::HarnessCommandsChanged { commands, .. } => Some(commands),
                _ => None,
            })
            .collect();
        assert_eq!(manifests.len(), 1, "published once for two identical sends");
        assert_eq!(
            manifests[0]
                .iter()
                .map(|c| (c.name.as_str(), c.hint.as_deref()))
                .collect::<Vec<_>>(),
            [("skill:ponytail", None), ("/ponytail", Some("lite|full"))]
        );
        assert_eq!(
            rig.store.harness_commands(rig.session).await.unwrap(),
            *manifests[0]
        );
        let used: Vec<(u32, Option<u32>)> = events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::ContextUsed { context, size, .. } => Some((*context, *size)),
                _ => None,
            })
            .collect();
        // The pre-turn update had no turn to publish on; the in-turn one did.
        assert_eq!(used, [(5_000, Some(200_000))]);
        let AgentEvent::TurnFinished { turn, context, .. } = events.last().unwrap() else {
            unreachable!()
        };
        assert_eq!(*context, 5_000);
        let row = rig.store.session(rig.session).await.unwrap().unwrap();
        assert_eq!(row.context_size, Some(200_000));
        let cost = rig.store.session_cost(rig.session).await.unwrap().unwrap();
        assert_eq!(
            cost.direct.returned,
            vec![Money::from_f64(0.25, "USD").unwrap()]
        );

        // omp's `agent_end` usage can land after the prompt response: no
        // turn open, still applied, attributed to the turn just closed.
        rig.fake
            .update(SessionUpdate::UsageUpdate(UsageUpdate::new(6_000, 200_000)));
        let e = tokio::time::timeout(Duration::from_secs(10), rig.events.recv())
            .await
            .unwrap()
            .unwrap();
        match e {
            AgentEvent::ContextUsed {
                turn: t,
                context,
                size,
                ..
            } => assert_eq!((t, context, size), (*turn, 6_000, Some(200_000))),
            other => panic!("{other:?}"),
        }
        assert!(
            !rig.actor.ask(IsRunning).await.unwrap(),
            "no continuation turn"
        );
    }

    /// 75 §7: a `session/prompt` response with `usage` lands as this turn's
    /// `TurnFinished.usage` and as one `remote_turns` row under the harness
    /// model (cache split kept); a response without `usage` writes no row
    /// and reports zero usage.
    #[tokio::test]
    async fn prompt_response_usage_is_the_turn_usage_and_a_ledger_row() {
        let mut rig = Rig::new(Duration::from_secs(3600)).await;
        // The harness announces its model; the row carries it.
        rig.fake.update(
            serde_json::from_value(json!({
                "sessionUpdate": "config_option_update",
                "configOptions": [{"id": "model", "name": "Model", "category": "model", "type": "select",
                    "currentValue": "a/one", "options": [{"value": "a/one", "name": "One"}]}],
            }))
            .unwrap(),
        );
        loop {
            let e = tokio::time::timeout(Duration::from_secs(10), rig.events.recv())
                .await
                .expect("model announced")
                .unwrap();
            if matches!(e, AgentEvent::HarnessModelChanged { .. }) {
                break;
            }
        }
        rig.input("go").await;
        let (id, _) = rig.fake.prompt().await;
        rig.fake.respond(
            &id,
            PromptResponse::new(StopReason::EndTurn).usage(
                AcpUsage::new(1500, 1200, 300)
                    .thought_tokens(50)
                    .cached_read_tokens(1000)
                    .cached_write_tokens(100),
            ),
        );
        let events = rig.until_finished().await;
        let AgentEvent::TurnFinished { usage, .. } = events.last().unwrap() else {
            unreachable!()
        };
        assert_eq!(
            (usage.prompt, usage.completion, usage.cached),
            (2300, 350, 1000),
            "input + cache → prompt, output + thought → completion, cachedRead → cached"
        );
        let cost = rig.store.session_cost(rig.session).await.unwrap().unwrap();
        assert_eq!(
            cost.remote_tokens
                .iter()
                .map(|r| (r.harness.as_str(), r.model.as_deref(), r.turns))
                .collect::<Vec<_>>(),
            [("omp", Some("a/one"), 1)]
        );
        assert_eq!(
            cost.direct.tokens,
            TokenTotals {
                input: 2300,
                output: 350,
                cache_read: 1000,
                cache_write: 100,
                unknown_calls: 0
            }
        );

        // No usage on the wire: zero usage, no row.
        rig.input("again").await;
        let (id, _) = rig.fake.prompt().await;
        rig.fake.end_turn(&id);
        let events = rig.until_finished().await;
        let AgentEvent::TurnFinished { usage, .. } = events.last().unwrap() else {
            unreachable!()
        };
        assert_eq!((usage.prompt, usage.completion, usage.cached), (0, 0, 0));
        let cost = rig.store.session_cost(rig.session).await.unwrap().unwrap();
        assert_eq!(cost.remote_tokens[0].turns, 1);
    }

    /// 06 §8.4: the recorded 2.0.22 and 1.18.30 `PromptResponse.usage`
    /// land as the turn's tokens either way; only the 1.x row is flagged a
    /// lower bound (its figure is the last request's), decided from
    /// `initialize.agentInfo`. No `usage_update` at all leaves the context
    /// gauge at zero with no size and the ledger row intact.
    #[tokio::test]
    async fn opencode_usage_is_flagged_lower_bound_by_version() {
        use agent_client_protocol::schema::v1::Implementation;
        let fixture = |version: &str| -> PromptResponse {
            let path = format!(
                "{}/../acp/fixtures/opencode-{version}/prompt.result.json",
                env!("CARGO_MANIFEST_DIR")
            );
            serde_json::from_str(&std::fs::read_to_string(&path).expect(&path)).unwrap()
        };
        for (version, lower, expect) in [
            ("2.0.22", false, (22273, 527, 18839)),
            ("1.18.30", true, (38635, 44, 38501)),
        ] {
            let mut rig = Rig::with_agent(
                Duration::from_secs(3600),
                Vec::new(),
                Some(Implementation::new("OpenCode", version)),
            )
            .await;
            rig.input("go").await;
            let (id, _) = rig.fake.prompt().await;
            rig.fake.respond(&id, fixture(version));
            let events = rig.until_finished().await;
            let AgentEvent::TurnFinished { usage, context, .. } = events.last().unwrap() else {
                unreachable!()
            };
            assert_eq!(
                (usage.prompt, usage.completion, usage.cached),
                expect,
                "{version}"
            );
            assert_eq!(*context, 0, "{version}: no usage_update");
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e, AgentEvent::ContextUsed { .. }))
            );
            let cost = rig.store.session_cost(rig.session).await.unwrap().unwrap();
            let r = &cost.remote_tokens[0];
            assert_eq!((r.turns, r.lower_bound), (1, lower), "{version}");
            assert_eq!(r.tokens.input, u64::from(expect.0));
            let row = rig.store.session(rig.session).await.unwrap().unwrap();
            assert_eq!(row.context_size, None, "{version}");
        }
    }

    /// 06 §8.4: a sub-agent's call forwarded by opencode 2 keeps its
    /// `<child>:<id>` id through the card events and the stored rows, and
    /// pairs with its result like any other call.
    #[tokio::test]
    async fn child_session_call_ids_with_a_colon_persist_and_pair() {
        let mut t = turn();
        let id = "ses_f00417af0ffewZH4KxaHpEZ653:toolu_0179KkBvErCkm8J5u2hVjtLG";
        let ev = translate(
            SessionUpdate::ToolCall(
                ToolCall::new(id, "Create hello.txt file: write").kind(ToolKind::Edit),
            ),
            &mut t,
        );
        assert!(matches!(&ev[..], [AgentEvent::ToolCallStarted { call_id, .. }] if call_id == id));
        let ev = translate(
            update(
                id,
                ToolCallUpdateFields::new()
                    .status(ToolCallStatus::Completed)
                    .content(vec![ToolCallContent::from("Wrote hello.txt")]),
            ),
            &mut t,
        );
        assert!(
            matches!(&ev[..], [AgentEvent::ToolCallFinished { call_id, output, .. }] if call_id == id && output == "Wrote hello.txt")
        );
        assert!(t.open.is_empty(), "the colon id matched its start");
        let (rows, history) = stored(&mut t).await;
        assert_eq!(rows[1].1.content.tool_calls()[0].call_id, id);
        assert_eq!(rows[2].1.content.tool_responses()[0].call_id, id);
        assert!(matches!(
            &history[1],
            HistoryEntry::Tool { call_id, result: Some((out, false)), .. }
                if call_id == id && out == "Wrote hello.txt"
        ));
    }

    /// 06 §8.3: a `session/request_permission` with no turn open is asked
    /// through the approval actor (destructive → ask) and the human's
    /// answer reaches the harness as `allow_once`, not `cancelled`.
    #[test]
    fn permission_with_no_turn_reaches_approval() {
        crate::with_test_registry(permission_scenario());
    }

    async fn permission_scenario() {
        let mut rig = Rig::new(Duration::from_secs(3600)).await;
        tools::registry::start(&rig.rt).await.unwrap();
        rig.input("go").await;
        let (id, _) = rig.fake.prompt().await;
        rig.fake.end_turn(&id);
        assert_eq!(stop_reasons(&rig.until_finished().await), ["end_turn"]);

        rig.fake.permission(
            "p1",
            ToolCallUpdate::new(
                "c9",
                ToolCallUpdateFields::new()
                    .title("rm -rf build")
                    .kind(ToolKind::Delete)
                    .raw_input(json!({"command": "rm -rf build"})),
            ),
            vec![
                PermissionOption::new("y", "Allow", PermissionOptionKind::AllowOnce),
                PermissionOption::new("n", "Reject", PermissionOptionKind::RejectOnce),
            ],
        );
        let asked = tokio::time::timeout(Duration::from_secs(10), rig.approvals.recv())
            .await
            .expect("approval asked")
            .unwrap();
        assert_eq!(asked.session, rig.session);
        assert_eq!(asked.tool, "rm -rf build");
        assert_eq!(asked.risk, tools::Risk::Destructive);
        assert_eq!(
            asked.args,
            json!({"command": "rm -rf build", "kind": "delete"})
        );
        let approval = ActorRef::<tools::Approval>::lookup(tools::APPROVAL_ACTOR)
            .unwrap()
            .unwrap();
        assert!(
            approval
                .ask(tools::Resolve {
                    id: asked.id,
                    decision: Decision::Allow,
                })
                .await
                .unwrap()
        );
        let reply = rig.fake.next().await;
        assert_eq!(reply["id"], "p1", "{reply}");
        assert_eq!(reply["result"]["outcome"]["outcome"], "selected", "{reply}");
        assert_eq!(reply["result"]["outcome"]["optionId"], "y", "{reply}");
    }

    /// omp 18.3.1's `configOptions`: mode, model, thinking (75 §8).
    fn omp_options(model: &str) -> Vec<SessionConfigOption> {
        serde_json::from_value(json!([
            {"id":"mode","name":"Mode","category":"mode","type":"select","currentValue":"default",
             "options":[{"value":"default","name":"Default"}]},
            {"id":"model","name":"Model","category":"model","type":"select","currentValue":model,
             "options":[{"value":"anthropic/claude-opus-4-8","name":"Claude Opus 4.8"},
                        {"value":"openai/gpt-5.4","name":"GPT-5.4"}]},
            {"id":"thinking","name":"Thinking","category":"thought_level","type":"select","currentValue":"medium",
             "options":[{"value":"off","name":"Off"},{"value":"medium","name":"Medium"},{"value":"xhigh","name":"Extra high"}]}
        ]))
        .unwrap()
    }

    fn current(options: &[acp::ConfigOption], id: &str) -> String {
        options.iter().find(|o| o.id == id).unwrap().current.clone()
    }

    /// kameo lifts a `Result` reply's `Err` into `SendError::HandlerError`; back to the plain result.
    fn reply<T, M>(r: Result<T, kameo::error::SendError<M, String>>) -> Result<T, String> {
        match r {
            Ok(t) => Ok(t),
            Err(kameo::error::SendError::HandlerError(e)) => Err(e),
            Err(e) => panic!("{e}"),
        }
    }

    /// 75 §8, idle: `SetOption{model}` goes straight to the harness, the
    /// response's list is what the caller gets, and the model select feeds
    /// `harness_model` (row + `HarnessModelChanged`). Then `effort` reaches
    /// omp's `thinking` option by category alias, and a harness refusal
    /// comes back with the option's values appended.
    #[tokio::test]
    async fn set_option_when_idle_applies_and_updates_the_harness_model() {
        let mut rig = Rig::with_options(
            Duration::from_secs(3600),
            omp_options("anthropic/claude-opus-4-8"),
        )
        .await;
        // Connect published the initial model.
        assert_eq!(rig.model_changed().await, "anthropic/claude-opus-4-8");
        let session = rig.session;
        let Rig {
            actor, fake, store, ..
        } = &mut rig;
        let ask = actor.ask(SetOption {
            config_id: "model".into(),
            value: "openai/gpt-5.4".into(),
        });
        let (r, ()) = tokio::join!(ask, async {
            let (id, config_id, value) = fake.set_option().await;
            assert_eq!(
                (config_id.as_str(), value.as_str()),
                ("model", "openai/gpt-5.4")
            );
            fake.respond(
                &id,
                SetSessionConfigOptionResponse::new(omp_options("openai/gpt-5.4")),
            );
        });
        let r = reply(r).unwrap();
        assert!(!r.pending);
        assert_eq!(current(&r.options, "model"), "openai/gpt-5.4");
        assert_eq!(
            store
                .session(session)
                .await
                .unwrap()
                .unwrap()
                .harness_model
                .as_deref(),
            Some("openai/gpt-5.4")
        );
        assert_eq!(rig.model_changed().await, "openai/gpt-5.4");
        assert_eq!(
            current(
                &reply(rig.actor.ask(HarnessOptions).await).unwrap(),
                "model"
            ),
            "openai/gpt-5.4"
        );

        // `effort` → the `thought_level` option, whatever the harness calls it.
        let Rig { actor, fake, .. } = &mut rig;
        let ask = actor.ask(SetOption {
            config_id: "effort".into(),
            value: "xhigh".into(),
        });
        let (r, ()) = tokio::join!(ask, async {
            let (id, config_id, value) = fake.set_option().await;
            assert_eq!((config_id.as_str(), value.as_str()), ("thinking", "xhigh"));
            fake.send(json!({"jsonrpc": "2.0", "id": id, "error": {
                "code": -32602, "message": "Unknown ACP thinking level"}}));
        });
        let e = reply(r).unwrap_err();
        assert!(
            e.starts_with("omp: Unknown ACP thinking level")
                && e.ends_with("(has: off, medium, xhigh)"),
            "{e}"
        );
    }

    /// 75 §8 "refuse, never ignore": no `thought_level` option → the error
    /// names what the harness has; a value the option does not list → its
    /// values. Neither reaches the wire.
    #[tokio::test]
    async fn set_option_refuses_a_missing_option_or_an_unlisted_value_with_the_list() {
        let opencode: Vec<SessionConfigOption> = serde_json::from_value(json!([
            {"id":"model","name":"Model","category":"model","type":"select","currentValue":"opencode/big-pickle",
             "options":[{"value":"opencode/big-pickle","name":"Big Pickle"}]},
            {"id":"mode","name":"Session Mode","category":"mode","type":"select","currentValue":"build",
             "options":[{"value":"build","name":"Build"}]}
        ]))
        .unwrap();
        let mut rig = Rig::with_options(Duration::from_secs(3600), opencode).await;
        assert_eq!(rig.model_changed().await, "opencode/big-pickle");
        let e = reply(
            rig.actor
                .ask(SetOption {
                    config_id: "effort".into(),
                    value: "high".into(),
                })
                .await,
        )
        .unwrap_err();
        assert_eq!(
            e,
            "omp: this harness offers no effort option (has: model, mode)"
        );
        let e = reply(
            rig.actor
                .ask(SetOption {
                    config_id: "model".into(),
                    value: "nope/none".into(),
                })
                .await,
        )
        .unwrap_err();
        assert_eq!(
            e,
            "omp: unknown value `nope/none` for model (has: opencode/big-pickle)"
        );
        assert!(
            rig.fake.from_client.try_recv().is_err(),
            "nothing was sent to the harness"
        );
    }

    /// 75 §8 "Mid-turn": a change asked during a prompt is queued
    /// (`pending: true`, nothing on the wire), then sent when the prompt
    /// returns — before the next queued input's `session/prompt` — and the
    /// new model lands as `HarnessModelChanged` right after that turn's finish.
    #[tokio::test]
    async fn set_option_mid_turn_is_queued_and_applied_at_prompt_done() {
        let mut rig = Rig::with_options(
            Duration::from_secs(3600),
            omp_options("anthropic/claude-opus-4-8"),
        )
        .await;
        rig.input("go").await;
        rig.input("again").await;
        let (prompt_id, _) = rig.fake.prompt().await;
        let r = reply(
            rig.actor
                .ask(SetOption {
                    config_id: "model".into(),
                    value: "openai/gpt-5.4".into(),
                })
                .await,
        )
        .unwrap();
        assert!(r.pending);
        assert_eq!(
            current(&r.options, "model"),
            "anthropic/claude-opus-4-8",
            "not yet applied"
        );
        assert!(rig.fake.from_client.try_recv().is_err(), "queued, not sent");
        // An unlisted value is refused at once even mid-turn.
        assert!(
            reply(
                rig.actor
                    .ask(SetOption {
                        config_id: "model".into(),
                        value: "nope".into(),
                    })
                    .await
            )
            .is_err()
        );

        rig.fake.end_turn(&prompt_id);
        let (id, config_id, value) = rig.fake.set_option().await;
        assert_eq!(
            (config_id.as_str(), value.as_str()),
            ("model", "openai/gpt-5.4")
        );
        rig.fake.respond(
            &id,
            SetSessionConfigOptionResponse::new(omp_options("openai/gpt-5.4")),
        );
        let (_, text) = rig.fake.prompt().await;
        assert_eq!(text, "again", "the queued input follows the option change");
        let events = rig.until_finished().await;
        assert_eq!(stop_reasons(&events), ["end_turn"]);
        // The model change precedes the next turn's start.
        loop {
            match rig.events.recv().await.unwrap() {
                AgentEvent::HarnessModelChanged { model, .. } => {
                    assert_eq!(model, "openai/gpt-5.4");
                    break;
                }
                AgentEvent::TurnStarted { .. } => panic!("turn started before the model change"),
                _ => {}
            }
        }
    }

    /// R2.2: a cancel the harness never acknowledges hits the kill timer:
    /// the open turn ends `cancelled` and the input queued behind it gets
    /// its error result too (its ticket is not dropped), then the actor stops.
    #[tokio::test]
    async fn kill_timer_drains_the_queue() {
        let mut rig = Rig::tuned(
            Duration::from_secs(60),
            |c| c.grace = Duration::from_millis(200),
            true,
        )
        .await;
        rig.input("first").await;
        rig.input("second").await;
        let (_, text) = rig.fake.prompt().await;
        assert_eq!(text, "first");
        rig.actor.tell(CancelTurn { keep: None }).await.unwrap();
        let m = rig.fake.next().await;
        assert_eq!(m["method"], "session/cancel", "{m}");
        // No PromptResponse ever comes.
        let first = rig.until_finished().await;
        assert_eq!(stop_reasons(&first), ["cancelled"], "{first:?}");
        let second = rig.until_finished().await;
        assert_eq!(stop_reasons(&second), ["error"], "{second:?}");
        assert!(errors(&second).contains(&"cancelled"), "{second:?}");
        rig.actor.wait_for_shutdown().await;
        let rows = rig.rows().await;
        assert!(
            rows.iter()
                .any(|(r, t)| *r == ChatRole::User && t == "second"),
            "the queued input is in the transcript with its result: {rows:?}"
        );
    }
}
