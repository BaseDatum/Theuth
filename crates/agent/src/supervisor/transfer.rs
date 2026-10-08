//! Design doc 139 §6: the ownership path's drain and the resume-on-take
//! dispatcher (W2).
//!
//! - [`DrainSession`]: §6.1 steps 1–5 for one session — `suspended{reason}`
//!   first, `Draining` told, wait for the turn, `Jobs::KillSession`, then
//!   `tell_cancel` + `stop_gracefully` + `wait_for_shutdown`. Runs off the
//!   supervisor's mailbox (a turn ending inside the window asks `Agents`),
//!   like `Drain`; [`SessionDrained`] brings the bookkeeping back in.
//! - D16: `Agents::draining_sessions` refuses inputs from step 1 until the
//!   demote lands ([`demoted`]) or `2 × release_wait_secs` passes.
//! - [`release_window`]: the owner's drain budget inside the taker's
//!   `release_wait_secs` (§6.1, Revision 1 Q4).
//! - [`on_resume_here`]: §7's per-session pass on the new owner, run by
//!   `resume_turn` right after `resume_plan`.

use std::time::{Duration, Instant};

use chrono::Utc;
use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use runtime::{Runtime, SessionId, TurnId};
use tools::jobs::{LostJob, LostReason, Reaped};
use tools::{JOBS_ACTOR, JobKind, Jobs};

use super::{Agents, LiveActor, child_waits};
use crate::AgentEvent;
use crate::durability::{ResumeCause, StepKind};
use crate::loop_::{CancelTurn, Draining};
use crate::store::Store;

/// `ask` → [`SessionDrainReport`]: drain one session for `reason` within
/// `deadline` (139 §6.1). Sent by `sessionsync::owner::release` in place of
/// `UnloadSession`.
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

/// The reason a release drain holds inputs for (D16); the only reason the
/// ownership path sends today.
pub const RELEASE: &str = "release";

/// Prefix of the `Err` string `Agents::route` answers for a session in
/// `draining_sessions` (D16). The RPC layer maps it to
/// `SESSION_DRAINING = -32011` (W4).
pub const DRAINING_PREFIX: &str = "session draining:";

/// The D16 refusal text: *moving to another engine* for a release (the owner
/// side does not learn the taker's name — `Msg::Release` carries none),
/// *pausing* otherwise.
pub fn draining_error(session: SessionId, reason: &str) -> String {
    if reason == RELEASE {
        format!("{DRAINING_PREFIX} {session} is moving to another engine — retry in a moment")
    } else {
        format!("{DRAINING_PREFIX} {session} is pausing for {reason} — resumes on the next start")
    }
}

impl Message<DrainSession> for Agents {
    type Reply = DelegatedReply<SessionDrainReport>;

    /// Off the mailbox, as `Drain`: the turn's own end asks this actor.
    /// The reason is remembered first so every input from here on is
    /// refused (D16); the actor entry is forgotten by [`SessionDrained`].
    async fn handle(
        &mut self,
        m: DrainSession,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        self.draining_sessions.insert(m.session, m.reason);
        let live = self
            .sessions
            .get(&m.session)
            .filter(|l| l.actor.is_alive())
            .map(|l| l.actor.clone());
        let me = ctx.actor_ref().clone();
        let (store, rt) = (self.store.clone(), self.rt.clone());
        tokio::spawn(async move {
            let started = Instant::now();
            let report = drain_session(&store, &rt, live, m).await;
            // Twice the taker's wait: the fence has moved (demote clears
            // it sooner) or the taker gave up (§6.2 "fence interplay").
            let hold = (m.reason == RELEASE).then(|| {
                (m.deadline.saturating_duration_since(started) + Duration::from_secs(1)) * 2
            });
            let _ = me
                .ask(SessionDrained {
                    session: m.session,
                    hold,
                })
                .await;
            if let Some(tx) = tx {
                tx.send(report);
            }
        });
        reply
    }
}

/// 139 §6.1 steps 1–5 over one session's `live` actor (if any).
pub(crate) async fn drain_session(
    store: &Store,
    rt: &Runtime,
    live: Option<LiveActor>,
    m: DrainSession,
) -> SessionDrainReport {
    // (1) Mark first, durably: a kill inside the window still reads as a
    // planned stop; a `turn_finished` written inside it closes the row.
    let open = suspend_open_turn(store, m.session, m.reason).await;
    if let Some(live) = &live {
        // (2) No new model call opens (the loop reads the flag each round).
        if let LiveActor::Native(a) = live {
            let _ = a.tell(Draining).await;
        }
        // (3) In-flight tool calls finish or the deadline cancels them.
        while live.is_running().await {
            let now = Instant::now();
            if now >= m.deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100).min(m.deadline - now)).await;
        }
    }
    // (4) The session's jobs: rows `lost` and screens first, then the signal
    // (W3's body). Before the actor stops so the rows precede `flush`.
    if let Ok(Some(jobs)) = ActorRef::<Jobs>::lookup(JOBS_ACTOR)
        && let Err(e) = jobs
            .ask(tools::jobs::KillSession { session: m.session })
            .await
    {
        tracing::warn!(session = %m.session, "139: drain: KillSession: {e}");
    }
    // (5) The cancel is processed before the stop (never dropped, D19); a
    // cancel while `draining` is a suspension in the loop (§6.1).
    if let Some(live) = &live {
        live.stop().await;
    }
    let Some(turn) = open else {
        return SessionDrainReport::default();
    };
    let still_open = matches!(store.open_turn(m.session).await, Ok(Some((t, _))) if t == turn);
    if still_open {
        rt.publish(AgentEvent::TurnSuspended {
            session: m.session,
            turn,
            reason: m.reason.to_owned(),
        })
        .await;
    }
    tracing::info!(session = %m.session, %turn, reason = m.reason, finished = !still_open, "139: session drained");
    SessionDrainReport {
        open: true,
        finished: !still_open,
        suspended: still_open,
    }
}

/// Step 1 on its own: the session's open turn gets `suspended{reason}` and
/// is returned; a session with no open turn writes nothing. Store-only, so
/// an engine without a supervisor (`sessionsync::owner::release` on a
/// pairing run or in its tests) still leaves the resumable row (I3).
pub async fn suspend_open_turn(store: &Store, session: SessionId, reason: &str) -> Option<TurnId> {
    let (turn, _) = match store.open_turn(session).await {
        Ok(open) => open?,
        Err(e) => {
            tracing::error!(%session, "139: drain: open_turn: {e:#}");
            return None;
        }
    };
    let payload = serde_json::json!({ "reason": reason });
    if let Err(e) = store
        .step(session, turn, StepKind::Suspended, None, &payload)
        .await
    {
        tracing::error!(%session, %turn, "139: drain: suspended step: {e:#}");
    }
    Some(turn)
}

/// `tell(CancelTurn)`, then a graceful stop: kameo processes the queued
/// cancel before the stop signal (`stop_gracefully`), so the turn's token
/// always fires — `kill()` would drop it with the mailbox (139 D19).
pub(crate) async fn stop_actor<A>(actor: &ActorRef<A>)
where
    A: kameo::Actor + Message<CancelTurn>,
{
    let _ = actor.tell(CancelTurn { keep: None }).await;
    let _ = actor.stop_gracefully().await;
    actor.wait_for_shutdown().await;
}

/// `ask` from the drain task once the actor is stopped: forget the live
/// entry as `UnloadSession` does; keep refusing inputs for `hold` (a
/// release — until [`demoted`] or the timer), or stop refusing now.
pub(super) struct SessionDrained {
    session: SessionId,
    hold: Option<Duration>,
}

impl Message<SessionDrained> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: SessionDrained, ctx: &mut Context<Self, Self::Reply>) {
        self.drop_queued(m.session).await;
        self.sessions.remove(&m.session);
        self.running.remove(&m.session);
        match m.hold {
            None => {
                self.draining_sessions.remove(&m.session);
            }
            Some(hold) => {
                let me = ctx.actor_ref().clone();
                tokio::spawn(async move {
                    tokio::time::sleep(hold).await;
                    let _ = me.tell(ClearDraining(m.session)).await;
                });
            }
        }
    }
}

/// `tell`: the session is no longer draining — the demote landed or the
/// hold expired.
pub struct ClearDraining(pub SessionId);

impl Message<ClearDraining> for Agents {
    type Reply = ();

    async fn handle(
        &mut self,
        ClearDraining(session): ClearDraining,
        _: &mut Context<Self, Self::Reply>,
    ) {
        self.draining_sessions.remove(&session);
    }
}

/// `sessionsync::owner::demote` → the taker's row landed: inputs for
/// `session` are no longer ours to refuse (they forward to the new owner).
pub async fn demoted(session: SessionId) {
    if let Ok(Some(agents)) = ActorRef::<Agents>::lookup(crate::AGENT_ACTOR) {
        let _ = agents.tell(ClearDraining(session)).await;
    }
}

/// The owner's drain window for a release (139 §6.1, Q4): the taker waits
/// `release_wait_secs + drain_secs` for `Released`; the owner drains for
/// `release_wait_secs − 1`, then flushes and replies inside that.
pub fn release_window(release_wait_secs: u64) -> Duration {
    Duration::from_secs(release_wait_secs.saturating_sub(1))
}

/// 139 §7: the per-session pass on the new owner for a resumed turn —
/// child waits settled or left to forwarding (`child_waits::settle_on_transfer`,
/// W3), the previous owner's jobs settled `lost` with the owner-changed
/// reason. A no-op for `crash | shutdown | update`.
pub async fn on_resume_here(
    agents: &mut Agents,
    session: SessionId,
    cause: ResumeCause,
    from: Option<&str>,
) {
    if !matches!(cause, ResumeCause::Transfer | ResumeCause::Takeover) {
        return;
    }
    child_waits::settle_on_transfer(agents, session, from).await;
    settle_transferred_jobs(agents, session, from).await;
}

/// §7.1 on the new owner: every `running` row another engine spawned →
/// `lost` through the store (no `adopt`, no `killpg`: I6), then the rows
/// reach the jobs actor as `Reaped` with `LostReason::OwnerChanged`.
/// W3's `reaper::settle_transferred` is not in the tree; this is the
/// smallest body over C's `jobs_settle_transferred` (no `<job_result>`
/// injection — the resumed batch reports its own `started` jobs).
async fn settle_transferred_jobs(agents: &mut Agents, session: SessionId, from: Option<&str>) {
    let now = Utc::now();
    let rows = match agents
        .store
        .jobs_settle_transferred(session, agents.store.engine(), now.timestamp_millis())
        .await
    {
        Ok(rows) => rows,
        Err(e) => return tracing::warn!(%session, "139: jobs_settle_transferred: {e:#}"),
    };
    if rows.is_empty() {
        return;
    }
    let mut jobs = Vec::with_capacity(rows.len());
    for row in rows {
        let screen = match row.kind {
            JobKind::Pty => agents
                .store
                .pty_screen(session, &row.id)
                .await
                .ok()
                .flatten(),
            JobKind::Shell => None,
        };
        jobs.push(LostJob {
            screen,
            tail: String::new(),
            deliver: None,
            reason: LostReason::OwnerChanged {
                engine: from.map_or_else(|| row.engine.clone(), str::to_owned),
            },
            row,
        });
    }
    tracing::info!(%session, n = jobs.len(), "139: previous owner's jobs settled lost");
    match ActorRef::<Jobs>::lookup(JOBS_ACTOR) {
        Ok(Some(actor)) => {
            let _ = actor
                .tell(Reaped {
                    restarted_at: now,
                    jobs,
                })
                .await;
        }
        _ => tracing::warn!("139: jobs actor not running: transferred jobs not listed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_window_leaves_the_last_second_for_the_flush() {
        assert_eq!(release_window(10), Duration::from_secs(9));
        assert_eq!(release_window(1), Duration::ZERO);
        assert_eq!(release_window(0), Duration::ZERO);
    }

    #[test]
    fn draining_error_is_recognisable_by_its_prefix() {
        let id = SessionId::new();
        let e = draining_error(id, RELEASE);
        assert!(
            e.starts_with(DRAINING_PREFIX) && e.contains("moving"),
            "{e}"
        );
        let e = draining_error(id, "update");
        assert!(
            e.starts_with(DRAINING_PREFIX) && e.contains("pausing for update"),
            "{e}"
        );
    }
}
