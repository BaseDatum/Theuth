//! Decision plugin (design doc 57): a local typed-probability oracle — laya, a ModernBERT
//! encoder with a decision head, run natively in candle ([`candle`], Metal on Apple silicon,
//! CPU elsewhere) — answering [`Question`]s with a probability per option and a selective gate
//! ([`gate`]) that says `Accept` or `Escalate(reason)`. Callers keep today's reasoning-model
//! path on `Escalate`; every family ships in `shadow` until its log says otherwise.
//!
//! Public surface consumed by other plugins: messages on the [`DECISION_ACTOR`] actor
//! ([`Engine`]): [`Ask`] → [`Decision`], [`Report`], [`Stats`] → [`FamilyStats`].

pub mod candle;
pub mod engine;
pub mod escalate;
pub mod gate;
pub mod laya;
pub mod log;
pub mod question;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use kameo::actor::Spawn;
use serde::{Deserialize, Serialize};

pub use engine::{AllStats, Engine, GetMode, Loader, Oracle, Record, Report, SetMode, Stats, Warm};
pub use escalate::{EscalateError, SideCaller, escalate};
pub use gate::{FamilyGate, Mode};
pub use log::FamilyStats;
pub use question::{
    Ask, Candidate, Decision, Disposition, Family, LogId, Outcome, Question, Reason,
};

/// Registry name of the [`Engine`] actor.
pub const DECISION_ACTOR: &str = "decision";

/// The checkpoint under `<data_dir>/models/` (57 §2), published by `release-models.sh`: the
/// Hugging Face `convaiinnovations/laya` layout (`engine::MODEL_FILES`).
pub const MODEL_SUBDIR: &str = "decision";

/// Where the model runs (`decision.device`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Device {
    /// Metal on an Apple-silicon Mac, CPU elsewhere.
    #[default]
    Auto,
    /// CPU, f32.
    Cpu,
    /// Metal, f16; falls back to CPU with a warning when the GPU is unavailable.
    Metal,
}

impl Device {
    /// Whether this resolves to Metal on the running machine.
    pub fn metal(self) -> bool {
        match self {
            Device::Auto => cfg!(all(target_os = "macos", target_arch = "aarch64")),
            Device::Cpu => false,
            Device::Metal => true,
        }
    }
}

/// `decision:` config section (57 §2). Every field has a default; the section may be absent.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct DecisionConfig {
    /// `false`: the actor answers every `Ask` with `Escalate(Disabled)`.
    pub enabled: bool,
    /// Checkpoint directory (`model.safetensors`, `encoder/config.json`,
    /// `tokenizer/tokenizer.json`, `rl_agent_config.json`); `None` → `<data_dir>/models/decision`.
    pub model_dir: Option<PathBuf>,
    /// `auto` (Metal on an Apple-silicon Mac, else CPU) | `cpu` | `metal`.
    pub device: Device,
    /// Unload the model after this long unused (humantime, e.g. `10m`).
    #[serde(deserialize_with = "duration")]
    pub idle_unload: Duration,
    /// Store the question text in the log, not only its hash.
    pub log_text: bool,
    /// Gate per decision family; a family not listed never runs. The YAML is a patch over the
    /// default table: `meeting.kind: { mode: active }` changes one field; a prefix key
    /// (`meeting: { mode: off }`, what Settings → Meetings writes) patches every family under it.
    #[serde(deserialize_with = "families")]
    pub families: BTreeMap<String, FamilyGate>,
}

impl Default for DecisionConfig {
    fn default() -> Self {
        DecisionConfig {
            enabled: true,
            model_dir: None,
            device: Device::Auto,
            idle_unload: Duration::from_secs(600),
            log_text: false,
            families: default_families(),
        }
    }
}

impl DecisionConfig {
    /// `model_dir`, or `<data_dir>/models/decision`.
    pub fn model_dir_in(&self, data_dir: &Path) -> PathBuf {
        self.model_dir
            .clone()
            .unwrap_or_else(|| data_dir.join("models").join(MODEL_SUBDIR))
    }
}

/// 57 §3.5: `meeting.kind` is the small-talk `Bool` (skip at P(true) ≥ 0.6, the phrasing bench's
/// τ), `meeting.brief` the ≤ 4-candidate `Choice` (publish at P ≥ 0.6, margin ≥ 0.3), and
/// `meeting.subject` the tier-2 subject extraction (never asked of the oracle; listed so its
/// rows fold into `decision_stats` and the `meeting` prefix switch covers it). `meeting.topic`
/// (step 4's `Bool`) is gone: the bench showed it confidently wrong, not uncertain.
fn default_families() -> BTreeMap<String, FamilyGate> {
    [
        ("meeting.kind", FamilyGate::shadow(0.60, 0.20)),
        ("meeting.brief", FamilyGate::shadow(0.60, 0.30)),
        ("meeting.subject", FamilyGate::shadow(0.50, 0.0)),
        ("browser.target", FamilyGate::shadow(0.90, 0.50)),
        ("browser.wait", FamilyGate::shadow(0.80, 0.30)),
        ("agent.readable", FamilyGate::shadow(0.85, 0.40)),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect()
}

/// One YAML entry under `families:`: any subset of a [`FamilyGate`]'s fields.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FamilyPatch {
    mode: Option<Mode>,
    tau_top: Option<f32>,
    tau_margin: Option<f32>,
    #[serde(default, deserialize_with = "opt_duration")]
    budget: Option<Duration>,
    #[serde(default, deserialize_with = "opt_duration")]
    escalate_budget: Option<Duration>,
}

impl FamilyPatch {
    fn apply(&self, g: &mut FamilyGate) {
        if let Some(m) = self.mode {
            g.mode = m;
        }
        if let Some(t) = self.tau_top {
            g.tau_top = t;
        }
        if let Some(t) = self.tau_margin {
            g.tau_margin = t;
        }
        if let Some(b) = self.budget {
            g.budget = b;
        }
        if let Some(b) = self.escalate_budget {
            g.escalate_budget = b;
        }
    }
}

/// Patch the default table (57 §3.4): exact family names first, then prefix keys, so
/// `meeting: { mode: off }` + `meeting.kind: { mode: active }` still ends with `meeting.kind`
/// off — the switch wins over a hand edit. A new fully-specified family is inserted.
fn families<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<BTreeMap<String, FamilyGate>, D::Error> {
    let patches: BTreeMap<String, FamilyPatch> = BTreeMap::deserialize(d)?;
    let mut table = default_families();
    let mut prefixes = Vec::new();
    for (k, p) in &patches {
        if let Some(g) = table.get_mut(k) {
            p.apply(g);
        } else if let (Some(mode), Some(tau_top), Some(tau_margin)) =
            (p.mode, p.tau_top, p.tau_margin)
        {
            let mut g = FamilyGate::shadow(tau_top, tau_margin);
            g.mode = mode;
            p.apply(&mut g);
            table.insert(k.clone(), g);
        } else {
            prefixes.push((format!("{k}."), p));
        }
    }
    for (prefix, p) in prefixes {
        for (_, g) in table.iter_mut().filter(|(k, _)| k.starts_with(&prefix)) {
            p.apply(g);
        }
    }
    Ok(table)
}

fn duration<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
    let s = String::deserialize(d)?;
    humantime::parse_duration(&s).map_err(serde::de::Error::custom)
}

fn opt_duration<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Duration>, D::Error> {
    Option::<String>::deserialize(d)?
        .map(|s| humantime::parse_duration(&s).map_err(serde::de::Error::custom))
        .transpose()
}

/// What `GetMode { prefix }` answers (57 §3.4): the switch position for the families under a
/// prefix (`meeting` → `meeting.kind` + `meeting.brief` + `meeting.subject`) and what
/// effectively runs — `off` with a `reason` when the plugin is disabled or the checkpoint is
/// not on disk.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ModeInfo {
    /// The configured mode (the most permissive one when the families disagree).
    pub mode: Mode,
    /// What runs on the next ask.
    pub effective: Mode,
    /// Why `effective` is `off` while `mode` is not.
    pub reason: Option<String>,
    /// Tier-1 budget of the first family under the prefix, ms.
    pub budget_ms: u64,
    /// Tier-2 budget, ms.
    pub escalate_budget_ms: u64,
}

/// The switch position for the families under `prefix` (see [`ModeInfo`]).
pub fn mode_for(cfg: &DecisionConfig, model_dir: &Path, prefix: &str) -> ModeInfo {
    let dot = format!("{prefix}.");
    let gates: Vec<&FamilyGate> = cfg
        .families
        .iter()
        .filter(|(k, _)| k.starts_with(&dot) || *k == prefix)
        .map(|(_, g)| g)
        .collect();
    let rank = |m: Mode| match m {
        Mode::Off => 0,
        Mode::Shadow => 1,
        Mode::Active => 2,
    };
    let mode = gates
        .iter()
        .map(|g| g.mode)
        .max_by_key(|m| rank(*m))
        .unwrap_or(Mode::Off);
    let (budget, escalate) = gates.first().map_or(
        (Duration::from_millis(400), Duration::from_secs(3)),
        |g| (g.budget, g.escalate_budget),
    );
    let missing: Vec<&str> = engine::MODEL_FILES
        .into_iter()
        .filter(|f| !model_dir.join(f).is_file())
        .collect();
    let (effective, reason) = if mode == Mode::Off {
        (Mode::Off, None)
    } else if !cfg.enabled {
        (Mode::Off, Some("the decision plugin is disabled (decision.enabled: false)".to_owned()))
    } else if !missing.is_empty() {
        (
            Mode::Off,
            Some(format!(
                "model missing: {} not in {}",
                missing.join(", "),
                model_dir.display()
            )),
        )
    } else {
        (mode, None)
    };
    ModeInfo {
        mode,
        effective,
        reason,
        budget_ms: budget.as_millis() as u64,
        escalate_budget_ms: escalate.as_millis() as u64,
    }
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
        let model_dir = cfg.model_dir_in(&rt.data_dir);
        let log = log::Log::new(rt.data_dir.join("decisions"));
        let engine =
            Engine::new(cfg, model_dir, log).with_config_path(rt.data_dir.join("config.yaml"));
        Engine::spawn(engine).register(DECISION_ACTOR)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_and_yaml() {
        let d = DecisionConfig::default();
        assert_eq!(d.families.len(), 6);
        assert!(d.families.values().all(|g| g.mode == Mode::Shadow));
        assert!(!d.families.contains_key("meeting.topic"), "step 4's topic family is gone (57 §3.5)");
        assert_eq!(d.families["browser.target"].tau_top, 0.90);
        assert_eq!((d.families["meeting.kind"].tau_top, d.families["meeting.brief"].tau_margin), (0.60, 0.30));
        assert_eq!(d.families["meeting.kind"].budget, Duration::from_millis(400));
        assert_eq!(d.families["meeting.kind"].escalate_budget, Duration::from_secs(3));
        let cfg: DecisionConfig = serde_yaml::from_str(
            "enabled: false\nidle_unload: 2h\nlog_text: true\ndevice: cpu\nfamilies:\n  meeting.kind: { mode: active, tau_top: 0.7, tau_margin: 0.2, budget: 1s }\n  new.family: { mode: shadow, tau_top: 0.5, tau_margin: 0.1 }\n",
        )
        .unwrap();
        assert!(!cfg.enabled);
        assert!(cfg.log_text);
        assert_eq!(cfg.idle_unload, Duration::from_secs(7200));
        assert_eq!(cfg.device, Device::Cpu);
        assert!(!cfg.device.metal());
        // A listed family patches its default row; a fully specified new one is added.
        assert_eq!(cfg.families.len(), 7);
        assert_eq!(cfg.families["meeting.kind"].mode, Mode::Active);
        assert_eq!(cfg.families["meeting.kind"].tau_top, 0.7);
        assert_eq!(cfg.families["meeting.kind"].budget, Duration::from_secs(1));
        assert_eq!(cfg.families["meeting.brief"].mode, Mode::Shadow);
        assert_eq!(cfg.families["new.family"].tau_top, 0.5);
    }

    #[test]
    fn prefix_entry_sets_every_family_under_it_and_wins() {
        let cfg: DecisionConfig = serde_yaml::from_str(
            "families:\n  meeting.kind: { mode: active }\n  meeting: { mode: off }\n",
        )
        .unwrap();
        assert_eq!(cfg.families.len(), 6);
        assert_eq!(cfg.families["meeting.kind"].mode, Mode::Off);
        assert_eq!(cfg.families["meeting.brief"].mode, Mode::Off);
        assert_eq!(cfg.families["meeting.subject"].mode, Mode::Off);
        // Sky's `config.yaml` line from step 4 (`meeting: { mode: shadow }`) still lands on every family.
        let sky: DecisionConfig = serde_yaml::from_str("families:\n  meeting: { mode: shadow }\n").unwrap();
        assert!(sky.families.iter().filter(|(k, _)| k.starts_with("meeting.")).all(|(_, g)| g.mode == Mode::Shadow));
        assert_eq!(cfg.families["browser.target"].mode, Mode::Shadow);
        // A partial unknown family is neither inserted nor an error.
        let cfg: DecisionConfig =
            serde_yaml::from_str("families:\n  nobody: { mode: active }\n").unwrap();
        assert_eq!(cfg.families.len(), 6);
    }

    #[test]
    fn mode_resolution_forces_off_with_a_reason() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = DecisionConfig::default();
        let m = mode_for(&cfg, dir.path(), "meeting");
        assert_eq!(m.mode, Mode::Shadow);
        assert_eq!(m.effective, Mode::Off);
        assert!(m.reason.as_deref().unwrap().starts_with("model missing: model.safetensors"));
        assert_eq!((m.budget_ms, m.escalate_budget_ms), (400, 3000));

        for f in engine::MODEL_FILES {
            let p = dir.path().join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
        let m = mode_for(&cfg, dir.path(), "meeting");
        assert_eq!((m.mode, m.effective, m.reason), (Mode::Shadow, Mode::Shadow, None));

        let off: DecisionConfig =
            serde_yaml::from_str("families:\n  meeting: { mode: off }\n").unwrap();
        let m = mode_for(&off, dir.path(), "meeting");
        assert_eq!((m.mode, m.effective, m.reason), (Mode::Off, Mode::Off, None));

        let disabled: DecisionConfig = serde_yaml::from_str("enabled: false\n").unwrap();
        let m = mode_for(&disabled, dir.path(), "meeting");
        assert_eq!(m.effective, Mode::Off);
        assert!(m.reason.as_deref().unwrap().contains("disabled"));

        // Families disagree → the most permissive is the reported switch position.
        let mixed: DecisionConfig = serde_yaml::from_str(
            "families:\n  meeting.kind: { mode: active }\n  meeting.brief: { mode: off }\n",
        )
        .unwrap();
        assert_eq!(mode_for(&mixed, dir.path(), "meeting").mode, Mode::Active);
        assert_eq!(mode_for(&mixed, dir.path(), "nothing").effective, Mode::Off);
    }
}
