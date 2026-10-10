//! 159 W1 — several objectives: the dominance gate, the front archive and the front chart (159 §6).
//! Owner: W1. The skeleton (C) froze the signatures below and the registration lines that call
//! them (`// 159:W1` in actor.rs, brief.rs, setup.rs, finalize.rs, study_draft.rs, rpc_impl.rs,
//! StudyView.tsx); every body here is W1's. Nothing in this file runs for a 140 study.

use tokio_util::sync::CancellationToken;

use crate::actor::Env;
use crate::error::{Error, Result};
use crate::finalize::Headline;
use crate::runner::SampleRequest;
use crate::setup::SetupRow;
use crate::store::{ExperimentRecord, Store, StudyRecord};
use crate::types::{ArchiveRow, GateCfg, GateInput, GateStep, KindView, Sample, StudySpec};

/// Not reached by a 140 study.
const OWNER: &str = "W1";

/// `GateRule::Dominance` (159 §6 "Comparing two trees", "The entry rule"): one `GateInput` per
/// objective is built by W1 from `candidate` and `reference` samples; returns
/// `Decide { detail: Some(rule = dominance) }`, `Traded` when the candidate joins the front
/// without dominating its parent.
pub async fn step(
    _env: &Env,
    _study: &StudyRecord,
    _cfg: &GateCfg,
    _input: &GateInput,
    _candidate: &[Sample],
    _reference: &[Sample],
) -> Result<GateStep> {
    Err(Error::NotBuilt(OWNER))
}

/// `Search::Front`: the parent for the next experiment (the member with the fewest children,
/// ties by the headline objective); the study worktree is checked out at it before the brief.
pub async fn parent_of(_env: &Env, _study: &StudyRecord, _slot: Option<u8>) -> Result<String> {
    Err(Error::NotBuilt(OWNER))
}

/// After a decision: apply the entry rule, update `archive` rows of kind `front` and the refs.
pub async fn settle(_env: &Env, _study: &StudyRecord, _exp: &ExperimentRecord) -> Result<()> {
    Err(Error::NotBuilt(OWNER))
}

/// `KindView::Front` for the Study view.
pub async fn view(_store: &Store, _study: &StudyRecord) -> Result<KindView> {
    Err(Error::NotBuilt(OWNER))
}

/// The brief's "Front so far" section; `""` adds nothing.
pub async fn brief_section(_store: &Store, _study: &StudyRecord) -> Result<String> {
    Err(Error::NotBuilt(OWNER))
}

/// Finalize's headline: `front of 3 · time_ms −21 % · peak_kb −12 %`.
pub async fn headline(_store: &Store, _study: &StudyRecord) -> Result<Option<Headline>> {
    Err(Error::NotBuilt(OWNER))
}

/// Draft card line 1: "Find the trade-offs between `time_ms` (lower) and `size_kb` (lower) of …".
pub fn card_line(spec: &StudySpec) -> String {
    format!(
        "Find the trade-offs between {} objectives of `{}` (W1)",
        spec.objectives.len(),
        spec.bench.command.trim()
    )
}

/// `autoresearch.archive` rows of kind `front`.
pub async fn archive(_store: &Store, _slug: &str) -> Result<Vec<ArchiveRow>> {
    Err(Error::NotBuilt(OWNER))
}

/// Setup rows this kind adds to the draft panel (`objectives`).
pub const SETUP_ROWS: &[SetupRow] = &[];

/// One-line value of a kind row for the draft panel.
pub fn setup_value(_spec: &StudySpec, _row: &str) -> String {
    String::new()
}

/// Set one kind leaf from the panel's text.
pub fn setup_set(_spec: &mut StudySpec, leaf: &str, _v: &str) -> std::result::Result<(), String> {
    Err(format!("{leaf}: not built ({OWNER})"))
}

/// `Scoring` is `bench` for W1; the scoring dispatch never names this module. Kept so every
/// wave module has the same surface.
pub async fn sample(
    _env: &Env,
    _study: &StudyRecord,
    _req: SampleRequest<'_>,
    _cancel: &CancellationToken,
) -> Result<Sample> {
    Err(Error::NotBuilt(OWNER))
}
