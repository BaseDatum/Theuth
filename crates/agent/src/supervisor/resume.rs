//! Design doc 109 §4: `ResumeTurn`, `resume_scan` and `OpenTurn`. **W1 fills
//! the `ResumeTurn` handler and `resume_scan`**; `OpenTurn` is complete (the
//! schedule plugin needs only that, §4.6 step 12).

use kameo::message::{Context, Message};
use runtime::{SessionId, TurnId};

use super::Agents;

/// `tell`: resume `turn` of `session` from its journal (§4.1). The actor is
/// created as `Agents::session` does for a lazy load and handed
/// `Turn::resume(store.resume_plan(session, turn))`.
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
        tracing::warn!(
            session = %m.session,
            turn = %m.turn,
            "109: ResumeTurn not implemented"
        );
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
    /// §4.6 step 10: every open turn, most recently active first, through a
    /// `Semaphore(durability.resume_parallel)` → [`ResumeTurn`]. Stub
    /// (contract wave): logs what it would resume and queues nothing. W1
    /// fills it, honouring `durability.resume` (`auto` | `ask` | `off`).
    pub(super) async fn resume_scan(&mut self) {
        match self.store.open_turns().await {
            Ok(open) if !open.is_empty() => tracing::info!(
                "109: {} open turn(s) found; resume_scan not implemented",
                open.len()
            ),
            Ok(_) => {}
            Err(e) => tracing::error!("109: open_turns: {e:#}"),
        }
    }
}
