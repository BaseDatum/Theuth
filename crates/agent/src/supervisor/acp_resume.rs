//! Design doc 109 §7.3 (W4) + 146 D1–D4: the boot trigger that brings an
//! ACP child back after a restart. 139 §7.4 named the parent's
//! `child_result` wait as the only trigger; 146 adds every mid-prompt child
//! of a parent here (`Store::midprompt_acp_children`) whatever the parent's
//! wait state, respawns `durability.acp_respawn_parallel` at a time, tells
//! the parent about every child that cannot come back (`<child_lost>`) and
//! publishes the [`BootPass`] counts the Attention actor renders as
//! `resume:boot:<ts>`. The child comes back through the ordinary
//! [`Agents::session`] → `acp_session` path (`session/load` on the stored
//! harness id when there is one) and is told [`Respawn`]; the actor decides
//! between the restored note and the delta handoff once it knows whether
//! the load succeeded (`acp_child.rs`, which this file never changes).

use std::collections::{HashMap, HashSet, VecDeque};

use chrono::{DateTime, Utc};
use runtime::SessionId;

use super::{Agents, LiveActor};
use crate::AgentEvent;
use crate::acp_child::Respawn;
use crate::durability::{BootPass, ResumeMode};
use crate::store::Scope;
use crate::store::durability::OpenTurnRow;

/// `TurnStarted.text` of a respawn whose harness never connected — the
/// child's `fail` turn, not a prompt accepted (`acp_child::RESPAWN_FAILED_INPUT`,
/// private to a file design 141 owns).
const RESPAWN_FAILED_INPUT: &str = "<runtime>respawn after a theuth restart</runtime>";

/// What one [`Agents::respawn_acp_child`] did.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    /// An actor was spawned and told [`Respawn`].
    Spawned,
    /// Nothing to do: a native child, no row, already live, result spared.
    Skipped,
    /// Cannot be respawned by this engine; why (D3).
    Lost(String),
}

/// 146: the pass's queue and counts, kept on `Agents.boot` until every
/// respawn settled (its harness accepted the prompt, or its `fail` turn ended).
pub(super) struct Boot {
    at: DateTime<Utc>,
    pass: BootPass,
    /// `(parent, child, name)` not yet spawned; a root ACP session has no parent.
    pending: VecDeque<(Option<SessionId>, SessionId, String)>,
    /// Spawned, harness not yet heard from: child → `(parent, name)`.
    inflight: HashMap<SessionId, (Option<SessionId>, String)>,
}

/// `ask` → `Option<BootPass>`: the latest counts this engine published (D4).
/// The Attention actor starts after the agent plugin, so a pass that settled
/// inside `ActivityRegistered` was published before anyone listened; it asks
/// once at start.
#[derive(Clone, Copy, Debug, Default)]
pub struct LastBootPass;

impl kameo::message::Message<LastBootPass> for Agents {
    type Reply = Option<BootPass>;

    async fn handle(
        &mut self,
        _: LastBootPass,
        _: &mut kameo::message::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.last_boot.clone()
    }
}

/// Chars of a prompt kept in the ACP journal row (D9).
const JOURNAL_EXCERPT: usize = 200;
/// The fresh-session handoff's header ([`handoff::render`]): a respawned
/// child whose first turn opens with it came back on a new harness session (D11).
const FRESH_MARK: &str = "could not be loaded";

impl Agents {
    /// §4.6 step 10, after `resume_scan`. Trigger set: every ACP child with
    /// an open journal turn (D9, `acp_open` from the pass's one
    /// `open_turns`) ∪ every `child_result` wait of a parent owned here ∪
    /// — for children from before the journal — every mid-prompt ACP child
    /// here by its last `messages` row (D1, bounded by
    /// `acp_respawn_window_days`). A child whose parent is gone, archived
    /// or owned elsewhere is lost (D3); the rest go through
    /// [`Self::respawn_acp_child`], `acp_respawn_parallel` at a time (D2).
    /// `resuming` is the native open-turn count the pass resumed. Honours
    /// `durability.resume`: only `auto` respawns anything.
    pub(super) async fn respawn_acp_children(
        &mut self,
        resuming: u32,
        acp_open: Vec<(Option<SessionId>, SessionId)>,
    ) {
        let at = Utc::now();
        let mut boot = Boot {
            at,
            pass: BootPass {
                at_ms: at.timestamp_millis().max(0) as u64,
                resuming,
                ..Default::default()
            },
            pending: VecDeque::new(),
            inflight: HashMap::new(),
        };
        if self.cfg.durability.resume != ResumeMode::Auto {
            self.announce(&boot.pass).await;
            return;
        }
        let mut seen = HashSet::new();
        let mut candidates: Vec<(Option<SessionId>, SessionId)> = Vec::new();
        tracing::info!(
            n = acp_open.len(),
            "146: ACP children with an open journal turn"
        );
        candidates.extend(acp_open.into_iter().filter(|(_, c)| seen.insert(*c)));
        match self.store.child_waits(Scope::Here).await {
            Ok(waits) => candidates.extend(waits.into_iter().filter_map(|w| {
                let child = w.target.parse::<SessionId>().ok()?;
                seen.insert(child).then_some((Some(w.session), child))
            })),
            Err(e) => tracing::error!("109: child_waits: {e:#}"),
        }
        let since =
            at - chrono::Duration::days(i64::from(self.cfg.durability.acp_respawn_window_days));
        match self.store.midprompt_acp_children(since).await {
            Ok(rows) => {
                tracing::info!(n = rows.len(), %since, "146: mid-prompt ACP children in the window");
                candidates.extend(
                    rows.into_iter()
                        .filter(|(c, _)| seen.insert(*c))
                        .map(|(c, p)| (Some(p), c)),
                );
            }
            Err(e) => tracing::error!("146: midprompt_acp_children: {e:#}"),
        }
        for (parent, child) in candidates {
            let name = match self.store.session(child).await {
                Ok(Some(row)) => row.name.unwrap_or_else(|| child.to_string()),
                _ => continue,
            };
            match self.parent_gate(parent).await {
                Ok(()) => boot.pending.push_back((parent, child, name)),
                Err(why) => self.lose(&mut boot, parent, child, name, why).await,
            }
        }
        self.boot = Some(boot);
        self.pump_respawns().await;
    }

    /// D9: the pass's open turns split into the ACP children's (`(parent,
    /// child)`, the respawn's) and the native ones (the resume's).
    pub(super) async fn split_acp_turns(
        &self,
        open: &[OpenTurnRow],
    ) -> (Vec<(Option<SessionId>, SessionId)>, Vec<OpenTurnRow>) {
        let (mut acp, mut native) = (Vec::new(), Vec::new());
        for row in open {
            match self.store.session(row.0).await {
                // 141: an observed (watch-only) harness child is nobody's to respawn.
                Ok(Some(s)) if s.agent.is_some() && driven(&s) => acp.push((s.parent, row.0)),
                _ => native.push(*row),
            }
        }
        (acp, native)
    }

    /// D1: the parent a lost child is reported to must exist, be owned here
    /// and not be archived; `Err` says why not (the child's lost reason). A
    /// root ACP session has no parent to gate on.
    async fn parent_gate(&self, parent: Option<SessionId>) -> Result<(), String> {
        let Some(parent) = parent else {
            return Ok(());
        };
        match self.store.session(parent).await {
            Ok(Some(row)) if !row.here => Err(format!("its parent is owned by {}", row.owner)),
            Ok(Some(row)) if row.archived => Err("its parent is archived".into()),
            Ok(Some(_)) => Ok(()),
            Ok(None) => Err("its parent session no longer exists".into()),
            Err(e) => Err(format!("its parent could not be read: {e:#}")),
        }
    }

    /// D2: spawn from the queue while fewer than `acp_respawn_parallel` are
    /// connecting, then publish the counts; `Agents.boot` goes once settled.
    async fn pump_respawns(&mut self) {
        let Some(mut boot) = self.boot.take() else {
            return;
        };
        let parallel = self.cfg.durability.acp_respawn_parallel.max(1);
        while boot.inflight.len() < parallel
            && let Some((parent, child, name)) = boot.pending.pop_front()
        {
            match self.respawn_acp_child(parent, child).await {
                Ok(Outcome::Spawned) => {
                    boot.inflight.insert(child, (parent, name));
                }
                Ok(Outcome::Skipped) => {}
                Ok(Outcome::Lost(why)) | Err(why) => {
                    self.lose(&mut boot, parent, child, name, why).await;
                }
            }
        }
        boot.pass.waiting = (boot.pending.len() + boot.inflight.len()) as u32;
        self.announce(&boot.pass).await;
        if boot.pass.waiting > 0 {
            self.boot = Some(boot);
        }
    }

    /// The supervisor's `AgentEvent` hook: a connecting child's first real
    /// `TurnStarted` means its harness accepted the prompt (respawned — on a
    /// fresh harness session when the text is D11's RESUME brief); a
    /// `TurnFinished` before one is its `fail` turn (lost). Either frees a slot.
    pub(super) async fn boot_child_event(&mut self, ev: &AgentEvent) {
        let (child, back, fresh) = match ev {
            AgentEvent::TurnStarted { session, text, .. } if text != RESPAWN_FAILED_INPUT => {
                (*session, true, text.contains(FRESH_MARK))
            }
            AgentEvent::TurnFinished { session, .. } => (*session, false, false),
            _ => return,
        };
        let Some(mut boot) = self.boot.take_if(|b| b.inflight.contains_key(&child)) else {
            return;
        };
        let Some((parent, name)) = boot.inflight.remove(&child) else {
            return;
        };
        if back {
            boot.pass.respawned += 1;
            if fresh {
                boot.pass.fresh += 1;
                tracing::info!(session = %child, "146: child back on a fresh harness session (D11)");
            }
        } else {
            self.lose(
                &mut boot,
                parent,
                child,
                name,
                "its harness did not come back (see the child's error)".into(),
            )
            .await;
        }
        self.boot = Some(boot);
        self.pump_respawns().await;
    }

    /// D9: the ACP journal, written where the supervisor hands a prompt to
    /// a harness child ([`Agents::route`], `enqueue_peer`): one open
    /// `input_accepted` while the child owes work. Nothing else is journaled
    /// for a harness; a failure is a log line, never a refused prompt.
    pub(super) async fn acp_journal_open(
        &self,
        child: SessionId,
        source: &str,
        text: &str,
        from: Option<&str>,
    ) {
        // 141: no journal for a session kind this engine does not drive.
        if !matches!(self.store.session(child).await, Ok(Some(row)) if driven(&row)) {
            return;
        }
        let parent = self
            .sessions
            .get(&child)
            .and_then(|l| l.parent.as_ref().map(|p| p.0));
        let payload = serde_json::json!({
            "source": source,
            "text": crate::store::durability::excerpt(text, JOURNAL_EXCERPT),
            "parent": parent,
            "from": from,
        });
        match self.store.acp_turn_open(child, &payload).await {
            Ok(Some(turn)) => {
                tracing::debug!(session = %child, %turn, source, "146: ACP turn journaled");
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(session = %child, "146: ACP journal: {e:#}"),
        }
    }

    /// D9's other half, on the bus: an ACP child's `TurnStarted` opens its
    /// journal turn when none is (a harness-queued or synthetic prompt the
    /// hand-off never saw); its `TurnFinished` closes it — except while the
    /// engine drains (D10): the `suspended{shutdown}` step the drain wrote
    /// stays, so the next boot reads an owed prompt and sends the continue
    /// prompt, never a cancel.
    pub(super) async fn acp_journal_event(&self, ev: &AgentEvent) {
        let is_acp = |s: &SessionId| matches!(self.sessions.get(s), Some(l) if matches!(l.actor, LiveActor::Acp(_)));
        match ev {
            AgentEvent::TurnStarted { session, text, .. } if is_acp(session) => {
                self.acp_journal_open(*session, "turn_started", text, None)
                    .await;
            }
            AgentEvent::TurnFinished {
                session,
                stop_reason,
                ..
            } if is_acp(session) => {
                if super::drain::draining() {
                    tracing::info!(session = %session, stop_reason, "146 D10: ACP turn cut by the engine's shutdown; journal left open");
                    return;
                }
                if let Err(e) = self.store.acp_turn_close(*session, stop_reason).await {
                    tracing::warn!(session = %session, "146: ACP journal close: {e:#}");
                }
            }
            _ => {}
        }
    }

    /// D3: a note on the child and a `<child_lost>` input to the parent
    /// (`inject` forwards it when the parent lives on another engine, 139
    /// R2; a parent that no longer exists, or a root session, is told
    /// nothing). The parent's `child_result` wait on it is cleared and its
    /// journal turn closed `lost`: no row re-raises it, so a lost child is
    /// reported once, not on every boot.
    async fn lose(
        &mut self,
        boot: &mut Boot,
        parent: Option<SessionId>,
        child: SessionId,
        name: String,
        why: String,
    ) {
        let at = crate::clock::hhmm(boot.at.with_timezone(&chrono::Local));
        tracing::warn!(session = %child, ?parent, "146: child not respawned: {why}");
        let note = format!("interrupted by the engine restart at {at}; not respawned: {why}");
        if let Err(e) = self.store.append_local_note(child, &note).await {
            tracing::warn!(session = %child, "146: lost note not written: {e:#}");
        }
        if let Err(e) = self.store.acp_turn_close(child, "lost").await {
            tracing::warn!(session = %child, "146: ACP journal close: {e:#}");
        }
        if let Some(parent) = parent {
            super::child_waits::child_result_delivered(self, parent, child).await;
            if !why.ends_with("no longer exists") {
                let body = format!(
                    "{name} was mid-prompt when the engine restarted at {at} and was not respawned: {why}. \
                     Resume it with send_message or spawn a fresh child with a RESUME brief."
                );
                let wrapped = format!(
                    "<child_lost name=\"{name}\" session=\"{child}\" at=\"{at}\" why=\"{}\">\n{body}\n</child_lost>",
                    why.replace('"', "'")
                );
                self.inject(parent, &format!("session:{child}"), body, wrapped, None)
                    .await;
            }
        }
        boot.pass.lost.push((child, name));
    }

    /// D4: the one INFO line and the bus event the Attention actor renders.
    async fn announce(&mut self, pass: &BootPass) {
        tracing::info!(
            resuming = pass.resuming,
            respawned = pass.respawned,
            fresh = pass.fresh,
            waiting = pass.waiting,
            lost = pass.lost.len(),
            lost_names = ?pass.lost.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>(),
            "146: boot pass"
        );
        self.last_boot = Some(pass.clone());
        self.rt.publish(pass.clone()).await;
    }

    /// One child of the table above. Native children are §4.6's business
    /// (their own open turn); a node's child cannot be respawned here. 139
    /// D13 gates, in order: the child is owned here (a child whose engine is
    /// gone is not respawned elsewhere), its harness is local, and its
    /// recorded `cwd` is a readable directory here — each a [`Outcome::Lost`]
    /// since 146. [`Outcome::Spawned`] when an actor was spawned and told
    /// [`Respawn`].
    pub(super) async fn respawn_acp_child(
        &mut self,
        parent: Option<SessionId>,
        child: SessionId,
    ) -> Result<Outcome, String> {
        let row = match self.store.session(child).await {
            Ok(Some(row)) => row,
            Ok(None) => return Ok(Outcome::Skipped),
            Err(e) => return Err(format!("{e:#}")),
        };
        let Some(agent) = row.agent.as_deref() else {
            return Ok(Outcome::Skipped);
        };
        // 141: only a child this engine drives; an observed one is left alone.
        if !driven(&row) {
            return Ok(Outcome::Skipped);
        }
        if !row.here {
            tracing::warn!(session = %child, owner = %row.owner, "139: child owned elsewhere; not respawned here");
            return Ok(Outcome::Lost(format!("it is owned by {}", row.owner)));
        }
        let (harness, engine) = crate::peers::parse_agent(agent)?;
        if !acp::load(&self.rt.data_dir)
            .map_err(|e| e.to_string())?
            .contains_key(engine)
        {
            tracing::warn!(session = %child, "109: `{agent}` is not a local harness; child not respawned");
            // A node's child (`<harness>@<node>` or a registered node) runs
            // elsewhere and is not this engine's to lose; a bare name that no
            // longer is in agents.yaml is.
            let node = harness.is_some()
                || self
                    .nodes
                    .read()
                    .unwrap_or_else(|p| p.into_inner())
                    .contains_key(engine);
            return Ok(if node {
                Outcome::Skipped
            } else {
                Outcome::Lost(format!("`{agent}` is not a local harness"))
            });
        }
        // A recorded `cwd` must exist here; none recorded → the harness's
        // default, as the lazy `acp_session` path uses.
        if let Some(cwd) = row.cwd.as_deref()
            && std::fs::read_dir(cwd).is_err()
        {
            tracing::warn!(session = %child, cwd, "139: cwd is not a readable directory here; child not respawned");
            return Ok(Outcome::Lost(format!(
                "its cwd {cwd} is not a readable directory here"
            )));
        }
        if self.live(child).is_some() {
            return Ok(Outcome::Skipped);
        }
        // §7.2: the child finished before the restart and its result is
        // spared `accepted` for the parent — nothing to respawn.
        if let Some(parent) = parent {
            let from = format!("session:{child}");
            let spared = self
                .store
                .pending_exchanges(parent, 64)
                .await
                .map_err(|e| format!("{e:#}"))?
                .iter()
                .any(|r| r.from == from);
            if spared {
                tracing::info!(session = %child, %parent, "109: child result already spared; not respawned");
                if let Err(e) = self.store.acp_turn_close(child, "end_turn").await {
                    tracing::warn!(session = %child, "146: ACP journal close: {e:#}");
                }
                return Ok(Outcome::Skipped);
            }
        }
        match self.relaunch_acp_child(child, Continue::Prompt).await {
            Ok(_) => Ok(Outcome::Spawned),
            Err(e) => {
                let message = format!("{agent}: respawn after restart failed: {e}");
                self.error(child, message.clone()).await;
                Err(message)
            }
        }
    }

    /// The one path a stopped ACP child comes back through — the boot pass
    /// and the header's Resume button (D8): a fresh actor from the row
    /// (`session/load` on the stored harness id when there is one), then
    /// what its first input is: [`Respawn`] (the restored note or the delta
    /// handoff → a prompt → `TurnStarted`), the 75 §10 [`CANCELLED_NOTE`]
    /// for a turn the user cancelled, or nothing (idle, the next
    /// `send_message` prompts it). Any live actor must be gone already.
    pub(super) async fn relaunch_acp_child(
        &mut self,
        child: SessionId,
        first: Continue,
    ) -> Result<LiveActor, String> {
        self.running.remove(&child);
        let (_, actor) = self
            .session(Some(child), None, None, (None, None))
            .await
            .map_err(|e| format!("{e:#}"))?;
        if let LiveActor::Acp(a) = &actor {
            match first {
                Continue::Prompt => {
                    // D9: the continue prompt is owed work too — a crash
                    // before the harness accepts it still reads as open.
                    self.acp_journal_open(child, "respawn", "<resume_handoff>", None)
                        .await;
                    let _ = a.tell(Respawn(self.cfg.durability.handoff_tokens)).await;
                }
                Continue::Cancelled => {
                    let _ = a
                        .tell(crate::acp_child::Note(
                            crate::acp_child::CANCELLED_NOTE.to_owned(),
                        ))
                        .await;
                }
                Continue::Idle => {}
            }
        }
        Ok(actor)
    }
}

/// 141 coordination: whether `row` is an ACP child this engine drives
/// (`store::durability::DRIVEN_KINDS`); 125's advisors and 141's observed
/// (watch-only) children are not.
fn driven(row: &crate::SessionSummary) -> bool {
    row.kind == crate::SessionKind::Chat
}

/// What a relaunched child is told first ([`Agents::relaunch_acp_child`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Continue {
    /// Mid-prompt when it died: the respawn prompt continues the work.
    Prompt,
    /// Its last turn was cancelled: a note not to pick the replay up (75 §10).
    Cancelled,
    /// Idle: reconnect only.
    Idle,
}

#[cfg(test)]
mod tests {
    use kameo::actor::Spawn;
    use runtime::TurnId;
    use serde_json::json;
    use tools::{Risk, WaitKind, WaitRow};

    use super::*;
    use crate::durability::StepKind;
    use crate::store::communication::AcceptExchange;
    use crate::store::durability::{calls_planned_in, step_in};
    use crate::supervisor::Live;
    use crate::supervisor::tests::agents_on;

    fn wait(parent: SessionId, call: &str, child: SessionId) -> WaitRow {
        WaitRow {
            session: parent,
            tool_call_id: call.into(),
            kind: WaitKind::ChildResult,
            target: child.to_string(),
            deadline: None,
            payload: json!({}),
        }
    }

    /// The §7.3 / 139 D13 table, extended by 146: a native child, a node's
    /// child, a child whose result the boot pass spared and a child with no
    /// row are skipped; a child owned by another engine and a child whose
    /// `cwd` is not here are lost (D3: note on the child, `<child_lost>` to
    /// the parent); a local ACP child a parent here waits on is respawned —
    /// once, and not again while its actor lives — and so is a mid-prompt
    /// child nobody waits on (D1); a mid-prompt child of an archived parent
    /// is lost; an idle child is not touched. A wait of a parent owned
    /// elsewhere is not this engine's trigger. `resume: off` respawns
    /// nothing. The pass publishes its counts (D4) and a child's first real
    /// `TurnStarted` moves it from waiting to respawned (D2).
    #[tokio::test]
    async fn respawn_decision_table() {
        let scratch = crate::TestDir::new("139");
        let dir = scratch.0.clone();
        // Engine `a`: a parent waiting on its own ACP child, and an ACP child
        // a parent on `b` will wait on.
        let a = crate::store::Store::open(
            &dir.join("sessions.sqlite"),
            Some(mesh::EngineId("a".into())),
        )
        .await
        .unwrap();
        let (a_parent, a_child, taken) = (SessionId::new(), SessionId::new(), SessionId::new());
        a.create_session(a_parent, "m", None, None, None)
            .await
            .unwrap();
        for (id, name) in [(a_child, "a-kid"), (taken, "taken")] {
            a.create_session(id, "omp", Some(a_parent), Some(name), None)
                .await
                .unwrap();
            a.set_acp(id, "omp", Some(&dir), &[]).await.unwrap();
        }
        a.wait_put(&wait(a_parent, "c1", a_child)).await.unwrap();
        a.close().await;

        let mut b = agents_on(dir.clone(), Some("b"), true).await;
        let store = b.store.clone();
        acp::save(
            &b.rt.data_dir,
            &serde_yaml::from_str("omp:\n  command: /nonexistent/theuth-test-harness\n").unwrap(),
        )
        .unwrap();
        let parent = SessionId::new();
        store
            .create_session(parent, "m", None, None, None)
            .await
            .unwrap();
        let turn = TurnId::new();
        store
            .step(parent, turn, StepKind::InputAccepted, None, &json!({}))
            .await
            .unwrap();
        let children: Vec<SessionId> = (0..5).map(|_| SessionId::new()).collect();
        let [native, node, spared, acp_child, bad_cwd] = children[..] else {
            unreachable!()
        };
        let calls: Vec<(genai::chat::ToolCall, Risk)> = (1..=8)
            .map(|i| {
                (
                    genai::chat::ToolCall {
                        call_id: format!("c{i}"),
                        fn_name: "send_message".into(),
                        fn_arguments: json!({}),
                        thought_signatures: None,
                    },
                    Risk::ReadOnly,
                )
            })
            .collect();
        let mut tx = store.begin_write().await.unwrap();
        step_in(
            &mut tx,
            parent,
            turn,
            StepKind::ToolBatch,
            None,
            &json!({"call_ids": ["c1", "c2", "c3", "c4", "c5", "c6", "c7", "c8"]}),
        )
        .await
        .unwrap();
        calls_planned_in(&mut tx, parent, turn, 0, &calls)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        for (name, id) in [
            ("native", native),
            ("node", node),
            ("spared", spared),
            ("acp", acp_child),
            ("bad_cwd", bad_cwd),
        ] {
            let agent = match name {
                "native" => "m",
                "node" => "omp@box",
                _ => "omp",
            };
            store
                .create_session(id, agent, Some(parent), Some(name), None)
                .await
                .unwrap();
            if name != "native" {
                let cwd = match name {
                    "bad_cwd" => Some(std::path::Path::new("/nonexistent/theuth-139-cwd")),
                    "acp" => Some(dir.as_path()),
                    _ => None,
                };
                store.set_acp(id, agent, cwd, &[]).await.unwrap();
            }
        }
        // Two waits on the ACP child (c4, c6): respawned once.
        let ghost = SessionId::new();
        for (i, child) in [
            native, node, spared, acp_child, ghost, acp_child, bad_cwd, taken,
        ]
        .iter()
        .enumerate()
        {
            store
                .wait_put(&wait(parent, &format!("c{}", i + 1), *child))
                .await
                .unwrap();
        }
        let (from, to) = (format!("session:{spared}"), format!("session:{parent}"));
        let request_id = format!("result:{}", TurnId::new());
        store
            .accept_exchange(&AcceptExchange {
                sender: &from,
                recipient: &to,
                sender_session: Some(spared),
                recipient_session: Some(parent),
                request_id: Some(&request_id),
                reply_to: None,
                request_scope: None,
                generation: 1,
                body: "done before the crash",
            })
            .await
            .unwrap();

        // 146 D1: a mid-prompt child nobody waits on, an idle one, and one
        // under an archived parent.
        let (mid, idle, arch, orphan) = (
            SessionId::new(),
            SessionId::new(),
            SessionId::new(),
            SessionId::new(),
        );
        store
            .create_session(arch, "m", None, None, None)
            .await
            .unwrap();
        store.set_archived(arch, true).await.unwrap();
        for (id, p, name) in [
            (mid, parent, "mid"),
            (idle, parent, "idle"),
            (orphan, arch, "orphan"),
        ] {
            store
                .create_session(id, "omp", Some(p), Some(name), None)
                .await
                .unwrap();
            store.set_acp(id, "omp", Some(&dir), &[]).await.unwrap();
            store
                .append(id, &genai::chat::ChatMessage::user("go"))
                .await
                .unwrap();
        }
        store
            .append(idle, &genai::chat::ChatMessage::assistant("done"))
            .await
            .unwrap();
        // 146 D9: a child with an open journal turn and no messages at all
        // (the hand-off was journaled, the harness never opened the turn),
        // and one whose messages look mid-prompt but whose journal says its
        // turn ended — the journal wins, the messages heuristic is only for
        // children without one.
        let (journaled, journaled_idle) = (SessionId::new(), SessionId::new());
        for (id, name) in [(journaled, "journaled"), (journaled_idle, "journaled_idle")] {
            store
                .create_session(id, "omp", Some(parent), Some(name), None)
                .await
                .unwrap();
            store.set_acp(id, "omp", Some(&dir), &[]).await.unwrap();
        }
        let payload = json!({"source": "input", "text": "go"});
        assert!(
            store
                .acp_turn_open(journaled, &payload)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .acp_turn_open(journaled, &payload)
                .await
                .unwrap()
                .is_none(),
            "one open turn per child"
        );
        store.acp_turn_open(journaled_idle, &payload).await.unwrap();
        store
            .append(journaled_idle, &genai::chat::ChatMessage::user("go"))
            .await
            .unwrap();
        assert!(
            store
                .acp_turn_close(journaled_idle, "end_turn")
                .await
                .unwrap()
        );
        assert!(
            !store
                .acp_turn_close(journaled_idle, "end_turn")
                .await
                .unwrap()
        );
        let since = chrono::Utc::now() - chrono::Duration::days(7);
        let found: Vec<_> = store.midprompt_acp_children(since).await.unwrap();
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found.contains(&(mid, parent)) && found.contains(&(orphan, arch)));
        let open = b.open_here().await;
        let (acp_open, native_open) = b.split_acp_turns(&open).await;
        assert_eq!(acp_open, vec![(Some(parent), journaled)], "{open:?}");
        assert_eq!(
            native_open.len(),
            1,
            "the parent's own open turn: {native_open:?}"
        );
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = Sink::spawn_with_mailbox(Sink(tx), runtime::subscriber_mailbox());
        b.rt.subscribe::<Sink, BootPass>(&sink).await;
        b.rt.subscribe::<Sink, AgentEvent>(&sink).await;

        assert_eq!(
            b.respawn_acp_child(Some(parent), native).await,
            Ok(Outcome::Skipped)
        );
        assert_eq!(
            b.respawn_acp_child(Some(parent), node).await,
            Ok(Outcome::Skipped)
        );
        assert_eq!(
            b.respawn_acp_child(Some(parent), spared).await,
            Ok(Outcome::Skipped)
        );
        assert_eq!(
            b.respawn_acp_child(Some(parent), ghost).await,
            Ok(Outcome::Skipped)
        );
        assert_eq!(
            b.respawn_acp_child(Some(parent), taken).await,
            Ok(Outcome::Lost("it is owned by a".into()))
        );
        assert!(
            matches!(b.respawn_acp_child(Some(parent), bad_cwd).await, Ok(Outcome::Lost(w)) if w.contains("not a readable directory")),
            "cwd not here"
        );
        assert!(b.sessions.is_empty(), "nothing spawned so far");

        std::sync::Arc::make_mut(&mut b.cfg).durability.resume = ResumeMode::Off;
        b.respawn_acp_children(0, acp_open.clone()).await;
        assert!(b.sessions.is_empty(), "resume: off respawns nothing");

        std::sync::Arc::make_mut(&mut b.cfg).durability.resume = ResumeMode::Auto;
        b.respawn_acp_children(3, acp_open).await;
        let acp_live = |b: &Agents, id: SessionId| {
            matches!(
                b.sessions.get(&id),
                Some(Live {
                    actor: LiveActor::Acp(_),
                    ..
                })
            )
        };
        assert!(
            acp_live(&b, acp_child),
            "the waited-on ACP child has a live actor"
        );
        assert!(acp_live(&b, mid), "the mid-prompt child has a live actor");
        assert!(
            acp_live(&b, journaled),
            "the journaled child has a live actor"
        );
        for id in [
            idle,
            journaled_idle,
            orphan,
            taken,
            bad_cwd,
            native,
            node,
            spared,
        ] {
            assert!(
                !acp_live(&b, id),
                "{id} must not be respawned: {:?}",
                b.sessions.keys()
            );
        }
        // D2: three connecting, none back yet; D3: taken, bad_cwd, orphan lost.
        let boot = b
            .boot
            .as_ref()
            .expect("pass not settled while children connect");
        assert_eq!(
            (boot.pass.resuming, boot.pass.respawned, boot.pass.waiting),
            (3, 0, 3)
        );
        let mut lost: Vec<&str> = boot.pass.lost.iter().map(|(_, n)| n.as_str()).collect();
        lost.sort_unstable();
        assert_eq!(lost, ["bad_cwd", "orphan", "taken"]);
        let notes = store.messages(orphan).await.unwrap();
        let last = notes.last().unwrap();
        assert_eq!(last.1.role, genai::chat::ChatRole::System);
        let note = crate::loop_::text_of(&last.1.content);
        assert!(
            note.contains("not respawned: its parent is archived"),
            "{note}"
        );
        // A lost child is not scanned again: its last row is the note.
        assert!(
            !store
                .midprompt_acp_children(since)
                .await
                .unwrap()
                .iter()
                .any(|(c, _)| *c == orphan)
        );

        // D2: the mid-prompt child's harness accepts the prompt.
        b.boot_child_event(&AgentEvent::TurnStarted {
            session: mid,
            turn: TurnId::new(),
            text: "<resume_handoff>…".into(),
            model: "omp".into(),
            turn_seq: 0,
        })
        .await;
        let boot = b.boot.as_ref().unwrap();
        assert_eq!((boot.pass.respawned, boot.pass.waiting), (1, 2));
        // The journaled child comes back on a fresh harness session (D11).
        b.boot_child_event(&AgentEvent::TurnStarted {
            session: journaled,
            turn: TurnId::new(),
            text: "<resume_handoff>\nThe theuth engine restarted at 1 and your previous harness session s could not be loaded.".into(),
            model: "omp".into(),
            turn_seq: 0,
        })
        .await;
        let boot = b.boot.as_ref().unwrap();
        assert_eq!(
            (boot.pass.respawned, boot.pass.fresh, boot.pass.waiting),
            (2, 1, 1)
        );
        // The other child's `fail` turn: lost, and the pass settles.
        b.boot_child_event(&AgentEvent::TurnStarted {
            session: acp_child,
            turn: TurnId::new(),
            text: RESPAWN_FAILED_INPUT.into(),
            model: "omp".into(),
            turn_seq: 0,
        })
        .await;
        assert!(b.boot.is_some(), "the fail turn's start settles nothing");
        b.boot_child_event(&AgentEvent::TurnFinished {
            session: acp_child,
            turn: TurnId::new(),
            stop_reason: "error".into(),
            usage: Default::default(),
            context: 0,
        })
        .await;
        assert!(b.boot.is_none(), "settled");
        // A lost child's wait row is gone (reported once); the others stay.
        let left: Vec<String> = store
            .child_waits(Scope::Here)
            .await
            .unwrap()
            .into_iter()
            .map(|w| w.target)
            .collect();
        for lost in [taken, bad_cwd, acp_child] {
            assert!(
                !left.contains(&lost.to_string()),
                "{lost} still waited on: {left:?}"
            );
        }
        for kept in [native, node, spared, ghost] {
            assert!(
                left.contains(&kept.to_string()),
                "{kept} wait dropped: {left:?}"
            );
        }

        let _ = sink
            .ask(AgentEvent::TitleChanged {
                session: parent,
                title: String::new(),
                tags: Vec::new(),
            })
            .await;
        let (mut passes, mut delivered) = (Vec::new(), Vec::new());
        while let Ok(e) = rx.try_recv() {
            match e {
                Seen::Boot(p) => passes.push(p),
                Seen::Event(ev) => {
                    if let AgentEvent::MessageDelivered {
                        session,
                        from,
                        text,
                    } = *ev
                    {
                        delivered.push((session, from, text));
                    }
                }
            }
        }
        let last = passes.last().expect("boot pass published");
        assert_eq!(
            (last.resuming, last.respawned, last.waiting, last.lost.len()),
            (3, 2, 0, 4)
        );
        assert!(passes.iter().any(|p| p.waiting == 3), "{passes:?}");
        assert_eq!(last.fresh, 1);
        let to_arch = delivered
            .iter()
            .find(|(s, f, _)| *s == arch && *f == format!("session:{orphan}"))
            .expect("<child_lost> to the archived parent");
        assert!(
            to_arch
                .2
                .starts_with("orphan was mid-prompt when the engine restarted at"),
            "{}",
            to_arch.2
        );
        assert!(
            to_arch
                .2
                .contains("spawn a fresh child with a RESUME brief")
        );
        assert!(
            delivered
                .iter()
                .any(|(s, f, _)| *s == parent && *f == format!("session:{taken}"))
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// What the fixture's bus carried.
    enum Seen {
        Boot(BootPass),
        Event(Box<AgentEvent>),
    }

    #[derive(kameo::Actor)]
    struct Sink(tokio::sync::mpsc::UnboundedSender<Seen>);

    impl kameo::message::Message<BootPass> for Sink {
        type Reply = ();
        async fn handle(
            &mut self,
            p: BootPass,
            _: &mut kameo::message::Context<Self, Self::Reply>,
        ) {
            let _ = self.0.send(Seen::Boot(p));
        }
    }

    impl kameo::message::Message<AgentEvent> for Sink {
        type Reply = ();
        async fn handle(
            &mut self,
            e: AgentEvent,
            _: &mut kameo::message::Context<Self, Self::Reply>,
        ) {
            let _ = self.0.send(Seen::Event(Box::new(e)));
        }
    }
}
