//! The `double:` config section (126 §12). Read once at startup from
//! `config.yaml` by `DoublePlugin::start` through `rt.section("double")` (the
//! 00 rule: config.yaml is read once; every key below is then live through
//! `double_settings_set`, which patches the running copy and persists to the
//! file — except `enabled` and `model.*`, which need a restart). Defaults
//! follow §12 with Revision 1's rulings: grace 30 s (ruling 3); L0–L2 only in
//! v1, so the `read_only` cap defaults to 2 (ruling 4); retention 365 days
//! with the golden + last-1,000-per-class exemptions (ruling 6).

use serde::{Deserialize, Serialize};

/// `double:`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct DoubleConfig {
    /// `false` (default): no actor, no tools, no prompt paragraph. Restart to change.
    pub enabled: bool,
    pub capture: Capture,
    pub ladder: Ladder,
    pub training: Training,
    /// `examples` / `labels` / `predictions` older than this are deleted,
    /// except golden rows and the last 1,000 per class (§5).
    pub retention_days: u32,
    /// Restart to change.
    pub model: Model,
}

/// Which card kinds double predicts and learns from (§6.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct Capture {
    pub approvals: bool,
    pub asks: bool,
    pub attention: bool,
    pub packets: bool,
    pub drafts: bool,
}

/// The autonomy ladder's knobs (§9).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct Ladder {
    /// L2 countdown; 30 s (Revision 1 ruling 3). Clamped to `deadline − 2 s`.
    pub grace_secs: u32,
    pub caps: Caps,
    /// Default `classes.mutating_opt_in` for new classes.
    pub mutating_opt_in: bool,
}

/// Max rung by risk (§9 caps). `mutating` may reach 3 only with the per-class opt-in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct Caps {
    /// 2 in the first release: L3 stays behind this cap (Revision 1 ruling 4).
    pub read_only: u8,
    pub mutating: u8,
    /// Always 1: destructive never climbs past L1 (§0 item 4).
    pub destructive: u8,
}

/// The training cycle (§8).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct Training {
    /// `auto` | `local` | `node:<name>` | `off` (§8.4; `crate::types::Venue::parse`).
    pub venue: String,
    /// Local hours Tier B may run, `HH:MM-HH:MM`.
    pub window: String,
    /// User idle required before a run starts or resumes.
    pub idle_min: u32,
    /// Cumulative `running` minutes per night per run.
    pub run_cap_min: u32,
    pub triggers: Triggers,
    /// Ship scrubbed bundles to the owner's own GPU nodes (never a `Vm`).
    pub share_data: bool,
    pub tiers: Tiers,
}

/// When Tier B becomes eligible (§7.3): whichever comes first. Tier A runs
/// every 25 labels or nightly regardless.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct Triggers {
    pub labels: u32,
    pub days: u32,
}

/// Per-tier switches (§7.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct Tiers {
    pub a: TierA,
    pub b: TierB,
    pub c: TierC,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct TierA {
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct TierB {
    pub enabled: bool,
    /// LoRA rank.
    pub rank: u32,
    pub batch: u32,
    pub epochs: u32,
    pub lr: f32,
}

/// Owner action only; never scheduled.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct TierC {
    pub enabled: bool,
}

/// Restart to change (§12).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct Model {
    /// A `decision.checkpoints` name.
    pub checkpoint: String,
    pub adapter_rank: u32,
}

impl Default for DoubleConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            capture: Capture::default(),
            ladder: Ladder::default(),
            training: Training::default(),
            retention_days: 365,
            model: Model::default(),
        }
    }
}

impl Default for Capture {
    fn default() -> Self {
        Self {
            approvals: true,
            asks: true,
            attention: true,
            packets: true,
            drafts: true,
        }
    }
}

impl Default for Ladder {
    fn default() -> Self {
        Self {
            grace_secs: 30,
            caps: Caps::default(),
            mutating_opt_in: false,
        }
    }
}

impl Default for Caps {
    fn default() -> Self {
        Self {
            read_only: 2,
            mutating: 1,
            destructive: 1,
        }
    }
}

impl Default for Training {
    fn default() -> Self {
        Self {
            venue: "auto".into(),
            window: "02:00-06:00".into(),
            idle_min: 15,
            run_cap_min: 90,
            triggers: Triggers::default(),
            share_data: true,
            tiers: Tiers::default(),
        }
    }
}

impl Default for Triggers {
    fn default() -> Self {
        Self {
            labels: 200,
            days: 7,
        }
    }
}

impl Default for Tiers {
    fn default() -> Self {
        Self {
            a: TierA::default(),
            b: TierB::default(),
            c: TierC::default(),
        }
    }
}

impl Default for TierA {
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl Default for TierB {
    fn default() -> Self {
        Self {
            enabled: true,
            rank: 8,
            batch: 8,
            epochs: 3,
            lr: 2.0e-4,
        }
    }
}

impl Default for TierC {
    fn default() -> Self {
        Self { enabled: false }
    }
}

impl Default for Model {
    fn default() -> Self {
        Self {
            checkpoint: "default".into(),
            adapter_rank: 8,
        }
    }
}

impl Caps {
    /// The cap for a risk class (`tools::Risk::as_str` spelling); a non-approval
    /// card (ask / attention / packet / draft) uses `read_only`.
    pub fn for_risk(&self, risk: Option<tools::Risk>) -> u8 {
        match risk {
            Some(tools::Risk::Destructive) => self.destructive.min(1),
            Some(tools::Risk::Mutating) => self.mutating,
            Some(tools::Risk::ReadOnly) | None => self.read_only,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §12 defaults with Revision 1's rulings; a partial YAML patches one key
    /// and keeps the rest.
    #[test]
    fn defaults_and_partial_yaml() {
        let d = DoubleConfig::default();
        assert!(!d.enabled);
        assert_eq!(d.ladder.grace_secs, 30);
        assert_eq!((d.ladder.caps.read_only, d.ladder.caps.mutating, d.ladder.caps.destructive), (2, 1, 1));
        assert_eq!(d.training.venue, "auto");
        assert_eq!(d.training.window, "02:00-06:00");
        assert_eq!((d.training.triggers.labels, d.training.triggers.days), (200, 7));
        assert_eq!(d.retention_days, 365);
        assert_eq!(d.model.checkpoint, "default");

        let yaml = "enabled: true\nladder:\n  grace_secs: 10\ntraining:\n  tiers:\n    b: { rank: 16 }\n";
        let c: DoubleConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(c.enabled);
        assert_eq!(c.ladder.grace_secs, 10);
        assert_eq!(c.ladder.caps, Caps::default());
        assert_eq!((c.training.tiers.b.rank, c.training.tiers.b.epochs), (16, 3));
        // Destructive is capped at 1 whatever the file says (§0 item 4).
        let c: DoubleConfig = serde_yaml::from_str("ladder: { caps: { destructive: 3 } }").unwrap();
        assert_eq!(c.ladder.caps.for_risk(Some(tools::Risk::Destructive)), 1);
    }
}
