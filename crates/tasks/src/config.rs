//! `tasks:` config section (67 §7).

use serde::{Deserialize, Serialize};

/// `tasks:` config section. Every key has a default; an absent section is the
/// defaults.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct TasksConfig {
    /// A checkout whose engine has been unreachable this long is orphaned and the owner re-woken (67 §7.5).
    pub orphan_secs: u64,
    /// A `doing` task with no progress note for this long is stale on the board.
    pub stale_secs: u64,
    /// Work sessions write a progress note to the thread at least this often (67 §7.5).
    pub note_secs: u64,
    /// Tasks a hired agent's session may change outside its reporting line (67 §7.3, 99); theuth and the user are uncapped.
    pub cross_task_write_cap: u32,
    /// The cap counts writes within this window; `0` = for the session's life (99).
    pub cross_task_write_window_secs: u64,
    /// No-progress back-off start (67 §7.3).
    pub backoff_min_secs: u64,
    /// No-progress back-off ceiling (67 §7.3).
    pub backoff_max_secs: u64,
    /// Engine names to prefer, in order, when placement policy leaves several candidates (67 §8).
    pub place_prefer: Vec<String>,
}

impl Default for TasksConfig {
    fn default() -> Self {
        Self {
            orphan_secs: 120,
            stale_secs: 900,
            note_secs: 300,
            cross_task_write_cap: 20,
            cross_task_write_window_secs: 86_400,
            backoff_min_secs: 120,
            backoff_max_secs: 1800,
            place_prefer: Vec::new(),
        }
    }
}
