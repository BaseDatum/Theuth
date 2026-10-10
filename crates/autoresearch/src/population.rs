//! 159 W4 — search, not a ratchet: cells, islands, sampled parents, elite refs (159 §9).
//! Owner: W4. Signatures frozen by C; refs come from `crate::refs` (W1).

use tokio_util::sync::CancellationToken;

use crate::actor::Env;
use crate::error::{Error, Result};
use crate::finalize::Headline;
use crate::runner::SampleRequest;
use crate::setup::SetupRow;
use crate::store::ExperimentRecord;
use crate::store::{Store, StudyRecord};
use crate::types::ArchiveRow;
use crate::types::{GateCfg, GateInput, GateStep, KindView, Sample, StudySpec};

/// Not reached by a 140 study.
const OWNER: &str = "W4";

/// `KindView::Archive` for the Study view.
pub async fn view(_store: &Store, _study: &StudyRecord) -> Result<KindView> {
    Err(Error::NotBuilt(OWNER))
}

/// The brief's per-kind section; `""` adds nothing.
pub async fn brief_section(_store: &Store, _study: &StudyRecord) -> Result<String> {
    Err(Error::NotBuilt(OWNER))
}

/// Finalize's headline for this kind; `None` = the scalar headline.
pub async fn headline(_store: &Store, _study: &StudyRecord) -> Result<Option<Headline>> {
    Err(Error::NotBuilt(OWNER))
}

/// Draft card line 1 for a population study.
pub fn card_line(spec: &StudySpec) -> String {
    format!("population study `{}` (W4)", spec.slug)
}

/// Setup rows this kind adds to the draft panel.
pub const SETUP_ROWS: &[SetupRow] = &[];

/// One-line value of a kind row for the draft panel.
pub fn setup_value(_spec: &StudySpec, _row: &str) -> String {
    String::new()
}

/// Set one kind leaf from the panel's text.
pub fn setup_set(_spec: &mut StudySpec, leaf: &str, _v: &str) -> std::result::Result<(), String> {
    Err(format!("{leaf}: not built ({OWNER})"))
}

/// Engine-side scoring for this kind (`Scoring` dispatch in actor.rs): one `Sample` per tree.
pub async fn sample(
    _env: &Env,
    _study: &StudyRecord,
    _req: SampleRequest<'_>,
    _cancel: &CancellationToken,
) -> Result<Sample> {
    Err(Error::NotBuilt(OWNER))
}

/// The kind's decision rule (`GateRule` dispatch in actor.rs).
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

/// `Search::Population`: the sampled parent elite for `slot` (weight = s(rank) / (1 + children)).
pub async fn parent_of(_env: &Env, _study: &StudyRecord, _slot: Option<u8>) -> Result<String> {
    Err(Error::NotBuilt(OWNER))
}

/// After a decision: archive acceptance (empty cell takes the child; occupied cell by the rule).
pub async fn settle(_env: &Env, _study: &StudyRecord, _exp: &ExperimentRecord) -> Result<()> {
    Err(Error::NotBuilt(OWNER))
}

/// `autoresearch.archive` rows of kind `cell`.
pub async fn archive(_store: &Store, _slug: &str) -> Result<Vec<ArchiveRow>> {
    Err(Error::NotBuilt(OWNER))
}
