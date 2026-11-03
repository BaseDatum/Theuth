//! Design doc 109 §9.2: graceful shutdown. `main.rs`, the rpc `shutdown`
//! method and `t update` call [`drain_engine`] **before** `rt.shutdown`
//! fires (every turn's cancel token is a child of it, `loop_.rs`
//! `start_turn`): in-flight turns get until the deadline, every open turn is
//! journaled `suspended` (durably, at the start, so a SIGKILL during the
//! window still reads as a planned stop), the turns still open at the
//! deadline get a `TurnSuspended` event, the job groups are killed (rows
//! `lost` first, §5.5 — awaited here so the rows are on disk before the
//! process exits), idle Monty workers are closed, and the existing shutdown
//! continues. The loop reads [`draining`]: no new model call opens during
//! the window and a batch cut by the token stays open for the resume.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use runtime::{Runtime, SessionId, TurnId};

use super::{Agents, LiveActor};
use crate::durability::{DurabilityConfig, StepKind};
use crate::loop_::Draining;
use crate::store::{Scope, Store};
use crate::{AGENT_ACTOR, AgentEvent, HistoryEntry, ResumeState};

/// Set by the first [`Drain`]; a second one (the token-fired path in
/// `main.rs` after an rpc `shutdown` already drained) is a no-op.
static DRAINING: AtomicBool = AtomicBool::new(false);

/// Whether a drain has begun in this process.
pub fn draining() -> bool {
    DRAINING.load(Ordering::SeqCst)
}

/// `ask` → [`DrainReport`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Drain {
    /// When to stop waiting and suspend what is still running.
    pub deadline: Instant,
    /// `TurnSuspended.reason`: `shutdown` | `update` | `parent_gone`.
    pub reason: &'static str,
}

/// What the drain did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, kameo::Reply)]
pub struct DrainReport {
    /// Open turns that got a `suspended` step.
    pub suspended: u32,
    /// Turns that finished inside the window.
    pub finished: u32,
}

/// `ask` → every open turn `(session, turn, newest step at)` in `Scope`,
/// most recent first (the attention inbox's `resume:` items, §8.3; the
/// schedule plugin). 139 §5.1 item 5: acting callers pass `Scope::Here`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenTurns(pub Scope);

impl Message<OpenTurns> for Agents {
    type Reply = Vec<(SessionId, TurnId, DateTime<Utc>)>;

    async fn handle(&mut self, m: OpenTurns, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.store.open_turns(m.0).await.unwrap_or_else(|e| {
            tracing::error!("109: open_turns: {e:#}");
            Vec::new()
        })
    }
}

impl Message<Drain> for Agents {
    type Reply = DelegatedReply<DrainReport>;

    /// Off the mailbox: a turn ending inside the window asks this actor
    /// (`child_finished`, continuations), so waiting here would deadlock.
    async fn handle(&mut self, m: Drain, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        if DRAINING.swap(true, Ordering::SeqCst) {
            tx.send(DrainReport::default());
            return reply;
        }
        let live: Vec<LiveActor> = self
            .sessions
            .values()
            .filter(|l| l.actor.is_alive())
            .map(|l| l.actor.clone())
            .collect();
        let (store, rt, pool) = (self.store.clone(), self.rt.clone(), self.pool.clone());
        tokio::spawn(async move {
            // Mark first (§9.2 step 4), durably: a kill inside the window
            // still leaves the planned-stop wording; `turn_finished` closes
            // the row of a turn that makes it.
            // 139 D16 (§5.1 item 6): a replica writes nothing for the owner's turn.
            let open = store.open_turns(Scope::Here).await.unwrap_or_else(|e| {
                tracing::error!("109: drain: open_turns: {e:#}");
                Vec::new()
            });
            let payload = serde_json::json!({ "reason": m.reason });
            for (session, turn, _) in &open {
                if let Err(e) = store
                    .step(*session, *turn, StepKind::Suspended, None, &payload)
                    .await
                {
                    tracing::error!(%session, %turn, "109: drain: suspended step: {e:#}");
                }
            }
            // §9.2 step 2: no new model call opens in the window (the loop
            // reads the flag at each `round`); in-flight tool calls finish.
            let mut busy: Vec<LiveActor> = Vec::with_capacity(live.len());
            for actor in live {
                if let LiveActor::Native(session) = &actor {
                    let _ = session.tell(Draining).await;
                }
                if actor.is_running().await {
                    busy.push(actor);
                }
            }
            let started = busy.len();
            let finished = wait_idle(m.deadline, &mut busy).await;
            tracing::info!(
                reason = m.reason,
                running = started,
                finished,
                "109: drain window closed"
            );
            // Still open after the window: the UI's planned-stop wording (§8.2).
            let mut suspended = 0;
            for (session, turn, _) in store.open_turns(Scope::Here).await.unwrap_or_default() {
                suspended += 1;
                rt.publish(AgentEvent::TurnSuspended {
                    session,
                    turn,
                    reason: m.reason.to_owned(),
                })
                .await;
            }
            // §9.2 step 6: the job groups go after the window, rows `lost`
            // first (§5.5). The token-fired `KillAll` that follows is a no-op.
            if let Ok(Some(jobs)) = ActorRef::<tools::Jobs>::lookup(tools::JOBS_ACTOR)
                && let Err(e) = jobs.ask(tools::jobs::KillAll).await
            {
                tracing::warn!("109: drain: KillAll: {e}");
            }
            // §9.2 step 5: idle workers exit cleanly; a checked-out one goes
            // with its turn (the previous dump stands, 08 §5).
            if let Some(pool) = pool {
                pool.close().await;
            }
            tx.send(DrainReport {
                suspended,
                finished,
            });
        });
        reply
    }
}

/// A turn whose end the drain waits for.
trait Busy {
    fn running(&self) -> impl Future<Output = bool> + Send;
}

impl Busy for LiveActor {
    fn running(&self) -> impl Future<Output = bool> + Send {
        self.is_running()
    }
}

/// Polls `busy` every 100 ms, dropping the ones that went idle, until none
/// is left or `deadline` passes; returns how many went idle.
async fn wait_idle<B: Busy>(deadline: Instant, busy: &mut Vec<B>) -> u32 {
    let first = busy.len();
    loop {
        let mut still = Vec::with_capacity(busy.len());
        for b in busy.drain(..) {
            if b.running().await {
                still.push(b);
            }
        }
        *busy = still;
        let now = Instant::now();
        if busy.is_empty() || now >= deadline {
            return (first - busy.len()) as u32;
        }
        tokio::time::sleep(Duration::from_millis(100).min(deadline - now)).await;
    }
}

/// §9.2/§9.3: drain for `durability.drain_secs` (default 10) before the
/// shutdown token fires. No supervisor (a pairing run, an attached client)
/// → an empty report at once; a drain that overruns its deadline by more
/// than a second is abandoned (the process exits without it).
pub async fn drain_engine(rt: &Runtime, reason: &'static str) -> DrainReport {
    let Ok(Some(agents)) = ActorRef::<Agents>::lookup(AGENT_ACTOR) else {
        return DrainReport::default();
    };
    let secs = rt
        .config
        .contains("durability")
        .then(|| rt.section::<DurabilityConfig>("durability").ok())
        .flatten()
        .unwrap_or_default()
        .drain_secs;
    let deadline = Instant::now() + Duration::from_secs(secs);
    let report = tokio::time::timeout_at(
        (deadline + Duration::from_secs(1)).into(),
        agents.ask(Drain { deadline, reason }),
    )
    .await;
    match report {
        Ok(Ok(r)) => {
            tracing::info!(
                reason,
                suspended = r.suspended,
                finished = r.finished,
                "109: drained"
            );
            r
        }
        Ok(Err(e)) => {
            tracing::warn!("109: drain: {e}");
            DrainReport::default()
        }
        Err(_) => {
            tracing::warn!("109: drain overran its deadline; exiting without it");
            DrainReport::default()
        }
    }
}

impl Agents {
    /// §10.3: `Loaded.resume` and the `HistoryEntry::Interrupted` partial
    /// of `session`'s open turn; `(None, None)` when no turn is open and the
    /// newest turn was never resumed. `running` = the live actor's
    /// `IsRunning` (the same answer `Drain` uses); `resumed` = the turn it
    /// relaunched through `Resume`, if any. An open turn is reported while
    /// nothing runs it here (waiting / suspended) or while it runs as the
    /// relaunched one; an ordinary turn that is simply in flight is neither
    /// a banner nor an interrupted partial. With no open turn, the newest
    /// turn's `resume_reports` row (§8.2) keeps the banner's counts until
    /// the next turn starts.
    pub(super) async fn resume_state(
        store: &Store,
        session: SessionId,
        running: bool,
        resumed: Option<TurnId>,
    ) -> (Option<ResumeState>, Option<HistoryEntry>) {
        // 139 §5.1 item 12 / §5.2: a display read, any owner — the row says
        // who runs the turn; `resumed` (this engine's flag) matters only when
        // `here`.
        let (owner, here) = match store.session(session).await {
            Ok(Some(row)) => ((!row.owner.is_empty()).then_some(row.owner), row.here),
            Ok(None) => (None, true),
            Err(e) => {
                tracing::error!(%session, "139: session: {e:#}");
                (None, true)
            }
        };
        let open = match store.open_turn(session).await {
            Ok(open) => open,
            Err(e) => {
                tracing::error!(%session, "109: open_turn: {e:#}");
                return (None, None);
            }
        };
        let Some((turn, _)) = open else {
            return match store.latest_resume_report(session).await {
                Ok(Some(row)) => (
                    Some(ResumeState {
                        turn: row.turn,
                        kind: row.kind,
                        interrupted_at: row.interrupted_at,
                        resumed: true,
                        report: Some(row.report),
                        owner,
                        here,
                        cause: Some(row.cause),
                    }),
                    None,
                ),
                Ok(None) => (None, None),
                Err(e) => {
                    tracing::error!(%session, "109: latest_resume_report: {e:#}");
                    (None, None)
                }
            };
        };
        if running && resumed != Some(turn) {
            return (None, None);
        }
        let report = match store.resume_report(session, turn).await {
            Ok(row) => row.map(|r| r.report),
            Err(e) => {
                tracing::error!(%session, %turn, "109: resume_report: {e:#}");
                None
            }
        };
        match store.resume_plan(session, turn).await {
            Ok(plan) => (
                Some(ResumeState {
                    turn,
                    kind: plan.kind,
                    interrupted_at: plan.interrupted_at,
                    resumed: running,
                    report,
                    owner,
                    here,
                    cause: Some(plan.cause),
                }),
                plan.partial.map(|p| HistoryEntry::Interrupted {
                    text: crate::loop_::text_of(&p.content),
                    at: p.at,
                    context: None,
                }),
            ),
            Err(e) => {
                tracing::error!(%session, %turn, "109: resume_plan: {e:#}");
                (None, None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use kameo::Actor;
    use kameo::actor::Spawn as _;
    use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

    use super::*;
    use crate::supervisor::resume::tests::{b_holding_a_turn, own_turn};
    use crate::supervisor::tests::agents_with;

    /// Idle once `Instant` passes.
    struct Until(Instant);

    impl Busy for Until {
        fn running(&self) -> impl Future<Output = bool> + Send {
            let running = Instant::now() < self.0;
            async move { running }
        }
    }

    /// A quick batch (idle after 150 ms) is counted `finished`; a slow one
    /// (never idle) runs the window out and is not.
    #[tokio::test]
    async fn wait_idle_counts_turns_that_make_the_window() {
        let t0 = Instant::now();
        let mut busy = vec![Until(t0 + Duration::from_millis(150))];
        assert_eq!(wait_idle(t0 + Duration::from_secs(5), &mut busy).await, 1);
        assert!(busy.is_empty() && t0.elapsed() < Duration::from_secs(2));

        let t0 = Instant::now();
        let mut busy = vec![Until(t0 + Duration::from_secs(60)), Until(t0)];
        assert_eq!(
            wait_idle(t0 + Duration::from_millis(300), &mut busy).await,
            1
        );
        assert!(busy.len() == 1 && t0.elapsed() >= Duration::from_millis(300));
    }

    struct Sink(UnboundedSender<AgentEvent>);

    impl Actor for Sink {
        type Args = Self;
        type Error = Infallible;

        async fn on_start(sink: Self, _: ActorRef<Self>) -> Result<Self, Infallible> {
            Ok(sink)
        }
    }

    impl Message<AgentEvent> for Sink {
        type Reply = ();

        async fn handle(&mut self, e: AgentEvent, _: &mut Context<Self, ()>) {
            let _ = self.0.send(e);
        }
    }

    /// `b`'s own open turn gets the `suspended` step and the event and the
    /// report counts it; `a`'s open turn in the same file (139 D16 / I1) gets
    /// neither and `OpenTurns(Scope::Here)` does not list it; `resume_state`
    /// names `a` as its owner with `here: false`. A second drain in the same
    /// process is a no-op.
    #[tokio::test]
    async fn drain_suspends_own_open_turns_once() {
        let (_dir, b, foreign, foreign_turn) = b_holding_a_turn(false).await;
        let (store, rt) = (b.store.clone(), b.rt.clone());
        let (tx, mut events) = unbounded_channel();
        let sink = Sink::spawn_with_mailbox(Sink(tx), runtime::subscriber_mailbox());
        rt.subscribe::<Sink, AgentEvent>(&sink).await;
        let (sid, turn) = own_turn(&b).await;
        let (state, _) = Agents::resume_state(&b.store, foreign, false, None).await;
        let state = state.unwrap();
        assert_eq!((state.owner.as_deref(), state.here), (Some("a"), false));
        let (state, _) = Agents::resume_state(&b.store, sid, false, None).await;
        let state = state.unwrap();
        assert_eq!((state.owner.as_deref(), state.here), (Some("b"), true));
        let agents = Agents::spawn(b);

        let here: Vec<SessionId> = agents
            .ask(OpenTurns(Scope::Here))
            .await
            .unwrap()
            .into_iter()
            .map(|(s, _, _)| s)
            .collect();
        assert_eq!(here, [sid]);
        assert_eq!(agents.ask(OpenTurns(Scope::All)).await.unwrap().len(), 2);

        let deadline = Instant::now() + Duration::from_millis(200);
        let report = agents
            .ask(Drain {
                deadline,
                reason: "update",
            })
            .await
            .unwrap();
        assert_eq!(
            report,
            DrainReport {
                suspended: 1,
                finished: 0
            }
        );
        let plan = store.resume_plan(sid, turn).await.unwrap();
        assert_eq!(plan.kind, crate::durability::ResumeKind::Suspended);
        let plan = store.resume_plan(foreign, foreign_turn).await.unwrap();
        assert_eq!(
            plan.kind,
            crate::durability::ResumeKind::Interrupted,
            "a's turn untouched"
        );
        let ev = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap();
        assert!(
            matches!(ev, Some(AgentEvent::TurnSuspended { session, turn: t, ref reason }) if session == sid && t == turn && reason == "update"),
            "{ev:?}"
        );
        assert!(events.try_recv().is_err(), "no TurnSuspended for a's turn");
        assert!(draining());
        let again = agents
            .ask(Drain {
                deadline,
                reason: "update",
            })
            .await
            .unwrap();
        assert_eq!(again, DrainReport::default());
    }

    /// `Loaded.resume` for the same open turn: `resumed: false` while ask
    /// mode waits (not running) and `true` once the actor relaunched that
    /// turn; no open turn → `None` either way. Once the resume settled, its
    /// `resume_reports` row rides along — while the turn is open and, after
    /// it finished, as the newest turn's state — until a newer turn starts.
    #[tokio::test]
    async fn resume_state_reports_relaunched_turn() {
        let (_scratch, a) = agents_with(false).await;
        let (sid, turn) = (SessionId::new(), TurnId::new());
        a.store
            .create_session(sid, "m", None, None, None)
            .await
            .unwrap();
        assert!(
            Agents::resume_state(&a.store, sid, true, None)
                .await
                .0
                .is_none()
        );
        a.store
            .step(
                sid,
                turn,
                StepKind::InputAccepted,
                None,
                &serde_json::json!({}),
            )
            .await
            .unwrap();
        let waiting = Agents::resume_state(&a.store, sid, false, None)
            .await
            .0
            .unwrap();
        assert!(waiting.turn == turn && !waiting.resumed);
        let relaunched = Agents::resume_state(&a.store, sid, true, Some(turn))
            .await
            .0
            .unwrap();
        assert_eq!(
            relaunched,
            ResumeState {
                resumed: true,
                ..waiting
            }
        );
        assert!(relaunched.report.is_none());
        // The resumed step settled: the report rides the open turn's state…
        let row = crate::durability::ResumeReportRow {
            session: sid,
            turn,
            kind: relaunched.kind,
            cause: relaunched.cause.unwrap(),
            interrupted_at: relaunched.interrupted_at,
            at: chrono::Utc::now(),
            report: crate::ResumeReport {
                replayed: vec![("c1".into(), "read".into())],
                interrupted: vec![("c2".into(), "write".into())],
                ..Default::default()
            },
        };
        a.store.resume_report_put(&row).await.unwrap();
        let settled = Agents::resume_state(&a.store, sid, true, Some(turn))
            .await
            .0
            .unwrap();
        assert_eq!(
            settled,
            ResumeState {
                report: Some(row.report.clone()),
                ..relaunched.clone()
            }
        );
        // …and, once the turn finished, as the newest turn's state (not running).
        a.store.finish_turn(sid, turn, "end_turn").await.unwrap();
        let (finished, partial) = Agents::resume_state(&a.store, sid, false, None).await;
        assert!(partial.is_none());
        assert_eq!(
            finished.unwrap(),
            ResumeState {
                resumed: true,
                report: Some(row.report),
                ..relaunched
            }
        );
        // A newer, ordinary turn clears it: nothing while it runs, the plain
        // waiting state (no report) once it does not.
        let next = TurnId::new();
        a.store
            .step(
                sid,
                next,
                StepKind::InputAccepted,
                None,
                &serde_json::json!({}),
            )
            .await
            .unwrap();
        assert!(
            Agents::resume_state(&a.store, sid, true, None)
                .await
                .0
                .is_none()
        );
        let waiting = Agents::resume_state(&a.store, sid, false, None)
            .await
            .0
            .unwrap();
        assert!(waiting.turn == next && !waiting.resumed && waiting.report.is_none());
    }

    /// An ordinary turn in flight (`input_accepted`, `model_call`, a partial
    /// row), actor running, never relaunched → no banner, no interrupted
    /// partial: a load of a session that is simply working is not a restart.
    #[tokio::test]
    async fn resume_state_ignores_an_ordinary_running_turn() {
        let (_scratch, a) = agents_with(false).await;
        let (sid, turn) = (SessionId::new(), TurnId::new());
        a.store
            .create_session(sid, "m", None, None, None)
            .await
            .unwrap();
        a.store
            .step(
                sid,
                turn,
                StepKind::InputAccepted,
                None,
                &serde_json::json!({}),
            )
            .await
            .unwrap();
        a.store
            .step(
                sid,
                turn,
                StepKind::ModelCall,
                Some("mc1"),
                &serde_json::json!({"request_id": "r1", "attempt": 1}),
            )
            .await
            .unwrap();
        a.store
            .partial_upsert(
                sid,
                turn,
                "mc1",
                &genai::chat::MessageContent::from_text("half"),
                None,
            )
            .await
            .unwrap();
        // Not running here (a crash): banner + the greyed partial, as before.
        let (state, partial) = Agents::resume_state(&a.store, sid, false, None).await;
        assert!(state.is_some_and(|s| s.turn == turn && !s.resumed));
        assert!(
            matches!(partial, Some(HistoryEntry::Interrupted { ref text, .. }) if text == "half"),
            "{partial:?}"
        );
        // Running as an ordinary turn: nothing.
        let (state, partial) = Agents::resume_state(&a.store, sid, true, None).await;
        assert!(state.is_none(), "no banner: {state:?}");
        assert!(partial.is_none(), "no interrupted partial: {partial:?}");
    }
}
