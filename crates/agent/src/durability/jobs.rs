//! Design doc 109 §5.4: the job reaper at start. **W2 fills this file.**
//! Runs once after `rearm_reminders` and before `resume_scan` (§4.6 step
//! 10): every `running` `jobs` row whose pgid is still alive is killed,
//! every `running` row is settled `lost`, and `<job_result exit="lost">` is
//! injected for jobs whose stub the model already had.

use crate::store::Store;

/// What one reaper pass did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReapReport {
    /// Process groups a SIGKILL of the previous engine leaked, killed now.
    pub killed: u32,
    /// `running` rows settled `lost`.
    pub lost: u32,
    /// Rows of sessions that no longer exist, deleted.
    pub orphaned: u32,
}

/// Stub (contract wave): settles nothing and reports an empty pass. W2
/// implements §5.4 against `Store::jobs_open` / `Store::job_settle`.
pub async fn reap_jobs(_store: &Store) -> ReapReport {
    ReapReport::default()
}
