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
    /// owner. I1's second line of defence: a session not `here` is never
    /// resumed (the message also arrives from the RPC and `Take::run`). For
    /// a transfer / takeover the taken turn's approvals and peer waits
    /// re-arm here first (§7.3), then §7's jobs / child-waits pass.
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
        match self.store.session(session).await {
            Ok(Some(row)) if row.here => {}
            Ok(Some(row)) => {
                return tracing::warn!(
                    session = %session, %turn, owner = %row.owner,
                    "139 I1: session owned elsewhere; not resumed here"
                );
            }
            Ok(None) => return tracing::warn!(session = %session, %turn, "109: no session row"),
            Err(e) => return tracing::warn!(session = %session, %turn, "109: session: {e:#}"),
        }
        let cause = cause.unwrap_or(plan.cause);
        if matches!(cause, ResumeCause::Transfer | ResumeCause::Takeover) {
            self.rebuild_waits_for(session).await;
        }
        // 139 §7 (W2 dispatcher, W3 bodies): the taken session's jobs and
        // child waits settle on the new owner before its actor exists.
        let lost_jobs = transfer::on_resume_here(self, session, cause, from.as_deref()).await;
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
                lost_jobs,
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

#[cfg(test)]
pub(crate) mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::durability::{ResumeKind, StepKind};
    use crate::store::Store;
    use crate::supervisor::tests::agents_on;

    /// 139 §11 W1's two-store shape: engine `a` writes a session with an
    /// open turn and closes the file; engine `b` opens the same file (a
    /// replica holding `a`'s rows) and gets an `Agents` over it. Returns
    /// `b`, `a`'s session, its open turn and the dir.
    pub(crate) async fn b_holding_a_turn(
        communication: bool,
    ) -> (Agents, SessionId, TurnId, PathBuf) {
        let dir = std::env::temp_dir().join(format!("theuth-139-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = Store::open(
            &dir.join("sessions.sqlite"),
            Some(mesh::EngineId("a".into())),
        )
        .await
        .unwrap();
        let (foreign, turn) = (SessionId::new(), TurnId::new());
        a.create_session(foreign, "m", None, None, None)
            .await
            .unwrap();
        a.step(
            foreign,
            turn,
            StepKind::InputAccepted,
            None,
            &serde_json::json!({}),
        )
        .await
        .unwrap();
        a.close().await;
        let b = agents_on(dir.clone(), Some("b"), communication).await;
        (b, foreign, turn, dir)
    }

    /// `b`'s own open turn in the shared file.
    pub(crate) async fn own_turn(b: &Agents) -> (SessionId, TurnId) {
        let (mine, turn) = (SessionId::new(), TurnId::new());
        b.store
            .create_session(mine, "m", None, None, None)
            .await
            .unwrap();
        b.store
            .step(
                mine,
                turn,
                StepKind::InputAccepted,
                None,
                &serde_json::json!({}),
            )
            .await
            .unwrap();
        (mine, turn)
    }

    /// I1: under `auto`, `b`'s scan resumes its own open turn and never
    /// `a`'s; a `ResumeTurn` aimed at `a`'s turn (the RPC, a stale take)
    /// spawns nothing either — the row is not `here`.
    #[tokio::test]
    async fn scan_and_resume_turn_skip_a_foreign_owners_turn() {
        let (mut b, foreign, foreign_turn, _dir) = b_holding_a_turn(false).await;
        let (mine, _) = own_turn(&b).await;
        std::sync::Arc::make_mut(&mut b.cfg).durability.resume = ResumeMode::Auto;

        b.resume_scan().await;
        assert!(b.sessions.contains_key(&mine), "own turn resumed");
        assert!(!b.sessions.contains_key(&foreign), "a's turn untouched");

        b.resume_turn(
            foreign,
            foreign_turn,
            None,
            Some(ResumeCause::Takeover),
            Some("a".into()),
        )
        .await;
        assert!(
            !b.sessions.contains_key(&foreign),
            "ensure!(row.here) holds"
        );
        let plan = b.store.resume_plan(foreign, foreign_turn).await.unwrap();
        assert_eq!(plan.kind, ResumeKind::Interrupted, "journal untouched");
    }
}
