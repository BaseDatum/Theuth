//! Wire types (126 §13): what the `double_*` RPC methods take and return, and
//! the enums the store, the events and the trainer share. Every type here is
//! `ts-rs` exported under the `ts` feature so `just ts` writes the desktop's
//! bindings.

use serde::{Deserialize, Serialize};

pub use crate::class::CardKind;

/// A training tier (§0 item 6); also `versions.tier` / `runs.tier`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Temperatures, k-NN index, logistic heads; in-process, deterministic.
    A,
    /// LoRA adapter on the encoder; data-triggered, gate-promoted.
    B,
    /// Full fine-tune; owner action only.
    C,
    /// The frozen zero-shot oracle.
    Baseline,
}

impl Tier {
    /// The column spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::A => "a",
            Self::B => "b",
            Self::C => "c",
            Self::Baseline => "baseline",
        }
    }

    /// Inverse of [`Self::as_str`].
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "a" => Self::A,
            "b" => Self::B,
            "c" => Self::C,
            "baseline" => Self::Baseline,
            _ => return None,
        })
    }
}

/// A training run's state (§8.1); also `runs.state`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// No run; the trigger has not fired.
    Idle,
    /// Trigger met, preconditions not yet checked.
    Eligible,
    /// Preconditions failed; re-checked every 60 s.
    Waiting,
    /// Taking the venue and the build slot.
    Acquiring,
    /// Training steps are executing.
    Running,
    /// A precondition broke mid-run; checkpointed at a step boundary.
    Paused,
    /// Candidate re-scored against the gate.
    Evaluating,
    /// Gate passed; version activated.
    Promoted,
    /// Gate failed; version kept inactive with its metrics.
    Rejected,
    /// A node deleted its bundle and checkpoints (§8.4).
    Cleaned,
}

impl RunState {
    /// The column spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Eligible => "eligible",
            Self::Waiting => "waiting",
            Self::Acquiring => "acquiring",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Evaluating => "evaluating",
            Self::Promoted => "promoted",
            Self::Rejected => "rejected",
            Self::Cleaned => "cleaned",
        }
    }

    /// Inverse of [`Self::as_str`].
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "idle" => Self::Idle,
            "eligible" => Self::Eligible,
            "waiting" => Self::Waiting,
            "acquiring" => Self::Acquiring,
            "running" => Self::Running,
            "paused" => Self::Paused,
            "evaluating" => Self::Evaluating,
            "promoted" => Self::Promoted,
            "rejected" => Self::Rejected,
            "cleaned" => Self::Cleaned,
            _ => return None,
        })
    }
}

/// Where a label came from (§6); also `labels.source`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum LabelSource {
    /// The owner's answer, weight 1.0.
    Direct,
    /// A rewind of the turn, weight 0.5.
    HindsightRewind,
    /// The next message judged a correction, 0.5 × P(true).
    HindsightCorrection,
    /// The turn was cancelled within 30 s, weight 0.25.
    HindsightInterrupt,
    /// The owner took over or reversed an L2/L3 answer, weight 3.0.
    Override,
    /// Nobody answered; the card's default, weight 0.25.
    Timeout,
    /// "don't learn from this one": weight 0, keeps the example count honest.
    Excluded,
}

impl LabelSource {
    /// The column spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::HindsightRewind => "hindsight_rewind",
            Self::HindsightCorrection => "hindsight_correction",
            Self::HindsightInterrupt => "hindsight_interrupt",
            Self::Override => "override",
            Self::Timeout => "timeout",
            Self::Excluded => "excluded",
        }
    }

    /// Inverse of [`Self::as_str`].
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "direct" => Self::Direct,
            "hindsight_rewind" => Self::HindsightRewind,
            "hindsight_correction" => Self::HindsightCorrection,
            "hindsight_interrupt" => Self::HindsightInterrupt,
            "override" => Self::Override,
            "timeout" => Self::Timeout,
            "excluded" => Self::Excluded,
            _ => return None,
        })
    }
}

/// How a prediction scored once the label arrived; also `predictions.outcome`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// `choice` equals the direct / override label.
    Correct,
    /// It does not.
    Wrong,
    /// No label yet.
    Unlabelled,
}

impl Outcome {
    /// The column spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Correct => "correct",
            Self::Wrong => "wrong",
            Self::Unlabelled => "unlabelled",
        }
    }

    /// Inverse of [`Self::as_str`].
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "correct" => Self::Correct,
            "wrong" => Self::Wrong,
            "unlabelled" => Self::Unlabelled,
            _ => return None,
        })
    }
}

/// One training run's id (`runs.id`), the reply of `double_train`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(transparent)]
pub struct RunId(pub String);

/// The §7.4 numbers for one version over one window, overall or per class.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Kpis {
    /// Labelled prediction rows the numbers are over.
    pub n: u32,
    /// Prequential accuracy, 0..=1.
    pub accuracy: f32,
    /// Mean squared error of `probs` against the one-hot label.
    pub brier: f32,
    /// Expected calibration error over 10 bins of `p_top`.
    pub ece: f32,
    /// Share of decisions answerable at ≤ 2 % error with the current gate.
    pub coverage_2pct: f32,
    /// L2/L3 answers (`acted = 1`).
    pub asks_avoided: u32,
    /// Σ median_answer_ms over acted rows + 30 s interruption cost each.
    pub minutes_saved: f32,
    /// Overrides per 100 acted rows.
    pub overrides_per_100: f32,
    /// Undos during grace per 100 acted rows.
    pub undo_rate_per_100: f32,
}

/// `double_status` (§13).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DoubleStatus {
    /// `double.enabled`; `false` = the plugin registered nothing.
    pub enabled: bool,
    /// Learning paused: predictions and rungs continue, labels stop accruing.
    pub paused: bool,
    /// `versions.id` of the active version; `None` before the first Tier A run (= baseline only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub active_version: Option<String>,
    /// The most recent `runs` row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub last_run: Option<DoubleRun>,
    /// RFC 3339 start of the next Tier B window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub next_window: Option<String>,
    /// The resolved venue: `local` | `node:<name>` | `off`.
    pub venue: String,
    /// Sessions with approvals off (§6.6): double learns only from asks there.
    pub approvals_off_sessions: u32,
    /// Overall, active version, last 30 days.
    pub kpis: Kpis,
    /// The same rows scored by the frozen zero-shot baseline.
    pub baseline: Kpis,
}

/// One `runs` row (§8.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DoubleRun {
    /// `runs.id`.
    pub id: RunId,
    /// Which tier.
    pub tier: Tier,
    /// `local` | `node:<name>`.
    pub venue: String,
    /// Where in the state machine.
    pub state: RunState,
    /// RFC 3339.
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    /// RFC 3339, when finished.
    pub ended_at: Option<String>,
    /// `promoted` / `rejected: <gate>` / an error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub result: Option<String>,
}

/// One row of `double_classes` (§13): the `classes` table plus its last-30-days numbers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DoubleClass {
    /// The §3 key.
    pub key: String,
    /// The key's kind prefix.
    pub kind: CardKind,
    /// Examples seen.
    pub n: u32,
    /// Current rung, 0..=3.
    pub rung: u8,
    /// Max rung for this class (§9 caps).
    pub cap: u8,
    /// Why the cap holds, when it is below the ladder's next rung (`mutating — opt in to allow L2`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub cap_reason: Option<String>,
    /// Per-class opt-in for L2 on a mutating class.
    pub mutating_opt_in: bool,
    /// Owner's median time-to-answer for this class.
    pub median_answer_ms: u32,
    /// Last 30 days, active version.
    pub kpis: Kpis,
    /// Last 30 days, baseline.
    pub baseline: Kpis,
    /// RFC 3339 of the newest example.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub last_decision_at: Option<String>,
}

/// One point of the learning curve (§7.4): a week, current vs baseline.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CurvePoint {
    /// RFC 3339 date of the week's Monday.
    pub week: String,
    /// The active version.
    pub current: Kpis,
    /// The frozen zero-shot oracle on the same rows.
    pub baseline: Kpis,
}

/// One scrubbed example as the UI shows it (`double_class`, the popover).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DoubleExample {
    /// `examples.id`.
    pub id: String,
    /// The card id as text.
    pub question_id: String,
    /// RFC 3339.
    pub asked_at: String,
    /// ≤ 160 chars of the scrubbed card text.
    pub card_excerpt: String,
    /// Option labels in card order.
    pub options: Vec<String>,
    /// What the owner chose (the direct / override label), when labelled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub chose: Option<String>,
    /// What the active version predicted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub predicted: Option<String>,
    /// How the active version's prediction scored.
    pub outcome: Outcome,
}

/// `double_class(key)` (§13).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DoubleClassDetail {
    /// The class row.
    pub class: DoubleClass,
    /// Weekly points, oldest first.
    pub curve: Vec<CurvePoint>,
    /// The last 50, newest first, scrubbed.
    pub examples: Vec<DoubleExample>,
    /// What one more correct answer would do ("12 more correct in a row").
    pub promote_hint: String,
}

/// `double_forget(target)` (§13).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case", tag = "target")]
pub enum ForgetTarget {
    /// Every example, label and prediction of one class; the class row stays at L0.
    Class {
        /// The §3 key.
        key: String,
    },
    /// Specific examples by id.
    Examples {
        /// `examples.id`s.
        ids: Vec<String>,
    },
}

/// `double_train(tier, venue)`: where to run (§8.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case", tag = "venue", content = "name")]
pub enum Venue {
    /// The configured chooser.
    Auto,
    /// This engine.
    Local,
    /// A named mesh engine.
    Node(String),
    /// Never train.
    Off,
}

impl Venue {
    /// The config spelling: `auto` | `local` | `node:<name>` | `off`.
    pub fn as_config(&self) -> String {
        match self {
            Self::Auto => "auto".into(),
            Self::Local => "local".into(),
            Self::Node(n) => format!("node:{n}"),
            Self::Off => "off".into(),
        }
    }

    /// Inverse of [`Self::as_config`]; anything else is `None`.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "auto" => Self::Auto,
            "local" => Self::Local,
            "off" => Self::Off,
            _ => Self::Node(s.strip_prefix("node:")?.to_owned()),
        })
    }
}

/// `double_export()` (§13): the archive written under `<data_dir>/double/export/`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DoubleExport {
    /// Absolute path of the scrubbed JSONL + embeddings archive.
    pub path: String,
    /// Rows written.
    pub examples: u32,
    /// Archive size.
    pub bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn venue_round_trips() {
        for s in ["auto", "local", "off", "node:gpu-box"] {
            assert_eq!(Venue::parse(s).unwrap().as_config(), s);
        }
        assert_eq!(Venue::parse("elsewhere"), None);
    }
}
