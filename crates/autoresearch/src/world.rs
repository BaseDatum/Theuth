//! 159 W6 — the metric lives in the world: launch press, deploy/measure/rollback under custody,
//! `Waiting` across fires, mSPRT (159 §11). Owner: W6. Signatures frozen by C; `wait`, `resume`
//! and `rearm` are the waiting-and-timer API W7 consumes (159 §18.4).

use tokio_util::sync::CancellationToken;

use crate::actor::Env;
use crate::error::{Error, Result};
use crate::finalize::Headline;
use crate::runner::SampleRequest;
use crate::setup::SetupRow;
use crate::store::ExperimentRecord;
use crate::store::{Store, StudyRecord};
use crate::types::{ExperimentState, StudyInfo, WaitInfo};
use crate::types::{GateCfg, GateInput, GateStep, KindView, Sample, StudySpec};

/// Not reached by a 140 study.
const OWNER: &str = "W6";

/// `KindView::World` for the Study view.
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

/// Draft card line 1 for a real-world study.
pub fn card_line(spec: &StudySpec) -> String {
    format!("real-world study `{}` (W6)", spec.slug)
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

/// Put `exp` into `ExperimentState::Waiting` with `info` (stored in `experiments.wait`), disable
/// the study's schedule file (W-i) and arm a timer for `info.until`.
pub async fn wait(
    _env: &Env,
    _study: &StudyRecord,
    _exp: &mut ExperimentRecord,
    _info: WaitInfo,
) -> Result<()> {
    Err(Error::NotBuilt(OWNER))
}

/// The timer fired (or W7's inner studies finished): run the kind's resume step; close the
/// experiment or extend the wait; re-enable the schedule file when it closes.
pub async fn resume(_env: &Env, _slug: &str, _n: u32) -> Result<ExperimentState> {
    Err(Error::NotBuilt(OWNER))
}

/// At engine start: re-arm one timer per waiting experiment from `experiments.wait.until`.
/// Returns how many. A ledger with no waiting experiment (every 140 ledger) re-arms nothing.
pub async fn rearm(env: &Env) -> Result<u32> {
    let _ = env;
    Ok(0)
}

/// `reconcile`: does a waiting experiment explain this study's disabled schedule file?
/// `false` for every study today, so reconcile pauses as before.
pub async fn explains_disabled(_store: &Store, _slug: &str) -> Result<bool> {
    Ok(false)
}

/// `autoresearch.launch {slug, n, token, approve}`: the press. Approve deploys both arms; refuse
/// closes the experiment as `Discarded`, "refused at launch".
pub async fn launch(
    _env: &Env,
    _slug: &str,
    _n: u32,
    _token: &str,
    _approve: bool,
) -> Result<StudyInfo> {
    Err(Error::NotBuilt(OWNER))
}
