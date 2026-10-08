//! Design doc 139 §6: the ownership path's drain and the resume-on-take
//! dispatcher. **Contract C stubs — W2 owns every body in this file.**
//!
//! - [`DrainSession`] / [`drain_session`]: §6.1 steps 1–5 (the per-session
//!   `Drain`: `suspended{reason}` first, `Draining` told, `wait_idle`,
//!   `Jobs::KillSession`, then `tell_cancel` + `stop_gracefully` +
//!   `wait_for_shutdown`). Today it calls `Agents::unload_session` so the
//!   tree behaves as before C.
//! - [`release_window`]: the owner's drain budget inside the taker's
//!   `release_wait_secs` (§6.1, Revision 1 Q4).
//! - [`on_resume_here`]: §7's per-session pass on the new owner, run by
//!   `resume_turn` right after `resume_plan` (W1 orders `rebuild_waits_for`
//!   around it). Stays a dispatcher: it calls W3's
//!   `child_waits::settle_on_transfer` and `durability::jobs::settle_transferred`.

use std::time::{Duration, Instant};

use kameo::message::{Context, Message};
use runtime::SessionId;

use super::Agents;
use crate::durability::ResumeCause;

/// `ask` → [`SessionDrainReport`]: drain one session for `reason` within
/// `deadline` (139 §6.1). Sent by `sessionsync::owner::release` (W2) in
/// place of `UnloadSession`. **Body: W2.**
#[derive(Clone, Copy, Debug)]
pub struct DrainSession {
    /// The session being released.
    pub session: SessionId,
    /// `release` on the ownership path; the `TurnSuspended` reason and the
    /// `suspended{"reason"}` payload.
    pub reason: &'static str,
    /// When the drain stops waiting for the turn and cancels it.
    pub deadline: Instant,
}

/// What [`DrainSession`] left in the journal (139 I3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, kameo::Reply)]
pub struct SessionDrainReport {
    /// A turn was open when the drain began.
    pub open: bool,
    /// The turn reached `turn_finished` inside the window.
    pub finished: bool,
    /// A `suspended{reason}` row stands (the turn did not finish).
    pub suspended: bool,
}

impl Message<DrainSession> for Agents {
    type Reply = SessionDrainReport;

    async fn handle(
        &mut self,
        m: DrainSession,
        _: &mut Context<Self, Self::Reply>,
    ) -> SessionDrainReport {
        drain_session(self, m).await
    }
}

/// 139 §6.1 steps 1–5. **W2 writes this body**; the C contract unloads as
/// today (`Agents::unload_session`), so `release` keeps its pre-139
/// behaviour until W2 lands.
pub async fn drain_session(agents: &mut Agents, m: DrainSession) -> SessionDrainReport {
    let _ = (m.reason, m.deadline);
    let had_actor = agents.unload_session(m.session).await;
    SessionDrainReport {
        open: had_actor,
        ..SessionDrainReport::default()
    }
}

/// The owner's drain window for a release (139 §6.1): the taker waits
/// `release_wait_secs` for `Released`, so the owner drains for all but the
/// last second, then flushes and replies. **W2** may refine.
pub fn release_window(release_wait_secs: u64) -> Duration {
    Duration::from_secs(release_wait_secs.saturating_sub(1))
}

/// 139 §7: the per-session pass on the new owner for a resumed turn —
/// jobs of the previous owner settled `lost` (`reaper::settle_transferred`,
/// W3), child waits settled or left to forwarding
/// (`child_waits::settle_on_transfer`, W3). A no-op for `crash | shutdown |
/// update`. **Dispatcher body: W2.** No-op in the C contract.
pub async fn on_resume_here(
    agents: &mut Agents,
    session: SessionId,
    cause: ResumeCause,
    from: Option<&str>,
) {
    let _ = (agents, session, cause, from);
}
