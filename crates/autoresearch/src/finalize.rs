//! W2 RATCHET — finalize (design doc 140 §11): interleaved base/final
//! confirmation (+ holdout), changesets by union-find over touched paths
//! each cherry-picked alone onto the base and measured, the report
//! document, the loud Deliver and the attention row, the knowledge record.
//! Each step is an `events` row so a restart resumes the step.

use crate::error::{Error, Result};
use crate::store::{Store, StudyRecord};
use crate::types::{Changeset, StopReason};

/// The headline of a finished study (140 §11.1).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Headline {
    /// `time_ms 52.1 → 41.2 (−20.9 %, CI ±0.8 %)`.
    pub text: String,
    /// Base mean after confirmation.
    pub base_mean: f64,
    /// Final mean after confirmation.
    pub final_mean: f64,
    /// Δ %.
    pub delta_pct: f64,
    /// Holdout line, when configured.
    pub holdout: Option<String>,
}

/// Run every step of §11 for `study`, stopped for `reason`; returns the
/// headline and the report document id. Idempotent per step.
pub async fn run(_store: &Store, _study: &StudyRecord, _reason: StopReason, _rt: &runtime::Runtime) -> Result<(Headline, String)> {
    Err(Error::NotBuilt("W2"))
}

/// Group kept commits into changesets that share no files (140 §11.2), unmeasured.
pub fn group(_kept: &[(u32, String, Vec<String>)]) -> Vec<Changeset> {
    // stub: W2
    Vec::new()
}
