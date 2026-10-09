//! `roster:` config section (67 §6, §11).

use serde::{Deserialize, Serialize};

/// `roster:` config section. Every key has a default; an absent section is the
/// defaults.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RosterConfig {
    /// `escalate` waits this long for a live answer before parking the task (67 §11).
    pub decision_wait_secs: u64,
    /// A desk idle this long closes its episode (67 §6.2).
    pub episode_idle_secs: u64,
    /// A desk at this percent of its context window closes its episode (67 §6.2).
    pub episode_context_pct: u8,
    /// Ceiling on `roster/journal/<name>.md` (67 §6.2).
    pub journal_max_bytes: u64,
    /// Follow-up rounds (child sessions) a desk may spend on one task per
    /// episode before it must write the deliverable and dispose (70 DESK-PROTOCOL 2).
    pub max_rounds: u32,
    /// 148 E3: the model the built-in `theuth` agent's desk and its work run
    /// on; `None` = the default for new sessions. Written back by
    /// [`crate::SetTheuthModel`]; the roster keeps the live value.
    pub theuth_model: Option<String>,
    /// 155 §7.4: an attention item's `alert` becomes visible only after this
    /// many ms, if the item is still there (herdr's re-checking delay).
    pub alert_delay_ms: u64,
}

impl Default for RosterConfig {
    fn default() -> Self {
        Self {
            decision_wait_secs: 120,
            episode_idle_secs: 21_600,
            episode_context_pct: 60,
            journal_max_bytes: 8192,
            max_rounds: 3,
            theuth_model: None,
            alert_delay_ms: 2000,
        }
    }
}
