//! W3 LOOP — the study's schedule file (design doc 140 §8.1):
//! `<data_dir>/schedules/autoresearch-<slug>.md` with `every: 0s`,
//! `max_runs`, `budget:`, `run_timeout:`, `approvals: false`, `quiet: true`,
//! `memory: clear`, `runtime: clear`, `cwd:`, `model:`, `enabled:` (and
//! `when:` + `every: 1m` with a `window:`). The plugin rewrites only
//! `enabled:` and deletes the file through `schedule::Done`.

use std::path::Path;

use crate::error::{Error, Result};
use crate::types::{StartWhen, StudySpec};

/// The file text for `spec` (140 §8.1).
pub fn text(_spec: &StudySpec, _cfg: &crate::config::AutoresearchConfig, _worktree: &Path, _model: &str, _start: &StartWhen) -> String {
    // stub: W3
    String::new()
}

/// `run_timeout` = `(crash_retries + confirm_runs + 2) × bench.timeout + checks.timeout + bench.wait + 15m`, at least 30 m.
pub fn run_timeout(_spec: &StudySpec, _cfg: &crate::config::AutoresearchConfig) -> std::time::Duration {
    // stub: W3
    std::time::Duration::from_secs(30 * 60)
}

/// Write the file (enabled) under `data_dir`.
pub fn write(_data_dir: &Path, _slug: &str, _text: &str) -> Result<()> {
    Err(Error::NotBuilt("W3"))
}

/// Flip `enabled:` through the schedule actor's `SetEnabled` (pause/resume, 140 §4.2).
pub async fn set_enabled(_slug: &str, _enabled: bool) -> Result<()> {
    Err(Error::NotBuilt("W3"))
}

/// End the loop and delete the file through `schedule::Done` (140 §8.1).
pub async fn done(_slug: &str, _reason: &str) -> Result<()> {
    Err(Error::NotBuilt("W3"))
}
