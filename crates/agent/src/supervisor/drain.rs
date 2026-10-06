//! Design doc 109 §9.2: graceful shutdown. `main.rs`, the rpc `shutdown`
//! method and `t update` call [`drain_engine`] **before** `rt.shutdown`
//! fires (every turn's cancel token is a child of it, `loop_.rs`
//! `start_turn`): in-flight turns get until the deadline, every open turn is
//! journaled `suspended` (durably, at the start, so a SIGKILL during the
//! window still reads as a planned stop), the turns still open at the
//! deadline get a `TurnSuspended` event, idle Monty workers are closed, and
//! the existing shutdown continues (jobs `KillAll` on the token, W2).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use runtime::{Runtime, SessionId, TurnId};

use super::{Agents, LiveActor};
use crate::durability::{DurabilityConfig, StepKind};
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

/// `ask` → every open turn `(session, turn, newest step at)`, most recent
/// first (the attention inbox's `resume:` items, §8.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenTurns;

impl Message<OpenTurns> for Agents {
    type Reply = Vec<(SessionId, TurnId, DateTime<Utc>)>;

    async fn handle(&mut self, _: OpenTurns, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.store.open_turns().await.unwrap_or_else(|e| {
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
            let open = store.open_turns().await.unwrap_or_else(|e| {
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
            let mut busy: Vec<LiveActor> = Vec::with_capacity(live.len());
            for actor in live {
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
            for (session, turn, _) in store.open_turns().await.unwrap_or_default() {
                suspended += 1;
                rt.publish(AgentEvent::TurnSuspended {
                    session,
                    turn,
                    reason: m.reason.to_owned(),
                })
                .await;
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
    /// of `session`'s open turn; `(None, None)` when no turn is open.
    pub(super) async fn resume_state(
        &self,
        session: SessionId,
    ) -> (Option<ResumeState>, Option<HistoryEntry>) {
        let open = match self.store.open_turns().await {
            Ok(open) => open,
            Err(e) => {
                tracing::error!(%session, "109: open_turns: {e:#}");
                return (None, None);
            }
        };
        let Some((_, turn, _)) = open.into_iter().find(|(s, _, _)| *s == session) else {
            return (None, None);
        };
        match self.store.resume_plan(session, turn).await {
            Ok(plan) => (
                Some(ResumeState {
                    turn,
                    kind: plan.kind,
                    interrupted_at: plan.interrupted_at,
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

    /// An open turn in the journal gets the `suspended` step and the event;
    /// the report counts it; a second drain in the same process is a no-op.
    #[tokio::test]
    async fn drain_suspends_open_turns_once() {
        let a = agents_with(false).await;
        let (store, rt) = (a.store.clone(), a.rt.clone());
        let (tx, mut events) = unbounded_channel();
        let sink = Sink::spawn_with_mailbox(Sink(tx), runtime::subscriber_mailbox());
        rt.subscribe::<Sink, AgentEvent>(&sink).await;
        let (sid, turn) = (SessionId::new(), TurnId::new());
        store
            .create_session(sid, "m", None, None, None)
            .await
            .unwrap();
        store
            .step(
                sid,
                turn,
                StepKind::InputAccepted,
                None,
                &serde_json::json!({}),
            )
            .await
            .unwrap();
        let agents = Agents::spawn(a);

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
        let ev = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap();
        assert!(
            matches!(ev, Some(AgentEvent::TurnSuspended { session, turn: t, ref reason }) if session == sid && t == turn && reason == "update"),
            "{ev:?}"
        );
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
}
