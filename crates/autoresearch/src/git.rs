//! W2 RATCHET — the git writer (design doc 140 §6): the `git` CLI with
//! `GIT_TERMINAL_PROMPT=0`, a fixed author, `-c core.hooksPath=/dev/null`,
//! only ever inside the study worktree. Worktree add/remove, base resolution
//! and the clean-tree check, keep (add + commit with trailers, evaluator
//! paths excluded), discard (save the patch, `reset --hard`, `clean -fd`),
//! drift repair, rejected refs, local branches. Never push, rebase or force.

use std::path::Path;

use crate::error::{Error, Result};
use crate::types::StudySpec;

/// Resolve `base` to a sha in `repo`; refuse a dirty tree unless `include_dirty` (140 §6 Base and branch).
pub fn resolve_base(_repo: &Path, _base: &str, _include_dirty: bool) -> Result<String> {
    Err(Error::NotBuilt("W2"))
}

/// `git worktree add -b autoresearch/<slug> <worktree> <base_sha>`; returns the worktree's HEAD.
pub fn worktree_add(
    _repo: &Path,
    _spec: &StudySpec,
    _worktree: &Path,
    _base_sha: &str,
) -> Result<String> {
    Err(Error::NotBuilt("W2"))
}

/// `git worktree remove --force` plus `git worktree prune` (finished studies, wave candidates, finalize scratch).
pub fn worktree_remove(_repo: &Path, _worktree: &Path) -> Result<()> {
    Err(Error::NotBuilt("W2"))
}

/// `HEAD` of a worktree.
pub fn head(_worktree: &Path) -> Result<String> {
    Err(Error::NotBuilt("W2"))
}

/// Current branch name of a worktree (`None` when detached).
pub fn branch(_worktree: &Path) -> Result<Option<String>> {
    Err(Error::NotBuilt("W2"))
}

/// The commit message trailers of a keep (140 §6 Keep).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeepMeta {
    /// `<slug>`.
    pub slug: String,
    /// `<n>`.
    pub n: u32,
    /// `autoresearch(<slug>) #<n>: <description>`.
    pub description: String,
    /// `time_ms 42.3 -> 41.2 (-2.6%, 3.1 sigma, n=3)`.
    pub metric_line: String,
    /// `Autoresearch-Category`.
    pub category: String,
    /// `Autoresearch-Deviations`, when any.
    pub deviations: Vec<String>,
}

/// `git add -A -- . ':!<evaluator_paths>'` + commit with trailers; returns the new sha.
pub fn keep(
    _worktree: &Path,
    _spec: &StudySpec,
    _meta: &KeepMeta,
    _author: Option<&str>,
) -> Result<String> {
    Err(Error::NotBuilt("W2"))
}

/// Save `git diff --binary <incumbent>` plus untracked files to `patch_path`,
/// then `reset --hard <incumbent>` and `clean -fd` (ignored files survive). Returns the patch's sha256.
pub fn discard(_worktree: &Path, _incumbent: &str, _patch_path: &Path) -> Result<String> {
    Err(Error::NotBuilt("W2"))
}

/// Drift repair at `experiment_begin` (140 §6): HEAD == incumbent, branch ==
/// `autoresearch/<slug>`, clean tree; otherwise save `stray.patch`, reset,
/// and return what was wrong (for the incident row and the brief).
pub fn repair(
    _worktree: &Path,
    _spec: &StudySpec,
    _incumbent: &str,
    _stray_patch: &Path,
) -> Result<Option<String>> {
    Err(Error::NotBuilt("W2"))
}

/// Revert a reviewed keep: incumbent ← previous, the commit kept reachable as
/// `refs/autoresearch/<slug>/rejected/<n>` (140 §9.1).
pub fn reject(_worktree: &Path, _slug: &str, _n: u32, _previous: &str) -> Result<()> {
    Err(Error::NotBuilt("W2"))
}

/// Normalised patch hash for the novelty check (140 §7.3): `git diff <incumbent>` with
/// whitespace-only changes and index lines removed, sha256.
pub fn patch_hash(_worktree: &Path, _incumbent: &str) -> Result<String> {
    Err(Error::NotBuilt("W2"))
}

/// `git diff <a> <b>` text, capped at `max_bytes` (the reviewer's and the detail view's diff).
pub fn diff_text(_worktree: &Path, _a: &str, _b: &str, _max_bytes: usize) -> Result<String> {
    Err(Error::NotBuilt("W2"))
}

/// Files touched by commit `sha`.
pub fn files_of(_worktree: &Path, _sha: &str) -> Result<Vec<String>> {
    Err(Error::NotBuilt("W2"))
}

/// `git cherry-pick <sha>` in a scratch worktree; `Ok(false)` = conflict (aborted).
pub fn cherry_pick(_worktree: &Path, _sha: &str) -> Result<bool> {
    Err(Error::NotBuilt("W2"))
}

/// Create or move a local branch at `sha` (finalize's per-changeset branches). Never pushed.
pub fn branch_at(_repo: &Path, _name: &str, _sha: &str) -> Result<()> {
    Err(Error::NotBuilt("W2"))
}

/// Pure-jj repo without git colocation → refused at draft time (140 §6).
pub fn is_git_repo(_repo: &Path) -> bool {
    // stub: W2
    false
}
