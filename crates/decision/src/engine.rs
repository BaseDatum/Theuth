//! The [`Engine`] actor (57 §2): one warm session behind one mailbox, lazy load, idle
//! unload, every answer logged. Step 1 ships the mailbox and the gate; the ort `Session`
//! and `spawn_blocking` inference arrive in step 2 — until then a present checkpoint
//! still escalates (`HardError`), a missing one escalates `ModelMissing` and is re-checked
//! on every `Ask` (the desktop app may download it while the daemon runs).

use std::path::PathBuf;
use std::time::Instant;

use kameo::message::{Context, Message};

use crate::DecisionConfig;
use crate::gate::{FamilyGate, Mode};
use crate::laya::LayaConfig;
use crate::log::{AskRow, FamilyStats, Log, Row};
use crate::question::{Ask, Decision, Disposition, Family, LogId, Outcome, Reason};

/// The checkpoint files [`Engine`] needs in `model_dir` (`release-models.sh` REQUIRED).
pub const MODEL_FILES: [&str; 3] = ["model.onnx", "tokenizer.json", "rl_agent_config.json"];

/// The loaded checkpoint. Step 2 adds the ort `Session`.
pub struct Loaded {
    /// laya's fast BPE tokenizer.
    pub tokenizer: tokenizers::Tokenizer,
    /// Temperatures and budgets.
    pub laya: LayaConfig,
    /// sha256 of `model.onnx`, part of every gate version.
    pub model_sha: String,
    /// Last `Ask`, for idle unload.
    pub last_used: Instant,
}

/// `tell`: what happened after a decision, appended to the log against `log_id`.
#[derive(Clone, Debug)]
pub struct Report {
    /// The decision's id.
    pub log_id: LogId,
    /// What happened.
    pub outcome: Outcome,
}

/// `ask` → [`FamilyStats`]: today's log folded for one family.
#[derive(Clone, Debug)]
pub struct Stats {
    /// Family.
    pub family: Family,
}

/// The `decision` actor.
#[derive(kameo::Actor)]
pub struct Engine {
    cfg: DecisionConfig,
    model_dir: PathBuf,
    log: Log,
    /// `None` until the first `Ask` finds the checkpoint (step 2 fills it).
    pub loaded: Option<Loaded>,
    /// `ModelMissing` is logged once, not per `Ask`.
    missing_logged: bool,
}

impl Engine {
    /// An engine over `cfg`; nothing is loaded until the first `Ask`.
    pub fn new(cfg: DecisionConfig, model_dir: PathBuf, log: Log) -> Self {
        Engine {
            cfg,
            model_dir,
            log,
            loaded: None,
            missing_logged: false,
        }
    }

    /// The files that are not in the model dir, checked fresh each time.
    pub fn missing_files(&self) -> Vec<&'static str> {
        MODEL_FILES
            .into_iter()
            .filter(|f| !self.model_dir.join(f).is_file())
            .collect()
    }

    fn escalate(
        &self,
        ask: &Ask,
        gate: Option<&FamilyGate>,
        reason: Reason,
        t: Instant,
    ) -> Decision {
        let model_sha = self.loaded.as_ref().map_or("", |l| l.model_sha.as_str());
        let d = Decision {
            family: ask.family.clone(),
            choice: "none".to_owned(),
            probabilities: Vec::new(),
            confidence: 0.0,
            act_probability: 0.0,
            disposition: Disposition::Escalate(reason),
            latency: t.elapsed(),
            gate_version: gate.map_or(0, |g| g.version(model_sha)),
            log_id: LogId::new(),
        };
        let mode = gate.map_or(Mode::Off, |g| g.mode);
        let text = self.cfg.log_text.then(|| ask.question.clone());
        if let Err(e) = self.log.append(&Row::Ask(Box::new(AskRow::new(
            &d,
            mode,
            model_sha,
            &[],
            Vec::new(),
            0.0,
            text,
        )))) {
            tracing::warn!("decision log: {e}");
        }
        d
    }
}

impl Message<Ask> for Engine {
    type Reply = Decision;

    async fn handle(&mut self, ask: Ask, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let t = Instant::now();
        if !self.cfg.enabled {
            return self.escalate(&ask, None, Reason::Disabled, t);
        }
        // Unknown families fail closed: only configured gates run (57 §0.5).
        let gate = self.cfg.families.get(&ask.family.0).copied();
        let Some(gate) = gate.filter(|g| g.mode != Mode::Off) else {
            return self.escalate(&ask, gate.as_ref(), Reason::Disabled, t);
        };
        let missing = self.missing_files();
        if !missing.is_empty() {
            if !self.missing_logged {
                tracing::info!(
                    "decision: model missing in {} ({}); every ask escalates until it lands",
                    self.model_dir.display(),
                    missing.join(", ")
                );
                self.missing_logged = true;
            }
            return self.escalate(&ask, Some(&gate), Reason::ModelMissing, t);
        }
        // step 2: load `Loaded` in spawn_blocking on first use, build_sequence, run the
        // session, softmax_with_temperature, gate.decide, gate.finish, log the full row.
        self.escalate(
            &ask,
            Some(&gate),
            Reason::HardError("inference not built yet (57 §6 step 2)".to_owned()),
            t,
        )
    }
}

impl Message<Report> for Engine {
    type Reply = ();

    async fn handle(&mut self, r: Report, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if let Err(e) = self.log.append(&Row::Outcome {
            log_id: r.log_id,
            ts: chrono::Utc::now(),
            outcome: r.outcome,
        }) {
            tracing::warn!("decision log: {e}");
        }
    }
}

impl Message<Stats> for Engine {
    type Reply = FamilyStats;

    async fn handle(&mut self, s: Stats, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.log.stats(&s.family).unwrap_or_else(|e| {
            tracing::warn!("decision log: {e}");
            FamilyStats {
                family: s.family.0,
                ..FamilyStats::default()
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use kameo::actor::Spawn;

    use super::*;
    use crate::question::Question;

    fn ask(family: &str) -> Ask {
        Ask::new(
            family,
            Question::Bool {
                statement: "the speakers name a ticket".into(),
                state: "we should look at SRE-2386".into(),
            },
        )
    }

    #[tokio::test]
    async fn missing_model_escalates_and_is_rechecked() {
        let dir = tempfile::tempdir().unwrap();
        let model_dir = dir.path().join("models/decision");
        let log = Log::new(dir.path().join("decisions"));
        let engine = Engine::spawn(Engine::new(
            DecisionConfig::default(),
            model_dir.clone(),
            log.clone(),
        ));

        let d = engine.ask(ask("meeting.topic")).await.unwrap();
        assert_eq!(d.disposition, Disposition::Escalate(Reason::ModelMissing));
        assert_eq!(d.choice, "none");
        assert!(d.probabilities.is_empty());

        // Unknown family: fail closed.
        let d = engine.ask(ask("nope")).await.unwrap();
        assert_eq!(d.disposition, Disposition::Escalate(Reason::Disabled));

        // Files land while running: the next Ask sees them (and step 1 still escalates, honestly).
        std::fs::create_dir_all(&model_dir).unwrap();
        for f in MODEL_FILES {
            std::fs::write(model_dir.join(f), b"x").unwrap();
        }
        let d = engine.ask(ask("meeting.topic")).await.unwrap();
        assert!(matches!(
            d.disposition,
            Disposition::Escalate(Reason::HardError(_))
        ));

        engine
            .tell(Report {
                log_id: d.log_id,
                outcome: Outcome::Unknown,
            })
            .await
            .unwrap();
        let s = engine
            .ask(Stats {
                family: "meeting.topic".into(),
            })
            .await
            .unwrap();
        assert_eq!(s.asks, 2);
        assert_eq!(s.escalations.get("model_missing"), Some(&1));
        assert_eq!(s.escalations.get("hard_error"), Some(&1));
        assert_eq!(s.unknown, 1);
    }

    #[tokio::test]
    async fn disabled_plugin_escalates_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = DecisionConfig {
            enabled: false,
            ..DecisionConfig::default()
        };
        let engine = Engine::spawn(Engine::new(
            cfg,
            dir.path().into(),
            Log::new(dir.path().join("d")),
        ));
        let d = engine.ask(ask("meeting.topic")).await.unwrap();
        assert_eq!(d.disposition, Disposition::Escalate(Reason::Disabled));
    }
}
