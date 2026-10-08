//! W6 — the brief (design doc 140 §7): render the §7.2 template from the
//! ledger — steers, keeps, recent rows, dead ends (FTS-deduplicated),
//! ideas, cases moved, playbook, category balance, mode line, incidents,
//! `code_map` of the scope — capped at `brief.max_bytes`, trimmed from the
//! bottom of each list, never mid-line.

use crate::error::{Error, Result};
use crate::store::{Store, StudyRecord};
use crate::types::Brief;

/// Everything the renderer needs that is not in the ledger.
#[derive(Clone, Debug, Default)]
pub struct Context {
    /// This experiment's number.
    pub n: u32,
    /// Explore mode (plateau).
    pub explore: bool,
    /// Incident lines (drift repair, abandoned experiment, …).
    pub incidents: Vec<String>,
    /// Budget left line's numbers: `(experiments, hours, usd)`.
    pub budget_left: (u32, f64, f64),
    /// `code_map(focus=scope, budget=brief.code_map_tokens)` text, when available.
    pub code_map: Option<String>,
}

/// Render the brief for `study` at experiment `cx.n` (140 §7.2).
pub async fn render(_store: &Store, _study: &StudyRecord, _cfg: &crate::config::BriefConfig, _cx: &Context) -> Result<Brief> {
    Err(Error::NotBuilt("W6"))
}

/// Trim `text` to `max_bytes` by dropping whole lines from the end of each
/// list section, never mid-line (140 §7.1).
pub fn trim(_text: &str, _max_bytes: usize) -> String {
    // stub: W6
    String::new()
}
