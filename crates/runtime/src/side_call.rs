//! The side-call message types (57 §2 `SideCall`): one non-streaming, no-tools,
//! no-history request on a role model, answered by the `agent` plugin's actor
//! through the kameo registry. They live here — plain data over [`SessionId`] —
//! so `decision` (below `agent` in 00 §10's order) can send one without a crate
//! edge to `agent`; `agent` re-exports them.

use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::SessionId;

/// What a native model attempt was for (51 §6.1); `model_calls.kind` in the store.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum CallKind {
    /// A tool round of a user turn.
    Round,
    /// Automatic or manual compaction.
    Compaction,
    /// Explicit durable harvest.
    Harvest,
    /// Title generation.
    Title,
    /// Vision-fallback question or inspection call (52 §5).
    Vision,
    /// Tier-2 side call of the local decision oracle (57 §0.9).
    Escalate,
    /// A model call made by dreaming's tools (136 §2.2): replay, judge, propose.
    Dream,
    /// 137: an endless-mode compactor call (one node of the summary tree).
    Endless,
    /// 140: an autoresearch side call (reviewer, advisor), charged to the study session.
    Autoresearch,
    /// 147 §6.4: a CodeMap generator side call, charged to the opening session.
    CodeMap,
    /// 125 R1: a native subagent's one-shot consultation of its ancestor's advisor.
    Consult,
    /// 153 §9: a dictation polish/rewrite side call.
    Polish,
    /// 219: the cell-nudge repair call (a plain run-me fence rewritten as a user cell).
    CellNudge,
}

impl CallKind {
    /// The stored string form.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Round => "round",
            Self::Compaction => "compaction",
            Self::Harvest => "harvest",
            Self::Title => "title",
            Self::Vision => "vision",
            Self::Escalate => "escalate",
            Self::Dream => "dream",
            Self::Endless => "endless",
            Self::Autoresearch => "autoresearch",
            Self::CodeMap => "codemap",
            Self::Consult => "consult",
            Self::Polish => "polish",
            Self::CellNudge => "cell_nudge",
        }
    }
}

impl fmt::Display for CallKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for CallKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "round" => Ok(Self::Round),
            "compaction" => Ok(Self::Compaction),
            "harvest" => Ok(Self::Harvest),
            "title" => Ok(Self::Title),
            "vision" => Ok(Self::Vision),
            "escalate" => Ok(Self::Escalate),
            "dream" => Ok(Self::Dream),
            "endless" => Ok(Self::Endless),
            "autoresearch" => Ok(Self::Autoresearch),
            "codemap" => Ok(Self::CodeMap),
            "consult" => Ok(Self::Consult),
            "polish" => Ok(Self::Polish),
            "cell_nudge" => Ok(Self::CellNudge),
            _ => Err(format!("unknown CallKind: {s:?}")),
        }
    }
}

/// Which model a [`SideCall`] runs on, resolved inside `agent` so callers never
/// see model ids (57 §2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelRole {
    /// `agent.escalate_model` → `agent.title_model` → the session's model.
    Escalate,
    /// `agent.title_model` → the session's model.
    Title,
    /// `agent.vision_model`; an error when unset (never the main model, 52 §3).
    Vision,
    /// The session's own model.
    Session,
    /// An explicit `provider/model` or bare id.
    Named(String),
    /// An explicit `provider/model` (or bare id) with that setting's own effort (194 D4e).
    /// `effort: None` behaves exactly like `Named`.
    NamedWithEffort {
        /// As in `Named`.
        model: String,
        /// The setting's effort keyword or token budget; `None` = no reasoning setting.
        effort: Option<String>,
    },
    /// `voice.polish_model` → `agent.title_model` → the session's model (153 §9.3).
    Polish,
}

/// `ask` on the `agent` actor → `Result<SideReply, String>`: one non-streaming,
/// no-tools, no-history request on a role model, run detached from the session's
/// turn and cost-attributed to `session` under `kind`. Fails with the provider's
/// error, `timed out` after `budget`, or `cancelled` when the session closes.
/// Consumers: the title turn, `decision::escalate`, the 57 §11 training composer,
/// 153 §9 polish/rewrite (`CallKind::Polish` on `ModelRole::Polish`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SideCall {
    /// Session the call belongs to (cost row owner; also the `Session` role's model).
    pub session: SessionId,
    /// Which model answers.
    pub role: ModelRole,
    /// System prompt.
    pub system: String,
    /// The single user message.
    pub user: String,
    /// Wall-clock budget for the whole call.
    pub budget: Duration,
    /// Cost tag (`model_calls.kind`): `Escalate` for the oracle, `Title` for titles, `Polish` for dictation polish.
    pub kind: CallKind,
}

/// `ask` on the `agent` actor → `Result<(), String>`: whether `session` can
/// take a [`SideCall`] right now — a live native session (C2). The `Err` is
/// the supervisor's text, the one a [`SideCall`] would fail with; consumers
/// (`codemap_generate`) refuse up front instead of starting work that fails.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SideCallable {
    /// The session a side call would be charged to.
    pub session: SessionId,
}

/// What a [`SideCall`] came back with.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SideReply {
    /// The model's text, concatenated; empty when it replied with nothing.
    pub text: String,
    /// Canonical id of the model that answered.
    pub model: String,
    /// Prompt and completion tokens the provider reported, when it did.
    pub input_tokens: Option<u32>,
    /// See `input_tokens`.
    pub output_tokens: Option<u32>,
    /// Wall clock from dispatch to the final event.
    pub latency: Duration,
    /// What the call cost, USD nanos — the priced amount `agent` settled in
    /// `model_calls` (provider-reported, else estimated from the catalog);
    /// `None` when unpriced or in another currency.
    #[serde(default)]
    pub cost_nanos: Option<u64>,
    /// The `model_calls.request_id` of the call, for provenance.
    #[serde(default)]
    pub request_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_kind_round_trips_its_string_form() {
        for k in [
            CallKind::Round,
            CallKind::Compaction,
            CallKind::Harvest,
            CallKind::Title,
            CallKind::Vision,
            CallKind::Escalate,
            CallKind::Dream,
            CallKind::Endless,
            CallKind::CodeMap,
            CallKind::Consult,
            CallKind::Polish,
            CallKind::CellNudge,
        ] {
            assert_eq!(k.as_str().parse::<CallKind>().unwrap(), k);
        }
        assert!("nope".parse::<CallKind>().is_err());
    }
}
