//! W5 WAVES — parallel mode (design doc 140 §10): N candidate worktrees from
//! the incumbent, distinct categories per slot, children mapped through the
//! parent session + slot, the bench queue, promotion of the best keep-candidate
//! and cherry-picks of disjoint winners each re-confirmed, candidate worktrees
//! removed. Each slot is an experiment with `wave` and `slot` set.

use crate::Studies;
use crate::error::{Error, Result};
use crate::types::Slot;

/// Open a wave: create the candidate worktrees and allocate slots (140 §10 step 1).
pub async fn open(_a: &mut Studies, _slug: &str, _wave: u32) -> Result<Vec<Slot>> {
    Err(Error::NotBuilt("W5"))
}

/// Resolve `(session, slot)` to the slot's worktree and experiment number (140 §10 step 2).
pub async fn resolve(_a: &Studies, _session: runtime::SessionId, _slot: u8) -> Result<(std::path::PathBuf, u32)> {
    Err(Error::NotBuilt("W5"))
}

/// Promote after every slot logged (140 §10 step 4); removes the candidate worktrees.
pub async fn promote(_a: &mut Studies, _slug: &str, _wave: u32) -> Result<()> {
    Err(Error::NotBuilt("W5"))
}
