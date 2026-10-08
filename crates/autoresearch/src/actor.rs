//! W3 LOOP — the `Studies` actor's handler bodies (design doc 140 §12.3).
//! [`crate::Studies`] (lib.rs) owns the struct and the `Message` impls; each
//! impl calls the function here of the same name, so this file is the whole
//! of the actor's logic: Create (worktree, custody, warm, baseline, schedule
//! file), Begin (drift repair, dangling experiment, brief), Run (novelty,
//! custody, scope, samples, gate, checks), Log (keep/discard, review, stop
//! rules), Notes, Control, Steer, IdeaOp, Status, Detail, Experiments,
//! Experiment, StartDraft. The `slot` paths are W5's (hand-off).

use crate::Studies;
use crate::error::{Error, Result};
use crate::types::*;

/// `Create` → `StudyInfo` in state `preparing` (140 §4.2 preparing).
pub async fn create(_a: &mut Studies, _m: Create) -> Result<StudyInfo> {
    Err(Error::NotBuilt("W3"))
}

/// `StartDraft` → the draft card's press (140 §12.4 `autoresearch_draft` invariant).
pub async fn start_draft(_a: &mut Studies, _m: StartDraft) -> Result<StudyInfo> {
    Err(Error::NotBuilt("W3"))
}

/// `Begin` → `Brief` (140 §4.3 open, §7).
pub async fn begin(_a: &mut Studies, _m: Begin) -> Result<Brief> {
    Err(Error::NotBuilt("W3"))
}

/// `Run` → `RunReport` (140 §4.3 measuring); blocks while measuring, cancel-safe.
pub async fn run(_a: &mut Studies, _m: Run) -> Result<RunReport> {
    Err(Error::NotBuilt("W3"))
}

/// `Log` → `LogReport` (140 §4.3 kept/discarded…, §9.1 review, §8.4 stop rules).
pub async fn log(_a: &mut Studies, _m: Log) -> Result<LogReport> {
    Err(Error::NotBuilt("W3"))
}

/// `Notes` → `NotesReport` (140 §7.3 playbook and ideas).
pub async fn notes(_a: &mut Studies, _m: Notes) -> Result<NotesReport> {
    Err(Error::NotBuilt("W3"))
}

/// `Control` → `StudyInfo` (140 §8.6).
pub async fn control(_a: &mut Studies, _m: Control) -> Result<StudyInfo> {
    Err(Error::NotBuilt("W3"))
}

/// `Steer` → the steer id (140 §7.3 Steers).
pub async fn steer(_a: &mut Studies, _m: SteerMsg) -> Result<SteerId> {
    Err(Error::NotBuilt("W3"))
}

/// `IdeaOp` → `Idea` (140 §7.3 Ideas).
pub async fn idea(_a: &mut Studies, _m: IdeaOp) -> Result<Idea> {
    Err(Error::NotBuilt("W3"))
}

/// `Status` → one or every `StudyInfo` (140 §12.5 `autoresearch.list`).
pub async fn status(_a: &mut Studies, _m: Status) -> Result<Vec<StudyInfo>> {
    Err(Error::NotBuilt("W3"))
}

/// `Detail` → `StudyDetail` with the chart series (140 §12.5 `autoresearch.get`).
pub async fn detail(_a: &mut Studies, _m: Detail) -> Result<StudyDetail> {
    Err(Error::NotBuilt("W3"))
}

/// `Experiments` → rows newest first (140 §12.5).
pub async fn experiments(_a: &mut Studies, _m: Experiments) -> Result<Vec<ExperimentRow>> {
    Err(Error::NotBuilt("W3"))
}

/// `Experiment` → `ExperimentDetail` (140 §12.5).
pub async fn experiment(_a: &mut Studies, _m: Experiment) -> Result<ExperimentDetail> {
    Err(Error::NotBuilt("W3"))
}

/// `StudyInfo` from a ledger row (shared by status, control, create).
pub fn info(_a: &Studies, _study: &crate::store::StudyRecord) -> StudyInfo {
    // stub: W3
    StudyInfo::default()
}
