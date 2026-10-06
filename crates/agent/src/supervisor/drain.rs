//! Design doc 109 §9.2: graceful shutdown. **W5 fills this file.** `main.rs`
//! asks `Drain { deadline }` on `rt.shutdown`; in-flight tool calls get until
//! the deadline, open turns that cannot finish get a `suspended` step and a
//! `TurnSuspended` event, then jobs are journaled `lost` and `KillAll` runs.

use std::time::Instant;

use kameo::message::{Context, Message};

use super::Agents;

/// `ask` → [`DrainReport`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Drain {
    /// When to stop waiting and suspend what is still running.
    pub deadline: Instant,
}

/// What the drain did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, kameo::Reply)]
pub struct DrainReport {
    /// Open turns that got a `suspended` step.
    pub suspended: u32,
    /// Turns that finished inside the window.
    pub finished: u32,
}

impl Message<Drain> for Agents {
    type Reply = DrainReport;

    /// Stub (contract wave): waits for nothing and suspends nothing.
    async fn handle(&mut self, m: Drain, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        tracing::warn!(
            remaining_ms = m
                .deadline
                .saturating_duration_since(Instant::now())
                .as_millis(),
            "109: Drain not implemented"
        );
        DrainReport::default()
    }
}
