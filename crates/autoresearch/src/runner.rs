//! W1 MEASURE — the bench runner (design doc 140 §5.1, §5.5, §8.5): owns the
//! benchmark process (never `shell()`), `/bin/bash -lc` in the worktree with
//! the `AUTORESEARCH_*` environment, its own process group, kill at
//! `bench.timeout`, output streamed through [`crate::scanner`] into
//! `runs/<n>/sample-<k>.log`; takes the bench slot and waits for the load
//! precondition; marks the sample `lost` when the engine dies.

use std::path::Path;

use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::types::{Sample, SampleRole, StudySpec};

/// What one sample needs besides the spec.
#[derive(Clone, Debug)]
pub struct SampleRequest<'a> {
    /// The study.
    pub slug: &'a str,
    /// Experiment number (0 for baseline/holdout outside an experiment).
    pub n: u32,
    /// Sample index.
    pub k: u32,
    /// The tree measured.
    pub tree_sha: &'a str,
    /// Role.
    pub role: SampleRole,
    /// cwd of the command.
    pub worktree: &'a Path,
    /// `evaluator/<segment>/` (custody copies; `AUTORESEARCH_EVALUATOR`).
    pub evaluator: &'a Path,
    /// `<root>/<slug>/runs/<n>/` for the log.
    pub runs_dir: &'a Path,
}

/// Run one scored sample of `spec.bench` (140 §5.1): bench slot + load wait
/// (§5.5), stream and scan, parse `METRIC`/`CASE`/`ASI`, exit 0 with the
/// primary present = a sample, anything else = a crash sample (`value: None`,
/// tail in the log). `cancel` kills the process group.
pub async fn sample(
    _spec: &StudySpec,
    _cfg: &crate::config::Bench,
    _req: SampleRequest<'_>,
    _cancel: &CancellationToken,
) -> Result<Sample> {
    Err(Error::NotBuilt("W1"))
}

/// Run `command` (checks, smoke, warm, holdout) with the same environment and
/// its own timeout; returns `(exit, tail, wall_ms)` (140 §5.1 Checks / Smoke tier).
pub async fn command(
    _command: &str,
    _timeout: std::time::Duration,
    _req: SampleRequest<'_>,
    _cancel: &CancellationToken,
) -> Result<(Option<i32>, String, u64)> {
    Err(Error::NotBuilt("W1"))
}

/// Last 30 lines / 4 KiB of `log` for the model (140 §5.1).
pub fn tail(_log: &Path) -> String {
    // stub: W1
    String::new()
}
