//! W6 — the reviewer and the advisor (design doc 140 §9): one model call per
//! keep with the fixed §9.1 rubric (diff ≤ 40 KiB, summarised by file beyond),
//! `CLEAN:` / `SUSPICIOUS:` parsed from exactly one line, one retry, fail
//! open as `Error`; the advisor consult at the first plateau (§9.2) returns
//! five ideas with categories.

use crate::error::{Error, Result};
use crate::store::{Store, StudyRecord};
use crate::types::{IdeaDraft, ReviewVerdict};

/// The §9.1 rubric with the placeholders filled.
pub fn prompt(_study: &StudyRecord, _n: u32, _idea: &str, _description: &str, _justification: Option<&str>, _line: &str, _deviations: &[String], _diff: &str) -> String {
    // stub: W6
    String::new()
}

/// Parse the reviewer's reply: `CLEAN: …` → `Clean`, `SUSPICIOUS: …` → `Flagged` (the
/// caller turns it into `Reverted` per `on_suspicious`); anything else `None`.
pub fn parse(_reply: &str) -> Option<ReviewVerdict> {
    // stub: W6
    None
}

/// Review one keep (140 §9.1): the model call charged to the study session, retry once, fail open.
pub async fn review(_store: &Store, _study: &StudyRecord, _n: u32, _model: &str, _rt: &runtime::Runtime) -> Result<ReviewVerdict> {
    Err(Error::NotBuilt("W6"))
}

/// The plateau consult (140 §9.2): five materially different ideas.
pub async fn advise(_store: &Store, _study: &StudyRecord, _model: Option<&str>, _rt: &runtime::Runtime) -> Result<Vec<IdeaDraft>> {
    Err(Error::NotBuilt("W6"))
}
