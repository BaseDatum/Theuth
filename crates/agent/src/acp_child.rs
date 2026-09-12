//! [`AcpChild`]: one external harness process bound to one wth session
//! (design doc 06 §3.2–§3.5, §8.7). It answers the same [`UserInput`],
//! [`CancelTurn`] and [`IsRunning`] messages as a native `Session`, so the
//! supervisor, roster, approvals and `<subagent_result>` delivery are
//! unchanged; every `session/update` becomes an [`AgentEvent`] on the wth
//! session id (§3.3), every `session/request_permission` goes through the
//! registry's one approval gate (§3.4, §8.8).

use std::collections::VecDeque;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use acp::{Connect, Harness, Permission, Update};
use agent_client_protocol::schema::v1::{
    PermissionOptionKind, RequestPermissionOutcome, SelectedPermissionOutcome, SessionUpdate,
    ToolCallStatus,
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

use crate::loop_::{CancelTurn, IsRunning, UserInput};
use crate::store::Store;
use crate::{AgentEvent, Usage};

/// Grace after `session/cancel` before the process is killed (06 §3.5).
const CANCEL_GRACE: Duration = Duration::from_secs(5);

/// The actor. Spawn with `AcpChild::spawn(AcpChild::new(..))`; the process
/// starts in `on_start` and dies with the actor.
pub struct AcpChild {
    id: SessionId,
    /// `agents.yaml` key; shown as the turn's model.
    name: String,
    store: Store,
    rt: Runtime,
    /// How to start the process; taken by `on_start`.
    connect: Option<Connect>,
    /// `Some` once `initialize` + `session/new|load` succeeded.
    harness: Option<Harness>,
    /// Why the connection failed; the next input is failed with it.
    dead: Option<String>,
    /// Inputs waiting for the connection or for the prompt in flight.
    queue: VecDeque<(Option<InputId>, String)>,
    /// The `session/prompt` in flight.
    turn: Option<Turn>,
    /// The row's title was checked/set.
    titled: bool,
}

/// Per-turn state the translation accumulates into (06 §3.3).
pub(crate) struct Turn {
    /// wth session.
    session: SessionId,
    /// wth turn.
    id: TurnId,
    /// Accumulated `agent_message_chunk` text: the assistant message persisted at the end.
    text: String,
    /// Last `tool_call_update` output: what the parent gets when the harness
    /// ends the turn without text (opencode after a rejected permission).
    last_tool: Option<String>,
    /// Last `usage_update.used`; `TurnFinished.context`.
    context: u32,
    /// Cancelled with the turn: pending permission asks answer `cancelled`.
    cancel: CancellationToken,
    /// Kill timer armed by [`CancelTurn`].
    kill: Option<AbortHandle>,
}

impl Turn {
    pub(crate) fn new(session: SessionId, id: TurnId) -> Self {
        Self {
            session,
            id,
            text: String::new(),
            last_tool: None,
            context: 0,
            cancel: CancellationToken::new(),
            kill: None,
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
    /// Bind session `id` to harness `name` (`entry`), prompting in `cwd`;
    /// `resume` is the stored ACP session id to `session/load`.
    pub fn new(
        id: SessionId,
        name: String,
        entry: &acp::Entry,
        cwd: PathBuf,
        resume: Option<String>,
        store: Store,
        rt: Runtime,
    ) -> Self {
        let connect = Connect {
            config: entry.config(),
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
            queue: VecDeque::new(),
            turn: None,
            titled: false,
        }
    }

    async fn publish(&self, ev: AgentEvent) {
        self.rt.publish(ev).await;
    }

    async fn append(&self, m: ChatMessage) {
        if let Err(e) = self.store.append(self.id, &m).await {
            tracing::error!(session = %self.id, "append: {e:#}");
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
            && let Err(e) = self
                .store
                .set_title(self.id, &text.trim().chars().take(60).collect::<String>())
                .await
        {
            tracing::warn!(session = %self.id, "set_title: {e:#}");
        }
    }

    /// Open a turn for `text` and persist it as the user message.
    async fn open(&mut self, id: Option<InputId>, text: &str) -> TurnId {
        let turn = TurnId::new();
        if let Some(id) = id {
            self.publish(AgentEvent::InputInjected {
                session: self.id,
                id,
                turn,
            })
            .await;
        }
        self.publish(AgentEvent::TurnStarted {
            session: self.id,
            turn,
            text: text.to_owned(),
            model: self.name.clone(),
        })
        .await;
        self.title(text).await;
        self.append(ChatMessage::user(text)).await;
        turn
    }

    /// Send one `session/prompt`; its result arrives as [`PromptDone`].
    async fn start(&mut self, id: Option<InputId>, text: String, me: &ActorRef<Self>) {
        let Some(harness) = &self.harness else { return };
        let fut = harness.prompt(text.clone());
        let turn = self.open(id, &text).await;
        self.turn = Some(Turn::new(self.id, turn));
        let me = me.clone();
        tokio::spawn(async move {
            let _ = me.tell(PromptDone(fut.await)).await;
        });
    }

    /// Close the turn in flight: persist the assistant text (or `error`, the
    /// last tool result, or `"(no text)"`), publish `Error` when given, then `TurnFinished`.
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
        self.append(ChatMessage::assistant(MessageContent::from_text(text)))
            .await;
        self.publish(AgentEvent::TurnFinished {
            session: self.id,
            turn: turn.id,
            stop_reason: stop_reason.to_owned(),
            usage: Usage::default(),
            context: turn.context,
        })
        .await;
    }

    /// A turn that never reached the harness (06 §8.7): the error is the
    /// assistant message, so `<subagent_result stop="error">` names the cause.
    async fn fail(&mut self, id: Option<InputId>, text: String, why: &str) {
        let turn = self.open(id, &text).await;
        self.turn = Some(Turn::new(self.id, turn));
        self.finish("error", Some(why.to_owned())).await;
    }

    /// Start the next queued input, if idle.
    async fn next(&mut self, me: &ActorRef<Self>) {
        if self.turn.is_none()
            && self.harness.is_some()
            && let Some((id, text)) = self.queue.pop_front()
        {
            self.start(id, text, me).await;
        }
    }

    /// The process is gone: end the turn in flight as an error and stop.
    async fn closed(&mut self, why: String, ctx: &mut Context<Self, ()>) {
        self.finish("error", Some(why)).await;
        if let Some(h) = self.harness.take() {
            h.close();
        }
        // ponytail: inputs queued behind a dead process are dropped; the next
        // `send_message` respawns the child from the store row (06 §3.2).
        if !self.queue.is_empty() {
            tracing::warn!(session = %self.id, dropped = self.queue.len(), "acp child closed with queued inputs");
        }
        ctx.stop();
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
                }]
            }
            // Pending / in-progress / partial content: the card already shows the call.
            None => Vec::new(),
        },
        SessionUpdate::Plan(p) => vec![AgentEvent::TextDelta {
            session,
            turn: id,
            text: Arc::from(format!("\n{}\n", acp::render::render_plan(&p))),
        }],
        SessionUpdate::UsageUpdate(u) => {
            turn.context = u32::try_from(u.used).unwrap_or(u32::MAX);
            Vec::new()
        }
        // user_message_chunk (load replay, 06 §8.11), commands, modes, config, info.
        _ => Vec::new(),
    }
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
                    self.publish(AgentEvent::Error {
                        session: self.id,
                        turn: None,
                        message: format!(
                            "{}: previous harness session not restored; starting fresh",
                            self.name
                        ),
                    })
                    .await;
                }
                if let Err(e) = self.store.set_acp_session(self.id, h.session_id()).await {
                    tracing::warn!(session = %self.id, "set_acp_session: {e:#}");
                }
                self.harness = Some(h);
                let me = ctx.actor_ref().clone();
                self.next(&me).await;
            }
            Err(e) => match self.queue.pop_front() {
                Some((id, text)) => {
                    self.fail(id, text, &e).await;
                    ctx.stop();
                }
                None => {
                    self.publish(AgentEvent::Error {
                        session: self.id,
                        turn: None,
                        message: e.clone(),
                    })
                    .await;
                    self.dead = Some(e);
                }
            },
        }
    }
}

// `files` is dropped: harness inputs come from text tools, so there is no
// image to hand over (17 §5).
impl Message<UserInput> for AcpChild {
    type Reply = ();

    async fn handle(
        &mut self,
        UserInput { id, text, files: _ }: UserInput,
        ctx: &mut Context<Self, Self::Reply>,
    ) {
        if let Some(why) = self.dead.clone() {
            self.fail(id, text, &why).await;
            ctx.stop();
            return;
        }
        if self.harness.is_none() || self.turn.is_some() {
            if let Some(id) = id {
                self.publish(AgentEvent::InputQueued {
                    session: self.id,
                    id,
                    text: text.clone(),
                })
                .await;
            }
            self.queue.push_back((id, text));
            return;
        }
        let me = ctx.actor_ref().clone();
        self.start(id, text, &me).await;
    }
}

impl Message<Update> for AcpChild {
    type Reply = ();

    async fn handle(&mut self, u: Update, ctx: &mut Context<Self, Self::Reply>) {
        match u {
            Update::Session(update) => {
                let Some(turn) = &mut self.turn else { return };
                let events = translate(update, turn);
                for ev in events {
                    self.publish(ev).await;
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
        self.finish("cancelled", None).await;
        ctx.stop();
    }
}

impl Message<IsRunning> for AcpChild {
    type Reply = bool;

    async fn handle(&mut self, _: IsRunning, _: &mut Context<Self, Self::Reply>) -> bool {
        self.turn.is_some()
    }
}

#[cfg(test)]
mod tests {
    use agent_client_protocol::schema::v1::{
        ContentBlock, ContentChunk, Plan, PlanEntry, PlanEntryPriority, PlanEntryStatus, ToolCall,
        ToolCallContent, ToolCallUpdate, ToolCallUpdateFields, ToolKind, UsageUpdate,
    };
    use serde_json::json;

    use super::*;

    fn turn() -> Turn {
        Turn::new(SessionId::new(), TurnId::new())
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
    fn plan_is_text_and_usage_is_context() {
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
        let ev = translate(SessionUpdate::Plan(plan), &mut t);
        let [AgentEvent::TextDelta { text, .. }] = &ev[..] else {
            panic!("{ev:?}");
        };
        assert!(
            text.contains("[x] read") && text.contains("[~] edit") && text.contains("[ ] test"),
            "{text}"
        );
        assert!(
            t.text.is_empty(),
            "plans are not persisted as assistant text"
        );

        let ev = translate(
            SessionUpdate::UsageUpdate(UsageUpdate::new(12_345, 200_000)),
            &mut t,
        );
        assert!(ev.is_empty());
        assert_eq!(t.context, 12_345);
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
