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

use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use runtime::{Runtime, SessionId, TurnId};
use tools::{JOBS_ACTOR, JobId, Jobs};

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
/// `draining_sessions` (D16): `SessionDraining:<reason>` (the
/// `DocError::NotHub` convention). The RPC layer ([`crate::rpc_support::session_draining`])
/// maps it to `SESSION_DRAINING` with the user-facing wording.
pub const DRAINING_PREFIX: &str = "SessionDraining:";

/// The D16 refusal in its wire form; `reason` is `release` on the ownership
/// path (the owner side does not learn the taker's name — `Msg::Release`
/// carries none, so no `:<next_owner>` suffix).
pub fn draining_error(reason: &str) -> String {
    format!("{DRAINING_PREFIX}{reason}")
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
    // (W3's body). Before the actor stops so the rows precede `flush`. Its
    // reminder timers stop too (139 R2 D4): the taker arms the rows.
    if let Ok(Some(jobs)) = ActorRef::<Jobs>::lookup(JOBS_ACTOR) {
        if let Err(e) = jobs
            .ask(tools::jobs::KillSession { session: m.session })
            .await
        {
            tracing::warn!(session = %m.session, "139: drain: KillSession: {e}");
        }
        let _ = jobs
            .tell(tools::CancelReminders { session: m.session })
            .await;
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
    let mut payload = serde_json::json!({ "reason": reason });
    // 178 F1: the accrued run time moves with the session.
    if let Some(ms) = crate::durability::run_clock::ran_ms(turn) {
        payload["ran_ms"] = ms.into();
    }
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
        self.session_drained(m.session, m.hold.is_some()).await;
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

impl Agents {
    /// [`SessionDrained`] without its timer: the live entry and the queue go. 191 (D3):
    /// only a `release` drain moves the session: its first-contact holds and its waiting full
    /// contact messages fail ("the chat moved; send again"), never released or carried.
    /// A desk pause drains too, and keeps its holds.
    pub(crate) async fn session_drained(&mut self, session: SessionId, release: bool) {
        if release {
            self.network_session_drained(session).await;
        }
        self.drop_queued(session).await;
        self.sessions.remove(&session);
        self.running.remove(&session);
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

/// `tell` from [`demoted`]: another machine's take of `session` landed, a release's or a
/// forced one's.
pub(crate) struct SessionMoved(pub SessionId);

impl Message<SessionMoved> for Agents {
    type Reply = ();

    /// 191 (P3 final review N3): a forced take has no drain, so its holds and waiting full
    /// contact tasks fail here "the chat moved; send again", as a release drain's do (after a
    /// release they are already gone: nothing is left to fail). Then the input refusal ends.
    async fn handle(
        &mut self,
        SessionMoved(session): SessionMoved,
        _: &mut Context<Self, Self::Reply>,
    ) {
        self.network_session_drained(session).await;
        self.draining_sessions.remove(&session);
    }
}

/// `sessionsync::owner::demote` → the taker's row landed: inputs for
/// `session` are no longer ours to refuse (they forward to the new owner),
/// its contact holds and waiting full contact tasks fail ([`SessionMoved`]),
/// and its reminder timers stop here (139 R2 D4; a takeover had no drain).
pub async fn demoted(session: SessionId) {
    if let Ok(Some(agents)) = ActorRef::<Agents>::lookup(crate::AGENT_ACTOR) {
        let _ = agents.tell(SessionMoved(session)).await;
    }
    if let Ok(Some(jobs)) = ActorRef::<Jobs>::lookup(JOBS_ACTOR) {
        let _ = jobs.tell(tools::CancelReminders { session }).await;
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
/// reason (`reaper::settle_transferred`, W3). Returns the lost job ids for
/// the resume report. A no-op for `crash | shutdown | update`.
pub async fn on_resume_here(
    agents: &mut Agents,
    session: SessionId,
    cause: ResumeCause,
    from: Option<&str>,
) -> Vec<JobId> {
    if !matches!(cause, ResumeCause::Transfer | ResumeCause::Takeover) {
        return Vec::new();
    }
    child_waits::settle_on_transfer(agents, session, from).await;
    let reaped = crate::durability::jobs::settle_transferred(
        &agents.store,
        session,
        from.unwrap_or("another engine"),
    )
    .await;
    // 171F3 R1: a user-cell kernel is the cells actor's to report, never the agent's.
    reaped
        .jobs
        .into_iter()
        .filter(|j| !j.row.is_cell_kernel())
        .map(|j| j.row.id)
        .collect()
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
    fn draining_error_is_the_wire_form() {
        assert_eq!(draining_error(RELEASE), "SessionDraining:release");
        assert_eq!(draining_error("update"), "SessionDraining:update");
        assert!(draining_error("update").starts_with(DRAINING_PREFIX));
    }

    /// 139 R2 D4: `RearmReminders{session}` arms that session's stored rows
    /// on this engine and no other session's; a row of a session owned
    /// elsewhere is never armed here (it fires on its owner). The timers go
    /// again with `CancelReminders` (the demote), rows untouched.
    #[test]
    fn rearm_reminders_for_arms_one_sessions_rows_here_only() {
        crate::with_test_registry(async {
            use crate::store::{ReminderRow, Store};
            let scratch = crate::TestDir::new("139r2");
            let dir = scratch.0.clone();
            let far = chrono::Utc::now().timestamp() + 600;
            let row = |session, id: &str| ReminderRow {
                id: id.into(),
                session,
                message: "x".into(),
                set_at: far - 600,
                fires_at: far,
                seconds: 600,
            };
            // Engine `a` owned `foreign` and set a reminder on it.
            let foreign = SessionId::new();
            let a = Store::open(
                &dir.join("sessions.sqlite"),
                Some(mesh::EngineId("a".into())),
            )
            .await
            .unwrap();
            a.create_session(foreign, "m", None, None, None)
                .await
                .unwrap();
            a.insert_reminder(&row(foreign, "r1")).await.unwrap();
            a.close().await;

            let mut b = super::super::tests::agents_on(dir.clone(), Some("b"), false).await;
            // A previous body's jobs actor died with its runtime without its
            // `on_stop` unregistration (what `tools::registry::start` evicts).
            if ActorRef::<Jobs>::lookup(JOBS_ACTOR)
                .ok()
                .flatten()
                .is_some_and(|j| !j.is_alive())
            {
                kameo::registry::ACTOR_REGISTRY
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .remove(JOBS_ACTOR);
            }
            let jobs = tools::jobs::start(&b.rt, Duration::ZERO, Duration::from_secs(30)).unwrap();
            let (taken, other) = (SessionId::new(), SessionId::new());
            for s in [taken, other] {
                b.store
                    .create_session(s, "m", None, None, None)
                    .await
                    .unwrap();
                b.store.insert_reminder(&row(s, "r1")).await.unwrap();
            }
            b.store.insert_reminder(&row(taken, "r2")).await.unwrap();
            let armed = || async {
                let mut v: Vec<(String, Vec<String>)> = jobs
                    .ask(tools::jobs::LiveCounts)
                    .await
                    .unwrap()
                    .into_iter()
                    .filter(|r| !r.reminders.is_empty())
                    .map(|r| (r.session.to_string(), r.reminders))
                    .collect();
                v.sort();
                v
            };

            b.rearm_reminders_for(Some(taken)).await;
            let mut want = Vec::new();
            for _ in 0..100 {
                want = armed().await;
                if want.iter().any(|(_, r)| r.len() == 2) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert_eq!(
                want,
                vec![(taken.to_string(), vec!["r1".to_owned(), "r2".to_owned()])]
            );

            b.rearm_reminders_for(Some(foreign)).await;
            tokio::time::sleep(Duration::from_millis(200)).await;
            assert_eq!(
                armed().await.len(),
                1,
                "a session owned by `a` arms nothing here"
            );
            assert_eq!(
                b.store.reminders().await.unwrap().len(),
                4,
                "rows untouched"
            );

            jobs.tell(tools::CancelReminders { session: taken })
                .await
                .unwrap();
            for _ in 0..100 {
                if armed().await.is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert!(armed().await.is_empty());
            assert_eq!(
                b.store.reminders().await.unwrap().len(),
                4,
                "the demote deletes no row"
            );
        });
    }
}
