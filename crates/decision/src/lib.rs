//! Decision plugin (design doc 57): a local typed-probability oracle — laya, a ModernBERT
//! encoder with a decision head, through the bundled ONNX Runtime — answering [`Question`]s
//! with a probability per option and a selective gate ([`gate`]) that says `Accept` or
//! `Escalate(reason)`. Callers keep today's reasoning-model path on `Escalate`; every
//! family ships in `shadow` until its log says otherwise.
//!
//! Public surface consumed by other plugins: messages on the [`DECISION_ACTOR`] actor
//! ([`Engine`]): [`Ask`] → [`Decision`], [`Report`], [`Stats`] → [`FamilyStats`].

pub mod engine;
pub mod gate;
pub mod laya;
pub mod log;
pub mod question;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use kameo::actor::Spawn;
use serde::Deserialize;

pub use engine::{Engine, Loader, Oracle, Report, Stats};
pub use gate::{FamilyGate, Mode};
pub use log::FamilyStats;
pub use question::{
    Ask, Candidate, Decision, Disposition, Family, LogId, Outcome, Question, Reason,
};

/// Registry name of the [`Engine`] actor.
pub const DECISION_ACTOR: &str = "decision";

/// The checkpoint under `<data_dir>/models/` (57 §2), published by `release-models.sh`.
pub const MODEL_SUBDIR: &str = "decision";

/// `decision:` config section (57 §2). Every field has a default; the section may be absent.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct DecisionConfig {
    /// `false`: the actor answers every `Ask` with `Escalate(Disabled)`.
    pub enabled: bool,
    /// Checkpoint directory (model.onnx, tokenizer.json, rl_agent_config.json); `None` → `<data_dir>/models/decision`.
    pub model_dir: Option<PathBuf>,
    /// ONNX Runtime intra-op threads.
    pub threads: usize,
    /// Unload the session after this long unused (humantime, e.g. `10m`).
    #[serde(deserialize_with = "duration")]
    pub idle_unload: Duration,
    /// Try the CoreML execution provider (step-2 experiment).
    pub coreml: bool,
    /// Store the question text in the log, not only its hash.
    pub log_text: bool,
    /// Gate per decision family; a family not listed never runs.
    pub families: BTreeMap<String, FamilyGate>,
}

impl Default for DecisionConfig {
    fn default() -> Self {
        DecisionConfig {
            enabled: true,
            model_dir: None,
            threads: 4,
            idle_unload: Duration::from_secs(600),
            coreml: false,
            log_text: false,
            families: [
                ("meeting.topic", FamilyGate::shadow(0.85, 0.40)),
                ("meeting.brief", FamilyGate::shadow(0.80, 0.35)),
                ("browser.target", FamilyGate::shadow(0.90, 0.50)),
                ("browser.wait", FamilyGate::shadow(0.80, 0.30)),
                ("agent.readable", FamilyGate::shadow(0.85, 0.40)),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
        }
    }
}

fn duration<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
    let s = String::deserialize(d)?;
    humantime::parse_duration(&s).map_err(serde::de::Error::custom)
}

/// The plugin: reads `decision`, spawns [`Engine`] as [`DECISION_ACTOR`]. The actor is
/// registered even when disabled so callers get a fast `Escalate(Disabled)` instead of a
/// registry miss. No tool and no prompt block in this step (57 §6 step 7).
pub struct DecisionPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for DecisionPlugin {
    fn name(&self) -> &'static str {
        "decision"
    }

    async fn start(self: Box<Self>, rt: &runtime::Runtime) -> anyhow::Result<()> {
        let cfg: DecisionConfig = rt.section("decision").unwrap_or_else(|e| {
            tracing::debug!("no [decision] config section, using defaults: {e}");
            DecisionConfig::default()
        });
        if !cfg.enabled {
            tracing::debug!("decision disabled by config");
        }
        let model_dir = cfg
            .model_dir
            .clone()
            .unwrap_or_else(|| rt.data_dir.join("models").join(MODEL_SUBDIR));
        let log = log::Log::new(rt.data_dir.join("decisions"));
        Engine::spawn(Engine::new(cfg, model_dir, rt.data_dir.clone(), log))
            .register(DECISION_ACTOR)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_and_yaml() {
        let d = DecisionConfig::default();
        assert_eq!(d.families.len(), 5);
        assert!(d.families.values().all(|g| g.mode == Mode::Shadow));
        assert_eq!(d.families["browser.target"].tau_top, 0.90);
        let cfg: DecisionConfig = serde_yaml::from_str(
            "enabled: false\nidle_unload: 2h\nlog_text: true\nfamilies:\n  meeting.topic: { mode: active, tau_top: 0.7, tau_margin: 0.2 }\n",
        )
        .unwrap();
        assert!(!cfg.enabled);
        assert!(cfg.log_text);
        assert_eq!(cfg.idle_unload, Duration::from_secs(7200));
        assert_eq!(cfg.threads, 4);
        // Listing a family replaces the default table: only it runs.
        assert_eq!(cfg.families.len(), 1);
        assert_eq!(cfg.families["meeting.topic"].mode, Mode::Active);
    }
}
