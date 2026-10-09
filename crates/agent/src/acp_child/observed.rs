//! 141 §7.2–§7.8: watch-only children — subagents the harness runs inside
//! itself. An [`acp::ChildAnnounce`] from any source upserts a `sessions`
//! row of `kind = observed` under this ACP child (or under another observed
//! row when nested) and publishes [`AgentEvent::ObservedChildChanged`];
//! child-addressed `session/update`s become the child's own transcript
//! through the parent's [`translate`]; a child-attributed permission ask is
//! keyed to the child row; a cancel is forwarded as `session/cancel` only
//! when the harness said it accepts one. Nothing here is driven: the actor
//! never prompts a child and the supervisor refuses to (141 §5.4).
//!
//! Publication order per observation (§7.5): `ObservedChildChanged` →
//! `TurnStarted` (if a child turn opens) → translated events → `TurnFinished`
//! (if it closes). Detail-only changes are coalesced to one event per
//! [`DETAIL_THROTTLE`] per child.

use std::collections::HashMap;
use std::time::Duration;

use acp::harness::opencode::task_v1;
use acp::omp::{self, Entry, Tailer};
use acp::{ChildAnnounce, ChildSource, ChildState, CostRule, Harness, Permission};
use agent_client_protocol::schema::MaybeUndefined;
use agent_client_protocol::schema::v1::{
    ContentChunk, SessionUpdate, ToolCallStatus, Usage as AcpUsage,
};
use genai::chat::{ChatMessage, MessageContent};
use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use runtime::{SessionId, TurnId};
use tokio::task::AbortHandle;
// tokio's clock, not std's: the throttle and the pending-traffic window
// follow a paused test clock like the actor's other timers.
use tokio::time::Instant;

use super::{AcpChild, NOT_FINISHED, Turn, meter_update, tokens_of, translate};
use crate::cost::TokenTotals;
use crate::store::ObservedUpsert;
use crate::{AgentEvent, ObservedMeta, ObservedSource, ObservedState, Usage};

/// Detail-only changes publish at most once per this interval per child (§7.5).
const DETAIL_THROTTLE: Duration = Duration::from_millis(250);
/// Child traffic that precedes its announce waits this long for it (§6.1);
/// the RFD requires the announce first, adapters may race.
const PENDING_TRAFFIC: Duration = Duration::from_secs(2);
/// omp tail poll period while any omp child is non-terminal (§7.3).
const OMP_POLL: Duration = Duration::from_millis(500);
/// How long the omp tail keeps reading a child after its last change (§6.4,
/// 141 R1 T1): the yield call/result and `session_exit` land in the jsonl
/// in the same second feed 1 reports `completed`.
const OMP_LINGER: Duration = Duration::from_secs(10);
/// `TurnStarted.text` of a watch-only child's turn when the source gave no
/// assignment text: a harness wrapper no UI renders as a user message.
const WATCHED_NOTE: &str = "<runtime>watch-only: the harness runs this subagent</runtime>";
/// `observed_stop` the engine writes when the harness is gone (141 D11).
pub(crate) const LOST_CONTACT: &str = "lost contact";

/// `tell` (supervisor, 141 §7.7): stop watch-only child `child` of this
/// harness — `session/cancel` on its external id when the harness announced
/// `capabilities.cancel`, else an `Error` on the child saying it cannot be
/// stopped here.
#[derive(Clone, Copy, Debug)]
pub struct CancelObserved {
    /// The observed row.
    pub child: SessionId,
}

/// `tell` (internal): the detail throttle for child `.0` elapsed; publish.
struct ObservedDirty(SessionId);

/// `tell` (internal): traffic buffered for unknown external `.0` is stale.
struct ObservedSweep(String);

/// `tell` (internal): the permission ask attributed to child `.0` was
/// answered; `requires_action` → `running` (141 Q5).
pub(super) struct PermissionAnswered(pub(super) String);

/// `tell` (internal): poll the omp transcripts (141 §7.3).
struct OmpTick;

/// Watch-only children of one harness connection (141 §7.2).
#[derive(Default)]
pub(crate) struct Observed {
    /// External id → row.
    by_external: HashMap<String, ObservedChild>,
    /// Child traffic ahead of its announce, with its arrival time.
    pending_traffic: HashMap<String, (Instant, Vec<SessionUpdate>)>,
    /// RFD `session_message_chunk` text accumulating per `(session external id, messageId)`
    /// into one directed line (§6.1).
    directed: HashMap<(String, String), Directed>,
    /// omp feeds; `None` when the harness is not a local omp.
    omp: Option<OmpFeeds>,
}

/// One inter-session message as it accumulates: sender, recipient, text.
#[derive(Clone, Default)]
struct Directed {
    sender: Option<String>,
    recipient: Option<String>,
    text: String,
}

struct ObservedChild {
    id: SessionId,
    /// The row's `parent`: this ACP child, or an observed parent when nested.
    parent: SessionId,
    name: String,
    /// The child's own model (`harness_model`), when the source reports one.
    model: Option<String>,
    meta: ObservedMeta,
    /// Open while `running | requires_action` (§5.2).
    turn: Option<Turn>,
    /// The pending detail-throttle timer.
    dirty: Option<AbortHandle>,
    last_emit: Instant,
    /// omp: the sum of the tail's per-turn `usage.cost.total` so far (141
    /// R1 T2) — the `counted` meter when feed 1 fell silent. On a new
    /// connection it is rebuilt from the transcript prefix the previous life
    /// replayed (R4, `OmpFeeds::rebuild`), so the high-water meter keeps
    /// counting post-restart turns.
    tail_cost: f64,
    /// omp: tool calls and Q3 tokens the tail has seen (141 R3 T10).
    tail_tools: u64,
    tail_tokens: u64,
}

impl ObservedChild {
    fn live(&self) -> bool {
        matches!(
            self.meta.state,
            ObservedState::Running | ObservedState::RequiresAction
        )
    }

    /// The omp tail still reads this child: non-terminal, or terminal for
    /// [`OMP_LINGER`] after its last change (141 R1 T1).
    fn tails(&self) -> bool {
        self.meta.source == ObservedSource::OmpTask
            && (!self.meta.state.is_terminal() || self.last_emit.elapsed() < OMP_LINGER)
    }

    fn event(&self) -> AgentEvent {
        AgentEvent::ObservedChildChanged {
            parent: self.parent,
            child: self.id,
            name: self.name.clone(),
            model: self.model.clone(),
            observed: self.meta.clone(),
        }
    }
}

/// The omp feeds inside the actor (141 §7.3): the tailer once omp wrote the
/// parent transcript, per-child byte cursors, the parent cursor for
/// `async-result` settlement and the poll timer.
struct OmpFeeds {
    tailer: Option<Tailer>,
    cursors: HashMap<String, u64>,
    parent_cursor: u64,
    poll: Option<AbortHandle>,
    /// Children seeded with a stored cursor whose tail sums (`tail_cost`,
    /// `tail_tools`, `tail_tokens`) are still to be rebuilt from the
    /// transcript before that cursor, once the tailer is located (141 R4).
    rebuild: Vec<String>,
}

/// `ChildSource` (acp) → `ObservedSource` (agent): the same four names.
fn source_of(s: ChildSource) -> ObservedSource {
    match s {
        ChildSource::Rfd => ObservedSource::Rfd,
        ChildSource::OpencodeExt => ObservedSource::OpencodeExt,
        ChildSource::OpencodeTask => ObservedSource::OpencodeTask,
        ChildSource::OmpTask => ObservedSource::OmpTask,
    }
}

/// `ChildState` → stored state, stop text (`Some(None)` clears) and the RFD
/// idle usage (141 §6.1 table).
fn state_of(s: &ChildState) -> (ObservedState, Option<Option<String>>, Option<AcpUsage>) {
    match s {
        ChildState::Pending => (ObservedState::Pending, None, None),
        ChildState::Running => (ObservedState::Running, None, None),
        ChildState::RequiresAction => (ObservedState::RequiresAction, None, None),
        ChildState::Idle { stop_reason, usage } => (
            if stop_reason.as_deref() == Some("cancelled") {
                ObservedState::Cancelled
            } else {
                ObservedState::Idle
            },
            Some(stop_reason.clone()),
            usage.clone(),
        ),
        ChildState::Completed => (ObservedState::Completed, None, None),
        ChildState::Failed { error } => (ObservedState::Failed, Some(error.clone()), None),
        ChildState::Cancelled => (ObservedState::Cancelled, None, None),
        ChildState::Unknown => (ObservedState::Unknown, Some(None), None),
    }
}

/// `TurnFinished.stop_reason` when a child turn closes on `state` (§5.2).
fn stop_reason_of(state: ObservedState) -> &'static str {
    match state {
        ObservedState::Failed => "error",
        ObservedState::Cancelled => "cancelled",
        ObservedState::Unknown => "unknown",
        _ => "end_turn",
    }
}

/// omp transcript usage as ledger tokens, in [`tokens_of`]'s terms (input
/// includes the cache).
fn omp_tokens(u: &omp::Usage) -> TokenTotals {
    TokenTotals {
        input: u
            .input
            .saturating_add(u.cache_read)
            .saturating_add(u.cache_write),
        output: u.output,
        cache_read: u.cache_read,
        cache_write: u.cache_write,
        unknown_calls: 0,
    }
}

impl AcpChild {
    /// The harness is omp (`agentInfo.name` `oh-my-pi` / `omp`).
    pub(super) fn is_omp(&self) -> bool {
        self.harness
            .as_ref()
            .and_then(Harness::agent_info)
            .is_some_and(|(name, _)| {
                let n = name.to_ascii_lowercase();
                n == "omp" || n == "oh-my-pi"
            })
    }

    /// `Connected` (141 §7.2 item 5): the stored watch-only children of this
    /// row and, recursively, of theirs; the omp feeds when the harness is a
    /// local omp (a dialled remote has no transcripts here).
    pub(super) async fn observed_seed(&mut self, me: &ActorRef<Self>) {
        let mut stack = vec![self.id];
        while let Some(p) = stack.pop() {
            let rows = match self.store.observed_children(p).await {
                Ok(rows) => rows,
                Err(e) => {
                    tracing::warn!(session = %self.id, "observed_children: {e:#}");
                    continue;
                }
            };
            for (row, meta) in rows {
                stack.push(row.id);
                self.observed.by_external.insert(
                    meta.external.clone(),
                    ObservedChild {
                        id: row.id,
                        parent: p,
                        name: row.name.unwrap_or_default(),
                        model: row.harness_model,
                        meta,
                        turn: None,
                        dirty: None,
                        last_emit: Instant::now(),
                        tail_cost: 0.0,
                        tail_tools: 0,
                        tail_tokens: 0,
                    },
                );
            }
        }
        // D11 (141 R1 T5): the rows were written by a connection that is
        // gone; whatever was running then is unknown until this one observes
        // it again (RFD re-announces, opencode re-streams open children,
        // the omp tail settles from disk).
        self.observed_lost().await;
        if self.is_omp() && !self.remote {
            let mut cursors = HashMap::new();
            let mut rebuild = Vec::new();
            for c in self.observed.by_external.values() {
                if c.meta.source == ObservedSource::OmpTask
                    && let Ok(Some(cur)) = self.store.observed_cursor(c.id).await
                {
                    cursors.insert(c.meta.external.clone(), cur);
                    if cur > 0 {
                        rebuild.push(c.meta.external.clone());
                    }
                }
            }
            self.observed.omp = Some(OmpFeeds {
                tailer: None,
                cursors,
                parent_cursor: 0,
                poll: None,
                rebuild,
            });
            self.omp_poll(me);
        }
    }

    /// 141 §7.2 item 3: a parent-session `tool_call(_update)` that carries omp
    /// task rows announces every row, and an opencode 1.x `task` call
    /// announces its one child (§6.3). The frame still reaches the parent's
    /// transcript as a tool event; the roster card replaces it in the UI only.
    pub(super) async fn observed_task_frame(
        &mut self,
        update: &SessionUpdate,
        me: &ActorRef<Self>,
    ) {
        if self.is_omp() {
            if let Some(frame) = omp::progress_rows(update) {
                for row in &frame.rows {
                    self.observed_upsert(row.announce(&frame), me).await;
                }
            }
        } else if self
            .harness
            .as_ref()
            .is_some_and(Harness::usage_is_lower_bound)
            && let Some(a) = task_v1::announce(update)
        {
            self.observed_upsert(a, me).await;
        }
    }

    /// 141 §7.2 item 1: one announce → the row (`Store::upsert_observed`),
    /// the in-memory entry, `ObservedChildChanged` (immediate on create or a
    /// state/title/cancel/model change, throttled when only `detail`
    /// moved), the child turn opening/closing with the state, the `counted`
    /// meter row, and any traffic that was waiting for this announce.
    pub(super) async fn observed_upsert(&mut self, a: ChildAnnounce, me: &ActorRef<Self>) {
        let source = source_of(a.source);
        // opencode 1.x (§6.3): the provisional `task:<toolCallId>` row learns
        // its real id from the completed frame — same row, re-keyed.
        if source == ObservedSource::OpencodeTask
            && let Some(tc) = &a.tool_call
        {
            let provisional = format!("task:{tc}");
            if a.child != provisional
                && let Some(mut c) = self.observed.by_external.remove(&provisional)
            {
                if let Err(e) = self.store.set_acp_session(c.id, &a.child).await {
                    tracing::warn!(session = %c.id, "set_acp_session: {e:#}");
                }
                c.meta.external = a.child.clone();
                self.observed.by_external.insert(a.child.clone(), c);
            }
        }
        // Nesting (§5.3): an unknown external parent falls back to this row.
        let parent = match &a.parent {
            None => self.id,
            Some(ext) => self.observed.by_external.get(ext).map_or(self.id, |c| c.id),
        };
        let (mut state, stop, usage) = match &a.state {
            Some(s) => {
                let (st, stop, usage) = state_of(s);
                (Some(st), stop, usage)
            }
            None => (None, None, None),
        };
        // F18 (141 R1 T3): a snapshot can only say `pending` about a row
        // that is new or still pending — omp's spawn acknowledgement carries
        // stale rows after the live ones already moved on.
        if state == Some(ObservedState::Pending)
            && self
                .observed
                .by_external
                .get(&a.child)
                .is_some_and(|c| c.meta.state != ObservedState::Pending)
        {
            state = None;
        }
        let mut detail = a.detail.clone();
        if let Some(u) = &usage
            && let Ok(v) = serde_json::to_value(u)
        {
            detail.insert("usage".into(), v);
        }
        let stop_ref = stop.as_ref().map(|s| s.as_deref());
        let mut up = ObservedUpsert::new(parent, source, &a.child);
        up.name = a.title.as_deref();
        up.model = a.model.as_deref();
        up.state = state;
        up.stop = stop_ref;
        up.can_cancel = a.can_cancel;
        up.description = a.description.as_deref();
        up.tool_call = a.tool_call.as_deref();
        up.cost_rule = a.cost_rule;
        up.detail = (!detail.is_empty()).then_some(&detail);
        let (id, created) = match self.store.upsert_observed(up).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(session = %self.id, child = %a.child, "upsert_observed: {e:#}");
                return;
            }
        };
        let Some((meta, name, model)) = self.observed_row(id).await else {
            return;
        };
        let prev = self.observed.by_external.remove(&a.child);
        let (turn, dirty, last_emit, before, (prev_tail_cost, prev_tail_tools, prev_tail_tokens)) =
            match prev {
                Some(p) => (
                    p.turn,
                    p.dirty,
                    p.last_emit,
                    Some((p.meta.state, p.name, p.model, p.meta.can_cancel)),
                    (p.tail_cost, p.tail_tools, p.tail_tokens),
                ),
                None => (None, None, Instant::now(), None, (0.0, 0, 0)),
            };
        let mut c = ObservedChild {
            id,
            parent,
            name,
            model,
            meta,
            turn,
            dirty,
            last_emit,
            tail_cost: prev_tail_cost,
            tail_tools: prev_tail_tools,
            tail_tokens: prev_tail_tokens,
        };
        let significant = created
            || before.as_ref().is_none_or(|(st, n, m, cc)| {
                *st != c.meta.state || *n != c.name || *m != c.model || *cc != c.meta.can_cancel
            });
        if significant {
            self.emit(&mut c).await;
        } else {
            self.emit_throttled(&mut c, me).await;
        }
        if let Some((_, n, m, _)) = &before {
            if *n != c.name {
                self.publish(AgentEvent::TitleChanged {
                    session: c.id,
                    title: c.name.clone(),
                    tags: Vec::new(),
                })
                .await;
            }
            if *m != c.model
                && let Some(model) = &c.model
            {
                self.publish(AgentEvent::HarnessModelChanged {
                    session: c.id,
                    model: model.clone(),
                })
                .await;
            }
        }
        // §6.3 (141 R1 T4): opencode 1.x streams nothing for the child; the
        // completed call's `<task_result>` is its one assistant message.
        if source == ObservedSource::OpencodeTask
            && c.meta.state.is_terminal()
            && let Some(text) = a
                .detail
                .get("result")
                .and_then(serde_json::Value::as_str)
                .filter(|t| !t.is_empty())
        {
            if c.turn.is_none() {
                self.open_child_turn(&mut c, None).await;
            }
            self.apply_child_update(
                &mut c,
                SessionUpdate::AgentMessageChunk(ContentChunk::new(text.into())),
            )
            .await;
        }
        // §5.2: a turn is open while running/requires_action.
        if c.live() {
            if c.turn.is_none() {
                self.open_child_turn(&mut c, None).await;
            }
        } else if c.turn.is_some() {
            let tokens = usage.as_ref().map(tokens_of);
            let stop = stop_reason_of(c.meta.state);
            self.close_child_turn(&mut c, stop, tokens).await;
        }
        // §7.6: only `counted` writes money; the cumulative cost is the
        // high-water meter of `<acp sid>/<external id>`.
        if let Some(cost) = a.detail.get("cost").and_then(serde_json::Value::as_f64) {
            self.meter_child(&c, cost).await;
        }
        let pending = self.observed.pending_traffic.remove(&a.child);
        self.observed.by_external.insert(a.child.clone(), c);
        if source == ObservedSource::OmpTask && self.observed.omp.is_some() {
            self.omp_poll(me);
        }
        if let Some((_, updates)) = pending {
            for u in updates {
                self.observed_traffic(&a.child, u, me).await;
            }
        }
    }

    /// The row as stored: `(observed, name, harness_model)`.
    async fn observed_row(&self, id: SessionId) -> Option<(ObservedMeta, String, Option<String>)> {
        match self.store.session(id).await {
            Ok(Some(row)) => {
                let meta = row.observed?;
                Some((meta, row.name.unwrap_or_default(), row.harness_model))
            }
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(session = %id, "session: {e:#}");
                None
            }
        }
    }

    async fn emit(&self, c: &mut ObservedChild) {
        if let Some(d) = c.dirty.take() {
            d.abort();
        }
        c.last_emit = Instant::now();
        self.publish(c.event()).await;
    }

    /// Detail-only change: now when the throttle allows, else once when it does.
    async fn emit_throttled(&self, c: &mut ObservedChild, me: &ActorRef<Self>) {
        let since = c.last_emit.elapsed();
        if since >= DETAIL_THROTTLE {
            self.emit(c).await;
        } else if c.dirty.is_none() {
            c.dirty = Some(Self::timer(
                me,
                DETAIL_THROTTLE - since,
                ObservedDirty(c.id),
            ));
        }
    }

    /// Write and publish a state the engine (not the harness) decided:
    /// traffic proving activity, a permission ask, an answered ask, lost contact.
    async fn set_child_state(
        &self,
        c: &mut ObservedChild,
        state: ObservedState,
        stop: Option<Option<&str>>,
    ) {
        let mut up = ObservedUpsert::new(c.parent, c.meta.source, &c.meta.external);
        up.state = Some(state);
        up.stop = stop;
        if let Err(e) = self.store.upsert_observed(up).await {
            tracing::warn!(session = %c.id, "upsert_observed: {e:#}");
            return;
        }
        c.meta.state = state;
        if let Some(s) = stop {
            c.meta.stop = s.map(str::to_owned);
        }
        self.emit(c).await;
    }

    /// Open the child's turn: `TurnStarted` on the child id, model = the
    /// child's own, else the harness's, else the harness name.
    async fn open_child_turn(&self, c: &mut ObservedChild, text: Option<String>) {
        let turn = TurnId::new();
        self.publish(AgentEvent::TurnStarted {
            session: c.id,
            turn,
            text: text.unwrap_or_else(|| WATCHED_NOTE.to_owned()),
            model: c
                .model
                .clone()
                .or_else(|| self.harness_model.clone())
                .unwrap_or_else(|| self.name.clone()),
            turn_seq: 0,
        })
        .await;
        c.turn = Some(Turn::new(c.id, turn, None));
    }

    /// Close the child's turn: unfinished calls answered, text flushed and
    /// persisted on the child, `tokens` (RFD idle usage) on the ledger under
    /// the child, then `TurnFinished`.
    async fn close_child_turn(
        &self,
        c: &mut ObservedChild,
        stop_reason: &str,
        tokens: Option<TokenTotals>,
    ) {
        let Some(mut turn) = c.turn.take() else {
            return;
        };
        turn.cancel.cancel();
        turn.disarm();
        turn.close("");
        // The parent's turn ends on an assistant row by invariant; a
        // watch-only child's history has no such reader, and the row would
        // be an empty bubble (141 R1 T3).
        for m in std::mem::take(&mut turn.messages) {
            if m.role == genai::chat::ChatRole::Assistant
                && m.content.tool_calls().is_empty()
                && crate::loop_::text_of(&m.content).is_empty()
            {
                continue;
            }
            if let Err(e) = self.store.append(c.id, &m).await {
                tracing::error!(session = %c.id, "append: {e:#}");
            }
        }
        let usage = match &tokens {
            Some(t) => {
                self.record_child_tokens(c, turn.id, t).await;
                Usage {
                    prompt: u32::try_from(t.input).unwrap_or(u32::MAX),
                    completion: u32::try_from(t.output).unwrap_or(u32::MAX),
                    cached: u32::try_from(t.cache_read).unwrap_or(u32::MAX),
                }
            }
            None => Usage::default(),
        };
        self.publish(AgentEvent::TurnFinished {
            session: c.id,
            turn: turn.id,
            stop_reason: stop_reason.to_owned(),
            usage,
            context: 0,
        })
        .await;
    }

    /// Tokens the source reported for the child → `remote_turns` under the
    /// child (§7.4; money stays with the cost rule).
    async fn record_child_tokens(&self, c: &ObservedChild, turn: TurnId, t: &TokenTotals) {
        match self
            .store
            .record_remote_turn(c.id, turn, &self.name, c.model.as_deref(), t, false)
            .await
        {
            Ok(bumped) => {
                for (session, revision) in bumped {
                    self.publish(AgentEvent::ModelUsageChanged { session, revision })
                        .await;
                }
            }
            Err(e) => tracing::warn!(session = %c.id, "record_remote_turn: {e:#}"),
        }
    }

    /// §7.6: a `counted` child's cumulative USD → the high-water meter of
    /// `<acp sid>/<external id>` under the child. Feed 1's `detail.cost` and
    /// the tail's summed `usage.cost.total` (141 R1 T2) measure the same
    /// money, so the higher wins and nothing doubles. Other cost rules write
    /// nothing.
    async fn meter_child(&self, c: &ObservedChild, cost: f64) {
        if c.meta.cost_rule != CostRule::Counted {
            return;
        }
        let Some(h) = &self.harness else { return };
        let Ok(u) = meter_update(
            &self.name,
            &format!("{}/{}", h.session_id(), c.meta.external),
            cost,
            "USD",
            true,
        ) else {
            return;
        };
        match self.store.apply_observed_meter(c.id, u).await {
            Ok(bumped) => {
                for (session, revision) in bumped {
                    self.publish(AgentEvent::ModelUsageChanged { session, revision })
                        .await;
                }
            }
            Err(e) => tracing::warn!(session = %c.id, "apply_observed_meter: {e:#}"),
        }
    }

    /// 141 §7.2 item 2: a `session/update` addressed to external id `ext`.
    /// Known child → its transcript (opening a turn, and moving a
    /// pending/idle/unknown child to `running` — traffic proves activity);
    /// unknown → buffered [`PENDING_TRAFFIC`] for the announce, then dropped.
    pub(super) async fn observed_traffic(
        &mut self,
        ext: &str,
        update: SessionUpdate,
        me: &ActorRef<Self>,
    ) {
        if matches!(
            update,
            SessionUpdate::SessionMessage(_) | SessionUpdate::SessionMessageChunk(_)
        ) {
            self.observed_directed(ext, update).await;
            return;
        }
        // Text and tool calls are the child's transcript; a child's plan,
        // usage or mode bookkeeping has no slot here and opens no turn.
        if !Self::continues(&update) {
            return;
        }
        let Some(mut c) = self.observed.by_external.remove(ext) else {
            let e = self
                .observed
                .pending_traffic
                .entry(ext.to_owned())
                .or_insert_with(|| (Instant::now(), Vec::new()));
            if e.1.is_empty() {
                Self::timer(me, PENDING_TRAFFIC, ObservedSweep(ext.to_owned()));
            }
            e.1.push(update);
            return;
        };
        if matches!(
            c.meta.state,
            ObservedState::Pending | ObservedState::Idle | ObservedState::Unknown
        ) {
            self.set_child_state(&mut c, ObservedState::Running, Some(None))
                .await;
        }
        let terminal = c.meta.state.is_terminal();
        if c.turn.is_none() {
            self.open_child_turn(&mut c, None).await;
        }
        self.apply_child_update(&mut c, update).await;
        // Late content for a finished child (opencode 1.x `task_result`, a
        // trailing frame): shown and stored, but no turn stays open.
        if terminal {
            self.close_child_turn(&mut c, "end_turn", None).await;
        }
        self.observed.by_external.insert(ext.to_owned(), c);
    }

    /// `translate` on the child's open turn; rows exist before the events (29 §4.2).
    async fn apply_child_update(&self, c: &mut ObservedChild, update: SessionUpdate) {
        let Some(turn) = &mut c.turn else { return };
        // 141 R3 T7: a result for a call this turn does not hold — the call
        // is in the child's stored transcript (an earlier turn closed it
        // "not finished", or a previous engine life stored the call alone).
        // Let `translate` pair it as usual, then patch the transcript
        // instead of appending a dangling response.
        let late = match &update {
            SessionUpdate::ToolCallUpdate(u)
                if matches!(
                    u.fields.status,
                    Some(ToolCallStatus::Completed | ToolCallStatus::Failed)
                ) && !turn.open.iter().any(|o| **o == *u.tool_call_id.0) =>
            {
                let id = u.tool_call_id.0.to_string();
                turn.open.push(id.clone());
                Some(id)
            }
            _ => None,
        };
        let events = translate(update, turn);
        for m in std::mem::take(&mut turn.messages) {
            let r = match &late {
                Some(id) if m.content.tool_responses().iter().any(|r| r.call_id == *id) => self
                    .store
                    .finish_tool_call(c.id, &m, NOT_FINISHED)
                    .await
                    .map(|_| ()),
                _ => self.store.append(c.id, &m).await.map(|_| ()),
            };
            if let Err(e) = r {
                tracing::error!(session = %c.id, "append: {e:#}");
            }
        }
        for ev in events {
            self.publish(ev).await;
        }
    }

    /// 141 §6.1: `session_message` / `session_message_chunk` on session `ext`
    /// (the harness's own, or a child's): one directed line in that
    /// transcript — `→ <recipient>: text` when it sent, `← <sender>: text`
    /// when it received — published as `MessageDelivered` and stored as an
    /// assistant row. Chunks accumulate per `messageId`; a whole message
    /// replaces them.
    pub(super) async fn observed_directed(&mut self, ext: &str, update: SessionUpdate) {
        let (message_id, sender, recipient, text, whole) = match update {
            SessionUpdate::SessionMessage(m) => {
                let text = match &m.content {
                    MaybeUndefined::Value(blocks) => blocks
                        .iter()
                        .map(acp::render::content_block_text)
                        .collect::<Vec<_>>()
                        .join(""),
                    _ => String::new(),
                };
                (
                    m.message_id.0.to_string(),
                    m.sender_session_id.map(|s| s.0.to_string()),
                    m.recipient_session_id.map(|s| s.0.to_string()),
                    text,
                    true,
                )
            }
            SessionUpdate::SessionMessageChunk(m) => (
                m.message_id.0.to_string(),
                m.sender_session_id.map(|s| s.0.to_string()),
                m.recipient_session_id.map(|s| s.0.to_string()),
                acp::render::content_block_text(&m.content),
                false,
            ),
            _ => return,
        };
        let key = (ext.to_owned(), message_id);
        let e = self.observed.directed.entry(key).or_default();
        if sender.is_some() {
            e.sender = sender;
        }
        if recipient.is_some() {
            e.recipient = recipient;
        }
        if whole {
            e.text = text;
        } else {
            e.text.push_str(&text);
        }
        let Directed {
            sender,
            recipient,
            text,
        } = e.clone();
        let own = self.harness.as_ref().map(Harness::session_id);
        let session = if Some(ext) == own {
            self.id
        } else {
            match self.observed.by_external.get(ext) {
                Some(c) => c.id,
                None => return,
            }
        };
        let label = |id: &Option<String>| -> String {
            match id.as_deref() {
                Some(x) if Some(x) == own => "harness".to_owned(),
                Some(x) => self
                    .observed
                    .by_external
                    .get(x)
                    .map_or_else(|| x.to_owned(), |c| c.name.clone()),
                None => "?".to_owned(),
            }
        };
        let (from, line) = if sender.as_deref() == Some(ext) {
            let to = label(&recipient);
            (format!("→ {to}"), format!("→ {to}: {text}"))
        } else {
            let from = label(&sender);
            (format!("← {from}"), format!("← {from}: {text}"))
        };
        // Chunks stream; the stored row is written once the message is whole
        // (a `session_message` upsert) so the transcript holds one line.
        if whole
            && let Err(e) = self
                .store
                .append(
                    session,
                    &ChatMessage::assistant(MessageContent::from_text(&line)),
                )
                .await
        {
            tracing::error!(session = %session, "append: {e:#}");
        }
        self.publish(AgentEvent::MessageDelivered {
            session,
            from,
            text,
        })
        .await;
    }

    /// 141 §7.2 item 4: a permission ask from a child session (RFD: wire
    /// `sessionId` = the child) or for a child's tool (opencode:
    /// `toolCallId` = `<child>:<id>`) is asked on the child row and the child
    /// shows `requires_action` until answered (Q5). `Some(p)` when the ask is
    /// the harness's own.
    pub(super) async fn observed_permission(
        &mut self,
        p: Permission,
        me: &ActorRef<Self>,
    ) -> Option<Permission> {
        let own = self.harness.as_ref().map(Harness::session_id);
        let ext = if Some(p.session.as_str()) != own {
            Some(p.session.clone())
        } else {
            p.tool_call
                .tool_call_id
                .0
                .split_once(':')
                .map(|(c, _)| c.to_owned())
                .filter(|c| self.observed.by_external.contains_key(c))
        };
        let Some(ext) = ext else {
            return Some(p);
        };
        let Some(mut c) = self.observed.by_external.remove(&ext) else {
            return Some(p);
        };
        if c.meta.state != ObservedState::RequiresAction {
            self.set_child_state(&mut c, ObservedState::RequiresAction, None)
                .await;
        }
        if c.turn.is_none() {
            self.open_child_turn(&mut c, None).await;
        }
        let cancel = c
            .turn
            .as_ref()
            .map_or_else(|| self.alive.clone(), |t| t.cancel.clone());
        let session = c.id;
        self.observed.by_external.insert(ext.clone(), c);
        self.ask_permission(p, session, cancel, Some((me.clone(), ext)));
        None
    }

    /// Every non-terminal child becomes `unknown` / "lost contact" (141 D11,
    /// §7.2 item 6): the harness that ran them is gone. omp children settle
    /// later from the transcript tail once a new connection resumes it.
    pub(super) async fn observed_lost(&mut self) {
        if let Some(f) = &mut self.observed.omp
            && let Some(p) = f.poll.take()
        {
            p.abort();
        }
        let exts: Vec<String> = self
            .observed
            .by_external
            .iter()
            .filter(|(_, c)| !c.meta.state.is_terminal() && c.meta.state != ObservedState::Unknown)
            .map(|(k, _)| k.clone())
            .collect();
        for ext in exts {
            let Some(mut c) = self.observed.by_external.remove(&ext) else {
                continue;
            };
            self.set_child_state(&mut c, ObservedState::Unknown, Some(Some(LOST_CONTACT)))
                .await;
            self.close_child_turn(&mut c, "unknown", None).await;
            self.observed.by_external.insert(ext, c);
        }
    }

    /// Arm the omp poll if it is not running and any omp child still tails
    /// (non-terminal, or terminal within [`OMP_LINGER`]).
    fn omp_poll(&mut self, me: &ActorRef<Self>) {
        let any = self.observed.by_external.values().any(ObservedChild::tails);
        let Some(f) = &mut self.observed.omp else {
            return;
        };
        if any && f.poll.is_none() {
            f.poll = Some(Self::timer(me, OMP_POLL, OmpTick));
        }
    }

    /// One omp tail poll (141 §7.3): locate the transcripts if not yet, read
    /// each non-terminal omp child from its cursor → the child's transcript,
    /// persist the cursor; `<id>.md` or a parent `async-result` for the job
    /// → `completed`.
    async fn omp_tick(&mut self, me: &ActorRef<Self>) {
        let Some(f) = &mut self.observed.omp else {
            return;
        };
        f.poll = None;
        if f.tailer.is_none()
            && let Some(h) = &self.harness
        {
            f.tailer = Tailer::locate(&self.omp_home, h.session_id());
        }
        let Some(tailer) = f.tailer.clone() else {
            self.omp_poll(me);
            return;
        };
        let settled = match tailer.settled_jobs(f.parent_cursor) {
            Ok((jobs, cursor)) => {
                f.parent_cursor = cursor;
                jobs
            }
            Err(e) => {
                tracing::debug!(session = %self.id, "omp settled_jobs: {e}");
                Vec::new()
            }
        };
        // 141 R4 (T2 follow-through): the sums behind the `counted` meter and
        // the tail stats restart with the process; rebuild them from the part
        // of each transcript the previous life already replayed (before its
        // stored cursor), then the reads below continue incrementally.
        for ext in std::mem::take(&mut f.rebuild) {
            let cursor = f.cursors.get(&ext).copied().unwrap_or(0);
            let Some(c) = self.observed.by_external.get_mut(&ext) else {
                continue;
            };
            match tailer.read_until(&ext, cursor) {
                Ok(entries) => {
                    for e in &entries {
                        if let Entry::Assistant { calls, usage, .. } = e {
                            c.tail_tools += calls.len() as u64;
                            if let Some(u) = usage {
                                c.tail_tokens += u.input + u.output + u.cache_write;
                                c.tail_cost += u.cost;
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::debug!(session = %self.id, child = %ext, "omp tail rebuild: {e}")
                }
            }
        }
        let exts: Vec<String> = self
            .observed
            .by_external
            .values()
            .filter(|c| c.tails())
            .map(|c| c.meta.external.clone())
            .collect();
        for ext in exts {
            let cursor = self
                .observed
                .omp
                .as_ref()
                .and_then(|f| f.cursors.get(&ext).copied())
                .unwrap_or(0);
            match tailer.read(&ext, cursor) {
                Ok((entries, next)) => {
                    if !entries.is_empty() {
                        self.omp_entries(&ext, &entries, me).await;
                    }
                    if next != cursor {
                        if let Some(c) = self.observed.by_external.get(&ext)
                            && let Err(e) = self.store.set_observed_cursor(c.id, next).await
                        {
                            tracing::warn!(session = %c.id, "set_observed_cursor: {e:#}");
                        }
                        if let Some(f) = &mut self.observed.omp {
                            f.cursors.insert(ext.clone(), next);
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => tracing::debug!(session = %self.id, child = %ext, "omp tail: {e}"),
            }
            if (settled.contains(&ext) || tailer.sidecar_done(&ext))
                && let Some(mut c) = self.observed.by_external.remove(&ext)
            {
                if !c.meta.state.is_terminal() {
                    // `<id>.json` → `detail.result` (§6.4) before the
                    // terminal event so the roster shows it with "done".
                    if let Some(r) = tailer.result_json(&ext) {
                        let mut detail = serde_json::Map::new();
                        detail.insert("result".into(), r);
                        let mut up = ObservedUpsert::new(c.parent, c.meta.source, &c.meta.external);
                        up.detail = Some(&detail);
                        if self.store.upsert_observed(up).await.is_ok() {
                            c.meta.detail["result"] = detail["result"].clone();
                        }
                    }
                    self.set_child_state(&mut c, ObservedState::Completed, Some(None))
                        .await;
                    self.close_child_turn(&mut c, "end_turn", None).await;
                }
                self.observed.by_external.insert(ext, c);
            }
        }
        self.omp_poll(me);
    }

    /// Transcript entries of omp child `ext` (141 §6.4 feed 2 table): the
    /// first user entry opens the turn with the assignment text, a model
    /// change names the child's model when unknown, assistant usage goes on
    /// the child's token ledger, and the rest replays through `translate`
    /// via `omp::to_updates`.
    async fn omp_entries(&mut self, ext: &str, entries: &[Entry], me: &ActorRef<Self>) {
        let Some(mut c) = self.observed.by_external.remove(ext) else {
            return;
        };
        let mut context = None;
        for e in entries {
            match e {
                Entry::User(text) if c.turn.is_none() => {
                    if !c.live() {
                        self.set_child_state(&mut c, ObservedState::Running, Some(None))
                            .await;
                    }
                    self.open_child_turn(&mut c, Some(text.clone())).await;
                }
                Entry::Model(m) if c.model.is_none() => {
                    let mut up = ObservedUpsert::new(c.parent, c.meta.source, &c.meta.external);
                    up.model = Some(m.as_str());
                    if self.store.upsert_observed(up).await.is_ok() {
                        c.model = Some(m.clone());
                        self.publish(AgentEvent::HarnessModelChanged {
                            session: c.id,
                            model: m.clone(),
                        })
                        .await;
                    }
                }
                Entry::Assistant { calls, usage, .. } => {
                    c.tail_tools += calls.len() as u64;
                    if let Some(u) = usage {
                        self.record_child_tokens(&c, TurnId::new(), &omp_tokens(u))
                            .await;
                        c.tail_tokens += u.input + u.output + u.cache_write;
                        context = Some(u.total_tokens);
                        if u.cost > 0.0 {
                            c.tail_cost += u.cost;
                            self.meter_child(&c, c.tail_cost).await;
                        }
                    }
                }
                _ => {}
            }
        }
        self.omp_tail_stats(&mut c, context, me).await;
        self.observed.by_external.insert(ext.to_owned(), c);
        for u in omp::to_updates(entries) {
            self.observed_traffic(ext, u, me).await;
        }
        // §5.2 (141 R1 T5): omp aborted the job (its exit, a cancel) — the
        // closing assistant entry says so; `Command aborted` tool results
        // landed above, inside the turn.
        let aborted = entries
            .iter()
            .any(|e| matches!(e, Entry::Assistant { stop_reason: Some(r), .. } if r == "aborted"));
        if aborted && let Some(mut c) = self.observed.by_external.remove(ext) {
            if !c.meta.state.is_terminal() {
                self.set_child_state(&mut c, ObservedState::Cancelled, Some(Some("aborted")))
                    .await;
                self.close_child_turn(&mut c, "cancelled", None).await;
            }
            self.observed.by_external.insert(ext.to_owned(), c);
        }
    }

    /// 141 R3 T10: the tail's own counts (tool calls, Q3 `tokens` =
    /// input + output + cacheWrite, the last response's total as
    /// `contextTokens`) go into the detail wherever they exceed what feed 1
    /// reported — feed 1 falls silent once the parent's turn ends (F20), so
    /// background children would otherwise read `0 tools · 0 tok` forever.
    async fn omp_tail_stats(
        &self,
        c: &mut ObservedChild,
        context: Option<u64>,
        me: &ActorRef<Self>,
    ) {
        let stored = |k: &str| c.meta.detail.get(k).and_then(serde_json::Value::as_u64);
        let mut d = serde_json::Map::new();
        if c.tail_tools > stored("toolCount").unwrap_or(0) {
            d.insert("toolCount".into(), c.tail_tools.into());
        }
        if c.tail_tokens > stored("tokens").unwrap_or(0) {
            d.insert("tokens".into(), c.tail_tokens.into());
        }
        if let Some(ctx) = context
            && ctx > 0
            && stored("contextTokens") != Some(ctx)
        {
            d.insert("contextTokens".into(), ctx.into());
        }
        if d.is_empty() {
            return;
        }
        let mut up = ObservedUpsert::new(c.parent, c.meta.source, &c.meta.external);
        up.detail = Some(&d);
        if let Err(e) = self.store.upsert_observed(up).await {
            tracing::warn!(session = %c.id, "upsert_observed: {e:#}");
            return;
        }
        if !c.meta.detail.is_object() {
            c.meta.detail = serde_json::Value::Object(serde_json::Map::new());
        }
        for (k, v) in d {
            c.meta.detail[k] = v;
        }
        self.emit_throttled(c, me).await;
    }
}

impl Message<CancelObserved> for AcpChild {
    type Reply = ();

    async fn handle(&mut self, m: CancelObserved, _: &mut Context<Self, Self::Reply>) {
        let Some(c) = self.observed.by_external.values().find(|c| c.id == m.child) else {
            return;
        };
        if c.meta.can_cancel
            && let Some(h) = &self.harness
        {
            if let Err(e) = h.cancel_session(&c.meta.external) {
                self.publish(AgentEvent::Error {
                    session: m.child,
                    turn: None,
                    message: format!("{}: session/cancel failed: {e}", self.name),
                })
                .await;
            }
            return;
        }
        let parent = self.store.session(c.parent).await.ok().flatten();
        let parent = parent
            .and_then(|r| r.name.filter(|n| !n.is_empty()).or(Some(r.title)))
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "its parent".to_owned());
        self.publish(AgentEvent::Error {
            session: m.child,
            turn: None,
            message: format!("{}: cannot stop this subagent; stop {parent}", self.name),
        })
        .await;
    }
}

impl Message<ObservedDirty> for AcpChild {
    type Reply = ();

    async fn handle(
        &mut self,
        ObservedDirty(id): ObservedDirty,
        _: &mut Context<Self, Self::Reply>,
    ) {
        let Some(key) = self
            .observed
            .by_external
            .iter()
            .find(|(_, c)| c.id == id)
            .map(|(k, _)| k.clone())
        else {
            return;
        };
        let Some(mut c) = self.observed.by_external.remove(&key) else {
            return;
        };
        c.dirty = None;
        self.emit(&mut c).await;
        self.observed.by_external.insert(key, c);
    }
}

impl Message<ObservedSweep> for AcpChild {
    type Reply = ();

    async fn handle(
        &mut self,
        ObservedSweep(ext): ObservedSweep,
        _: &mut Context<Self, Self::Reply>,
    ) {
        if let Some((since, updates)) = self.observed.pending_traffic.get(&ext)
            && since.elapsed() >= PENDING_TRAFFIC
        {
            tracing::warn!(session = %self.id, child = %ext, n = updates.len(), "child traffic for an unannounced session dropped");
            self.observed.pending_traffic.remove(&ext);
        }
    }
}

impl Message<PermissionAnswered> for AcpChild {
    type Reply = ();

    async fn handle(
        &mut self,
        PermissionAnswered(ext): PermissionAnswered,
        _: &mut Context<Self, Self::Reply>,
    ) {
        let Some(mut c) = self.observed.by_external.remove(&ext) else {
            return;
        };
        if c.meta.state == ObservedState::RequiresAction {
            self.set_child_state(&mut c, ObservedState::Running, None)
                .await;
        }
        self.observed.by_external.insert(ext, c);
    }
}

impl Message<OmpTick> for AcpChild {
    type Reply = ();

    async fn handle(&mut self, _: OmpTick, ctx: &mut Context<Self, Self::Reply>) {
        let me = ctx.actor_ref().clone();
        self.omp_tick(&me).await;
    }
}
