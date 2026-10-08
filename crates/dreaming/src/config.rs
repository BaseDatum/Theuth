//! The `dreaming:` section of config.yaml (design doc 136 §9.2). Read once at
//! start like every section; `dreaming.set_settings` rewrites part of it
//! (136 W3). Every field has a default, so an absent section is the design's
//! defaults with `budget_usd: 1.0`.

use serde::{Deserialize, Serialize};

/// `dreaming:` (136 §9.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct DreamConfig {
    /// `false`: no routine file installed, no signals.json, no prompt block;
    /// the 136 knowledge keys are still honoured.
    pub enabled: bool,
    /// Local `["HH:MM", "HH:MM"]` the sensor may fire inside; may cross midnight.
    pub window: [String; 2],
    /// Minutes since the last root turn before the sensor fires.
    pub idle_minutes: u32,
    /// Below this many sessions since the last dream the night is Recall + Journal only.
    pub min_sessions: u32,
    /// A day past either count allows a second window the same night.
    pub heavy_day: HeavyDay,
    /// The night's money, USD; `[sky]` $1 default, entirely configurable.
    pub budget_usd: f64,
    /// `run_timeout:` of the bundled routine (humantime).
    pub run_timeout: String,
    /// The four model tiers; `None` = the design's default per tier.
    pub models: Models,
    /// Per-phase count caps.
    pub caps: Caps,
    /// Type lifetimes in days (knowledge's own `knowledge.lifetimes` wins for rank; 136 §3.1).
    pub lifetimes: Lifetimes,
    /// The replay gate (136 §7.2).
    pub gate: Gate,
    /// Standing preferences (136 §7.1 rung 3).
    pub preferences: Preferences,
    /// Weekly digest row, `"<Day> HH:MM"` local.
    pub digest: String,
    /// Mesh: only this engine dreams; `None` = the engine that installed the routine.
    pub home_engine: Option<String>,
}

/// `heavy_day:`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(
    feature = "ts",
    derive(ts_rs::TS),
    ts(export, rename = "DreamHeavyDay")
)]
#[serde(default)]
pub struct HeavyDay {
    /// New concepts since the last dream.
    pub concepts: u32,
    /// Sessions since the last dream.
    pub sessions: u32,
}

/// `models:` — `provider/model` per tier; `None` = default (136 §9.2).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(
    feature = "ts",
    derive(ts_rs::TS),
    ts(export, rename = "DreamConfigModels")
)]
#[serde(default)]
pub struct Models {
    /// `None` = `decide_many` only, model tail off.
    pub triage: Option<String>,
    /// Default: `agent.model` (136 D12); always written as the routine's `model:`.
    pub tidy: Option<String>,
    /// v1: `tidy` (the proposer runs in the night's own turn).
    pub propose: Option<String>,
    /// Default: `agent.escalate_model` when set and ≠ the proposer model, else the
    /// proposer model (`judge_same_model` recorded on the night).
    pub judge: Option<String>,
}

/// What the `agent:` section offers as tier defaults (136 D12).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentDefaults {
    /// `agent.model`.
    pub model: String,
    /// `agent.escalate_model`, when set and non-empty.
    pub escalate_model: Option<String>,
}

impl AgentDefaults {
    /// Read the `agent:` section (defaults when absent).
    pub fn from_runtime(rt: &runtime::Runtime) -> Self {
        let a: agent::AgentConfig = rt.section("agent").unwrap_or_default();
        Self {
            model: a.model,
            escalate_model: a.escalate_model.filter(|m| !m.trim().is_empty()),
        }
    }
}

impl DreamConfig {
    /// The models that run each tier (136 D12): the config's pick, else the
    /// default from the `agent:` section. The flag is `judge_same_model`.
    pub fn resolve_models(&self, agent: &AgentDefaults) -> (crate::types::ResolvedModels, bool) {
        fn pick(m: &Option<String>) -> Option<&str> {
            m.as_deref().map(str::trim).filter(|m| !m.is_empty())
        }
        let tidy = pick(&self.models.tidy).unwrap_or(&agent.model).to_owned();
        let propose = pick(&self.models.propose).unwrap_or(&tidy).to_owned();
        let (judge, same) = match pick(&self.models.judge) {
            Some(j) => (j.to_owned(), j == propose),
            None => match agent.escalate_model.as_deref() {
                Some(e) if e != propose => (e.to_owned(), false),
                _ => (propose.clone(), true),
            },
        };
        (
            crate::types::ResolvedModels {
                tidy,
                propose,
                judge,
                target: agent.model.clone(),
            },
            same,
        )
    }
}

/// `caps:`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "DreamCaps"))]
#[serde(default)]
pub struct Caps {
    /// Fresh entries triaged per night.
    pub triage: u32,
    /// Merges per night.
    pub merges: u32,
    /// Archives per night.
    pub archives: u32,
    /// Verification sample per night.
    pub verifications: u32,
    /// New cases per night.
    pub cases: u32,
    /// Edits per target per night.
    pub edits_per_target: u32,
    /// Targets per night.
    pub targets: u32,
    /// Attention rows per 7 days.
    pub rows_per_week: u32,
}

/// `lifetimes:` in days.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(
    feature = "ts",
    derive(ts_rs::TS),
    ts(export, rename = "DreamLifetimes")
)]
#[serde(default)]
pub struct Lifetimes {
    /// `record` archived after this.
    pub record: u32,
    /// `fact` verify-by.
    pub fact: u32,
    /// `procedure` archived after this unopened.
    pub procedure: u32,
    /// `entity` / hosts / project verify-by.
    pub entity: u32,
}

/// `gate:`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "DreamGate"))]
#[serde(default)]
pub struct Gate {
    /// Mean improvement required.
    pub margin: f64,
    /// Largest per-case regression allowed.
    pub max_regression: f64,
    /// Fewer val cases → `unverified`.
    pub min_val_cases: u32,
    /// Share of cases hashed into `val`.
    pub val_fraction: f64,
}

/// `preferences:`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(
    feature = "ts",
    derive(ts_rs::TS),
    ts(export, rename = "DreamPreferences")
)]
#[serde(default)]
pub struct Preferences {
    /// A line backed by ≥ 2 explicit instructions is promoted without asking.
    pub auto_from_explicit: bool,
    /// Lines in the block.
    pub cap_lines: u32,
}

impl Default for DreamConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            window: ["01:00".into(), "06:00".into()],
            idle_minutes: 20,
            min_sessions: 1,
            heavy_day: HeavyDay::default(),
            budget_usd: 1.0,
            run_timeout: "45m".into(),
            models: Models::default(),
            caps: Caps::default(),
            lifetimes: Lifetimes::default(),
            gate: Gate::default(),
            preferences: Preferences::default(),
            digest: "Mon 08:00".into(),
            home_engine: None,
        }
    }
}

impl Default for HeavyDay {
    fn default() -> Self {
        Self {
            concepts: 300,
            sessions: 40,
        }
    }
}

impl Default for Caps {
    fn default() -> Self {
        Self {
            triage: 300,
            merges: 40,
            archives: 60,
            verifications: 15,
            cases: 60,
            edits_per_target: 4,
            targets: 3,
            rows_per_week: 3,
        }
    }
}

impl Default for Lifetimes {
    fn default() -> Self {
        Self {
            record: 14,
            fact: 30,
            procedure: 90,
            entity: 60,
        }
    }
}

impl Default for Gate {
    fn default() -> Self {
        Self {
            margin: 0.1,
            max_regression: 0.2,
            min_val_cases: 3,
            val_fraction: 0.34,
        }
    }
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            auto_from_explicit: true,
            cap_lines: 20,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_design_9_2_with_one_dollar() {
        let c: DreamConfig = serde_yaml::from_str("{}").unwrap();
        assert_eq!(c, DreamConfig::default());
        assert!(c.enabled);
        assert_eq!(c.budget_usd, 1.0);
        assert_eq!(c.window, ["01:00", "06:00"]);
        assert_eq!((c.idle_minutes, c.min_sessions), (20, 1));
        assert_eq!((c.heavy_day.concepts, c.heavy_day.sessions), (300, 40));
        assert_eq!(c.run_timeout, "45m");
        assert_eq!(c.models, Models::default());
        assert_eq!(
            (c.caps.triage, c.caps.edits_per_target, c.caps.rows_per_week),
            (300, 4, 3)
        );
        assert_eq!(
            (
                c.lifetimes.record,
                c.lifetimes.fact,
                c.lifetimes.procedure,
                c.lifetimes.entity
            ),
            (14, 30, 90, 60)
        );
        assert_eq!(
            (c.gate.margin, c.gate.max_regression, c.gate.min_val_cases),
            (0.1, 0.2, 3)
        );
        assert!(c.preferences.auto_from_explicit && c.preferences.cap_lines == 20);
        assert_eq!(c.digest, "Mon 08:00");
        assert!(c.home_engine.is_none());
    }

    #[test]
    fn partial_section_keeps_other_defaults() {
        let c: DreamConfig = serde_yaml::from_str(
            "budget_usd: 5\nmodels: {judge: openai/gpt-5.6-sol}\nwindow: ['23:00', '05:00']",
        )
        .unwrap();
        assert_eq!(c.budget_usd, 5.0);
        assert_eq!(c.models.judge.as_deref(), Some("openai/gpt-5.6-sol"));
        assert!(c.models.tidy.is_none());
        assert_eq!(c.window, ["23:00", "05:00"]);
        assert_eq!(c.caps, Caps::default());
    }

    /// D12: `tidy` defaults to `agent.model`, `propose` to `tidy`, `judge` to
    /// `agent.escalate_model` unless it is the proposer (then the proposer,
    /// flagged), `target` to `agent.model`; a config pick wins.
    #[test]
    fn model_tiers_resolve_from_the_agent_section() {
        let agent = AgentDefaults {
            model: "openai/gpt-5.6-sol".into(),
            escalate_model: Some("anthropic/claude-opus-4-1".into()),
        };
        let (r, same) = DreamConfig::default().resolve_models(&agent);
        assert_eq!(
            (
                r.tidy.as_str(),
                r.propose.as_str(),
                r.judge.as_str(),
                r.target.as_str()
            ),
            (
                "openai/gpt-5.6-sol",
                "openai/gpt-5.6-sol",
                "anthropic/claude-opus-4-1",
                "openai/gpt-5.6-sol"
            )
        );
        assert!(!same);
        let no_escalate = AgentDefaults {
            escalate_model: None,
            ..agent.clone()
        };
        let (r, same) = DreamConfig::default().resolve_models(&no_escalate);
        assert_eq!(r.judge, "openai/gpt-5.6-sol");
        assert!(same, "no escalate model: the proposer judges, flagged");
        let same_as_propose = AgentDefaults {
            escalate_model: Some("openai/gpt-5.6-sol".into()),
            ..agent.clone()
        };
        assert!(DreamConfig::default().resolve_models(&same_as_propose).1);
        let picked = DreamConfig {
            models: Models {
                tidy: Some("anthropic/claude-haiku-4-5".into()),
                judge: Some(" ".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let (r, same) = picked.resolve_models(&agent);
        assert_eq!(r.tidy, "anthropic/claude-haiku-4-5");
        assert_eq!(r.propose, "anthropic/claude-haiku-4-5", "propose v1 = tidy");
        assert_eq!(
            r.judge, "anthropic/claude-opus-4-1",
            "a blank pick is no pick"
        );
        assert!(!same);
    }
}
