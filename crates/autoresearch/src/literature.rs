//! 159 W5 — ideas from the literature: plugin-side search through `Backend::invoke_tool`, quoted
//! and verified ideas with `citations` rows (159 §10). Owner: W5. Signatures frozen by C.
//! Literature is an opt-in block, not a kind: there is no `view`, `headline` or `step` here.

use crate::actor::Env;
use crate::error::{Error, Result};
use crate::setup::SetupRow;
use crate::store::{Store, StudyRecord};
use crate::types::StudySpec;

/// Not reached by a 140 study.
const OWNER: &str = "W5";

/// At Start: fetch each `literature.seed` URL and keep the ideas whose quote occurs in the page.
pub async fn seed(_env: &Env, _study: &StudyRecord) -> Result<u32> {
    Err(Error::NotBuilt(OWNER))
}

/// At a plateau (beside the advisor consult) and every `literature.every` keeps: queries, search,
/// extraction, verification, storage. Returns how many ideas were stored.
pub async fn consult(_env: &Env, _study: &StudyRecord) -> Result<u32> {
    Err(Error::NotBuilt(OWNER))
}

/// The `[paper] <idea> — “<quote>” (<title>, <host>)` lines for the brief's ideas block; `""` = none.
pub async fn brief_section(_store: &Store, _study: &StudyRecord) -> Result<String> {
    Err(Error::NotBuilt(OWNER))
}

/// The draft card's note: "Ideas: yours, the model's, and up to 12 literature searches …"; `""` when off.
pub fn card_note(_spec: &StudySpec) -> String {
    String::new()
}

/// Setup rows this block adds (`literature`, `hooks` under advanced).
pub const SETUP_ROWS: &[SetupRow] = &[];

/// One-line value of a block row for the draft panel.
pub fn setup_value(_spec: &StudySpec, _row: &str) -> String {
    String::new()
}

/// Set one block leaf from the panel's text.
pub fn setup_set(_spec: &mut StudySpec, leaf: &str, _v: &str) -> std::result::Result<(), String> {
    Err(format!("{leaf}: not built ({OWNER})"))
}
