//! 159 W7 — the mechanism as data (D13): `protocol.md`, `fire.md`, `advisor.md` and
//! `mechanism.yaml` in `<root>/mechanism/`, validated against a compiled schema; the gate,
//! scoring, custody, budgets, toolset and skill are not in it (159 §12). Owner: W7.
//! The three `*_prompt` readers return `None` until W7, so every compiled text stays in force.

use std::path::Path;

use crate::error::{Error, Result};
use crate::types::MechanismInfo;

/// Not reached by a 140 study.
const OWNER: &str = "W7";

/// Load the mechanism pinned at `sha` (or the active one when `None`), validated. A missing or
/// invalid file falls back to the compiled default with an incident line.
pub fn load(_root: &Path, _sha: Option<&str>) -> Result<Mechanism> {
    Err(Error::NotBuilt(OWNER))
}

/// The protocol text for the brief; `None` = the compiled `brief::protocol`.
pub fn protocol(_root: &Path, _sha: Option<&str>) -> Option<String> {
    None
}

/// The fire prompt for the schedule file; `None` = the compiled text in `schedule_file::text`.
pub fn fire_prompt(_root: &Path, _sha: Option<&str>) -> Option<String> {
    None
}

/// The advisor system prompt; `None` = the compiled `review::ADVISOR_SYSTEM`.
pub fn advisor_prompt(_root: &Path, _sha: Option<&str>) -> Option<String> {
    None
}

/// `autoresearch.mechanism {op}`: list, show or adopt (a press) a version.
pub async fn rpc(_root: &Path, _op: crate::types::MechanismOp) -> Result<MechanismInfo> {
    Err(Error::NotBuilt(OWNER))
}

/// A loaded, validated mechanism (W7 fills the fields).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mechanism {
    /// The pinned git sha of `<root>/mechanism/`.
    pub sha: String,
    /// `mechanism.yaml`'s `version`.
    pub version: u32,
}
