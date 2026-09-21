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
}

/// `ask` on the `agent` actor → `Result<SideReply, String>`: one non-streaming,
/// no-tools, no-history request on a role model, run detached from the session's
/// turn and cost-attributed to `session` under `kind`. Fails with the provider's
/// error, `timed out` after `budget`, or `cancelled` when the session closes.
/// Consumers: the title turn, `decision::escalate`, the 57 §11 training composer.
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
    /// Cost tag (`model_calls.kind`): `Escalate` for the oracle, `Title` for titles.
    pub kind: CallKind,
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
        ] {
            assert_eq!(k.as_str().parse::<CallKind>().unwrap(), k);
        }
        assert!("nope".parse::<CallKind>().is_err());
    }
}
