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
use std::time::Duration;

use acp::{Connect, Harness, Permission, Transport, Update};
use agent_client_protocol::schema::v1::{
    PermissionOptionKind, Plan, PlanEntryStatus, RequestPermissionOutcome,
    SelectedPermissionOutcome, SessionUpdate, ToolCallStatus,
};
use genai::chat::{ChatMessage, MessageContent};
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
use crate::cost::{MeterUpdate, Money};
use crate::loop_::{
    CancelPeerScope, CancelTurn, IsRunning, PEER_INTAKE, PeerInput, PeerMeta, SuppressPeerInputs,
    UserInput, peer_wrapper, refuse_peer,
};
use crate::store::Store;
use crate::store::communication::CommError;
use crate::todo::{Status, Todo, TodoSource};
use crate::{AgentEvent, Usage};

/// Grace after `session/cancel` before the process is killed (06 §3.5).
const CANCEL_GRACE: Duration = Duration::from_secs(5);

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
    /// The `session/prompt` in flight.
    turn: Option<Turn>,
    /// The row's title was checked/set.
    titled: bool,
    /// An unusable `usage_update.cost` was already logged (once per connection).
    meter_warned: bool,
    /// `agents.yaml` `model:` — shown when the harness reports no model (06 §8.2).
    fallback_model: Option<String>,
    /// The model last persisted and published for this session (06 §8.2):
    /// the harness's `configOptions` model select, else [`Self::fallback_model`].
    harness_model: Option<String>,
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
}

/// Per-turn state the translation accumulates into (06 §3.3).
pub(crate) struct Turn {
    /// theuth session.
    session: SessionId,
    /// theuth turn.
    id: TurnId,
    /// Accumulated `agent_message_chunk` text: the assistant message persisted at the end.
    text: String,
    /// Last `tool_call_update` output: what the parent gets when the harness
    /// ends the turn without text (opencode after a rejected permission).
    last_tool: Option<String>,
    /// Last `usage_update.used`; `TurnFinished.context`.
    context: u32,
    /// `usage_update.cost` of the last update (51 §6.3): the harness's
    /// cumulative total, taken by the actor after translation.
    pub(crate) meter: Option<(f64, String)>,
    /// Cancelled with the turn: pending permission asks answer `cancelled`.
    cancel: CancellationToken,
    /// Kill timer armed by [`CancelTurn`].
    kill: Option<AbortHandle>,
    /// Owning A2A request scope of the input that opened the turn (50 §6.3):
    /// the turn is that task's work and reports as such.
    scope: Option<Uuid>,
}

impl Turn {
    pub(crate) fn new(session: SessionId, id: TurnId, scope: Option<Uuid>) -> Self {
        Self {
            session,
            id,
            text: String::new(),
            last_tool: None,
            context: 0,
            meter: None,
            cancel: CancellationToken::new(),
            kill: None,
            scope,
        }
    }
}

/// `tell` (internal): `Harness::connect` returned.
struct Connected(Result<Harness, String>);

/// `tell` (internal): `session/prompt` returned with its stop reason, or failed.
struct PromptDone(Result<String, String>);

/// `tell` (internal): the cancel grace for turn `.0` elapsed.
struct KillTimer(TurnId);

impl AcpChild {
    /// Bind session `id` to harness `name` reached over `transport`, prompting
    /// in `cwd`; `resume` is the stored ACP session id to `session/load`;
    /// `fallback_model` is the entry's `model:` label (06 §8.2).
    #[allow(clippy::too_many_arguments)] // one call site (the supervisor); a builder would only rename the fields
    pub fn new(
        id: SessionId,
        name: String,
        transport: Transport,
        cwd: PathBuf,
        resume: Option<String>,
        fallback_model: Option<String>,
        store: Store,
        rt: Runtime,
    ) -> Self {
        let remote = matches!(transport, Transport::Dial(_));
        let connect = Connect {
            transport,
            cwd,
            resume,
            stderr_log: rt.data_dir.join("logs").join(format!("acp-{name}.log")),
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
            titled: false,
            meter_warned: false,
            fallback_model,
            harness_model: None,
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

    /// First prompt of a fresh row names the session, like a native first turn.
    async fn title(&mut self, text: &str) {
        if self.titled {
            return;
        }
        self.titled = true;
        if let Ok(Some(row)) = self.store.session(self.id).await
            && row.title.is_empty()
        {
            let title: String = text.trim().chars().take(60).collect();
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
        self.turn = Some(Turn::new(self.id, turn, q.scope));
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

    /// Send one `session/prompt`; its result arrives as [`PromptDone`].
    async fn start(&mut self, q: Queued, me: &ActorRef<Self>) {
        let Some(harness) = &self.harness else { return };
        let fut = harness.prompt(q.text.clone());
        if self.open(&q).await.is_err() {
            return;
        }
        self.worked = true;
        let me = me.clone();
        tokio::spawn(async move {
            let _ = me.tell(PromptDone(fut.await)).await;
        });
    }

    /// Close the turn in flight: persist the assistant text (or `error`, the
    /// last tool result, or `"(no text)"`), publish `Error` when given, report
    /// the persisted result by turn ([`PeerTurnFinished`], 50 §6.3), then
    /// `TurnFinished`, then hand the state back to the supervisor
    /// ([`TurnReturned`]; it keeps this child counted until the parent holds
    /// the result).
    async fn finish(&mut self, stop_reason: &str, error: Option<String>) {
        let Some(turn) = self.turn.take() else { return };
        turn.cancel.cancel();
        if let Some(k) = turn.kill {
            k.abort();
        }
        if let Some(e) = &error {
            self.publish(AgentEvent::Error {
                session: self.id,
                turn: Some(turn.id),
                message: e.clone(),
            })
            .await;
        }
        let text = if turn.text.is_empty() {
            error
                .or_else(|| {
                    turn.last_tool
                        .map(|t| format!("[no reply; last tool result]\n{t}"))
                })
                .unwrap_or_else(|| "(no text)".into())
        } else {
            turn.text
        };
        self.append(ChatMessage::assistant(MessageContent::from_text(&text)))
            .await;
        continuation::notify(PeerTurnFinished {
            session: self.id,
            turn: turn.id,
            scope: turn.scope,
            text,
            stop_reason: stop_reason.to_owned(),
        })
        .await;
        self.publish(AgentEvent::TurnFinished {
            session: self.id,
            turn: turn.id,
            stop_reason: stop_reason.to_owned(),
            usage: Usage::default(),
            context: turn.context,
        })
        .await;
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

    /// Start the next queued input, if idle.
    async fn next(&mut self, me: &ActorRef<Self>) {
        if self.turn.is_none()
            && self.harness.is_some()
            && let Some(q) = self.queue.pop_front()
        {
            self.start(q, me).await;
        }
    }

    /// The connection is gone. A child process that exited is known
    /// stopped: end the turn in flight as an error, fail the queue, stop. A
    /// dialled remote is not, once it was ever prompted (work it started can
    /// outlive the prompt and this connection): that work is recorded
    /// unknown first ([`Self::lose`], 47 §4.2 rule 6). The next
    /// `send_message` respawns the child from the store row either way (06 §3.2).
    async fn closed(&mut self, why: String, ctx: &mut Context<Self, ()>) {
        if let Some(h) = self.harness.take() {
            h.close();
        }
        if self.remote && self.worked {
            self.lose(why, ctx).await;
            return;
        }
        self.finish("error", Some(why.clone())).await;
        self.drain(&why).await;
        ctx.stop();
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
    /// (06 §3.4) off the mailbox; a cancelled turn answers `cancelled`.
    fn permission(&self, p: Permission) {
        let Some(turn) = &self.turn else {
            p.respond(RequestPermissionOutcome::Cancelled);
            return;
        };
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
        let cancel = turn.cancel.clone();
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

/// 06 §3.3: one `session/update` → the [`AgentEvent`]s to publish, with the
/// text and context window accumulating on `turn`.
pub(crate) fn translate(update: SessionUpdate, turn: &mut Turn) -> Vec<AgentEvent> {
    let (session, id) = (turn.session, turn.id);
    match update {
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
            let mut out = vec![AgentEvent::ToolCallStarted {
                session,
                turn: id,
                call_id: call_id.clone(),
                name: tc.title,
                args: with_kind(tc.raw_input, tc.kind),
            }];
            // Some harnesses report a finished call in one message.
            if let Some(is_error) = finished(tc.status) {
                let output = output(&tc.content, tc.raw_output.as_ref());
                turn.last_tool = Some(output.clone());
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
                let output = output(
                    u.fields.content.as_deref().unwrap_or_default(),
                    u.fields.raw_output.as_ref(),
                );
                turn.last_tool = Some(output.clone());
                vec![AgentEvent::ToolCallFinished {
                    session,
                    turn: id,
                    call_id: u.tool_call_id.0.to_string(),
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
        SessionUpdate::UsageUpdate(u) => {
            turn.context = u32::try_from(u.used).unwrap_or(u32::MAX);
            turn.meter = u.cost.map(|c| (c.amount, c.currency));
            Vec::new()
        }
        // user_message_chunk (load replay, 06 §8.11), commands, modes, config, info.
        _ => Vec::new(),
    }
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
            tokio::spawn(async move {
                let r = Harness::connect(c, tx).await;
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
        if let Some(h) = self.harness.take() {
            h.close();
        }
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
                let model = h
                    .model()
                    .map(str::to_owned)
                    .or_else(|| self.fallback_model.clone());
                if let Some(model) = model {
                    self.set_harness_model(model).await;
                }
                // 51 §6.3: a session this wrapper created has a known-zero
                // baseline; persist that before any prompt so the first
                // cumulative total is attributed in full.
                // ponytail: the zero row needs a currency; USD is the only one
                // OMP emits. A non-USD harness's first total opens an imported
                // epoch instead — teach `apply_meter` to carry a zero known-zero
                // baseline across a currency change when that shows up.
                if !h.resumed()
                    && let Ok(zero) = meter_update(&self.name, h.session_id(), 0.0, "USD", true)
                {
                    self.apply_meter(zero).await;
                }
                self.harness = Some(h);
                let me = ctx.actor_ref().clone();
                self.next(&me).await;
            }
            Err(e) => {
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
        };
        if let Some(why) = self.dead.clone() {
            self.fail(q, &why).await;
            ctx.stop();
            return;
        }
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
/// supervisor fails the exchange visibly. It also has no reverse peer tools:
/// nothing here pretends it can answer.
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
            text: peer_wrapper(&m.record, m.reply_expected),
            scope: m.request_scope,
            peer: Some(PeerMeta {
                id: m.record.receipt.id,
                generation: m.generation,
                isolated: false,
            }),
        };
        if let Some(why) = self.dead.clone() {
            self.fail(q, &why).await;
            ctx.stop();
            return Ok(());
        }
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
            Update::Session(update) => {
                let Some(turn) = &mut self.turn else { return };
                let events = translate(update, turn);
                let meter = turn.meter.take();
                for ev in events {
                    self.publish(ev).await;
                }
                if let Some((amount, currency)) = meter {
                    self.meter(amount, &currency).await;
                }
            }
            Update::Permission(p) => self.permission(p),
            Update::Closed(why) => self.closed(why, ctx).await,
        }
    }
}

impl Message<PromptDone> for AcpChild {
    type Reply = ();

    async fn handle(&mut self, PromptDone(r): PromptDone, ctx: &mut Context<Self, Self::Reply>) {
        match r {
            Ok(stop) => self.finish(&stop, None).await,
            // A remote prompt that failed is an unknown outcome (the
            // transport may be gone with work still running): the blocker
            // goes on the ledger before this turn returns. The connection
            // itself is kept; a real disconnect follows as `Closed`.
            Err(e) if self.remote => {
                let why = self.unknown(e).await;
                self.finish("error", Some(why)).await;
            }
            Err(e) => self.finish("error", Some(e)).await,
        }
        let me = ctx.actor_ref().clone();
        self.next(&me).await;
    }
}

impl Message<CancelTurn> for AcpChild {
    type Reply = ();

    async fn handle(&mut self, _: CancelTurn, ctx: &mut Context<Self, Self::Reply>) {
        let Some(turn) = &mut self.turn else { return };
        let Some(h) = &self.harness else { return };
        if turn.kill.is_some() {
            return;
        }
        turn.cancel.cancel();
        if let Err(e) = h.cancel() {
            self.closed(e, ctx).await;
            return;
        }
        let id = turn.id;
        let me = ctx.actor_ref().clone();
        turn.kill = Some(
            tokio::spawn(async move {
                tokio::time::sleep(CANCEL_GRACE).await;
                let _ = me.tell(KillTimer(id)).await;
            })
            .abort_handle(),
        );
    }
}

impl Message<KillTimer> for AcpChild {
    type Reply = ();

    async fn handle(&mut self, KillTimer(id): KillTimer, ctx: &mut Context<Self, Self::Reply>) {
        if !self.turn.as_ref().is_some_and(|t| t.id == id) {
            return;
        }
        if let Some(h) = self.harness.take() {
            h.close();
        }
        // Closing our end kills a child process; a remote may still be working.
        if self.remote {
            self.lose("cancel not acknowledged within grace".into(), ctx)
                .await;
            return;
        }
        self.finish("cancelled", None).await;
        ctx.stop();
    }
}

/// A prompt in flight, or an input waiting for the connection/prompt: the
/// row is busy either way (the ledger, not this projection, is authority).
impl Message<IsRunning> for AcpChild {
    type Reply = bool;

    async fn handle(&mut self, _: IsRunning, _: &mut Context<Self, Self::Reply>) -> bool {
        self.turn.is_some() || !self.queue.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use agent_client_protocol::schema::v1::{
        ContentBlock, ContentChunk, Cost, PlanEntry, PlanEntryPriority, ToolCall, ToolCallContent,
        ToolCallUpdate, ToolCallUpdateFields, ToolKind, UsageUpdate,
    };
    use serde_json::json;

    use super::*;

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

        let ev = translate(
            SessionUpdate::UsageUpdate(UsageUpdate::new(12_345, 200_000)),
            &mut t,
        );
        assert!(ev.is_empty());
        assert_eq!(t.context, 12_345);
        assert_eq!(t.meter, None);

        let ev = translate(
            SessionUpdate::UsageUpdate(UsageUpdate::new(12_345, 200_000).cost(Cost::new(0.35, "USD"))),
            &mut t,
        );
        assert!(ev.is_empty());
        assert_eq!(t.meter, Some((0.35, "USD".into())));
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
        assert_eq!(cost.direct.returned, vec![Money::from_f64(0.35, "USD").unwrap()]);
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
        let store = Store::open(&dir.join("sessions.sqlite")).await.unwrap();
        let id = SessionId::new();
        store.create_session(id, "m", None, None, None).await.unwrap();
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
}
