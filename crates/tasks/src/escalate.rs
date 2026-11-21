//! `escalate` — up the line, wait, then park (67 §11, decision 8). The
//! ledger writes the blocker and wakes the manager; this is the live wait:
//! the escalating tool call subscribes to its `decision:<id>` and the
//! `decide` tool (or the user's ask card) resolves it. Late answers still
//! clear the blocker in the ledger, which re-wakes the owner.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::sync::oneshot;

static WAIT: OnceLock<Duration> = OnceLock::new();

/// Name the live wait (`roster.decision_wait_secs`); once.
pub fn set_wait(d: Duration) {
    let _ = WAIT.set(d);
}

/// The live wait; the design default (120 s) until the plugin read the config.
pub fn wait_secs() -> Duration {
    WAIT.get().copied().unwrap_or(Duration::from_secs(120))
}

/// Waiters per decision and the answers already given.
#[derive(Default)]
pub struct Escalations {
    pending: HashMap<String, Vec<oneshot::Sender<String>>>,
    // ponytail: grows one string per answered decision per process; prune on a cadence if it ever shows.
    answered: HashMap<String, String>,
}

impl Escalations {
    /// A receiver for `decision`'s answer; resolved at once when it was already given.
    pub fn subscribe(&mut self, decision: &str) -> oneshot::Receiver<String> {
        let (tx, rx) = oneshot::channel();
        match self.answered.get(decision) {
            Some(a) => {
                let _ = tx.send(a.clone());
            }
            None => self
                .pending
                .entry(decision.to_owned())
                .or_default()
                .push(tx),
        }
        rx
    }

    /// Resolve every waiter on `decision` and remember the answer.
    pub fn answer(&mut self, decision: &str, text: &str) {
        for tx in self.pending.remove(decision).unwrap_or_default() {
            let _ = tx.send(text.to_owned());
        }
        self.answered.insert(decision.to_owned(), text.to_owned());
    }

    /// 207: an `escalate` is still waiting live on `decision` (a receiver that has not timed out).
    pub fn waiting(&self, decision: &str) -> bool {
        self.pending
            .get(decision)
            .is_some_and(|txs| txs.iter().any(|tx| !tx.is_closed()))
    }
}

/// The answer within `dur`, else `None` (the task stays parked).
pub async fn wait(rx: oneshot::Receiver<String>, dur: Duration) -> Option<String> {
    tokio::time::timeout(dur, rx).await.ok()?.ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn escalations_wait_times_out_then_late_answer() {
        let mut e = Escalations::default();
        let rx = e.subscribe("decision:1");
        assert_eq!(wait(rx, Duration::from_millis(10)).await, None);
        e.answer("decision:1", "ship it");
        let rx = e.subscribe("decision:1");
        assert_eq!(
            wait(rx, Duration::from_millis(10)).await,
            Some("ship it".into())
        );
    }

    #[tokio::test]
    async fn answer_resolves_every_waiter() {
        let mut e = Escalations::default();
        let a = e.subscribe("decision:2");
        let b = e.subscribe("decision:2");
        e.answer("decision:2", "no");
        assert_eq!(a.await.unwrap(), "no");
        assert_eq!(b.await.unwrap(), "no");
    }

    #[tokio::test]
    async fn waiting_only_while_a_receiver_is_live() {
        let mut e = Escalations::default();
        assert!(!e.waiting("decision:3"));
        let rx = e.subscribe("decision:3");
        assert!(e.waiting("decision:3"));
        assert_eq!(wait(rx, Duration::from_millis(1)).await, None);
        assert!(!e.waiting("decision:3"));
        let rx = e.subscribe("decision:3");
        e.answer("decision:3", crate::LEFT_ANSWER);
        assert!(!e.waiting("decision:3"));
        assert_eq!(rx.await.unwrap(), crate::LEFT_ANSWER);
    }
}
