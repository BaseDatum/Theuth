//! Replay rows (159 §13.1, §13.5): keyed by case and candidate hash; autoresearch implements
//! [`Rows`] over its `judgments` table, dreaming over `replays.jsonl`. Owner: W2.

use serde::{Deserialize, Serialize};

use crate::case::Split;

/// One replay: a case answered by a candidate and judged.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplayRow {
    /// Case id.
    pub case_id: String,
    /// Written at log time; old dreaming rows lack it and the reader derives it from the id.
    #[serde(default)]
    pub split: Option<Split>,
    /// [`crate::candidate_hash`] of the artefact.
    pub candidate_hash: String,
    /// 0–1.
    pub score: f64,
    /// `dream` or `study:<slug>`; old rows default to `dream`.
    #[serde(default)]
    pub owner: String,
    /// The judge's reason.
    #[serde(default)]
    pub reason: String,
    /// RFC 3339.
    pub at: String,
}

/// Where replay rows live.
pub trait Rows {
    /// Append one row.
    fn append(&mut self, row: ReplayRow) -> Result<(), String>;
    /// The newest row for a case and candidate.
    fn latest(&self, case_id: &str, candidate_hash: &str) -> Option<ReplayRow>;
}
