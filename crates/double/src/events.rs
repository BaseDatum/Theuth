//! Bus events (126 §13, §13.1). Published on the runtime bus with
//! `rt.publish(ev)`; the RPC bridge subscribes to each with
//! `rt.subscribe::<Bridge, T>` and forwards it on `subscribe_events`
//! (crates/rpc/src/bridge.rs), where the `type` tag is the Rust type name.
//! All four are `ts-rs` exported under `ts`, like `tools::ApprovalRequested`.

use runtime::SessionId;
use serde::{Deserialize, Serialize};

use crate::class::CardKind;
use crate::types::{RunState, Tier};

/// A prediction was written for a card, before the owner saw it (§7.1 step 6).
/// Re-published for every pending card when a client (re)subscribes (§4.4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct DoublePredicted {
    /// `ApprovalId` / `AskId` / packet key / draft id / attention item id, as text.
    pub question_id: String,
    /// The card's session, when it has one (packets and attention items may not);
    /// the bridge's per-session filter reads it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub session: Option<SessionId>,
    /// Which card kind.
    pub kind: CardKind,
    /// The §3 key.
    pub class: String,
    /// Option label as rendered on the card (`Allow`, `Start now`, …); `none` = escalate.
    pub choice: String,
    /// Ask: the option index `choice` maps to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub index: Option<u32>,
    /// Calibrated top-1.
    pub p: f32,
    /// Every option incl. `none`.
    pub probs: Vec<(String, f32)>,
    /// The class's rung when predicted, 0..=3.
    pub rung: u8,
    /// RFC 3339 grace end at L2; `None` at L0/L1; `Some(now)` at L3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub will_act_at: Option<String>,
    /// `versions.id` the prediction names.
    pub version: String,
    /// Why.
    pub explain: Explain,
}

impl wire::WireEvent for DoublePredicted {
    const TOPIC: &'static str = "double.predicted";
}

/// Why the double predicted what it did (the chip popover, §10).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Explain {
    /// The ≤ 3 nearest past decisions.
    pub neighbours: Vec<Neighbour>,
    /// What each part of the head contributed to the top choice.
    pub parts: Parts,
    /// What would promote the class ("12 more correct in a row").
    pub promote_hint: String,
}

/// One nearest past decision.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Neighbour {
    /// RFC 3339.
    pub asked_at: String,
    /// ≤ 160 chars of the scrubbed card text.
    pub card_excerpt: String,
    /// What the owner chose.
    pub chose: String,
}

/// The head's contributions to the top choice (§7.1 step 5).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Parts {
    /// Zero-shot oracle.
    pub oracle: f32,
    /// k-NN memory.
    pub memory: f32,
    /// Class prior.
    pub prior: f32,
}

/// The double answered a card at L2 (grace expired) or L3 (§4.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct DoubleAnswered {
    /// The card answered.
    pub question_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    /// The card's session, when it has one.
    pub session: Option<SessionId>,
    /// The §3 key.
    pub class: String,
    /// The option label sent.
    pub choice: String,
    /// The version that predicted it.
    pub version: String,
    /// RFC 3339 end of the undo window, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub undo_until: Option<String>,
}

impl wire::WireEvent for DoubleAnswered {
    const TOPIC: &'static str = "double.answered";
}

/// A training run changed state (§8.1); Settings shows it live.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct DoubleTrainingState {
    /// `runs.id`.
    pub run_id: String,
    /// Which tier runs.
    pub tier: Tier,
    /// `local` | `node:<name>`.
    pub venue: String,
    /// Where in the state machine.
    pub state: RunState,
    /// 0..=1 while running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub progress: Option<f32>,
    /// One human-readable line (`epoch 2/3`, `rejected: golden`).
    pub message: String,
}

impl wire::WireEvent for DoubleTrainingState {
    const TOPIC: &'static str = "double.training";
}

/// A candidate passed the gate (or `double_rollback` ran) and is now active (§8.5).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct DoubleVersionActivated {
    /// The now-active `versions.id`.
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    /// The version it replaced.
    pub previous: Option<String>,
    /// The version's `metrics` JSON.
    pub metrics: serde_json::Value,
}

impl wire::WireEvent for DoubleVersionActivated {
    const TOPIC: &'static str = "double.version";
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire tag is the type name and optional fields vanish when `None`
    /// (what the desktop's zustand store keys on, §13.1).
    #[test]
    fn wire_shape() {
        let v = serde_json::to_value(DoublePredicted {
            question_id: "q".into(),
            session: None,
            kind: CardKind::Approval,
            class: "approval:shell:mutating:git push:root".into(),
            choice: "Allow".into(),
            index: None,
            p: 0.91,
            probs: vec![
                ("Allow".into(), 0.91),
                ("Deny".into(), 0.05),
                ("none".into(), 0.04),
            ],
            rung: 2,
            will_act_at: Some("2026-10-07T02:00:30+00:00".into()),
            version: "baseline".into(),
            explain: Explain::default(),
        })
        .unwrap();
        assert_eq!(v["type"], "DoublePredicted");
        assert_eq!(v["kind"], "approval");
        assert!(v.get("session").is_none() && v.get("index").is_none());
        assert_eq!(v["probs"][0][0], "Allow");
        let t = serde_json::to_value(DoubleTrainingState {
            run_id: "r".into(),
            tier: Tier::B,
            venue: "node:gpu".into(),
            state: RunState::Running,
            progress: Some(0.5),
            message: "epoch 2/3".into(),
        })
        .unwrap();
        assert_eq!(
            (t["type"].as_str(), t["tier"].as_str(), t["state"].as_str()),
            (Some("DoubleTrainingState"), Some("b"), Some("running"))
        );
    }
}
