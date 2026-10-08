//! W2 RATCHET — the cross-study knowledge record (design doc 140 §7.3):
//! at finalize one `type: record` with id `records/autoresearch-<repo>-<slug>-<date>`
//! (goal, metric, base → final, kept changesets with deltas, top dead ends,
//! playbook); at setup/Start the most recent record for the same repo seeds
//! the first playbook.

use crate::error::{Error, Result};
use crate::store::{Store, StudyRecord};

/// Write the record for a finished study; returns its knowledge id.
pub async fn write(
    _store: &Store,
    _study: &StudyRecord,
    _headline: &str,
    _rt: &runtime::Runtime,
) -> Result<String> {
    Err(Error::NotBuilt("W2"))
}

/// The playbook of the most recent record for `repo`, if any.
pub async fn seed_playbook(_repo: &str, _rt: &runtime::Runtime) -> Result<Option<String>> {
    Err(Error::NotBuilt("W2"))
}
