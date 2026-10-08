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

use super::{Agents, LiveActor, transfer};
use crate::durability::{ResumeCause, ResumeMode};
use crate::loop_::Resume;
use crate::store::Scope;

/// `tell`: resume `turn` of `session` from its journal (§4.1). The actor is
/// created as `Agents::session` does for a lazy load and handed
/// `Turn::resume(store.resume_plan(session, turn))`. From the boot scan
/// (`auto`), the user's Resume (`ask`, W5) or — 139 §6.2 step 9 — the
/// taker right after `set_owner`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResumeTurn {
    /// Session.
    pub session: SessionId,
    /// The open turn.
    pub turn: TurnId,
    /// 139 §6.3: `None` = the boot rule from the journal (`ResumePlan::cause`
    /// — the scan and the `resume_turn` RPC); `Some(Transfer | Takeover)`
    /// from `Take::run`.
    pub cause: Option<ResumeCause>,
    /// 139 §6.3: the previous owner for `transfer` / `takeover`.
    pub from: Option<String>,
}

impl Message<ResumeTurn> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: ResumeTurn, _: &mut Context<Self, Self::Reply>) {
        self.resume_turn(m.session, m.turn, None, m.cause, m.from)
            .await;
    }
}

/// `ask` → `Option<TurnId>`: the session's open turn, if any (§4.6 step 12:
/// the schedule plugin keeps a run open while its session is being resumed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenTurn(pub SessionId);

impl Message<OpenTurn> for Agents {
    type Reply = Option<TurnId>;

    async fn handle(&mut self, m: OpenTurn, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        // 139 §5.1 item 4: one session, any owner; the RPC decides routing.
        match self.store.open_turn(m.0).await {
            Ok(open) => open.map(|(t, _)| t),
            Err(e) => {
                tracing::error!("109: open_turn: {e:#}");
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
        // 139 D1: a replica never reaches `resume_turn` for the owner's turn.
        let open = match self.store.open_turns(Scope::Here).await {
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
                    self.resume_turn(session, turn, Some(slots.clone()), None, None)
                        .await;
                }
            }
        }
    }

    /// One `ResumeTurn`: the plan from the journal, the actor as a lazy load
    /// creates it, the hand-off. A session already running (an input beat
    /// the scan) resumes lazily inside that turn (`Turn::settle_open_turns`).
    /// 139: `cause` overrides the plan's boot rule; `from` names the previous
    /// owner. W1 adds `ensure!(row.here)` before `self.session` and the
    /// `rebuild_waits_for` ordering around `transfer::on_resume_here`.
    pub(super) async fn resume_turn(
        &mut self,
        session: SessionId,
        turn: TurnId,
        slots: Option<Arc<Semaphore>>,
        cause: Option<ResumeCause>,
        from: Option<String>,
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
        let cause = cause.unwrap_or(plan.cause);
        // 139 §7 (W2 dispatcher, W3 bodies): the taken session's jobs, child
        // waits and questions settle / re-arm on the new owner first.
        transfer::on_resume_here(self, session, cause, from.as_deref()).await;
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
        match actor
            .ask(Resume {
                plan,
                slots,
                cause,
                from,
            })
            .await
        {
            Ok(true) => tracing::info!(session = %session, %turn, "109: resuming turn"),
            Ok(false) => {
                tracing::info!(session = %session, %turn, "109: turn in flight; resumed lazily")
            }
            Err(e) => tracing::warn!(session = %session, %turn, "109: resume refused: {e}"),
        }
    }
}
