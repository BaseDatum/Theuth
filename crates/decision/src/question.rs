//! The public API (57 §2): typed questions in, a probability per option and a
//! selective disposition out. The caller owns the candidates; the model picks an id.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// One option of a [`Question::Choice`]: `id` is opaque to the model and comes back
/// verbatim in [`Decision::choice`]; `text` is what the model reads (≤ 48 tokens).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Candidate {
    /// Opaque id, returned as the choice.
    pub id: String,
    /// Human description; empty → the id alone is rendered.
    pub text: String,
}

/// A typed question. `state` and `instructions` are untrusted data to the classifier (28).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Question {
    /// Pick one candidate id; the engine appends the mandatory `none` control.
    Choice {
        /// What to decide.
        instructions: String,
        /// The bounded table of options, insertion order kept.
        candidates: Vec<Candidate>,
        /// Serialized context the decision is about.
        state: String,
    },
    /// Does the statement hold for `state`?
    Bool {
        /// The statement.
        statement: String,
        /// Serialized context.
        state: String,
    },
    /// An ordinal level, one criterion per level.
    Score {
        /// What to rate.
        instructions: String,
        /// Level criteria, index = level.
        levels: Vec<String>,
        /// Serialized context.
        state: String,
    },
}

/// A decision family: which thresholds apply and how the log is grouped, e.g. `meeting.topic`.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(transparent)]
pub struct Family(pub String);

impl From<&str> for Family {
    fn from(s: &str) -> Self {
        Family(s.to_owned())
    }
}

impl std::fmt::Display for Family {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Identifies one log row for [`Outcome`] patching. Time-ordered.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(transparent)]
pub struct LogId(pub uuid::Uuid);

impl LogId {
    /// A fresh id.
    pub fn new() -> Self {
        LogId(uuid::Uuid::now_v7())
    }
}

impl Default for LogId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for LogId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// `ask` → [`Decision`]. Over `budget` the reply is `Escalate(Timeout)`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Ask {
    /// Which gate applies.
    pub family: Family,
    /// The question.
    pub question: Question,
    /// Wall-clock budget; default 400 ms (57 §2).
    pub budget: Duration,
}

impl Ask {
    /// An `Ask` with the default budget.
    pub fn new(family: impl Into<Family>, question: Question) -> Self {
        Ask {
            family: family.into(),
            question,
            budget: Duration::from_millis(400),
        }
    }
}

/// Why the oracle did not accept.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "reason", content = "detail", rename_all = "snake_case")]
pub enum Reason {
    /// Calibrated top-1 under `tau_top`.
    LowTop1,
    /// Top-1 − top-2 under `tau_margin`.
    LowMargin,
    /// Tokenizer / ONNX / IO failure; the message.
    HardError(String),
    /// The state did not fit the token budget.
    Truncated,
    /// Over [`Ask::budget`].
    Timeout,
    /// Plugin or family off.
    Disabled,
    /// The checkpoint is not in the model dir.
    ModelMissing,
}

/// What the caller does with the answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "disposition", rename_all = "snake_case")]
pub enum Disposition {
    /// Act on [`Decision::choice`]; no model turn.
    Accept,
    /// Keep today's path.
    Escalate(Reason),
    /// Shadow mode: proceed as today; the inner decision is what the gate would have said.
    Shadow(Box<Decision>),
}

impl Disposition {
    /// `accept` / `escalate` / `shadow`, for the log.
    pub fn kind(&self) -> &'static str {
        match self {
            Disposition::Accept => "accept",
            Disposition::Escalate(_) => "escalate",
            Disposition::Shadow(_) => "shadow",
        }
    }

    /// The escalation reason, through a `Shadow` wrapper.
    pub fn reason(&self) -> Option<&Reason> {
        match self {
            Disposition::Accept => None,
            Disposition::Escalate(r) => Some(r),
            Disposition::Shadow(inner) => inner.disposition.reason(),
        }
    }
}

/// Reply to [`Ask`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
pub struct Decision {
    /// The family asked.
    pub family: Family,
    /// Candidate id, `none`, `true`/`false`, or the level index.
    pub choice: String,
    /// Calibrated, insertion order, sums to 1; empty when nothing ran.
    pub probabilities: Vec<(String, f32)>,
    /// Entropy-derived (Choice/Score) or `max(p, 1−p)` (Bool).
    pub confidence: f32,
    /// laya's act head, logged only.
    pub act_probability: f32,
    /// Accept, escalate, or shadow.
    pub disposition: Disposition,
    /// Wall-clock time in the engine.
    pub latency: Duration,
    /// Thresholds + model artifact, for comparable log rows.
    pub gate_version: u32,
    /// For [`Outcome`] patching via `Report`.
    pub log_id: LogId,
    /// 1 = the local oracle, 2 = the cheap-model side call (`escalate`), 3 = the main model turn (57 §2).
    #[serde(default = "tier_one")]
    pub tier: u8,
    /// `false` when `probabilities` is a model's self-reported confidence, not the calibrated head.
    #[serde(default = "yes")]
    pub calibrated: bool,
    /// The rendered state was right-truncated to the model's window, or an option marker fell
    /// past it (57 §0.5). A hard error only when the family does not allow a cut
    /// (`Escalate(Truncated)`); otherwise the answer stands and the caller sees the cut here.
    #[serde(default)]
    pub truncated: bool,
    /// The pooled encoder state of the rendered question (126 §7.2): `Some`
    /// only for a family whose gate says `embed: true` (the `double.*`
    /// families); never logged — the log row picks its fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
}

const fn tier_one() -> u8 {
    1
}

const fn yes() -> bool {
    true
}

impl Decision {
    /// Wrap into shadow mode: the caller proceeds as today, the would-be disposition is kept inside.
    pub fn shadowed(self) -> Decision {
        let mut outer = self.clone();
        outer.disposition = Disposition::Shadow(Box::new(self));
        outer
    }
}

/// What actually happened, reported back against [`Decision::log_id`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    /// The model turn (or the user) agreed with the choice.
    Confirmed,
    /// It picked something else.
    Corrected {
        /// What was picked instead.
        actual: String,
    },
    /// Nobody checked.
    Unknown,
}
