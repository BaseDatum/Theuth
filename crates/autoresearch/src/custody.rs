//! W1 MEASURE — evaluator custody and post-hoc scope (design doc 140 §5.3):
//! copy the evaluator scripts and `evaluator_paths` into
//! `<root>/<slug>/evaluator/<segment>/` with sha256 per file, verify the
//! worktree's copies before every scored sample, diff the worktree against the
//! incumbent for `deviations` and `off_limits` hits.

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::{Error, Result};
use crate::types::StudySpec;

/// Copy the evaluator into `evaluator_dir` and return `{path: sha256}` (140 §5.3 Custody).
/// Commands that are not repo paths are frozen as text under `commands.json`.
pub fn take(_spec: &StudySpec, _repo: &Path, _evaluator_dir: &Path) -> Result<BTreeMap<String, String>> {
    Err(Error::NotBuilt("W1"))
}

/// Hash the worktree's copies of every custody path; `Ok(vec![])` = intact,
/// otherwise the changed paths (the refusal names them).
pub fn verify(_custody: &BTreeMap<String, String>, _worktree: &Path) -> Result<Vec<String>> {
    Err(Error::NotBuilt("W1"))
}

/// What the worktree changed vs the incumbent (140 §5.3 Scope).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScopeDiff {
    /// Every changed path.
    pub changed: Vec<String>,
    /// Paths outside `scope`.
    pub deviations: Vec<String>,
    /// Paths matching `off_limits` (a keep is impossible).
    pub off_limits: Vec<String>,
    /// `git diff --numstat` totals, whitespace-only excluded.
    pub added: u32,
    /// See `added`.
    pub removed: u32,
}

/// `git status --porcelain=v2` + `git diff --numstat` of the worktree, classified against the spec's globs.
pub fn scope_diff(_spec: &StudySpec, _worktree: &Path) -> Result<ScopeDiff> {
    Err(Error::NotBuilt("W1"))
}

/// sha256 hex of a file.
pub fn sha256_file(_path: &Path) -> Result<String> {
    Err(Error::NotBuilt("W1"))
}
