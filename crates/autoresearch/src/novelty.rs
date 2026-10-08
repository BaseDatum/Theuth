//! W6 — the novelty check at `experiment_run` (design doc 140 §7.3): a
//! normalised patch hash equal to a past discarded experiment's → refused
//! (no sample spent); FTS over past `idea + description` ≥ `novelty.threshold`
//! in the same category → `similar` rows, refused only when flagged twice in
//! a row; `force: true` + `reason` overrides. Also marks matching ideas `tried`.

use crate::error::{Error, Result};
use crate::store::Store;
use crate::types::{RunArgs, SimilarExperiment};

/// What the check found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Novelty {
    /// Refusal text, when the run must not spend a sample.
    pub refused: Option<String>,
    /// Similar past experiments (named in the result either way).
    pub similar: Vec<SimilarExperiment>,
}

/// Check `args` + `patch_hash` against the study's past (140 §7.3 Novelty check).
pub async fn check(_store: &Store, _slug: &str, _args: &RunArgs, _patch_hash: &str, _threshold: f64, _flagged_before: bool) -> Result<Novelty> {
    Err(Error::NotBuilt("W6"))
}

/// Mark backlog ideas whose text matches `idea` (FTS) as `tried` in experiment `n`.
pub async fn mark_tried(_store: &Store, _slug: &str, _idea: &str, _n: u32) -> Result<Vec<i64>> {
    Err(Error::NotBuilt("W6"))
}
