//! The [`Engine`] actor (57 §2): one warm session behind one mailbox, lazy load on the
//! first `Ask`, inference in `spawn_blocking`, `Ask.budget` honoured (a late result is
//! still logged, never acted on), idle unload, every answer logged. A missing checkpoint
//! escalates `ModelMissing` and is re-checked on every `Ask` (the desktop app may download
//! it while the daemon runs). One engine per checkpoint (57 §4.1 item 6): an `Ask` whose
//! family names another checkpoint is forwarded to that engine's actor ([`crate::actor_name`]).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;

use crate::DecisionConfig;
use crate::candle::Loaded;
use crate::gate::{FamilyGate, Mode};
use crate::laya::{self, RowOut, Sequence};
use crate::log::{AskRow, FamilyStats, Log, Row};
use crate::question::{
    Ask, Candidate, Decision, Disposition, Family, LogId, Outcome, Question, Reason,
};
use crate::training::{self, Store, TrainingInfo};

/// The checkpoint files [`Engine`] needs in `model_dir` — the Hugging Face
/// `convaiinnovations/laya` layout, `release-models.sh` REQUIRED.
pub const MODEL_FILES: [&str; 4] = [
    "model.safetensors",
    "encoder/config.json",
    "tokenizer/tokenizer.json",
    "rl_agent_config.json",
];

/// The mandatory negative control appended to every `Choice` (57 §0.4).
pub const NONE_ID: &str = "none";

/// The model's identity for `gate_version` and the log, shared with the oracle that fills it:
/// `size:<bytes>` until the background sha256 lands, then the hex. Rows written in between
/// carry the size tag (and a `gate_version` derived from it); the fold does not care.
pub type ModelSha = Arc<Mutex<String>>;

/// What the engine runs: [`Loaded`] in production, a fake in the timeout test.
pub trait Oracle: Send {
    /// The model's identity, see [`ModelSha`].
    fn model_sha(&self) -> ModelSha;
    /// Render one question.
    fn build(&self, q: &Question) -> Result<Sequence, String>;
    /// One forward pass.
    fn run(&mut self, rows: &[Sequence]) -> anyhow::Result<Vec<RowOut>>;
    /// Cold-load wall clock, logged once.
    fn load_time(&self) -> Duration;
}

/// Opens an [`Oracle`] from `(model_dir, cfg)`; blocking.
pub type Loader =
    Arc<dyn Fn(&Path, &DecisionConfig) -> anyhow::Result<Box<dyn Oracle>> + Send + Sync>;

type Shared = Arc<Mutex<Box<dyn Oracle>>>;

/// `ask` → `Vec<Decision>`, one per ask in order (57 §4.1 item 1): every question rendered,
/// **one** collate and **one** forward pass, then the per-question gate and log row exactly as
/// for a single [`Ask`]. All asks must name the same checkpoint (the first one's is used; any
/// other escalates `HardError`); the budget is the largest one asked. `browser_run` asks its
/// three questions this way; the meeting oracle's two asks are two round trips today.
#[derive(Clone, Debug)]
pub struct AskMany {
    /// The questions, answered in this order.
    pub asks: Vec<Ask>,
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

/// `ask` → `Vec<FamilyStats>`: every configured family, for `decision_stats` (57 §7).
#[derive(Clone, Copy, Debug, Default)]
pub struct AllStats;

/// `ask` → [`ModeInfo`]: the switch position for the families under `prefix` (57 §3.4).
#[derive(Clone, Debug)]
pub struct GetMode {
    /// `meeting` covers `meeting.topic` and `meeting.brief`.
    pub prefix: String,
}

/// `ask` → `Result<ModeInfo, String>`: set the mode of every family under `prefix`, written to
/// `config.yaml` (`decision.families.<prefix>: { mode: … }`) first, then live — effective from
/// the next `Ask`. A failed write changes nothing.
#[derive(Clone, Debug)]
pub struct SetMode {
    /// See [`GetMode::prefix`].
    pub prefix: String,
    /// The new mode.
    pub mode: Mode,
}

/// `tell`: a decision made outside the engine — `escalate`'s tier-2 answer — appended to the
/// log as an ask row (`tier: 2`, no logits, the mode of its gate). `note` is the one place the
/// log carries text by design: a short subject string from tier-2 subject extraction (57 §3.5).
#[derive(Clone, Debug)]
pub struct Record {
    /// The decision.
    pub decision: Decision,
    /// A short annotation (`AskRow::note`), e.g. the extracted subject.
    pub note: Option<String>,
}

/// `ask` → [`TrainingInfo`]: whether the families under `prefix` capture training examples
/// (57 §11.4 item 3), and how much is on disk.
#[derive(Clone, Debug)]
pub struct GetTrain {
    /// See [`GetMode::prefix`].
    pub prefix: String,
}

/// `ask` → `Result<TrainingInfo, String>`: set `train` on every family under `prefix`, written
/// to `config.yaml` (`decision.families.<prefix>: { mode: …, train: … }`) first, then live. A
/// failed write changes nothing.
#[derive(Clone, Debug)]
pub struct SetTrain {
    /// See [`GetMode::prefix`].
    pub prefix: String,
    /// Capture on or off.
    pub on: bool,
}

/// `tell`: one training row for `prefix` (57 §11.4). Dropped unless a family under `prefix`
/// has `train: true` *now* — flipping it off stops writes at once. Example rows get the
/// engine's `model_sha`; every row is secret-scrubbed by the store.
#[derive(Clone, Debug)]
pub struct Capture {
    /// The family prefix (`meeting`).
    pub prefix: String,
    /// The row.
    pub row: training::Row,
}

/// `ask` → `Result<u64, String>`: load the session (checkpoint present, plugin enabled) and run
/// one throwaway forward so a meeting's first real `Ask` pays neither the cold load nor the
/// first-inference cost (57 §3.5 item 5). Nothing is logged; the reply is the wall clock in ms.
#[derive(Clone, Copy, Debug)]
pub struct Warm {
    /// How long to wait for the load.
    pub budget: Duration,
}

/// `tell`: drop the warm session now (57 §2.1 item 1) — the meeting sends it when recording
/// stops, the browser when a session that ran `browser_run` closes. In-flight blocking runs
/// keep their `Arc` and finish; a cold load still in flight is dropped on landing; the idle
/// watchdog is disarmed. `checkpoint: None` = this engine's own; `Some(name)` is forwarded to
/// that checkpoint's engine. Nothing loaded → nothing happens.
#[derive(Clone, Debug)]
pub struct Release {
    /// Which checkpoint; `None` = the engine addressed.
    pub checkpoint: Option<String>,
    /// For the log line: `decision: <name> released (<reason>)`.
    pub reason: &'static str,
}

/// `ask` → `Vec<LoadedInfo>`: the checkpoints resident right now (57 §2.1 item 4). The
/// `default` engine answers for every checkpoint (it asks its siblings); another engine only
/// for itself.
#[derive(Clone, Copy, Debug, Default)]
pub struct LoadedNow;

/// One resident checkpoint, for `decision_loaded` and the Settings line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct LoadedInfo {
    /// Checkpoint name (`default`, `browser`).
    pub checkpoint: String,
    /// Seconds since the session was installed.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub since_secs: u64,
    /// The **whole daemon's** memory in MB, not this checkpoint's share: one number, the same
    /// on every entry (57 §2.1 item 4). macOS `phys_footprint` (Activity Monitor's Memory);
    /// resident set on Linux — see [`crate::ProcessMemory`].
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub rss_mb: u64,
}

/// `tell` from the idle watchdog: drop the session if nothing asked since. The epoch names
/// the arming [`Engine::touch`]; a tick from an earlier arming (before a `Release`) is stale.
#[derive(Clone, Copy, Debug)]
struct Unload(u64);

/// The `decision` actor — one per checkpoint.
#[derive(kameo::Actor)]
pub struct Engine {
    cfg: DecisionConfig,
    /// This engine's checkpoint name (`default` unless [`Engine::with_checkpoint`]).
    checkpoint: String,
    /// This engine's checkpoint directory.
    model_dir: PathBuf,
    /// The `default` checkpoint directory, which `mode_for` resolves the others from.
    default_dir: PathBuf,
    /// This checkpoint's idle unload ([`DecisionConfig::idle_unload_for`]).
    idle: Duration,
    log: Log,
    /// `<data_dir>/decisions/training`, beside the log.
    training: Store,
    loader: Loader,
    /// `config.yaml`, for `SetMode`; `None` = live only (tests, non-default checkpoints).
    config_path: Option<PathBuf>,
    /// Warm session, shared with in-flight blocking runs.
    loaded: Option<Shared>,
    /// When `loaded` was installed.
    loaded_since: Option<Instant>,
    /// A cold load that outran the asking budget; polled by the next `Ask`.
    loading: Option<JoinHandle<anyhow::Result<Box<dyn Oracle>>>>,
    last_used: Instant,
    /// The armed watchdog's epoch; `None` = not armed.
    watchdog: Option<u64>,
    /// Armings so far; each tick carries the epoch that armed it.
    epochs: u64,
    /// `ModelMissing` is logged once, not per `Ask`.
    missing_logged: bool,
    /// Cold-load time, written into the next logged row.
    pending_load_ms: Option<u64>,
    /// The loaded model's identity, readable without taking the session lock.
    model_sha: ModelSha,
}

impl Engine {
    /// The `default` engine over `cfg` with the real [`Loaded`]; nothing loads until the
    /// first `Ask`.
    pub fn new(cfg: DecisionConfig, model_dir: PathBuf, log: Log) -> Self {
        Self::with_loader(cfg, model_dir, log, Arc::new(open))
    }

    /// An engine whose checkpoint comes from `loader` (tests).
    pub fn with_loader(cfg: DecisionConfig, model_dir: PathBuf, log: Log, loader: Loader) -> Self {
        Engine {
            idle: cfg.idle_unload_for(crate::DEFAULT_CHECKPOINT),
            cfg,
            checkpoint: crate::DEFAULT_CHECKPOINT.to_owned(),
            default_dir: model_dir.clone(),
            model_dir,
            training: Store::new(log.dir().join("training")),
            log,
            loader,
            config_path: None,
            loaded: None,
            loaded_since: None,
            loading: None,
            last_used: Instant::now(),
            watchdog: None,
            epochs: 0,
            missing_logged: false,
            pending_load_ms: None,
            model_sha: Arc::default(),
        }
    }

    /// Make this the engine of checkpoint `name` living in `dir` (57 §4.1 item 6); `new`'s
    /// `model_dir` stays the `default` one for `mode_for`. The idle unload becomes `name`'s.
    pub fn with_checkpoint(mut self, name: &str, dir: PathBuf) -> Self {
        self.checkpoint = name.to_owned();
        self.model_dir = dir;
        self.idle = self.cfg.idle_unload_for(name);
        self
    }

    /// Where `SetMode` persists (`<data_dir>/config.yaml`).
    pub fn with_config_path(mut self, path: PathBuf) -> Self {
        self.config_path = Some(path);
        self
    }

    /// Whether any family under `prefix` captures training examples.
    pub fn train(&self, prefix: &str) -> bool {
        let dot = format!("{prefix}.");
        self.cfg
            .families
            .iter()
            .any(|(k, g)| k.starts_with(&dot) && g.train)
    }

    /// `Err` unless some `<prefix>.*` family is configured.
    fn check_prefix(&self, prefix: &str) -> Result<(), String> {
        let dot = format!("{prefix}.");
        if self.cfg.families.keys().any(|k| k.starts_with(&dot)) {
            Ok(())
        } else {
            Err(format!("no `{prefix}.*` decision family is configured"))
        }
    }

    /// Persist the prefix's switch positions as one flow mapping — both fields every time, so
    /// `SetMode` does not drop `train` and `SetTrain` does not drop `mode` (the config writer
    /// replaces the whole value).
    fn write_prefix(&self, prefix: &str, mode: Mode, train: bool) -> Result<(), String> {
        let Some(path) = &self.config_path else {
            return Ok(());
        };
        runtime::write_nested_config_value(
            path,
            "decision",
            "families",
            prefix,
            &format!("{{ mode: {mode}, train: {train} }}"),
        )
        .map_err(|e| format!("config.yaml: {e}"))
    }

    /// See [`crate::mode_for`].
    pub fn mode(&self, prefix: &str) -> crate::ModeInfo {
        crate::mode_for(&self.cfg, &self.default_dir, prefix)
    }

    /// The engine that answers `family`: `None` when it is this one, else the sibling's
    /// registry name (57 §4.1 item 6). Unknown families stay here and fail closed.
    fn route(&self, family: &Family) -> Option<String> {
        let name = self.cfg.families.get(&family.0)?.checkpoint();
        (name != self.checkpoint).then(|| crate::actor_name(name))
    }

    /// The other checkpoints' engines, for relaying `SetMode` / `SetTrain` (default only).
    fn siblings(&self) -> Vec<ActorRef<Engine>> {
        if self.checkpoint != crate::DEFAULT_CHECKPOINT {
            return Vec::new();
        }
        self.cfg
            .checkpoint_dirs(&self.default_dir)
            .keys()
            .filter(|n| *n != crate::DEFAULT_CHECKPOINT)
            .filter_map(|n| ActorRef::<Engine>::lookup(crate::actor_name(n).as_str()).ok().flatten())
            .collect()
    }

    /// The files that are not in the model dir, checked fresh each time.
    pub fn missing_files(&self) -> Vec<&'static str> {
        MODEL_FILES
            .into_iter()
            .filter(|f| !self.model_dir.join(f).is_file())
            .collect()
    }

    fn model_sha(&self) -> String {
        self.model_sha.lock().clone()
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
            tier: 1,
            calibrated: true,
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
                let (loader, dir, cfg) =
                    (self.loader.clone(), self.model_dir.clone(), self.cfg.clone());
                tokio::task::spawn_blocking(move || loader(&dir, &cfg))
            }
        };
        let mut handle = handle;
        match tokio::time::timeout(budget, &mut handle).await {
            Ok(Ok(Ok(oracle))) => {
                let load = oracle.load_time();
                self.model_sha = oracle.model_sha();
                tracing::info!(
                    "decision: model loaded in {load:?} from {} ({})",
                    self.model_dir.display(),
                    self.model_sha()
                );
                let shared: Shared = Arc::new(Mutex::new(oracle));
                self.loaded = Some(shared.clone());
                self.loaded_since = Some(Instant::now());
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

    fn touch(&mut self, me: ActorRef<Self>) {
        self.last_used = Instant::now();
        if self.watchdog.is_some() || self.idle.is_zero() {
            return;
        }
        self.epochs += 1;
        self.watchdog = Some(self.epochs);
        self.arm(me, self.idle, self.epochs);
    }

    /// One watchdog tick after `after`.
    fn arm(&self, me: ActorRef<Self>, after: Duration, epoch: u64) {
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            let _ = me.tell(Unload(epoch)).await;
        });
    }

    /// Drop the session (57 §2.1 item 1): in-flight runs keep their `Arc`; a cold load still in
    /// flight is detached so its result is dropped on landing; the watchdog is disarmed.
    fn release(&mut self, reason: &str) {
        self.watchdog = None;
        self.loading = None;
        let was = self.loaded.take().is_some();
        if let Some(since) = self.loaded_since.take() {
            tracing::info!(
                "decision: {} released ({reason}) after {:?} resident",
                self.checkpoint,
                since.elapsed()
            );
        } else if was {
            tracing::info!("decision: {} released ({reason})", self.checkpoint);
        }
    }

    /// This engine's [`LoadedInfo`], if a session is resident.
    fn loaded_info(&self) -> Option<LoadedInfo> {
        let since = self.loaded_since.filter(|_| self.loaded.is_some())?;
        Some(LoadedInfo {
            checkpoint: self.checkpoint.clone(),
            since_secs: since.elapsed().as_secs(),
            rss_mb: crate::process_memory().footprint / 1_000_000,
        })
    }
}

/// The production [`Loader`]: [`Loaded`] on `cfg.device`, f16 on Metal, f32 on CPU.
fn open(dir: &Path, cfg: &DecisionConfig) -> anyhow::Result<Box<dyn Oracle>> {
    let metal = cfg.device.metal();
    let dtype = if metal {
        candle_core::DType::F16
    } else {
        candle_core::DType::F32
    };
    Loaded::open(dir, metal, dtype).map(|l| Box::new(l) as Box<dyn Oracle>)
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
/// choice, the gate's verdict (a lost option marker is always a hard error, a cut state one
/// unless the family says `cut_ok`), then the family's mode.
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
    let hard =
        (seq.markers_lost || (seq.state_cut && !gate.cut_ok)).then_some(Reason::Truncated);
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
        tier: 1,
        calibrated: true,
    };
    gate.finish(d)
}

/// A decision that never ran: no probabilities, `choice = none`. The engine's `escalate`
/// logs one; the late-row task and the forwarder build theirs here.
fn escalated(
    family: Family,
    gate: &FamilyGate,
    reason: Reason,
    latency: Duration,
    sha: &str,
) -> Decision {
    Decision {
        family,
        choice: NONE_ID.to_owned(),
        probabilities: Vec::new(),
        confidence: 0.0,
        act_probability: 0.0,
        disposition: Disposition::Escalate(reason),
        latency,
        gate_version: gate.version(sha),
        log_id: LogId::new(),
        tier: 1,
        calibrated: true,
    }
}

/// One question of a batch: its ask, its gate and the question as rendered (`none` applied).
struct Item {
    ask: Ask,
    gate: FamilyGate,
    q: Question,
}

impl Engine {
    /// The batch core behind [`Ask`] and [`AskMany`]: gate lookup per question, one session,
    /// one `build` per question, **one** forward, one decision + log row per question. The
    /// budget is the largest asked; over it every question answers `Escalate(Timeout)` now
    /// and the late rows are still logged (57 §2).
    async fn run_batch(&mut self, asks: Vec<Ask>, me: ActorRef<Self>) -> Vec<Decision> {
        let t = Instant::now();
        if !self.cfg.enabled {
            return asks
                .iter()
                .map(|a| self.escalate(a, None, Reason::Disabled, t))
                .collect();
        }
        // Unknown families fail closed: only configured gates run (57 §0.5). A family on
        // another checkpoint is a routing mistake upstream, named as such.
        let mut items = Vec::with_capacity(asks.len());
        let mut out: Vec<Option<Decision>> = Vec::with_capacity(asks.len());
        for ask in asks {
            match self.cfg.families.get(&ask.family.0).cloned() {
                Some(g) if g.checkpoint() != self.checkpoint => {
                    let reason = Reason::HardError(format!(
                        "family {} is on checkpoint `{}`, this engine is `{}`",
                        ask.family,
                        g.checkpoint(),
                        self.checkpoint
                    ));
                    out.push(Some(self.escalate(&ask, Some(&g), reason, t)));
                }
                Some(g) if g.mode != Mode::Off => {
                    let q = if g.none {
                        with_none(ask.question.clone())
                    } else {
                        ask.question.clone()
                    };
                    items.push(Item { ask, gate: g, q });
                    out.push(None);
                }
                g => out.push(Some(self.escalate(&ask, g.as_ref(), Reason::Disabled, t))),
            }
        }
        if items.is_empty() {
            return out.into_iter().flatten().collect();
        }
        let budget = items.iter().map(|i| i.ask.budget).max().unwrap_or_default();
        let shared = match self.session(budget).await {
            Ok(s) => s,
            Err(reason) => {
                let ds: Vec<Decision> = items
                    .iter()
                    .map(|i| self.escalate(&i.ask, Some(&i.gate), reason.clone(), t))
                    .collect();
                return merge(out, ds);
            }
        };
        self.touch(me);

        let qs: Vec<Question> = items.iter().map(|i| i.q.clone()).collect();
        let batch_forward = self.cfg.batch_forward;
        let mut run = tokio::task::spawn_blocking(
            move || -> Result<(Vec<(Sequence, RowOut)>, String), Reason> {
                let mut o = shared.lock();
                let seqs = qs
                    .iter()
                    .map(|q| o.build(q))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(Reason::HardError)?;
                // One padded batch, or one forward per row (each padded to its own length;
                // `examples/bench.rs` (b2)/(b3) measures which wins on this device).
                let outs = if batch_forward || seqs.len() == 1 {
                    o.run(&seqs)
                } else {
                    seqs.iter()
                        .map(|s| o.run(std::slice::from_ref(s)))
                        .collect::<anyhow::Result<Vec<_>>>()
                        .map(|v| v.into_iter().flatten().collect())
                }
                .map_err(|e| Reason::HardError(format!("{e:#}")))?;
                if outs.len() != seqs.len() {
                    return Err(Reason::HardError(format!(
                        "{} rows for {} questions",
                        outs.len(),
                        seqs.len()
                    )));
                }
                Ok((seqs.into_iter().zip(outs).collect(), o.model_sha().lock().clone()))
            },
        );
        let texts: Vec<Option<Question>> = items
            .iter()
            .map(|i| self.cfg.log_text.then(|| i.ask.question.clone()))
            .collect();
        let elapsed = budget.saturating_sub(t.elapsed());
        let ds: Vec<Decision> = match tokio::time::timeout(elapsed, &mut run).await {
            Ok(Ok(Ok((rows, sha)))) => {
                let load_ms = self.pending_load_ms.take();
                items
                    .iter()
                    .zip(rows)
                    .zip(texts)
                    .map(|((i, (seq, row)), text)| {
                        let d =
                            decide(&i.gate, &i.ask.family, &i.q, &seq, &row, t.elapsed(), &sha);
                        let mut r = AskRow::new(
                            &d,
                            i.gate.mode,
                            &sha,
                            &seq.input_ids,
                            row.logits.clone(),
                            row.temperature,
                            text,
                        );
                        r.load_ms = load_ms;
                        self.append(Row::Ask(Box::new(r)));
                        d
                    })
                    .collect()
            }
            Ok(Ok(Err(reason))) => items
                .iter()
                .map(|i| self.escalate(&i.ask, Some(&i.gate), reason.clone(), t))
                .collect(),
            Ok(Err(e)) => {
                let reason = Reason::HardError(format!("run panicked: {e}"));
                items
                    .iter()
                    .map(|i| self.escalate(&i.ask, Some(&i.gate), reason.clone(), t))
                    .collect()
            }
            Err(_) => {
                // Over budget: answer now, log the late rows when they land (57 §2). The late
                // rows keep the real probabilities under `timeout`, so fitting sees them.
                let (log, sha) = (self.log.clone(), self.model_sha());
                let now: Vec<Decision> = items
                    .iter()
                    .map(|i| {
                        escalated(i.ask.family.clone(), &i.gate, Reason::Timeout, t.elapsed(), &sha)
                    })
                    .collect();
                tokio::spawn(async move {
                    let rows: Vec<Row> = match run.await {
                        Ok(Ok((rows, sha))) => items
                            .iter()
                            .zip(rows)
                            .zip(texts)
                            .map(|((i, (seq, row)), text)| {
                                let mut d = decide(
                                    &i.gate,
                                    &i.ask.family,
                                    &i.q,
                                    &seq,
                                    &row,
                                    t.elapsed(),
                                    &sha,
                                );
                                d.disposition = Disposition::Escalate(Reason::Timeout);
                                Row::Ask(Box::new(AskRow::new(
                                    &d,
                                    i.gate.mode,
                                    &sha,
                                    &seq.input_ids,
                                    row.logits,
                                    row.temperature,
                                    text,
                                )))
                            })
                            .collect(),
                        Ok(Err(reason)) => items
                            .iter()
                            .zip(texts)
                            .map(|(i, text)| {
                                let d = escalated(
                                    i.ask.family.clone(),
                                    &i.gate,
                                    reason.clone(),
                                    t.elapsed(),
                                    &sha,
                                );
                                Row::Ask(Box::new(AskRow::new(
                                    &d,
                                    i.gate.mode,
                                    &sha,
                                    &[],
                                    Vec::new(),
                                    0.0,
                                    text,
                                )))
                            })
                            .collect(),
                        Err(_) => return,
                    };
                    for row in rows {
                        if let Err(e) = log.append(&row) {
                            tracing::warn!("decision log: {e}");
                        }
                    }
                });
                now
            }
        };
        merge(out, ds)
    }

    /// What forwarded asks answer when the sibling engine `to` is not running: `HardError`
    /// per ask, nothing logged (the sibling owns those rows).
    fn unreachable(&self, to: &str, asks: &[Ask]) -> Vec<Decision> {
        let sha = self.model_sha();
        asks.iter()
            .map(|a| {
                let gate = self
                    .cfg
                    .families
                    .get(&a.family.0)
                    .cloned()
                    .unwrap_or_else(|| FamilyGate::shadow(1.0, 1.0));
                let reason = Reason::HardError(format!("checkpoint engine `{to}` is not running"));
                escalated(a.family.clone(), &gate, reason, Duration::ZERO, &sha)
            })
            .collect()
    }
}

/// The sibling engine registered as `to`, if it is running.
fn sibling(to: &str) -> Option<ActorRef<Engine>> {
    ActorRef::<Engine>::lookup(to).ok().flatten()
}

/// Fill the `None` slots of `out` (the questions that ran) with `ds`, in order.
fn merge(out: Vec<Option<Decision>>, ds: Vec<Decision>) -> Vec<Decision> {
    let mut ds = ds.into_iter();
    out.into_iter()
        .map(|d| d.unwrap_or_else(|| ds.next().expect("one decision per ran question")))
        .collect()
}

impl Message<Ask> for Engine {
    type Reply = DelegatedReply<Decision>;

    async fn handle(&mut self, ask: Ask, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let Some(to) = self.route(&ask.family) else {
            let me = ctx.actor_ref().clone();
            let d = self.run_batch(vec![ask], me).await.remove(0);
            return ctx.reply(d);
        };
        // Another checkpoint's family: forward off this mailbox (57 §4.1 item 6).
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let fallback = self.unreachable(&to, std::slice::from_ref(&ask)).remove(0);
        tokio::spawn(async move {
            let d = match sibling(&to) {
                Some(s) => s.ask(ask).await.unwrap_or_else(|e| {
                    tracing::warn!("decision: forward to {to}: {e}");
                    fallback
                }),
                None => fallback,
            };
            tx.send(d);
        });
        reply
    }
}

impl Message<AskMany> for Engine {
    type Reply = DelegatedReply<Vec<Decision>>;

    async fn handle(&mut self, m: AskMany, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let Some(to) = m.asks.first().and_then(|a| self.route(&a.family)) else {
            let me = ctx.actor_ref().clone();
            let ds = self.run_batch(m.asks, me).await;
            return ctx.reply(ds);
        };
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let fallback = self.unreachable(&to, &m.asks);
        tokio::spawn(async move {
            let ds = match sibling(&to) {
                Some(s) => s.ask(m).await.unwrap_or_else(|e| {
                    tracing::warn!("decision: forward to {to}: {e}");
                    fallback
                }),
                None => fallback,
            };
            tx.send(ds);
        });
        reply
    }
}

impl Message<Unload> for Engine {
    type Reply = ();

    async fn handle(&mut self, Unload(epoch): Unload, ctx: &mut Context<Self, Self::Reply>) {
        if self.watchdog != Some(epoch) {
            return; // disarmed by a `Release` (or superseded) since this tick was armed
        }
        let since = self.last_used.elapsed();
        if since >= self.idle {
            self.release(&format!("idle {since:?}"));
        } else {
            self.arm(ctx.actor_ref().clone(), self.idle - since, epoch);
        }
    }
}

impl Message<Release> for Engine {
    type Reply = ();

    async fn handle(&mut self, m: Release, _: &mut Context<Self, Self::Reply>) {
        match m.checkpoint.as_deref().filter(|c| *c != self.checkpoint) {
            None => self.release(m.reason),
            Some(to) => {
                let to = crate::actor_name(to);
                tokio::spawn(async move {
                    if let Some(s) = sibling(&to) {
                        let _ = s.tell(Release { checkpoint: None, ..m }).await;
                    }
                });
            }
        }
    }
}

impl Message<LoadedNow> for Engine {
    type Reply = DelegatedReply<Vec<LoadedInfo>>;

    async fn handle(&mut self, _: LoadedNow, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let mut own: Vec<LoadedInfo> = self.loaded_info().into_iter().collect();
        let siblings = self.siblings();
        if siblings.is_empty() {
            return ctx.reply(own);
        }
        // Off the mailbox: a sibling mid-load holds its own mailbox for up to its budget.
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        tokio::spawn(async move {
            for s in siblings {
                if let Ok(Ok(more)) =
                    tokio::time::timeout(Duration::from_secs(2), s.ask(LoadedNow)).await
                {
                    own.extend(more);
                }
            }
            tx.send(own);
        });
        reply
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

impl Engine {
    fn family_stats(&self, family: &Family) -> FamilyStats {
        let mut s = self.log.stats(family).unwrap_or_else(|e| {
            tracing::warn!("decision log: {e}");
            FamilyStats {
                family: family.0.clone(),
                ..FamilyStats::default()
            }
        });
        s.mode = self.cfg.families.get(&family.0).map_or(Mode::Off, |g| g.mode);
        s
    }
}

impl Message<Stats> for Engine {
    type Reply = FamilyStats;

    async fn handle(&mut self, s: Stats, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.family_stats(&s.family)
    }
}

impl Message<AllStats> for Engine {
    type Reply = Vec<FamilyStats>;

    async fn handle(&mut self, _: AllStats, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.cfg
            .families
            .keys()
            .map(|k| self.family_stats(&Family(k.clone())))
            .collect()
    }
}

impl Message<GetMode> for Engine {
    type Reply = crate::ModeInfo;

    async fn handle(&mut self, m: GetMode, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.mode(&m.prefix)
    }
}

impl Message<SetMode> for Engine {
    type Reply = Result<crate::ModeInfo, String>;

    async fn handle(&mut self, m: SetMode, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.check_prefix(&m.prefix)?;
        self.write_prefix(&m.prefix, m.mode, self.train(&m.prefix))?;
        let dot = format!("{}.", m.prefix);
        for (_, g) in self
            .cfg
            .families
            .iter_mut()
            .filter(|(k, _)| k.starts_with(&dot))
        {
            g.mode = m.mode;
        }
        tracing::info!("decision: {}.* mode → {}", m.prefix, m.mode);
        // The siblings have no `config_path`, so the relay is live-only there.
        for s in self.siblings() {
            let _ = s.tell(m.clone()).await;
        }
        Ok(self.mode(&m.prefix))
    }
}

impl Message<GetTrain> for Engine {
    type Reply = TrainingInfo;

    async fn handle(&mut self, m: GetTrain, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.training.info(&m.prefix, self.train(&m.prefix))
    }
}

impl Message<SetTrain> for Engine {
    type Reply = Result<TrainingInfo, String>;

    async fn handle(&mut self, m: SetTrain, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.check_prefix(&m.prefix)?;
        self.write_prefix(&m.prefix, self.mode(&m.prefix).mode, m.on)?;
        let dot = format!("{}.", m.prefix);
        for (_, g) in self
            .cfg
            .families
            .iter_mut()
            .filter(|(k, _)| k.starts_with(&dot))
        {
            g.train = m.on;
        }
        tracing::info!("decision: {}.* train → {}", m.prefix, m.on);
        for s in self.siblings() {
            let _ = s.tell(m.clone()).await;
        }
        Ok(self.training.info(&m.prefix, m.on))
    }
}

impl Message<Capture> for Engine {
    type Reply = ();

    async fn handle(&mut self, mut c: Capture, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if !self.train(&c.prefix) {
            return;
        }
        if let training::Row::Example(e) = &mut c.row
            && e.model_sha.is_empty()
        {
            e.model_sha = self.model_sha();
        }
        if let Err(e) = self.training.append(&c.prefix, &c.row) {
            tracing::warn!("decision training: {e}");
        }
    }
}

impl Message<Record> for Engine {
    type Reply = ();

    async fn handle(&mut self, r: Record, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let d = &r.decision;
        let mode = self.cfg.families.get(&d.family.0).map_or(Mode::Off, |g| g.mode);
        let sha = self.model_sha();
        let mut row = AskRow::new(d, mode, &sha, &[], Vec::new(), 0.0, None);
        row.note = r.note;
        self.append(Row::Ask(Box::new(row)));
    }
}

impl Message<Warm> for Engine {
    type Reply = Result<u64, String>;

    async fn handle(&mut self, w: Warm, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let t = Instant::now();
        if !self.cfg.enabled {
            return Err("disabled".into());
        }
        let shared = self.session(w.budget).await.map_err(|r| format!("{r:?}"))?;
        self.touch(ctx.actor_ref().clone());
        let run = tokio::task::spawn_blocking(move || -> Result<(), String> {
            let mut o = shared.lock();
            let q = Question::Bool {
                statement: "warm-up".into(),
                state: "warm-up".into(),
            };
            let seq = o.build(&q)?;
            o.run(std::slice::from_ref(&seq)).map_err(|e| format!("{e:#}"))?;
            Ok(())
        });
        match tokio::time::timeout(w.budget.saturating_sub(t.elapsed()), run).await {
            Ok(Ok(Ok(()))) => {
                let ms = t.elapsed().as_millis() as u64;
                tracing::info!("decision: warm-up done in {ms} ms");
                Ok(ms)
            }
            Ok(Ok(Err(e))) => Err(e),
            Ok(Err(e)) => Err(format!("warm-up panicked: {e}")),
            Err(_) => Err("warm-up timed out".into()),
        }
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
        for f in MODEL_FILES {
            let p = model_dir.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
    }

    /// Answers `[0.1, 0.9]` for two options after sleeping `delay`.
    struct Fake {
        delay: Duration,
    }

    impl Oracle for Fake {
        fn model_sha(&self) -> ModelSha {
            Arc::new(Mutex::new("fake".into()))
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
                state_cut: false,
                markers_lost: false,
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
        Arc::new(move |_, _| Ok(Box::new(Fake { delay }) as Box<dyn Oracle>))
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
        let e = Engine::with_loader(cfg, model_dir.clone(), log.clone(), fake_loader(Duration::ZERO));
        // The safetensors layout, subdirs included; one present file drops out of the list.
        assert_eq!(
            e.missing_files(),
            [
                "model.safetensors",
                "encoder/config.json",
                "tokenizer/tokenizer.json",
                "rl_agent_config.json"
            ]
        );
        std::fs::create_dir_all(model_dir.join("encoder")).unwrap();
        std::fs::write(model_dir.join("encoder/config.json"), b"{}").unwrap();
        assert_eq!(
            e.missing_files(),
            ["model.safetensors", "tokenizer/tokenizer.json", "rl_agent_config.json"]
        );
        let engine = Engine::spawn(e);

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
                    ..FamilyGate::shadow(0.8, 0.5)
                },
            )]
            .into(),
            ..DecisionConfig::default()
        };
        let engine = Engine::spawn(Engine::with_loader(
            cfg,
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

    /// Counts `run` calls and the rows per call; answers like [`Fake`].
    struct Counting {
        calls: Arc<Mutex<Vec<usize>>>,
        cut: bool,
    }

    impl Oracle for Counting {
        fn model_sha(&self) -> ModelSha {
            Arc::new(Mutex::new("fake".into()))
        }
        fn build(&self, q: &Question) -> Result<Sequence, String> {
            let mut s = Fake {
                delay: Duration::ZERO,
            }
            .build(q)?;
            s.state_cut = self.cut;
            s.truncated = self.cut;
            Ok(s)
        }
        fn run(&mut self, rows: &[Sequence]) -> anyhow::Result<Vec<RowOut>> {
            self.calls.lock().push(rows.len());
            Fake {
                delay: Duration::ZERO,
            }
            .run(rows)
        }
        fn load_time(&self) -> Duration {
            Duration::ZERO
        }
    }

    fn jev_question(n: usize) -> Question {
        Question::Choice {
            instructions: "op".into(),
            candidates: (1..=n)
                .map(|i| Candidate {
                    id: i.to_string(),
                    text: format!("[{i}] link"),
                })
                .collect(),
            state: "page".into(),
        }
    }

    #[tokio::test]
    async fn ask_many_is_one_forward_with_per_question_rows_and_closed_option_sets() {
        let dir = tempfile::tempdir().unwrap();
        touch_files(dir.path());
        let calls = Arc::new(Mutex::new(Vec::new()));
        let loader: Loader = {
            let calls = calls.clone();
            Arc::new(move |_, _| {
                Ok(Box::new(Counting {
                    calls: calls.clone(),
                    cut: true,
                }) as Box<dyn Oracle>)
            })
        };
        // Both families on this (default) engine: one jev-shaped (no `none`, cut ok), one classic.
        let mut jev = FamilyGate::browser(0.5);
        jev.checkpoint = None;
        jev.mode = Mode::Active;
        let cfg = DecisionConfig {
            families: [
                ("browser.operation".to_owned(), jev),
                ("meeting.brief".to_owned(), FamilyGate::shadow(0.5, 0.1)),
                ("meeting.off".to_owned(), FamilyGate { mode: Mode::Off, ..FamilyGate::shadow(0.5, 0.1) }),
            ]
            .into(),
            batch_forward: true,
            ..DecisionConfig::default()
        };
        let log = Log::new(dir.path().join("d"));
        let engine = Engine::spawn(Engine::with_loader(cfg, dir.path().into(), log, loader));
        let ds = engine
            .ask(AskMany {
                asks: vec![
                    Ask::new("browser.operation", jev_question(3)),
                    Ask::new("meeting.off", jev_question(2)),
                    Ask::new("meeting.brief", jev_question(2)),
                ],
            })
            .await
            .unwrap();
        assert_eq!(*calls.lock(), vec![2], "one forward for the two questions that ran");
        assert_eq!(ds.len(), 3);
        // Order kept, the off family escalated in place.
        assert_eq!(ds[0].family.0, "browser.operation");
        assert_eq!(ds[1].disposition, Disposition::Escalate(Reason::Disabled));
        assert_eq!(ds[2].family.0, "meeting.brief");
        // No `none` on the closed set; the cut state was not a hard error; argmax accepted.
        let labels: Vec<&str> = ds[0].probabilities.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["1", "2", "3"]);
        assert_eq!((ds[0].choice.as_str(), &ds[0].disposition), ("3", &Disposition::Accept));
        // The classic family got `none` and the cut state is its hard error (57 §0.5).
        let labels: Vec<&str> = ds[2].probabilities.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["1", "2", "none"]);
        assert_eq!(ds[2].disposition.reason(), Some(&Reason::Truncated));
        // Two log rows, one per question that ran, plus the disabled one.
        let s = engine.ask(AllStats).await.unwrap();
        assert_eq!(s.iter().map(|f| f.asks).sum::<u64>(), 3);
    }

    #[tokio::test]
    async fn family_on_another_checkpoint_is_forwarded_or_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        touch_files(&dir.path().join("decision"));
        touch_files(&dir.path().join("decision-browser"));
        let cfg = DecisionConfig {
            families: [
                ("browser.operation".to_owned(), FamilyGate { mode: Mode::Active, ..FamilyGate::browser(0.5) }),
                ("meeting.brief".to_owned(), FamilyGate::shadow(0.5, 0.1)),
            ]
            .into(),
            ..DecisionConfig::default()
        };
        let log = Log::new(dir.path().join("d"));
        let default_dir = dir.path().join("decision");
        let main = Engine::spawn(Engine::with_loader(
            cfg.clone(),
            default_dir.clone(),
            log.clone(),
            fake_loader(Duration::ZERO),
        ));
        // No sibling registered yet: fail closed, named.
        let d = main.ask(Ask::new("browser.operation", jev_question(2))).await.unwrap();
        assert!(
            matches!(&d.disposition, Disposition::Escalate(Reason::HardError(e)) if e.contains("decision/browser")),
            "{d:?}"
        );
        // With the sibling up, the default engine forwards and the sibling answers (no `none`).
        let browser = Engine::spawn(
            Engine::with_loader(cfg, default_dir.clone(), log, fake_loader(Duration::ZERO))
                .with_checkpoint("browser", dir.path().join("decision-browser")),
        );
        browser.register(crate::actor_name("browser")).unwrap();
        let d = main.ask(Ask::new("browser.operation", jev_question(2))).await.unwrap();
        assert_eq!((d.choice.as_str(), &d.disposition), ("2", &Disposition::Accept));
        assert_eq!(d.probabilities.len(), 2);
        let ds = main
            .ask(AskMany {
                asks: vec![Ask::new("browser.operation", jev_question(2)); 3],
            })
            .await
            .unwrap();
        assert_eq!(ds.len(), 3);
        assert!(ds.iter().all(|d| d.disposition == Disposition::Accept));
        // A default-checkpoint family sent to the sibling is forwarded the other way; with no
        // `decision` actor registered in this test it fails closed, naming it.
        let d = browser.ask(Ask::new("meeting.brief", jev_question(2))).await.unwrap();
        assert!(matches!(&d.disposition, Disposition::Escalate(Reason::HardError(e)) if e.contains("`decision`")), "{d:?}");
        // `mode_for` at the default engine checks the browser checkpoint's files for `browser`.
        std::fs::remove_file(dir.path().join("decision-browser/model.safetensors")).unwrap();
        let m = main.ask(GetMode { prefix: "browser".into() }).await.unwrap();
        assert!(m.reason.as_deref().unwrap().contains("decision-browser"), "{m:?}");
        assert_eq!(main.ask(GetMode { prefix: "meeting".into() }).await.unwrap().reason, None);
        // SetMode on the default relays live to the sibling.
        main.ask(SetMode { prefix: "browser".into(), mode: Mode::Off }).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let d = browser.ask(Ask::new("browser.operation", jev_question(2))).await.unwrap();
        assert_eq!(d.disposition, Disposition::Escalate(Reason::Disabled));
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
        let loader: Loader = Arc::new(move |_, _| {
            n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Box::new(Fake {
                delay: Duration::ZERO,
            }) as Box<dyn Oracle>)
        });
        let engine = Engine::spawn(Engine::with_loader(
            cfg,
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

    /// 57 §2.1: spawning loads nothing; a question loads; `Release` drops it now (no idle
    /// wait) and disarms the watchdog; the next question loads again; the browser engine
    /// gets its own, shorter idle.
    #[tokio::test]
    async fn release_drops_now_and_nothing_loads_at_start() {
        let dir = tempfile::tempdir().unwrap();
        touch_files(dir.path());
        let cfg = DecisionConfig {
            idle_unload: Duration::from_secs(3600),
            families: [("meeting.topic".to_owned(), FamilyGate::shadow(0.85, 0.4))].into(),
            ..DecisionConfig::default()
        };
        assert_eq!(cfg.idle_unload_for("default"), Duration::from_secs(3600));
        assert_eq!(cfg.idle_unload_for("browser"), crate::BROWSER_IDLE_UNLOAD);
        let loads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let n = loads.clone();
        let loader: Loader = Arc::new(move |_, _| {
            n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Box::new(Fake {
                delay: Duration::ZERO,
            }) as Box<dyn Oracle>)
        });
        let engine = Engine::spawn(Engine::with_loader(
            cfg,
            dir.path().into(),
            Log::new(dir.path().join("d")),
            loader,
        ));
        // Other tests register `decision/browser` engines in the same process registry; the
        // default engine's answer includes them, so count only its own entry.
        let mine = |infos: Vec<LoadedInfo>| infos.into_iter().filter(|i| i.checkpoint == "default").count();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(mine(engine.ask(LoadedNow).await.unwrap()), 0);
        assert_eq!(loads.load(std::sync::atomic::Ordering::SeqCst), 0);

        engine.ask(ask("meeting.topic")).await.unwrap();
        assert_eq!(mine(engine.ask(LoadedNow).await.unwrap()), 1);
        engine
            .tell(Release {
                checkpoint: None,
                reason: "test",
            })
            .await
            .unwrap();
        assert_eq!(mine(engine.ask(LoadedNow).await.unwrap()), 0);
        // A `Release` naming another checkpoint leaves this one alone (and is a no-op when
        // that checkpoint has no engine).
        engine.ask(ask("meeting.topic")).await.unwrap();
        engine
            .tell(Release {
                checkpoint: Some("nobody".into()),
                reason: "test",
            })
            .await
            .unwrap();
        assert_eq!(mine(engine.ask(LoadedNow).await.unwrap()), 1);
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
            Log::new(dir.path().join("d")),
        ));
        let d = engine.ask(ask("meeting.topic")).await.unwrap();
        assert_eq!(d.disposition, Disposition::Escalate(Reason::Disabled));
    }

    #[tokio::test]
    async fn train_switch_writes_a_merged_prefix_patch_and_gates_capture() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.yaml");
        std::fs::write(
            &config,
            "decision:\n  families:\n    meeting: { mode: shadow }\n",
        )
        .unwrap();
        let log = Log::new(dir.path().join("decisions"));
        let e = Engine::with_loader(
            DecisionConfig::default(),
            dir.path().join("m"),
            log,
            fake_loader(Duration::ZERO),
        )
        .with_config_path(config.clone());
        let engine = Engine::spawn(e);
        let get = |prefix: &str| GetTrain {
            prefix: prefix.to_owned(),
        };
        let info = engine.ask(get("meeting")).await.unwrap();
        assert!(
            !info.enabled
                && info.examples_total == 0
                && info.dir.ends_with("decisions/training/meeting")
        );

        // Off: a captured row is dropped and no directory appears.
        let example = || {
            training::Example::new(
                "meeting",
                Mode::Shadow,
                "them: hi".into(),
                vec![],
                vec![],
                None,
                training::Verdict::Skip,
            )
        };
        engine
            .tell(Capture {
                prefix: "meeting".into(),
                row: training::Row::Example(Box::new(example())),
            })
            .await
            .unwrap();
        assert_eq!(engine.ask(get("meeting")).await.unwrap().examples_total, 0);
        assert!(!dir.path().join("decisions/training").exists());

        // On: the config line keeps `mode`, the families flip live, the row lands with the model sha.
        let info = engine
            .ask(SetTrain {
                prefix: "meeting".into(),
                on: true,
            })
            .await
            .unwrap();
        assert!(info.enabled);
        let yaml = std::fs::read_to_string(&config).unwrap();
        assert!(
            yaml.contains("    meeting: { mode: shadow, train: true }"),
            "{yaml}"
        );
        let whole: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        let cfg: DecisionConfig = serde_yaml::from_value(whole["decision"].clone()).unwrap();
        assert!(
            cfg.families["meeting.kind"].train
                && cfg.families["meeting.brief"].train
                && !cfg.families["browser.target"].train
        );
        assert_eq!(cfg.families["meeting.kind"].mode, Mode::Shadow);
        engine
            .tell(Capture {
                prefix: "meeting".into(),
                row: training::Row::Example(Box::new(example())),
            })
            .await
            .unwrap();
        let info = engine.ask(get("meeting")).await.unwrap();
        assert_eq!((info.examples_today, info.examples_total), (1, 1));
        assert!(info.bytes > 0);
        // Another prefix is untouched; an unknown one is an error.
        assert!(!engine.ask(get("browser")).await.unwrap().enabled);
        assert!(
            engine
                .ask(SetTrain {
                    prefix: "nobody".into(),
                    on: true
                })
                .await
                .is_err()
        );

        // `SetMode` keeps `train` in the line; `SetTrain(false)` keeps the mode and stops writes.
        engine
            .ask(SetMode {
                prefix: "meeting".into(),
                mode: Mode::Active,
            })
            .await
            .unwrap();
        assert!(
            std::fs::read_to_string(&config)
                .unwrap()
                .contains("meeting: { mode: active, train: true }")
        );
        engine
            .ask(SetTrain {
                prefix: "meeting".into(),
                on: false,
            })
            .await
            .unwrap();
        assert!(
            std::fs::read_to_string(&config)
                .unwrap()
                .contains("meeting: { mode: active, train: false }")
        );
        engine
            .tell(Capture {
                prefix: "meeting".into(),
                row: training::Row::Example(Box::new(example())),
            })
            .await
            .unwrap();
        assert_eq!(engine.ask(get("meeting")).await.unwrap().examples_total, 1);
    }
}
