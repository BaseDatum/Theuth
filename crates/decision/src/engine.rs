//! The [`Engine`] actor (57 §2): one warm session behind one mailbox, lazy load on the
//! first `Ask`, inference in `spawn_blocking`, `Ask.budget` honoured (a late result is
//! still logged, never acted on), idle unload, every answer logged. A missing checkpoint
//! escalates `ModelMissing` and is re-checked on every `Ask` (the desktop app may download
//! it while the daemon runs).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use kameo::message::{Context, Message};
use parking_lot::Mutex;
use tokio::task::JoinHandle;

use crate::gate::{FamilyGate, Mode};
use crate::laya::{self, Loaded, Provider, RowOut, Sequence, Units};
use crate::{DecisionConfig, EngineKind};
use crate::log::{AskRow, FamilyStats, Log, Row};
use crate::question::{
    Ask, Candidate, Decision, Disposition, Family, LogId, Outcome, Question, Reason,
};

/// The checkpoint files the ORT engine needs in `model_dir` (`release-models.sh` REQUIRED).
pub const MODEL_FILES: [&str; 3] = ["model.onnx", "tokenizer.json", "rl_agent_config.json"];

/// The laya `safetensors` checkpoint the candle engine needs (57 §12.8; `candle::CANDLE_FILES`).
pub const CANDLE_FILES: [&str; 4] = [
    "model.safetensors",
    "encoder/config.json",
    "tokenizer/tokenizer.json",
    "rl_agent_config.json",
];

/// Which files [`Engine::missing_files`] checks for `engine`.
pub fn model_files(engine: EngineKind) -> &'static [&'static str] {
    match engine {
        EngineKind::Ort => &MODEL_FILES,
        EngineKind::Candle => &CANDLE_FILES,
    }
}

/// The mandatory negative control appended to every `Choice` (57 §0.4).
pub const NONE_ID: &str = "none";

/// What the engine runs: [`Loaded`] in production, a fake in the timeout test.
pub trait Oracle: Send {
    /// sha256 of the model artifact, for `gate_version`.
    fn model_sha(&self) -> &str;
    /// Render one question.
    fn build(&self, q: &Question) -> Result<Sequence, String>;
    /// One forward pass.
    fn run(&mut self, rows: &[Sequence]) -> anyhow::Result<Vec<RowOut>>;
    /// Cold-load wall clock, logged once.
    fn load_time(&self) -> Duration;
}

impl Oracle for Loaded {
    fn model_sha(&self) -> &str {
        &self.model_sha
    }
    fn build(&self, q: &Question) -> Result<Sequence, String> {
        Loaded::build(self, q)
    }
    fn run(&mut self, rows: &[Sequence]) -> anyhow::Result<Vec<RowOut>> {
        Loaded::run(self, rows)
    }
    fn load_time(&self) -> Duration {
        self.load_time
    }
}

/// Opens an [`Oracle`] from `(model_dir, data_dir, cfg)`; blocking.
pub type Loader =
    Arc<dyn Fn(&Path, &Path, &DecisionConfig) -> anyhow::Result<Box<dyn Oracle>> + Send + Sync>;

type Shared = Arc<Mutex<Box<dyn Oracle>>>;

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

/// `tell` from the idle watchdog: drop the session if nothing asked since.
#[derive(Clone, Copy, Debug)]
struct Unload;

/// The `decision` actor.
#[derive(kameo::Actor)]
pub struct Engine {
    cfg: DecisionConfig,
    model_dir: PathBuf,
    /// `<data_dir>`: the runtime's home (`models::onnx::ort_lib`).
    data_dir: PathBuf,
    log: Log,
    loader: Loader,
    /// Warm session, shared with in-flight blocking runs.
    loaded: Option<Shared>,
    /// A cold load that outran the asking budget; polled by the next `Ask`.
    loading: Option<JoinHandle<anyhow::Result<Box<dyn Oracle>>>>,
    last_used: Instant,
    watchdog: bool,
    /// `ModelMissing` is logged once, not per `Ask`.
    missing_logged: bool,
    /// Cold-load time, written into the next logged row.
    pending_load_ms: Option<u64>,
    /// The loaded model's sha, readable without taking the session lock.
    model_sha: String,
}

impl Engine {
    /// An engine over `cfg` with the real checkpoint ([`Loaded`] or `candle::CandleLoaded` by
    /// `cfg.engine`); nothing loads until the first `Ask`.
    pub fn new(cfg: DecisionConfig, model_dir: PathBuf, data_dir: PathBuf, log: Log) -> Self {
        Self::with_loader(cfg, model_dir, data_dir, log, Arc::new(open))
    }

    /// An engine whose checkpoint comes from `loader` (tests).
    pub fn with_loader(
        cfg: DecisionConfig,
        model_dir: PathBuf,
        data_dir: PathBuf,
        log: Log,
        loader: Loader,
    ) -> Self {
        Engine {
            cfg,
            model_dir,
            data_dir,
            log,
            loader,
            loaded: None,
            loading: None,
            last_used: Instant::now(),
            watchdog: false,
            missing_logged: false,
            pending_load_ms: None,
            model_sha: String::new(),
        }
    }

    /// The files that are not in the model dir, checked fresh each time.
    pub fn missing_files(&self) -> Vec<&'static str> {
        model_files(self.cfg.engine)
            .iter()
            .copied()
            .filter(|f| !self.model_dir.join(f).is_file())
            .collect()
    }

    fn model_sha(&self) -> String {
        self.model_sha.clone()
    }

    fn append(&self, row: Row) {
        if let Err(e) = self.log.append(&row) {
            tracing::warn!("decision log: {e}");
        }
    }

    /// A decision that never ran: no probabilities, `choice = none`.
    fn escalate(
        &self,
        ask: &Ask,
        gate: Option<&FamilyGate>,
        reason: Reason,
        t: Instant,
    ) -> Decision {
        let model_sha = self.model_sha();
        let d = Decision {
            family: ask.family.clone(),
            choice: NONE_ID.to_owned(),
            probabilities: Vec::new(),
            confidence: 0.0,
            act_probability: 0.0,
            disposition: Disposition::Escalate(reason),
            latency: t.elapsed(),
            gate_version: gate.map_or(0, |g| g.version(&model_sha)),
            log_id: LogId::new(),
        };
        let mode = gate.map_or(Mode::Off, |g| g.mode);
        let text = self.cfg.log_text.then(|| ask.question.clone());
        self.append(Row::Ask(Box::new(AskRow::new(
            &d,
            mode,
            &model_sha,
            &[],
            Vec::new(),
            0.0,
            text,
        ))));
        d
    }

    /// The session, loading it within `budget` if the files are there. `Err(reason)` = escalate.
    async fn session(&mut self, budget: Duration) -> Result<Shared, Reason> {
        if let Some(l) = &self.loaded {
            return Ok(l.clone());
        }
        let handle = match self.loading.take() {
            Some(h) => h,
            None => {
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
                    return Err(Reason::ModelMissing);
                }
                self.missing_logged = false;
                let (loader, dir, data_dir, cfg) = (
                    self.loader.clone(),
                    self.model_dir.clone(),
                    self.data_dir.clone(),
                    self.cfg.clone(),
                );
                tokio::task::spawn_blocking(move || loader(&dir, &data_dir, &cfg))
            }
        };
        let mut handle = handle;
        match tokio::time::timeout(budget, &mut handle).await {
            Ok(Ok(Ok(oracle))) => {
                let load = oracle.load_time();
                self.model_sha = oracle.model_sha().to_owned();
                tracing::info!(
                    "decision: model loaded in {load:?} from {} (sha256 {})",
                    self.model_dir.display(),
                    &self.model_sha[..12.min(self.model_sha.len())]
                );
                let shared: Shared = Arc::new(Mutex::new(oracle));
                self.loaded = Some(shared.clone());
                self.pending_load_ms = Some(load.as_millis() as u64);
                Ok(shared)
            }
            Ok(Ok(Err(e))) => {
                tracing::warn!("decision: model load failed: {e:#}");
                Err(Reason::HardError(format!("load: {e:#}")))
            }
            Ok(Err(e)) => Err(Reason::HardError(format!("load panicked: {e}"))),
            Err(_) => {
                // Still loading: keep the handle for the next ask, answer within budget.
                self.loading = Some(handle);
                Err(Reason::Timeout)
            }
        }
    }

    fn touch(&mut self, ctx: &Context<Self, Decision>) {
        self.last_used = Instant::now();
        if self.watchdog || self.cfg.idle_unload.is_zero() {
            return;
        }
        self.watchdog = true;
        let me = ctx.actor_ref().clone();
        let idle = self.cfg.idle_unload;
        tokio::spawn(async move {
            tokio::time::sleep(idle).await;
            let _ = me.tell(Unload).await;
        });
    }
}

/// The production [`Loader`]: ORT ([`Loaded`]) or candle by `cfg.engine`.
fn open(dir: &Path, data_dir: &Path, cfg: &DecisionConfig) -> anyhow::Result<Box<dyn Oracle>> {
    match cfg.engine {
        EngineKind::Ort => {
            let provider = if cfg.coreml {
                Provider::CoreMl(Units::All)
            } else {
                Provider::Cpu
            };
            Loaded::open(dir, data_dir, cfg.threads, provider).map(|l| Box::new(l) as Box<dyn Oracle>)
        }
        #[cfg(feature = "candle")]
        EngineKind::Candle => {
            let metal = cfg.device.metal();
            let dtype = if metal {
                candle_core::DType::F16
            } else {
                candle_core::DType::F32
            };
            crate::candle::CandleLoaded::open(dir, metal, dtype).map(|l| Box::new(l) as Box<dyn Oracle>)
        }
        #[cfg(not(feature = "candle"))]
        EngineKind::Candle => {
            anyhow::bail!("decision.engine = candle, but this build has no `candle` feature")
        }
    }
}

/// `none` appended once to a `Choice`; other questions unchanged.
pub fn with_none(q: Question) -> Question {
    match q {
        Question::Choice {
            instructions,
            mut candidates,
            state,
        } => {
            if !candidates.iter().any(|c| c.id == NONE_ID) {
                candidates.push(Candidate {
                    id: NONE_ID.to_owned(),
                    text: "none of these".to_owned(),
                });
            }
            Question::Choice {
                instructions,
                candidates,
                state,
            }
        }
        other => other,
    }
}

/// One run's result folded into a [`Decision`]: labels zipped with probabilities, argmax
/// choice, the gate's verdict (truncation is a hard error), then the family's mode.
fn decide(
    gate: &FamilyGate,
    family: &Family,
    q: &Question,
    seq: &Sequence,
    out: &RowOut,
    latency: Duration,
    model_sha: &str,
) -> Decision {
    let probabilities: Vec<(String, f32)> = laya::labels(q)
        .into_iter()
        .zip(out.probs.iter().copied())
        .collect();
    let choice = probabilities
        .iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map_or_else(|| NONE_ID.to_owned(), |(l, _)| l.clone());
    let hard = seq.truncated.then_some(Reason::Truncated);
    let d = Decision {
        family: family.clone(),
        choice,
        confidence: out.confidence,
        act_probability: out.act_p,
        disposition: gate.decide(&probabilities, hard),
        probabilities,
        latency,
        gate_version: gate.version(model_sha),
        log_id: LogId::new(),
    };
    gate.finish(d)
}

impl Message<Ask> for Engine {
    type Reply = Decision;

    async fn handle(&mut self, ask: Ask, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let t = Instant::now();
        if !self.cfg.enabled {
            return self.escalate(&ask, None, Reason::Disabled, t);
        }
        // Unknown families fail closed: only configured gates run (57 §0.5).
        let gate = self.cfg.families.get(&ask.family.0).copied();
        let Some(gate) = gate.filter(|g| g.mode != Mode::Off) else {
            return self.escalate(&ask, gate.as_ref(), Reason::Disabled, t);
        };
        let shared = match self.session(ask.budget).await {
            Ok(s) => s,
            Err(reason) => return self.escalate(&ask, Some(&gate), reason, t),
        };
        self.touch(ctx);

        let q = with_none(ask.question.clone());
        let mut run = {
            let (shared, q) = (shared.clone(), q.clone());
            tokio::task::spawn_blocking(move || -> Result<(Sequence, RowOut, String), Reason> {
                let mut o = shared.lock();
                let seq = o.build(&q).map_err(Reason::HardError)?;
                let out = o
                    .run(std::slice::from_ref(&seq))
                    .map_err(|e| Reason::HardError(format!("{e:#}")))?;
                let out = out
                    .into_iter()
                    .next()
                    .ok_or_else(|| Reason::HardError("no row".into()))?;
                Ok((seq, out, o.model_sha().to_owned()))
            })
        };
        let text = self.cfg.log_text.then(|| ask.question.clone());
        let elapsed = ask.budget.saturating_sub(t.elapsed());
        match tokio::time::timeout(elapsed, &mut run).await {
            Ok(Ok(Ok((seq, out, sha)))) => {
                let d = decide(&gate, &ask.family, &q, &seq, &out, t.elapsed(), &sha);
                let mut row = AskRow::new(
                    &d,
                    gate.mode,
                    &sha,
                    &seq.input_ids,
                    out.logits.clone(),
                    out.temperature,
                    text,
                );
                row.load_ms = self.pending_load_ms.take();
                self.append(Row::Ask(Box::new(row)));
                d
            }
            Ok(Ok(Err(reason))) => self.escalate(&ask, Some(&gate), reason, t),
            Ok(Err(e)) => self.escalate(
                &ask,
                Some(&gate),
                Reason::HardError(format!("run panicked: {e}")),
                t,
            ),
            Err(_) => {
                // Over budget: answer now, log the late result when it lands (57 §2). The
                // late row keeps the real probabilities under `timeout`, so fitting sees them.
                let (log, family, mode, sha) = (
                    self.log.clone(),
                    ask.family.clone(),
                    gate.mode,
                    self.model_sha(),
                );
                tokio::spawn(async move {
                    let row = match run.await {
                        Ok(Ok((seq, out, sha))) => {
                            let mut d = decide(&gate, &family, &q, &seq, &out, t.elapsed(), &sha);
                            d.disposition = Disposition::Escalate(Reason::Timeout);
                            AskRow::new(
                                &d,
                                mode,
                                &sha,
                                &seq.input_ids,
                                out.logits,
                                out.temperature,
                                text,
                            )
                        }
                        Ok(Err(reason)) => {
                            let d = Decision {
                                family,
                                choice: NONE_ID.to_owned(),
                                probabilities: Vec::new(),
                                confidence: 0.0,
                                act_probability: 0.0,
                                disposition: Disposition::Escalate(reason),
                                latency: t.elapsed(),
                                gate_version: gate.version(&sha),
                                log_id: LogId::new(),
                            };
                            AskRow::new(&d, mode, &sha, &[], Vec::new(), 0.0, text)
                        }
                        Err(_) => return,
                    };
                    if let Err(e) = log.append(&Row::Ask(Box::new(row))) {
                        tracing::warn!("decision log: {e}");
                    }
                });
                Decision {
                    family: ask.family.clone(),
                    choice: NONE_ID.to_owned(),
                    probabilities: Vec::new(),
                    confidence: 0.0,
                    act_probability: 0.0,
                    disposition: Disposition::Escalate(Reason::Timeout),
                    latency: t.elapsed(),
                    gate_version: gate.version(&self.model_sha()),
                    log_id: LogId::new(),
                }
            }
        }
    }
}

impl Message<Unload> for Engine {
    type Reply = ();

    async fn handle(&mut self, _: Unload, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let idle = self.cfg.idle_unload;
        let since = self.last_used.elapsed();
        if since >= idle {
            self.watchdog = false;
            if self.loaded.take().is_some() {
                tracing::info!("decision: session unloaded after {since:?} idle");
            }
        } else {
            let me = ctx.actor_ref().clone();
            tokio::spawn(async move {
                tokio::time::sleep(idle - since).await;
                let _ = me.tell(Unload).await;
            });
        }
    }
}

impl Message<Report> for Engine {
    type Reply = ();

    async fn handle(&mut self, r: Report, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.append(Row::Outcome {
            log_id: r.log_id,
            ts: chrono::Utc::now(),
            outcome: r.outcome,
        });
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
    use crate::laya::{CLS, MASK, SEP};

    fn ask(family: &str) -> Ask {
        Ask::new(
            family,
            Question::Bool {
                statement: "the speakers name a ticket".into(),
                state: "we should look at SRE-2386".into(),
            },
        )
    }

    fn touch_files(model_dir: &Path) {
        std::fs::create_dir_all(model_dir).unwrap();
        for f in MODEL_FILES {
            std::fs::write(model_dir.join(f), b"x").unwrap();
        }
    }

    /// Answers `[0.1, 0.9]` for two options after sleeping `delay`.
    struct Fake {
        delay: Duration,
    }

    impl Oracle for Fake {
        fn model_sha(&self) -> &str {
            "fake"
        }
        fn build(&self, q: &Question) -> Result<Sequence, String> {
            let k = laya::labels(q).len();
            let mut ids = vec![CLS, 7, SEP];
            let markers: Vec<i64> = (0..k)
                .map(|_| {
                    let m = ids.len() as i64;
                    ids.extend([MASK, 9]);
                    m
                })
                .collect();
            ids.extend([SEP, 11, SEP]);
            Ok(Sequence {
                attention_mask: vec![1; ids.len()],
                marker_mask: vec![true; k],
                input_ids: ids,
                marker_pos: markers,
                qtype: 2,
                truncated: false,
            })
        }
        fn run(&mut self, rows: &[Sequence]) -> anyhow::Result<Vec<RowOut>> {
            std::thread::sleep(self.delay);
            Ok(rows
                .iter()
                .map(|r| {
                    let k = r.marker_pos.len();
                    let mut probs = vec![0.1 / (k.max(2) - 1) as f32; k];
                    probs[k - 1] = 0.9;
                    RowOut {
                        logits: vec![0.0; k],
                        probs,
                        temperature: 1.0,
                        act_p: 0.5,
                        confidence: 0.9,
                    }
                })
                .collect())
        }
        fn load_time(&self) -> Duration {
            Duration::from_millis(1)
        }
    }

    fn fake_loader(delay: Duration) -> Loader {
        Arc::new(move |_, _, _| Ok(Box::new(Fake { delay }) as Box<dyn Oracle>))
    }

    #[tokio::test]
    async fn missing_model_escalates_and_is_rechecked() {
        let dir = tempfile::tempdir().unwrap();
        let model_dir = dir.path().join("models/decision");
        let log = Log::new(dir.path().join("decisions"));
        let cfg = DecisionConfig {
            families: [("meeting.topic".to_owned(), FamilyGate::shadow(0.85, 0.4))].into(),
            ..DecisionConfig::default()
        };
        let engine = Engine::spawn(Engine::with_loader(
            cfg,
            model_dir.clone(),
            dir.path().into(),
            log.clone(),
            fake_loader(Duration::ZERO),
        ));

        let d = engine.ask(ask("meeting.topic")).await.unwrap();
        assert_eq!(d.disposition, Disposition::Escalate(Reason::ModelMissing));
        assert_eq!(d.choice, "none");
        assert!(d.probabilities.is_empty());

        // Unknown family: fail closed.
        let d = engine.ask(ask("nope")).await.unwrap();
        assert_eq!(d.disposition, Disposition::Escalate(Reason::Disabled));

        // Files land while running: the next Ask loads and runs; shadow wraps an accept.
        touch_files(&model_dir);
        let d = engine.ask(ask("meeting.topic")).await.unwrap();
        assert_eq!(d.choice, "true");
        assert_eq!(
            d.probabilities,
            vec![("false".to_owned(), 0.1), ("true".to_owned(), 0.9)]
        );
        assert!(
            matches!(&d.disposition, Disposition::Shadow(inner) if inner.disposition == Disposition::Accept)
        );
        assert_ne!(d.gate_version, 0);

        engine
            .tell(Report {
                log_id: d.log_id,
                outcome: Outcome::Confirmed,
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
        assert_eq!(s.accepted, 1);
        assert_eq!(s.escalations.get("model_missing"), Some(&1));
        assert_eq!(s.confirmed, 1);
        assert_eq!(s.agreement, Some(1.0));
    }

    #[tokio::test]
    async fn choice_gets_none_once_and_active_accepts() {
        let dir = tempfile::tempdir().unwrap();
        touch_files(dir.path());
        let cfg = DecisionConfig {
            families: [(
                "browser.target".to_owned(),
                FamilyGate {
                    mode: Mode::Active,
                    tau_top: 0.8,
                    tau_margin: 0.5,
                },
            )]
            .into(),
            ..DecisionConfig::default()
        };
        let engine = Engine::spawn(Engine::with_loader(
            cfg,
            dir.path().into(),
            dir.path().into(),
            Log::new(dir.path().join("d")),
            fake_loader(Duration::ZERO),
        ));
        let q = Question::Choice {
            instructions: "which".into(),
            candidates: vec![
                Candidate {
                    id: "e3".into(),
                    text: "Sign in".into(),
                },
                Candidate {
                    id: "e7".into(),
                    text: "Help".into(),
                },
            ],
            state: "page".into(),
        };
        let d = engine
            .ask(Ask::new("browser.target", q.clone()))
            .await
            .unwrap();
        let labels: Vec<&str> = d.probabilities.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["e3", "e7", "none"]);
        assert_eq!(d.choice, "none");
        assert_eq!(d.disposition, Disposition::Accept);
        // A caller-supplied `none` is not doubled.
        let with = with_none(with_none(q));
        let Question::Choice { candidates, .. } = with else {
            unreachable!()
        };
        assert_eq!(candidates.iter().filter(|c| c.id == "none").count(), 1);
        assert_eq!(candidates.len(), 3);
    }

    #[tokio::test]
    async fn over_budget_escalates_timeout_and_logs_late() {
        let dir = tempfile::tempdir().unwrap();
        touch_files(dir.path());
        let log = Log::new(dir.path().join("d"));
        let cfg = DecisionConfig {
            families: [("meeting.topic".to_owned(), FamilyGate::shadow(0.85, 0.4))].into(),
            ..DecisionConfig::default()
        };
        let engine = Engine::spawn(Engine::with_loader(
            cfg,
            dir.path().into(),
            dir.path().into(),
            log.clone(),
            fake_loader(Duration::from_millis(300)),
        ));
        let mut a = ask("meeting.topic");
        a.budget = Duration::from_millis(50);
        let t = Instant::now();
        let d = engine.ask(a).await.unwrap();
        assert_eq!(d.disposition, Disposition::Escalate(Reason::Timeout));
        assert!(
            t.elapsed() < Duration::from_millis(250),
            "{:?}",
            t.elapsed()
        );
        // The late result still lands in the log with the timeout reason and real probabilities.
        tokio::time::sleep(Duration::from_millis(400)).await;
        let s = engine
            .ask(Stats {
                family: "meeting.topic".into(),
            })
            .await
            .unwrap();
        assert_eq!(s.asks, 1);
        assert_eq!(s.escalations.get("timeout"), Some(&1));
        let file = std::fs::read_dir(dir.path().join("d"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let text = std::fs::read_to_string(file).unwrap();
        assert!(
            text.contains("\"probs\":[[\"false\",0.1],[\"true\",0.9]]"),
            "{text}"
        );
        // Fast enough now: a normal answer.
        let mut a = ask("meeting.topic");
        a.budget = Duration::from_secs(5);
        let d = engine.ask(a).await.unwrap();
        assert!(matches!(d.disposition, Disposition::Shadow(_)));
    }

    #[tokio::test]
    async fn idle_unload_drops_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        touch_files(dir.path());
        let cfg = DecisionConfig {
            idle_unload: Duration::from_millis(100),
            families: [("meeting.topic".to_owned(), FamilyGate::shadow(0.85, 0.4))].into(),
            ..DecisionConfig::default()
        };
        let loads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let n = loads.clone();
        let loader: Loader = Arc::new(move |_, _, _| {
            n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Box::new(Fake {
                delay: Duration::ZERO,
            }) as Box<dyn Oracle>)
        });
        let engine = Engine::spawn(Engine::with_loader(
            cfg,
            dir.path().into(),
            dir.path().into(),
            Log::new(dir.path().join("d")),
            loader,
        ));
        engine.ask(ask("meeting.topic")).await.unwrap();
        engine.ask(ask("meeting.topic")).await.unwrap();
        assert_eq!(loads.load(std::sync::atomic::Ordering::SeqCst), 1);
        tokio::time::sleep(Duration::from_millis(350)).await;
        engine.ask(ask("meeting.topic")).await.unwrap();
        assert_eq!(loads.load(std::sync::atomic::Ordering::SeqCst), 2);
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
            dir.path().into(),
            Log::new(dir.path().join("d")),
        ));
        let d = engine.ask(ask("meeting.topic")).await.unwrap();
        assert_eq!(d.disposition, Disposition::Escalate(Reason::Disabled));
    }
}
