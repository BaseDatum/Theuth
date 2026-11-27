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

/// Reversible suspension flag set by [`SuspendForBackground`] and cleared by
/// [`ThawFromBackground`]. Separate from [`DRAINING`], which is a one-way
/// process-exit latch.
static SUSPENDED: AtomicBool = AtomicBool::new(false);

/// Whether a drain has begun in this process.
pub fn draining() -> bool {
    DRAINING.load(Ordering::SeqCst)
}

/// Whether the engine is currently suspended for OS backgrounding (e.g. iOS).
pub fn suspended() -> bool {
    SUSPENDED.load(Ordering::SeqCst)
}

/// Held by tests that set [`draining`] or [`suspended`] or need them unset:
/// the flags are process-wide, so tests serialize on this mutex.
#[cfg(test)]
pub(crate) static DRAIN_FLAG: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Clear [`draining`] (under [`DRAIN_FLAG`]).
#[cfg(test)]
pub(crate) fn reset_draining() {
    DRAINING.store(false, Ordering::SeqCst);
}

/// `ask` → [`DrainReport`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Drain {
    /// When to stop waiting and suspend what is still running.
    pub deadline: Instant,
    /// `TurnSuspended.reason`: `shutdown` | `update` | `parent_gone`.
    pub reason: &'static str,
}

/// `ask` → [`DrainReport`]: reversible durable suspension for OS backgrounding
/// (e.g. iOS `sceneDidEnterBackground`). Leaves [`DRAINING`] untouched so the
/// process can resume cleanly on [`ThawFromBackground`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SuspendForBackground;

/// `ask` → `u32`: reversible resume after [`SuspendForBackground`] (e.g. iOS
/// `sceneWillEnterForeground`). Clears the suspension flag, tears down stale
/// pre-freeze live turn handles, and immediately triggers `resume_scan` so
/// interrupted turns resume without waiting for `stream_idle_secs`. Returns
/// the number of open turns handed to the resume scan.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThawFromBackground;

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
        let open = self.store.open_turns(m.0).await.unwrap_or_else(|e| {
            tracing::error!("109: open_turns: {e:#}");
            Vec::new()
        });
        // 179 D8: a paused agent's turns are not offered for a Resume.
        let mut out = Vec::with_capacity(open.len());
        for row in open {
            if self.paused_root(row.0).await.is_none() {
                out.push(row);
            }
        }
        out
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
        // 179: a paused agent's tree was suspended by its pause; a second
        // `suspended` row without a run clock would count the paused time.
        let held = self.held_roots();
        tokio::spawn(async move {
            // Mark first (§9.2 step 4), durably: a kill inside the window
            // still leaves the planned-stop wording; `turn_finished` closes
            // the row of a turn that makes it.
            // 139 D16 (§5.1 item 6): a replica writes nothing for the owner's turn.
            let open = store.open_turns(Scope::Here).await.unwrap_or_else(|e| {
                tracing::error!("109: drain: open_turns: {e:#}");
                Vec::new()
            });
            let open = not_held(&store, &held, open).await;
            for (session, turn, _) in &open {
                // 178 F1: the turn's accrued run time (cumulative across
                // resumes) rides on the row; a turn not running here has none.
                let mut payload = serde_json::json!({ "reason": m.reason });
                if let Some(ms) = crate::durability::run_clock::ran_ms(*turn) {
                    payload["ran_ms"] = ms.into();
                }
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
                match &actor {
                    LiveActor::Native(session) => {
                        let _ = session.tell(Draining).await;
                    }
                    #[cfg(not(target_os = "ios"))]
                    LiveActor::Acp(_) => {}
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
            for (session, turn, _) in not_held(
                &store,
                &held,
                store.open_turns(Scope::Here).await.unwrap_or_default(),
            )
            .await
            {
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

/// How long [`SuspendForBackground`] waits for cancelled streams to flush their
/// partials before checkpointing the SQLite WAL (fits inside iOS's ~5 s
/// background transition budget).
const BG_SUSPEND_WAIT: Duration = Duration::from_secs(3);

/// `TurnSuspended.reason` recorded by [`SuspendForBackground`].
pub const IOS_BACKGROUND_REASON: &str = "ios_background";

impl Message<SuspendForBackground> for Agents {
    type Reply = DelegatedReply<DrainReport>;

    async fn handle(
        &mut self,
        _: SuspendForBackground,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        SUSPENDED.store(true, Ordering::SeqCst);
        let live: Vec<LiveActor> = self
            .sessions
            .values()
            .filter(|l| l.actor.is_alive())
            .map(|l| l.actor.clone())
            .collect();
        let (store, rt) = (self.store.clone(), self.rt.clone());
        let held = self.held_roots();
        tokio::spawn(async move {
            // 1. Durably journal `suspended` for every open local turn first,
            //    so an OS freeze or jetsam mid-suspend reads as a planned stop.
            let open = store.open_turns(Scope::Here).await.unwrap_or_else(|e| {
                tracing::error!("ios_background: open_turns: {e:#}");
                Vec::new()
            });
            let open = not_held(&store, &held, open).await;
            for (session, turn, _) in &open {
                let mut payload = serde_json::json!({ "reason": IOS_BACKGROUND_REASON });
                if let Some(ms) = crate::durability::run_clock::ran_ms(*turn) {
                    payload["ran_ms"] = ms.into();
                }
                if let Err(e) = store
                    .step(*session, *turn, StepKind::Suspended, None, &payload)
                    .await
                {
                    tracing::error!(%session, %turn, "ios_background: suspended step: {e:#}");
                }
            }
            // 2. Cancel active sessions so in-flight LLM streams flush their
            //    live `assistant_partial` (via `Turn::stream` while
            //    `suspending()`) and close HTTP sockets cleanly without
            //    marking the turns cancelled.
            let mut busy: Vec<LiveActor> = Vec::with_capacity(live.len());
            for actor in live {
                if actor.is_running().await {
                    actor.tell_cancel(None).await;
                    busy.push(actor);
                }
            }
            let finished = wait_idle(Instant::now() + BG_SUSPEND_WAIT, &mut busy).await;
            // 3. Publish `TurnSuspended` for every remaining open local turn.
            let mut suspended_count = 0;
            for (session, turn, _) in not_held(
                &store,
                &held,
                store.open_turns(Scope::Here).await.unwrap_or_default(),
            )
            .await
            {
                suspended_count += 1;
                rt.publish(AgentEvent::TurnSuspended {
                    session,
                    turn,
                    reason: IOS_BACKGROUND_REASON.to_owned(),
                })
                .await;
            }
            // 4. Truncate the WAL into the main SQLite file before iOS freezes
            //    or jetsams the process.
            if let Err(e) = store.wal_checkpoint_truncate().await {
                tracing::warn!("ios_background: wal_checkpoint_truncate: {e:#}");
            }
            tracing::info!(
                suspended = suspended_count,
                finished,
                "ios_background: suspended and checkpointed"
            );
            tx.send(DrainReport {
                suspended: suspended_count,
                finished,
            });
        });
        reply
    }
}

impl Message<ThawFromBackground> for Agents {
    type Reply = u32;

    async fn handle(
        &mut self,
        _: ThawFromBackground,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        SUSPENDED.store(false, Ordering::SeqCst);
        let open = self.open_here().await;
        let (_, open) = self.unpaused_turns(Vec::new(), open).await;
        let open_sessions: std::collections::HashSet<SessionId> =
            open.iter().map(|(s, _, _)| *s).collect();
        // Proactively tear down any stale live session handles left from
        // pre-freeze (still marked running, dead, or holding an open turn) so
        // `resume_scan` starts fresh actors instead of colliding with stale
        // sockets or `inflight` tasks.
        let stale: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|(id, live)| {
                !live.actor.is_alive()
                    || self.running.contains(id)
                    || open_sessions.contains(id)
            })
            .map(|(id, _)| *id)
            .collect();
        for id in stale {
            if let Some(live) = self.sessions.remove(&id) {
                live.actor.kill().await;
            }
            self.running.remove(&id);
            if let Some(timer) = self.child_turns.remove(&id).and_then(|ct| ct.timer) {
                timer.abort();
            }
            crate::durability::run_clock::stop_session(id);
            self.endless.detach(id);
        }
        let count = open.len() as u32;
        if !open.is_empty() {
            tracing::info!(open = count, "ios_background: thawing and resuming open turns");
            self.resume_scan(&open).await;
        }
        count
    }
}

/// `open` minus the turns of sessions under a `held` root (179).
async fn not_held(
    store: &Store,
    held: &std::collections::HashSet<SessionId>,
    open: Vec<(SessionId, TurnId, DateTime<Utc>)>,
) -> Vec<(SessionId, TurnId, DateTime<Utc>)> {
    if held.is_empty() {
        return open;
    }
    let mut out = Vec::with_capacity(open.len());
    for row in open {
        match store.root_of(row.0).await {
            Ok(root) if held.contains(&root) => {}
            _ => out.push(row),
        }
    }
    out
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

/// Suspend the local agent supervisor for OS backgrounding (e.g. iOS
/// `sceneDidEnterBackground`): flushes live partials, cancels in-flight LLM
/// streams, records `StepKind::Suspended { reason: "ios_background" }`, and
/// runs `PRAGMA wal_checkpoint(TRUNCATE)`. Reversible via [`thaw_engine`].
pub async fn suspend_engine(_rt: &Runtime) -> DrainReport {
    let Ok(Some(agents)) = ActorRef::<Agents>::lookup(AGENT_ACTOR) else {
        return DrainReport::default();
    };
    let report = tokio::time::timeout(
        BG_SUSPEND_WAIT + Duration::from_secs(2),
        agents.ask(SuspendForBackground),
    )
    .await;
    match report {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            tracing::warn!("ios_background: suspend_engine: {e}");
            DrainReport::default()
        }
        Err(_) => {
            tracing::warn!("ios_background: suspend_engine timed out");
            DrainReport::default()
        }
    }
}

/// Resume the local agent supervisor after [`suspend_engine`] (e.g. iOS
/// `sceneWillEnterForeground`): clears the suspension latch, tears down stale
/// pre-freeze handles, and triggers `resume_scan` immediately. Returns the
/// number of open turns handed to the resume scan.
pub async fn thaw_engine(_rt: &Runtime) -> u32 {
    let Ok(Some(agents)) = ActorRef::<Agents>::lookup(AGENT_ACTOR) else {
        SUSPENDED.store(false, Ordering::SeqCst);
        return 0;
    };
    match agents.ask(ThawFromBackground).await {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!("ios_background: thaw_engine: {e}");
            SUSPENDED.store(false, Ordering::SeqCst);
            0
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
        // 183 R1 (H3): the engine a moved or taken-over session came from.
        let from = || async {
            store
                .moved(session)
                .await
                .ok()
                .and_then(|m| m.into_values().next_back())
                .map(|m| m.from_id)
                .filter(|f| !f.is_empty())
        };
        let moved = |c: crate::durability::ResumeCause| {
            matches!(
                c,
                crate::durability::ResumeCause::Transfer | crate::durability::ResumeCause::Takeover
            )
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
                        from: if moved(row.cause) { from().await } else { None },
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
                    from: if moved(plan.cause) {
                        from().await
                    } else {
                        None
                    },
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
        let _flag = DRAIN_FLAG.lock().await;
        reset_draining();
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
        reset_draining();
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

    /// `SuspendForBackground` / `ThawFromBackground`: reversible lifecycle that
    /// leaves desktop's one-way `DRAINING` latch untouched, records
    /// `StepKind::Suspended { reason: "ios_background" }` for local open turns,
    /// checkpoints WAL, and triggers `resume_scan` immediately on thaw.
    #[tokio::test]
    async fn suspend_for_background_is_reversible_and_thaw_resumes() {
        let _flag = DRAIN_FLAG.lock().await;
        reset_draining();
        SUSPENDED.store(false, Ordering::SeqCst);

        let (_dir, mut b, foreign, foreign_turn) = b_holding_a_turn(false).await;
        // Use `ResumeMode::Ask` so `resume_scan` does not spawn real model HTTP
        // requests in this offline unit test while still verifying the full
        // suspend -> thaw -> suspend -> thaw cycle.
        std::sync::Arc::make_mut(&mut b.cfg).durability.resume = crate::durability::ResumeMode::Ask;
        let (store, rt) = (b.store.clone(), b.rt.clone());
        let (tx, mut events) = unbounded_channel();
        let sink = Sink::spawn_with_mailbox(Sink(tx), runtime::subscriber_mailbox());
        rt.subscribe::<Sink, AgentEvent>(&sink).await;
        let (sid, turn) = own_turn(&b).await;
        let agents = Agents::spawn(b);

        // First suspend cycle.
        let report = agents.ask(SuspendForBackground).await.unwrap();
        assert!(suspended(), "SUSPENDED flag set");
        assert!(!draining(), "desktop one-way DRAINING latch untouched");
        assert_eq!(
            report,
            DrainReport {
                suspended: 1,
                finished: 0
            }
        );
        let plan = store.resume_plan(sid, turn).await.unwrap();
        assert_eq!(plan.kind, crate::durability::ResumeKind::Suspended);
        let foreign_plan = store.resume_plan(foreign, foreign_turn).await.unwrap();
        assert_eq!(
            foreign_plan.kind,
            crate::durability::ResumeKind::Interrupted,
            "foreign turn untouched"
        );
        let ev = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap();
        assert!(
            matches!(
                ev,
                Some(AgentEvent::TurnSuspended {
                    session,
                    turn: t,
                    ref reason
                }) if session == sid && t == turn && reason == IOS_BACKGROUND_REASON
            ),
            "{ev:?}"
        );

        // Thaw clears SUSPENDED and immediately scans the 1 open local turn.
        let thawed = agents.ask(ThawFromBackground).await.unwrap();
        assert!(!suspended(), "SUSPENDED cleared on thaw");
        assert_eq!(thawed, 1);

        // Reversible: a second background cycle works cleanly too.
        let second = agents.ask(SuspendForBackground).await.unwrap();
        assert!(suspended());
        assert_eq!(second.suspended, 1);
        let thawed_again = agents.ask(ThawFromBackground).await.unwrap();
        assert!(!suspended());
        assert_eq!(thawed_again, 1);
    }
}
