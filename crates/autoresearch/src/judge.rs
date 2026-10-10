//! 159 W3 — judged objectives: blind pairs in both orders, the Wilson win-share bound, the
//! Bradley–Terry ladder and the human pair (159 §8). Owner: W3. Signatures frozen by C.

use tokio_util::sync::CancellationToken;

use crate::actor::Env;
use crate::error::{Error, Result};
use crate::finalize::Headline;
use crate::runner::SampleRequest;
use crate::setup::SetupRow;
use crate::store::{Store, StudyRecord};
use crate::types::{GateCfg, GateInput, GateStep, KindView, Sample, StudySpec};
use crate::types::{JudgmentRow, StudyInfo};

/// Not reached by a 140 study.
const OWNER: &str = "W3";

/// `KindView::Ratings` for the Study view.
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

/// Draft card line 1 for a judged study.
pub fn card_line(spec: &StudySpec) -> String {
    format!("judged study `{}` (W3)", spec.slug)
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

/// `autoresearch.judgments {slug, n?}`.
pub async fn judgments(_store: &Store, _slug: &str, _n: Option<u32>) -> Result<Vec<JudgmentRow>> {
    Err(Error::NotBuilt(OWNER))
}

/// `autoresearch.human_pair {slug, id, winner}`: Sky's answer (`a` | `b` | `same`) to a blind pair,
/// stored with `human = 1`; feeds the agreement figure.
pub async fn human_pair(_env: &Env, _slug: &str, _id: i64, _winner: &str) -> Result<StudyInfo> {
    Err(Error::NotBuilt(OWNER))
}
