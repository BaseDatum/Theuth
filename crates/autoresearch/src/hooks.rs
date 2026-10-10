//! 159 W5 — before/after hooks (D5): custody evaluator files the plugin runs through
//! `Backend::plugin_command`; stdout (≤ 8 KiB) reaches the next brief as fenced data.
//! Owner: W5. Signatures frozen by C. Both return `Ok(None)` when `spec.hooks` is `None`, which is
//! every 140 study, so the registration lines in actor.rs are inert today.

use crate::actor::Env;
use crate::error::Result;
use crate::store::{ExperimentRecord, StudyRecord};

/// `hooks.before` at `experiment_begin`, in the study worktree, before the brief renders.
/// `Some(stdout)` becomes the brief section "From your hooks".
pub async fn before(_env: &Env, study: &StudyRecord, _n: u32) -> Result<Option<String>> {
    if study.spec.hooks.is_none() {
        return Ok(None);
    }
    Ok(None)
}

/// `hooks.after` when an experiment closes, with `AUTORESEARCH_VERDICT` and
/// `AUTORESEARCH_DELTA_PCT` set. `Some(stdout)` goes into the next brief.
pub async fn after(
    _env: &Env,
    study: &StudyRecord,
    _exp: &ExperimentRecord,
) -> Result<Option<String>> {
    if study.spec.hooks.is_none() {
        return Ok(None);
    }
    Ok(None)
}
