//! Design doc 109 §4: `ResumeTurn`, `resume_scan` and `OpenTurn`. The scan
//! (§4.6 step 10) runs after the reaper and the wait rebuild; `auto` hands
//! every open turn to its actor, most recently active first, through one
//! `Semaphore(durability.resume_parallel)` the turns acquire themselves (the
//! supervisor never blocks on it); `ask` leaves the open set for the user
//! (W5's Attention item); `off` runs nothing. `OpenTurn` answers the
//! schedule plugin (§4.6 step 12).

use std::sync::Arc;

use kameo::message::{Context, Message};
use runtime::{SessionId, TurnId};
use tokio::sync::Semaphore;

use super::{Agents, LiveActor};
use crate::durability::ResumeMode;
use crate::loop_::Resume;

/// `tell`: resume `turn` of `session` from its journal (§4.1). The actor is
/// created as `Agents::session` does for a lazy load and handed
/// `Turn::resume(store.resume_plan(session, turn))`. From the boot scan
/// (`auto`) or the user's Resume (`ask`, W5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResumeTurn {
    /// Session.
    pub session: SessionId,
    /// The open turn.
    pub turn: TurnId,
}

impl Message<ResumeTurn> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: ResumeTurn, _: &mut Context<Self, Self::Reply>) {
        self.resume_turn(m.session, m.turn, None).await;
    }
}

/// `ask` → `Option<TurnId>`: the session's open turn, if any (§4.6 step 12:
/// the schedule plugin keeps a run open while its session is being resumed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenTurn(pub SessionId);

impl Message<OpenTurn> for Agents {
    type Reply = Option<TurnId>;

    async fn handle(&mut self, m: OpenTurn, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        match self.store.open_turns().await {
            Ok(open) => open
                .into_iter()
                .find(|(s, _, _)| *s == m.0)
                .map(|(_, t, _)| t),
            Err(e) => {
                tracing::error!("109: open_turns: {e:#}");
                None
            }
        }
    }
}

impl Agents {
    /// §4.6 step 10: every open turn, most recently active first, to its
    /// actor; `resume_parallel` run at a time, the rest wait on the shared
    /// slot inside their turn task. Honours `durability.resume`.
    pub(super) async fn resume_scan(&mut self) {
        let open = match self.store.open_turns().await {
            Ok(open) => open,
            Err(e) => return tracing::error!("109: open_turns: {e:#}"),
        };
        if open.is_empty() {
            return;
        }
        let mode = self.cfg.durability.resume;
        tracing::info!(
            "109: {} open turn(s); durability.resume = {mode:?}",
            open.len()
        );
        match mode {
            ResumeMode::Off => {}
            // The open set is the Attention item's source (§8.3, W5); the
            // user's Resume sends `ResumeTurn`.
            ResumeMode::Ask => {}
            ResumeMode::Auto => {
                let slots = Arc::new(Semaphore::new(self.cfg.durability.resume_parallel.max(1)));
                for (session, turn, _) in open {
                    self.resume_turn(session, turn, Some(slots.clone())).await;
                }
            }
        }
    }

    /// One `ResumeTurn`: the plan from the journal, the actor as a lazy load
    /// creates it, the hand-off. A session already running (an input beat
    /// the scan) resumes lazily inside that turn (`Turn::settle_open_turns`).
    pub(super) async fn resume_turn(
        &mut self,
        session: SessionId,
        turn: TurnId,
        slots: Option<Arc<Semaphore>>,
    ) {
        let plan = match self.store.resume_plan(session, turn).await {
            Ok(plan) => plan,
            Err(e) => {
                return tracing::warn!(session = %session, %turn, "109: resume_plan: {e:#}");
            }
        };
        if plan.last == crate::durability::LastStep::Finished {
            return;
        }
        let actor = match self.session(Some(session), None, None, (None, None)).await {
            Ok((_, LiveActor::Native(actor))) => actor,
            // W4: an ACP child with an open turn respawns through `session/load`.
            Ok((_, LiveActor::Acp(_))) => {
                return tracing::info!(session = %session, %turn, "109: ACP session left to its harness");
            }
            Err(e) => {
                return tracing::warn!(session = %session, %turn, "109: session: {e:#}");
            }
        };
        match actor.ask(Resume { plan, slots }).await {
            Ok(true) => tracing::info!(session = %session, %turn, "109: resuming turn"),
            Ok(false) => {
                tracing::info!(session = %session, %turn, "109: turn in flight; resumed lazily")
            }
            Err(e) => tracing::warn!(session = %session, %turn, "109: resume refused: {e}"),
        }
    }

    /// 109 §7.2: the spared `result:<turn>` exchanges of children whose
    /// parents never consumed them, delivered as a fresh result is
    /// (`enqueue_peer`); the `enqueued` set and the exchange's own state make
    /// a second delivery impossible.
    pub(super) async fn deliver_spared_results(&mut self) {
        let spared = match self.store.spared_child_results().await {
            Ok(spared) => spared,
            Err(e) => return tracing::error!("109: spared_child_results: {e:#}"),
        };
        for (parent, child, id) in spared {
            if !self.enqueued.insert(id) {
                continue;
            }
            let generation = match self.communication.0.write().route(child, parent) {
                Ok(g) => g,
                Err(e) => {
                    self.enqueued.remove(&id);
                    tracing::info!(session = %child, %parent, %id, "109: spared result not delivered: {e:?}");
                    continue;
                }
            };
            let record = match self.store.peer_record(parent, id).await {
                Ok(Some(record)) => record,
                Ok(None) => {
                    self.enqueued.remove(&id);
                    continue;
                }
                Err(e) => {
                    self.enqueued.remove(&id);
                    tracing::warn!(%id, "109: peer_record: {e:#}");
                    continue;
                }
            };
            if let Err(reason) = self.enqueue_peer(record, generation, false, false).await {
                self.enqueued.remove(&id);
                match self.store.fail_exchange(id, &reason).await {
                    Ok(r) => self.peer_changed(&r).await,
                    Err(e) => tracing::error!(%id, "fail_exchange: {e:#}"),
                }
                self.error(parent, format!("subagent result not delivered: {reason}"))
                    .await;
            } else {
                tracing::info!(session = %parent, %child, %id, "109: spared child result delivered");
            }
        }
    }
}
