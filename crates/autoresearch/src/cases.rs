//! 159 W2 — tuning prompts: the engine-side case runner, the paired gate on val cases and the
//! case strip (159 §7, §13). Owner: W2. Signatures frozen by C; bodies W2's. `crates/evalcore`
//! holds the shared pieces (split, judge prompt, paired gate, rows) that dreaming shares.

use tokio_util::sync::CancellationToken;

use crate::actor::Env;
use crate::error::{Error, Result};
use crate::finalize::Headline;
use crate::runner::SampleRequest;
use crate::setup::SetupRow;
use crate::store::{Store, StudyRecord};
use crate::types::CaseRow;
use crate::types::{GateCfg, GateInput, GateStep, KindView, Sample, StudySpec};

/// Not reached by a 140 study.
const OWNER: &str = "W2";

/// `KindView::Cases` for the Study view.
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

/// Draft card line 1 for a prompt study.
pub fn card_line(spec: &StudySpec) -> String {
    format!("prompt study `{}` (W2)", spec.slug)
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

/// `autoresearch.cases {slug, n?}`: the case × experiment rows the heat strip shows.
pub async fn rows(_store: &Store, _slug: &str, _n: Option<u32>) -> Result<Vec<CaseRow>> {
    Err(Error::NotBuilt(OWNER))
}
