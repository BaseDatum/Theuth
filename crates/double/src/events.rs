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
#[serde(tag = "type")]
pub struct DoublePredicted {
    /// `ApprovalId` / `AskId` / packet key / draft id / attention item id, as text.
    pub question_id: String,
    /// The card's session, when it has one (packets and attention items may not);
    /// the bridge's per-session filter reads it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub session: Option<SessionId>,
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
    pub explain: Explain,
}

/// Why the double predicted what it did (the chip popover, §10).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Explain {
    /// The ≤ 3 nearest past decisions.
    pub neighbours: Vec<Neighbour>,
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
    pub oracle: f32,
    pub memory: f32,
    pub prior: f32,
}

/// The double answered a card at L2 (grace expired) or L3 (§4.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub struct DoubleAnswered {
    pub question_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub session: Option<SessionId>,
    pub class: String,
    pub choice: String,
    pub version: String,
    /// RFC 3339 end of the undo window, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub undo_until: Option<String>,
}

/// A training run changed state (§8.1); Settings shows it live.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub struct DoubleTrainingState {
    pub run_id: String,
    pub tier: Tier,
    /// `local` | `node:<name>`.
    pub venue: String,
    pub state: RunState,
    /// 0..=1 while running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub progress: Option<f32>,
    pub message: String,
}

/// A candidate passed the gate (or `double_rollback` ran) and is now active (§8.5).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub struct DoubleVersionActivated {
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub previous: Option<String>,
    /// The version's `metrics` JSON.
    pub metrics: serde_json::Value,
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
            probs: vec![("Allow".into(), 0.91), ("Deny".into(), 0.05), ("none".into(), 0.04)],
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
        assert_eq!((t["type"].as_str(), t["tier"].as_str(), t["state"].as_str()), (Some("DoubleTrainingState"), Some("b"), Some("running")));
    }
}
