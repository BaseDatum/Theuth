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
    /// 194 rows 4–5: the effort theuth's desk and its work run on; `None` = the
    /// default for new sessions. Written back by [`crate::SetTheuthEffort`] (and
    /// cleared or dropped by [`crate::SetTheuthModel`]); the roster keeps the live value.
    pub theuth_effort: Option<String>,
    /// 155 §7.4: an attention item's `alert` becomes visible only after this
    /// many ms, if the item is still there (herdr's re-checking delay).
    pub alert_delay_ms: u64,
    /// 207: `roster.attention` — the flat list's score weights and timers.
    pub attention: AttentionConfig,
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
            theuth_effort: None,
            alert_delay_ms: 2000,
            attention: AttentionConfig::default(),
        }
    }
}

/// 207: `roster.attention:` — read on every recheck (boot snapshot: a change needs an engine restart).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AttentionConfig {
    /// A `finished` item leaves the list this long after the stop.
    pub stopped_ttl_mins: u64,
    /// The coarse re-score / expiry tick.
    pub rescore_secs: u64,
    /// Score weights.
    pub weights: ScoreWeights,
}

impl Default for AttentionConfig {
    fn default() -> Self {
        Self {
            stopped_ttl_mins: 45,
            rescore_secs: 60,
            weights: ScoreWeights::default(),
        }
    }
}

/// 207 §3: the score weights (`roster.attention.weights`); every key has a default.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScoreWeights {
    /// Base: the item stops an agent (approval, ask, secret, hand-off, decision, resume, first contact, a study launch).
    pub stops_agent: u32,
    /// Base: mesh notices (node, home).
    pub mesh: u32,
    /// Base: yours to do (review, hire, blocked, a paused budget).
    pub yours: u32,
    /// Base: an artifact shown to you.
    pub shown: u32,
    /// Base: notices (a budget at 80 %, the restart notice).
    pub notice: u32,
    /// Base: a session stopped failed or timed out.
    pub stopped_failed: u32,
    /// Base: a session stopped otherwise.
    pub stopped: u32,
    /// Base: answered by your double.
    pub double: u32,
    /// The item's session is live.
    pub live: u32,
    /// Per session stopped behind the item.
    pub blocks_each: u32,
    /// Cap of the blocks points.
    pub blocks_max: u32,
    /// Deadline points when due (0 above half the span left).
    pub deadline_max: u32,
    /// A destructive approval.
    pub destructive: u32,
    /// A mutating approval.
    pub mutating: u32,
    /// Per task priority weight.
    pub task_per_weight: u32,
    /// The task's project is in `tasks.project_order`.
    pub task_ranked_project: u32,
    /// Cap of the task points.
    pub task_max: u32,
    /// Per minute waited.
    pub age_per_min: u32,
    /// Cap of the age points.
    pub age_max: u32,
}

impl Default for ScoreWeights {
    fn default() -> Self {
        Self {
            stops_agent: 600,
            mesh: 450,
            yours: 300,
            shown: 120,
            notice: 120,
            stopped_failed: 150,
            stopped: 80,
            double: 20,
            live: 300,
            blocks_each: 40,
            blocks_max: 200,
            deadline_max: 250,
            destructive: 100,
            mutating: 30,
            task_per_weight: 10,
            task_ranked_project: 10,
            task_max: 100,
            age_per_min: 1,
            age_max: 60,
        }
    }
}
