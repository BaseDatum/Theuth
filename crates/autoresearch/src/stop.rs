//! W3 LOOP — stop and pause rules (design doc 140 §8.4), evaluated after
//! every experiment closes, first match wins: user stop, experiments,
//! money, hours/deadline, plateau (explore once, then stop), overfit, crash
//! streak, custody refusals, review flags.

use crate::error::{Error, Result};
use crate::store::{Store, StudyRecord};
use crate::types::{PauseReason, StopReason};

/// What the rules said.
#[derive(Clone, Debug, PartialEq)]
pub enum Rule {
    /// Keep going.
    Continue,
    /// Next brief in explore mode + one advisor consult (first plateau).
    Explore,
    /// Pause with a reason.
    Pause(PauseReason),
    /// Finalize with a reason.
    Stop(StopReason),
}

/// Evaluate §8.4 for `study` from the ledger and `spend_usd` (the session tree's ledger sum).
pub async fn evaluate(
    _store: &Store,
    _study: &StudyRecord,
    _cfg: &crate::config::AutoresearchConfig,
    _spend_usd: f64,
) -> Result<Rule> {
    Err(Error::NotBuilt("W3"))
}

/// Hours since Start excluding paused time.
pub fn hours_running(_study: &StudyRecord, _now: chrono::DateTime<chrono::Utc>) -> f64 {
    // stub: W3
    0.0
}
