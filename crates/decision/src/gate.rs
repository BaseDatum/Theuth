//! The selective gate (57 §0.5): accept iff no hard error ∧ top-1 ≥ `tau_top` ∧
//! top-1 − top-2 ≥ `tau_margin`. Thresholds are per family and fitted from the log;
//! `Mode` decides whether an accept changes anything.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::question::{Decision, Disposition, Reason};

/// `shadow`: compute, log, return `Shadow(inner)`; `active`: the verdict stands; `off`: never run.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Run and log; the caller proceeds as today.
    Shadow,
    /// Accepts short-circuit the model turn.
    Active,
    /// Every `Ask` → `Escalate(Disabled)`.
    #[default]
    Off,
}

impl Mode {
    /// `shadow` / `active` / `off`, the config spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Shadow => "shadow",
            Mode::Active => "active",
            Mode::Off => "off",
        }
    }
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Mode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "shadow" => Ok(Mode::Shadow),
            "active" => Ok(Mode::Active),
            "off" => Ok(Mode::Off),
            _ => Err(format!("unknown mode {s:?}: shadow | active | off")),
        }
    }
}

/// One family's gate (57 §2 YAML).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FamilyGate {
    /// Shadow / active / off.
    pub mode: Mode,
    /// Minimum calibrated top-1 probability.
    pub tau_top: f32,
    /// Minimum top-1 − top-2.
    pub tau_margin: f32,
    /// Tier-1 wall clock (`Ask.budget`); humantime, default 400 ms.
    #[serde(default = "default_budget", with = "humantime_serde")]
    #[schemars(with = "String")]
    pub budget: Duration,
    /// Tier-2 wall clock (the cheap-model side call); humantime, default 3 s.
    #[serde(default = "default_escalate_budget", with = "humantime_serde")]
    #[schemars(with = "String")]
    pub escalate_budget: Duration,
    /// Capture training examples for this family (57 §11): off by default, every family.
    #[serde(default)]
    pub train: bool,
    /// Consent to upload this family's captured rows to theuth (57 §7.1); off by default,
    /// meaningless without `train` — clearing `train` clears it. Read by the step-11 uploader.
    #[serde(default)]
    pub share: bool,
    /// Which `decision.checkpoints` entry answers (57 §4.1 item 6); `None` = `default`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<String>,
    /// Append the `none` control to a `Choice` (57 §0.4). `false` for a head fine-tuned on a
    /// closed option set — cklxx/laya-browser learned jev's exact option list, and a `none`
    /// slot it never saw would take mass unpredictably.
    #[serde(default = "yes")]
    pub none: bool,
    /// A right-truncated state is not a hard error. `false` (57 §0.5) for transcript
    /// families, where a cut may hide the decisive line; `true` for the browser head, which
    /// was trained on states cut exactly like this (the head budget dominates the sequence).
    #[serde(default)]
    pub cut_ok: bool,
}

const fn yes() -> bool {
    true
}

const fn default_budget() -> Duration {
    Duration::from_millis(400)
}

const fn default_escalate_budget() -> Duration {
    Duration::from_secs(3)
}

mod humantime_serde {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&humantime::format_duration(*d).to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let s = String::deserialize(d)?;
        humantime::parse_duration(&s).map_err(serde::de::Error::custom)
    }
}

impl FamilyGate {
    /// A conservative shadow gate with the default budgets on the default checkpoint.
    pub const fn shadow(tau_top: f32, tau_margin: f32) -> Self {
        FamilyGate {
            mode: Mode::Shadow,
            tau_top,
            tau_margin,
            budget: default_budget(),
            escalate_budget: default_escalate_budget(),
            train: false,
            share: false,
            checkpoint: None,
            none: true,
            cut_ok: false,
        }
    }

    /// A shadow gate for the jev-shaped browser questions (57 §4.1 item 4): the `browser`
    /// checkpoint, no `none` control, state cut allowed, 2 s tier-1 budget (three questions
    /// over ~45 candidates in one forward on Metal), no margin — argmax executes as jev does.
    pub fn browser(tau_top: f32) -> Self {
        FamilyGate {
            tau_margin: 0.0,
            budget: Duration::from_secs(2),
            checkpoint: Some(crate::BROWSER_CHECKPOINT.to_owned()),
            none: false,
            cut_ok: true,
            ..FamilyGate::shadow(tau_top, 0.0)
        }
    }

    /// The checkpoint name this family runs on (`default` when unset).
    pub fn checkpoint(&self) -> &str {
        self.checkpoint
            .as_deref()
            .unwrap_or(crate::DEFAULT_CHECKPOINT)
    }

    /// The threshold verdict, mode-independent: a hard error wins, then `tau_top`, then
    /// `tau_margin`. K = 1 has no runner-up, so the margin is top-1 itself.
    pub fn decide(&self, probs: &[(String, f32)], hard_error: Option<Reason>) -> Disposition {
        if let Some(r) = hard_error {
            return Disposition::Escalate(r);
        }
        if probs.is_empty() {
            return Disposition::Escalate(Reason::HardError("no options".into()));
        }
        let (mut top1, mut top2) = (0.0f32, 0.0f32);
        for &(_, p) in probs {
            if p > top1 {
                top2 = top1;
                top1 = p;
            } else if p > top2 {
                top2 = p;
            }
        }
        if top1 < self.tau_top {
            Disposition::Escalate(Reason::LowTop1)
        } else if top1 - top2 < self.tau_margin {
            Disposition::Escalate(Reason::LowMargin)
        } else {
            Disposition::Accept
        }
    }

    /// Apply the mode to a computed decision: `Shadow` wraps, `Off` escalates, `Active` passes.
    pub fn finish(&self, mut decision: Decision) -> Decision {
        match self.mode {
            Mode::Active => decision,
            Mode::Shadow => decision.shadowed(),
            Mode::Off => {
                decision.disposition = Disposition::Escalate(Reason::Disabled);
                decision
            }
        }
    }

    /// Changes whenever the thresholds or the model artifact change, so log rows are comparable
    /// (57 §2). Mode is not part of it: flipping shadow → active keeps the rows comparable.
    pub fn version(&self, model_sha: &str) -> u32 {
        let mut h = Sha256::new();
        h.update(self.tau_top.to_bits().to_le_bytes());
        h.update(self.tau_margin.to_bits().to_le_bytes());
        h.update(model_sha.as_bytes());
        let d = h.finalize();
        u32::from_le_bytes([d[0], d[1], d[2], d[3]])
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::question::LogId;

    fn probs(ps: &[f32]) -> Vec<(String, f32)> {
        ps.iter()
            .enumerate()
            .map(|(i, &p)| (i.to_string(), p))
            .collect()
    }

    const G: FamilyGate = FamilyGate {
        mode: Mode::Active,
        tau_top: 0.8,
        tau_margin: 0.4,
        budget: Duration::from_millis(400),
        escalate_budget: Duration::from_secs(3),
        train: false,
        share: false,
        checkpoint: None,
        none: true,
        cut_ok: false,
    };

    #[test]
    fn accept() {
        assert_eq!(
            G.decide(&probs(&[0.9, 0.05, 0.05]), None),
            Disposition::Accept
        );
    }

    #[test]
    fn low_top1() {
        assert_eq!(
            G.decide(&probs(&[0.7, 0.2, 0.1]), None),
            Disposition::Escalate(Reason::LowTop1)
        );
    }

    #[test]
    fn low_margin() {
        let g = FamilyGate { tau_top: 0.5, ..G };
        assert_eq!(
            g.decide(&probs(&[0.55, 0.45]), None),
            Disposition::Escalate(Reason::LowMargin)
        );
    }

    #[test]
    fn hard_error_wins() {
        assert_eq!(
            G.decide(&probs(&[1.0]), Some(Reason::Truncated)),
            Disposition::Escalate(Reason::Truncated)
        );
        assert_eq!(
            G.decide(&[], None),
            Disposition::Escalate(Reason::HardError("no options".into()))
        );
    }

    #[test]
    fn k1_margin_is_top1() {
        assert_eq!(G.decide(&probs(&[0.85]), None), Disposition::Accept);
        assert_eq!(
            G.decide(&probs(&[0.79]), None),
            Disposition::Escalate(Reason::LowTop1)
        );
    }

    #[test]
    fn bool_two_options() {
        let p = vec![("false".to_owned(), 0.1), ("true".to_owned(), 0.9)];
        assert_eq!(G.decide(&p, None), Disposition::Accept);
        let p = vec![("false".to_owned(), 0.35), ("true".to_owned(), 0.65)];
        assert_eq!(G.decide(&p, None), Disposition::Escalate(Reason::LowTop1));
    }

    fn decision(d: Disposition) -> Decision {
        Decision {
            family: "t".into(),
            choice: "a".into(),
            probabilities: probs(&[0.9, 0.1]),
            confidence: 0.5,
            act_probability: 0.5,
            disposition: d,
            latency: Duration::ZERO,
            gate_version: 1,
            log_id: LogId::new(),
            tier: 1,
            calibrated: true,
        }
    }

    #[test]
    fn shadow_wraps_and_off_escalates() {
        let d = decision(Disposition::Accept);
        let shadow = FamilyGate {
            mode: Mode::Shadow,
            ..G
        }
        .finish(d.clone());
        match &shadow.disposition {
            Disposition::Shadow(inner) => assert_eq!(**inner, d),
            other => panic!("{other:?}"),
        }
        assert_eq!(shadow.disposition.reason(), None);
        assert_eq!(shadow.choice, "a");
        let off = FamilyGate {
            mode: Mode::Off,
            ..G
        }
        .finish(d.clone());
        assert_eq!(off.disposition, Disposition::Escalate(Reason::Disabled));
        assert_eq!(G.finish(d.clone()), d);
    }

    #[test]
    fn version_tracks_thresholds_and_model() {
        let v = G.version("sha-a");
        assert_eq!(v, G.version("sha-a"));
        assert_ne!(v, G.version("sha-b"));
        assert_ne!(v, FamilyGate { tau_top: 0.81, ..G }.version("sha-a"));
        assert_eq!(
            v,
            FamilyGate {
                mode: Mode::Shadow,
                ..G
            }
            .version("sha-a")
        );
    }
}
