//! Decision plugin (design doc 57): a local typed-probability oracle — laya, a ModernBERT
//! encoder with a decision head, run natively in candle ([`candle`], Metal on Apple silicon,
//! CPU elsewhere) — answering [`Question`]s with a probability per option and a selective gate
//! ([`gate`]) that says `Accept` or `Escalate(reason)`. Callers keep today's reasoning-model
//! path on `Escalate`; every family ships in `shadow` until its log says otherwise.
//!
//! Public surface consumed by other plugins: messages on the [`DECISION_ACTOR`] actor
//! ([`Engine`]): [`Ask`] → [`Decision`], [`AskMany`] → `Vec<Decision>`, [`Report`],
//! [`Stats`] → [`FamilyStats`]. One engine per checkpoint (57 §4.1 item 6): the `default`
//! one is [`DECISION_ACTOR`], the others `decision/<name>` ([`actor_name`]); an `Ask` sent to
//! any of them is routed to the one its family's gate names.

pub mod candle;
pub mod engine;
pub mod escalate;
pub mod gate;
pub mod gliner;
pub mod laya;
pub mod log;
pub mod question;
pub mod subject;
pub mod tool;
pub mod training;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use kameo::actor::Spawn;
use serde::{Deserialize, Serialize};

pub use engine::{
    AllStats, AskMany, Capture, DeleteTraining, Engine, GetMode, GetTrain, LoadedInfo, LoadedNow,
    Loader, Oracle, Record, Release, Report, Row, Scores, SetMode, SetShare, SetTrain, Stats, Warm,
};
pub use escalate::{EscalateError, SideCaller, escalate};
pub use gate::{FamilyGate, Mode};
pub use log::FamilyStats;
pub use question::{
    Ask, Candidate, Decision, Disposition, Family, LogId, Outcome, Question, Reason,
};
pub use training::TrainingInfo;

/// Registry name of the `default` checkpoint's [`Engine`] actor.
pub const DECISION_ACTOR: &str = "decision";

/// The checkpoint under `<data_dir>/models/` (57 §2), published by `release-models.sh`: the
/// Hugging Face `convaiinnovations/laya` layout (`engine::model_files(Kind::Laya)`).
pub const MODEL_SUBDIR: &str = "decision";

/// The checkpoint name a family runs on when its gate names none (`decision.checkpoints.default`
/// = `model_dir`).
pub const DEFAULT_CHECKPOINT: &str = "default";

/// The browser head's checkpoint name (57 §4.1 item 6): cklxx/laya-browser v17s under
/// `<data_dir>/models/decision-browser/` unless `decision.checkpoints.browser` says otherwise.
pub const BROWSER_CHECKPOINT: &str = "browser";

/// Default of [`DecisionConfig::idle_unload`] (57 §2.1 item 2): the safety net behind the
/// explicit [`Release`] the meeting sends at stop.
pub const IDLE_UNLOAD: Duration = Duration::from_secs(180);

/// The `browser` checkpoint's idle default (57 §2.1 item 2): loop steps are sub-second, so a
/// run never trips it, and a session that stops calling `browser_run` frees 0.9 GB sooner.
pub const BROWSER_IDLE_UNLOAD: Duration = Duration::from_secs(120);

/// The GLiNER2.5-Decide checkpoint name (57 §2.2): `<data_dir>/models/decision-gliner/` unless
/// `decision.checkpoints.gliner` says otherwise; [`Kind::Gliner`] unless configured.
pub const GLINER_CHECKPOINT: &str = "gliner";

/// The `decide` tool's family (92 §2); sub-families are `tool.<task>`.
pub const TOOL_FAMILY: &str = "tool";

/// Which classifier family a checkpoint is (`decision.checkpoints.<name>.kind`): the files
/// expected on disk ([`engine::model_files`]) and the [`Oracle`] that loads it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// laya: ModernBERT + decision head, one `[MASK]` per option ([`candle::Loaded`]).
    #[default]
    Laya,
    /// GLiNER2.5-Decide: DeBERTa-v3-large + per-label head ([`gliner::Loaded`], GLINER-3).
    Gliner,
}

/// One `decision.checkpoints.<name>` entry: a bare directory (`browser: /path`, or `~` for the
/// convention) or `{ dir, idle_unload, kind }` with every field optional.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(from = "CheckpointEntry")]
pub struct Checkpoint {
    /// The checkpoint directory; `None` = the convention (`<models>/decision-<name>`).
    pub dir: Option<PathBuf>,
    /// This checkpoint's idle unload, overriding [`DecisionConfig::idle_unload`] and the
    /// browser default.
    pub idle_unload: Option<Duration>,
    /// The classifier family; `laya` unless said otherwise.
    pub kind: Kind,
}

impl From<PathBuf> for Checkpoint {
    fn from(dir: PathBuf) -> Self {
        Checkpoint {
            dir: Some(dir),
            idle_unload: None,
            kind: Kind::Laya,
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CheckpointEntry {
    Dir(Option<PathBuf>),
    Full {
        #[serde(default)]
        dir: Option<PathBuf>,
        #[serde(default, deserialize_with = "opt_duration")]
        idle_unload: Option<Duration>,
        #[serde(default)]
        kind: Kind,
    },
}

impl From<CheckpointEntry> for Checkpoint {
    fn from(e: CheckpointEntry) -> Self {
        match e {
            CheckpointEntry::Dir(dir) => Checkpoint {
                dir,
                idle_unload: None,
                kind: Kind::Laya,
            },
            CheckpointEntry::Full {
                dir,
                idle_unload,
                kind,
            } => Checkpoint {
                dir,
                idle_unload,
                kind,
            },
        }
    }
}

/// This process's memory, one number for the whole daemon (the checkpoints share it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProcessMemory {
    /// Resident set in bytes. On macOS this still counts pages `free` handed back with
    /// `MADV_FREE_REUSABLE` until the kernel reclaims them, so a released checkpoint can stay
    /// "resident" here for a long time while costing nothing.
    pub resident: u64,
    /// macOS `phys_footprint` — what Activity Monitor calls Memory and what the OS actually
    /// charges the process; equals `resident` on Linux (`/proc/self/statm`). `0` where neither
    /// works. This is the number [`LoadedInfo::rss_mb`] reports.
    pub footprint: u64,
}

/// [`ProcessMemory`] now (`proc_pid_rusage` on macOS, `/proc/self/statm` on Linux).
pub fn process_memory() -> ProcessMemory {
    #[cfg(target_os = "macos")]
    {
        let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
        // SAFETY: `info` is a zeroed `rusage_info_v2`, the layout RUSAGE_INFO_V2 fills.
        let rc = unsafe {
            libc::proc_pid_rusage(
                std::process::id() as libc::c_int,
                libc::RUSAGE_INFO_V2,
                (&raw mut info).cast(),
            )
        };
        if rc == 0 {
            ProcessMemory {
                resident: info.ri_resident_size,
                footprint: info.ri_phys_footprint,
            }
        } else {
            ProcessMemory::default()
        }
    }
    #[cfg(target_os = "linux")]
    {
        let resident = std::fs::read_to_string("/proc/self/statm")
            .ok()
            .and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok())
            .map_or(0, |pages| pages * 4096);
        ProcessMemory {
            resident,
            footprint: resident,
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        ProcessMemory::default()
    }
}

/// Registry name of the [`Engine`] for `checkpoint`: [`DECISION_ACTOR`] for `default`, else
/// `decision/<name>`.
pub fn actor_name(checkpoint: &str) -> String {
    if checkpoint == DEFAULT_CHECKPOINT {
        DECISION_ACTOR.to_owned()
    } else {
        format!("{DECISION_ACTOR}/{checkpoint}")
    }
}

/// Where the model runs (`decision.device`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Device {
    /// Metal on an Apple-silicon Mac; elsewhere whatever `models.accelerator` resolves to
    /// (CUDA when this binary was built with `decision/cuda`, else CPU).
    #[default]
    Auto,
    /// CPU, f32.
    Cpu,
    /// Metal, f16; falls back to CPU with a warning when the GPU is unavailable.
    Metal,
    /// CUDA device 0, f32; needs the `cuda` build and a device, else CPU with a warning.
    Cuda,
}

impl Device {
    /// Whether this resolves to Metal on the running machine.
    pub fn metal(self) -> bool {
        self.gpu() == candle::Gpu::Metal
    }

    /// The GPU this asks for on the running machine, `Auto` read through `models.accelerator`
    /// (`directml` is ORT-only and means `auto` here).
    pub fn gpu(self) -> candle::Gpu {
        use models::accelerator::Accelerator;
        match self {
            Device::Cpu => candle::Gpu::None,
            Device::Metal => candle::Gpu::Metal,
            Device::Cuda => candle::Gpu::Cuda,
            Device::Auto => match models::accelerator::get() {
                Accelerator::Cpu => candle::Gpu::None,
                Accelerator::Cuda => candle::Gpu::Cuda,
                // WebGPU and DirectML are ONNX-session providers; candle resolves as `auto`.
                Accelerator::Auto | Accelerator::Directml | Accelerator::Webgpu => {
                    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
                        candle::Gpu::Metal
                    } else if cfg!(feature = "cuda") {
                        candle::Gpu::Cuda
                    } else {
                        candle::Gpu::None
                    }
                }
            },
        }
    }
}

/// Default of [`DecisionConfig::batch_forward`]: `examples/bench.rs` (b2)/(b3) on an M3 Max,
/// Metal f16, the browser step (3 rows, S 476/1024/583) — one batch padded to S 1024: p50
/// 1034 ms; three single-row forwards: p50 569 ms. The forward is ~linear in padded tokens.
const BATCH_FORWARD: bool = false;

/// `decision:` config section (57 §2). Every field has a default; the section may be absent.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct DecisionConfig {
    /// `false`: the actor answers every `Ask` with `Escalate(Disabled)`.
    pub enabled: bool,
    /// The `default` checkpoint directory (`model.safetensors`, `encoder/config.json`,
    /// `tokenizer/tokenizer.json`, `rl_agent_config.json`); `None` → `<data_dir>/models/decision`.
    /// Kept for old config files; `checkpoints.default` wins when both are set.
    pub model_dir: Option<PathBuf>,
    /// Checkpoints by name (57 §4.1 item 6): `default` (= `model_dir`) and `browser`
    /// (`<data_dir>/models/decision-browser`) exist unless overridden; any other name is a
    /// new checkpoint a family may point at with `checkpoint: <name>`. Each entry is a bare
    /// directory or `{ dir, idle_unload, kind }` ([`Checkpoint`]).
    pub checkpoints: BTreeMap<String, Checkpoint>,
    /// `auto` (Metal on an Apple-silicon Mac, else CPU) | `cpu` | `metal`.
    pub device: Device,
    /// Unload a checkpoint after this long unused (humantime, e.g. `3m`) — the safety net behind
    /// the explicit [`Release`] (57 §2.1). `checkpoints.<name>.idle_unload` overrides it; the
    /// `browser` checkpoint defaults to [`BROWSER_IDLE_UNLOAD`].
    #[serde(deserialize_with = "duration")]
    pub idle_unload: Duration,
    /// Store the question text in the log, not only its hash.
    pub log_text: bool,
    /// `AskMany`: run the batch as one padded forward (`true`) or as one forward per question,
    /// each padded to its own length (`false`). Default from `examples/bench.rs` (b2)/(b3) on
    /// Metal, where the forward is close to linear in padded tokens.
    pub batch_forward: bool,
    /// Gate per decision family; a family not listed never runs. The YAML is a patch over the
    /// default table: `meeting.kind: { mode: active }` changes one field; a prefix key
    /// (`meeting: { mode: off }`, what Settings → Meetings writes) patches every family under it;
    /// `tool.mail: { train: true }` derives a new sub-family from its parent `tool`.
    #[serde(deserialize_with = "families")]
    pub families: BTreeMap<String, FamilyGate>,
    /// The `decide` / `decide_many` runtime functions (92 §3).
    pub tool: ToolConfig,
}

/// `decision.tool` (92 §3, §8): the caps and the batch size of `decide_many`.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct ToolConfig {
    /// Most questions per `decide_many` call; over it the call fails and says to loop over slices.
    pub max_rows: usize,
    /// Most bytes of `state` per question; over it that row is an error.
    pub max_state_bytes: usize,
    /// Rows per `AskMany` (one padded forward); `examples/decide_bench.rs` picks it.
    pub batch: usize,
}

impl Default for ToolConfig {
    fn default() -> Self {
        ToolConfig {
            max_rows: 2000,
            max_state_bytes: 4096,
            batch: 16,
        }
    }
}

impl Default for DecisionConfig {
    fn default() -> Self {
        DecisionConfig {
            enabled: true,
            model_dir: None,
            checkpoints: BTreeMap::new(),
            device: Device::Auto,
            idle_unload: IDLE_UNLOAD,
            log_text: false,
            batch_forward: BATCH_FORWARD,
            families: default_families(),
            tool: ToolConfig::default(),
        }
    }
}

impl DecisionConfig {
    /// The `default` checkpoint: `checkpoints.default`, else `model_dir`, else
    /// `<data_dir>/models/decision`.
    pub fn model_dir_in(&self, data_dir: &Path) -> PathBuf {
        self.checkpoints
            .get(DEFAULT_CHECKPOINT)
            .and_then(|c| c.dir.as_ref())
            .or(self.model_dir.as_ref())
            .cloned()
            .unwrap_or_else(|| data_dir.join("models").join(MODEL_SUBDIR))
    }

    /// The idle unload of `checkpoint` (57 §2.1 item 2): its own `idle_unload` if configured,
    /// else [`BROWSER_IDLE_UNLOAD`] for `browser`, else [`Self::idle_unload`].
    pub fn idle_unload_for(&self, checkpoint: &str) -> Duration {
        self.checkpoints
            .get(checkpoint)
            .and_then(|c| c.idle_unload)
            .unwrap_or(if checkpoint == BROWSER_CHECKPOINT {
                BROWSER_IDLE_UNLOAD
            } else {
                self.idle_unload
            })
    }

    /// The classifier family of `checkpoint`: its `kind` if configured, else gliner for the
    /// `gliner` checkpoint, else laya.
    pub fn kind_for(&self, checkpoint: &str) -> Kind {
        self.checkpoints.get(checkpoint).map_or_else(
            || {
                if checkpoint == GLINER_CHECKPOINT {
                    Kind::Gliner
                } else {
                    Kind::Laya
                }
            },
            |c| c.kind,
        )
    }

    /// Every checkpoint directory by name, given the `default` one: `browser` is
    /// `<default's parent>/decision-browser` unless configured, every configured entry is
    /// included, and every checkpoint some family points at is present (an unconfigured name
    /// resolves to `<default's parent>/decision-<name>`, so the engine reports it missing
    /// instead of the family failing closed on a registry miss).
    pub fn checkpoint_dirs(&self, default_dir: &Path) -> BTreeMap<String, PathBuf> {
        let models = default_dir.parent().unwrap_or(default_dir);
        let mut dirs = BTreeMap::new();
        dirs.insert(DEFAULT_CHECKPOINT.to_owned(), default_dir.to_path_buf());
        dirs.insert(
            BROWSER_CHECKPOINT.to_owned(),
            models.join(format!("{MODEL_SUBDIR}-{BROWSER_CHECKPOINT}")),
        );
        for name in self.families.values().map(FamilyGate::checkpoint) {
            dirs.entry(name.to_owned())
                .or_insert_with(|| models.join(format!("{MODEL_SUBDIR}-{name}")));
        }
        for (name, c) in &self.checkpoints {
            if name == DEFAULT_CHECKPOINT {
                continue;
            }
            match &c.dir {
                Some(dir) => {
                    dirs.insert(name.clone(), dir.clone());
                }
                None => {
                    dirs.entry(name.clone())
                        .or_insert_with(|| models.join(format!("{MODEL_SUBDIR}-{name}")));
                }
            }
        }
        dirs
    }

    /// The directory of `checkpoint`, given the `default` one (see [`Self::checkpoint_dirs`]).
    pub fn checkpoint_dir(&self, default_dir: &Path, checkpoint: &str) -> PathBuf {
        self.checkpoint_dirs(default_dir)
            .remove(checkpoint)
            .unwrap_or_else(|| default_dir.to_path_buf())
    }
}

/// 57 §3.5: `meeting.kind` is the small-talk `Bool` (skip at P(true) ≥ 0.6, the phrasing bench's
/// τ), `meeting.brief` the ≤ 4-candidate `Choice` (publish at P ≥ 0.6, margin ≥ 0.3), and
/// `meeting.subject` the tier-2 subject extraction (never asked of the oracle; listed so its
/// rows fold into `decision_stats` and the `meeting` prefix switch covers it). `meeting.topic`
/// (step 4's `Bool`) is gone: the bench showed it confidently wrong, not uncertain.
///
/// 57 §4.1 item 4: the browser System-1 loop's three jev-shaped questions on the `browser`
/// checkpoint — `browser.operation` executes at P ≥ 0.5, `browser.target` /
/// `browser.type_target` at P ≥ 0.25 (cklxx shipped ungated; the bench decides), no margin, no
/// `none`, state cut allowed. Shadow by default: `browser_run` decides but does not act.
///
/// 92 §2: `tool` is the `decide` tool's family ([`FamilyGate::tool`]).
fn default_families() -> BTreeMap<String, FamilyGate> {
    [
        ("meeting.kind", FamilyGate::shadow(0.60, 0.20)),
        ("meeting.brief", FamilyGate::shadow(0.60, 0.30)),
        ("meeting.subject", FamilyGate::shadow(0.50, 0.0)),
        ("browser.operation", FamilyGate::browser(0.50)),
        ("browser.target", FamilyGate::browser(0.25)),
        ("browser.type_target", FamilyGate::browser(0.25)),
        ("agent.readable", FamilyGate::shadow(0.85, 0.40)),
        (TOOL_FAMILY, FamilyGate::tool()),
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
    train: Option<bool>,
    share: Option<bool>,
    checkpoint: Option<String>,
    none: Option<bool>,
    cut_ok: Option<bool>,
    temperature: Option<f32>,
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
        if let Some(t) = self.train {
            g.train = t;
        }
        if let Some(s) = self.share {
            g.share = s;
        }
        // 57 §7.1: Share is consent to upload what Capture writes; nothing captured, nothing shared.
        if !g.train {
            g.share = false;
        }
        if let Some(c) = &self.checkpoint {
            g.checkpoint = (c != DEFAULT_CHECKPOINT).then(|| c.clone());
        }
        if let Some(n) = self.none {
            g.none = n;
        }
        if let Some(c) = self.cut_ok {
            g.cut_ok = c;
        }
        if let Some(t) = self.temperature {
            g.temperature = t;
        }
    }
}

/// Patch the default table (57 §3.4): exact family names first, then prefix keys, so
/// `meeting: { mode: off }` + `meeting.kind: { mode: active }` still ends with `meeting.kind`
/// off — the switch wins over a hand edit. A new fully-specified family is inserted; a new
/// `parent.child` key whose parent is a family (`tool.mail`) is a sub-family derived from it
/// (92 §2), any subset of fields.
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
        } else if let Some(mut g) = k
            .rsplit_once('.')
            .and_then(|(parent, _)| table.get(parent).cloned())
        {
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
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ModeInfo {
    /// The configured mode (the most permissive one when the families disagree).
    pub mode: Mode,
    /// What runs on the next ask.
    pub effective: Mode,
    /// Why `effective` is `off` while `mode` is not.
    pub reason: Option<String>,
    /// `decision.enabled` — `false` hides Settings → Labs (57 §7.1).
    pub enabled: bool,
    /// The prefix's checkpoint files are on disk.
    pub installed: bool,
    /// Tier-1 budget of the first family under the prefix, ms.
    pub budget_ms: u64,
    /// Tier-2 budget, ms.
    pub escalate_budget_ms: u64,
}

/// The switch position for the families under `prefix` (see [`ModeInfo`]). `model_dir` is the
/// `default` checkpoint; the files checked are those of the checkpoint the prefix's first
/// family names (`browser` → `decision-browser/` beside it, 57 §4.1 item 6).
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
    let (budget, escalate) = gates
        .first()
        .map_or((Duration::from_millis(400), Duration::from_secs(3)), |g| {
            (g.budget, g.escalate_budget)
        });
    let (checkpoint, model_dir) = gates.first().map_or_else(
        || (DEFAULT_CHECKPOINT, model_dir.to_path_buf()),
        |g| {
            (
                g.checkpoint(),
                cfg.checkpoint_dir(model_dir, g.checkpoint()),
            )
        },
    );
    let missing: Vec<&str> = engine::model_files(cfg.kind_for(checkpoint))
        .iter()
        .copied()
        .filter(|f| !model_dir.join(f).is_file())
        .collect();
    let (effective, reason) = if mode == Mode::Off {
        (Mode::Off, None)
    } else if !cfg.enabled {
        (
            Mode::Off,
            Some("the decision plugin is disabled (decision.enabled: false)".to_owned()),
        )
    } else if !missing.is_empty() {
        let what = if checkpoint == DEFAULT_CHECKPOINT {
            "model missing:".to_owned()
        } else {
            format!("checkpoint missing: {MODEL_SUBDIR}-{checkpoint} —")
        };
        (
            Mode::Off,
            Some(format!(
                "{what} {} not in {}",
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
        enabled: cfg.enabled,
        installed: missing.is_empty(),
        budget_ms: budget.as_millis() as u64,
        escalate_budget_ms: escalate.as_millis() as u64,
    }
}

/// The plugin: reads `decision`, spawns one [`Engine`] per checkpoint directory
/// ([`DecisionConfig::checkpoint_dirs`]) — `default` as [`DECISION_ACTOR`], the others as
/// [`actor_name`] — each lazily loaded and idle-unloaded on its own. The actors are registered
/// even when disabled so callers get a fast `Escalate(Disabled)` instead of a registry miss.
/// Only the default engine writes `config.yaml`; it relays `SetMode` / `SetTrain` to the
/// others so every engine's live table agrees.
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
        for (name, dir) in cfg.checkpoint_dirs(&model_dir) {
            let mut engine = Engine::new(cfg.clone(), model_dir.clone(), log.clone())
                .with_checkpoint(&name, dir);
            if name == DEFAULT_CHECKPOINT {
                engine = engine.with_config_path(rt.data_dir.join("config.yaml"));
            }
            Engine::spawn(engine).register(actor_name(&name))?;
        }
        // 92 §1.2: the `decide` tools, once the registry is up; absent when the plugin or the
        // `tool` family is off, and so is the prompt paragraph.
        if cfg.enabled
            && cfg
                .families
                .get(TOOL_FAMILY)
                .is_some_and(|g| g.mode != Mode::Off)
        {
            let decide = std::sync::Arc::new(tool::Decide::new(
                cfg.clone(),
                &rt.data_dir.join("decisions"),
            ));
            let rt = rt.clone();
            tokio::spawn(async move {
                if let Some(reg) = rt
                    .lookup_when::<tools::ToolRegistry>(tools::REGISTRY_ACTOR)
                    .await
                {
                    tool::register(&reg, decide).await;
                }
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_and_yaml() {
        let d = DecisionConfig::default();
        assert_eq!(d.families.len(), 8);
        assert!(
            d.families
                .iter()
                .all(|(k, g)| (g.mode == Mode::Active) == (k == TOOL_FAMILY))
        );
        assert!(d.families.values().all(|g| g.temperature == 1.0));
        // 92 §2: the `decide` tool's family — no thresholds, `none` kept, cut reported, gliner.
        let tool = &d.families[TOOL_FAMILY];
        assert_eq!(
            (
                tool.tau_top,
                tool.tau_margin,
                tool.checkpoint(),
                tool.none,
                tool.cut_ok,
                tool.budget,
                tool.train
            ),
            (
                0.0,
                0.0,
                GLINER_CHECKPOINT,
                true,
                true,
                Duration::from_secs(2),
                false
            )
        );
        assert_eq!(d.kind_for(GLINER_CHECKPOINT), Kind::Gliner);
        assert_eq!(
            (d.tool.max_rows, d.tool.max_state_bytes, d.tool.batch),
            (2000, 4096, 16)
        );
        assert!(
            !d.families.contains_key("meeting.topic"),
            "step 4's topic family is gone (57 §3.5)"
        );
        // 57 §4.1 item 4/6: the three jev-shaped questions on the `browser` checkpoint.
        let op = &d.families["browser.operation"];
        assert_eq!(
            (
                op.tau_top,
                op.tau_margin,
                op.checkpoint(),
                op.none,
                op.cut_ok
            ),
            (0.50, 0.0, "browser", false, true)
        );
        assert_eq!(d.families["browser.target"].tau_top, 0.25);
        assert_eq!(
            d.families["browser.type_target"].budget,
            Duration::from_secs(2)
        );
        assert_eq!(d.families["meeting.kind"].checkpoint(), DEFAULT_CHECKPOINT);
        assert!(d.families["meeting.kind"].none && !d.families["meeting.kind"].cut_ok);
        assert_eq!(
            (
                d.families["meeting.kind"].tau_top,
                d.families["meeting.brief"].tau_margin
            ),
            (0.60, 0.30)
        );
        assert_eq!(
            d.families["meeting.kind"].budget,
            Duration::from_millis(400)
        );
        assert_eq!(
            d.families["meeting.kind"].escalate_budget,
            Duration::from_secs(3)
        );
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
        assert_eq!(cfg.families.len(), 9);
        assert_eq!(cfg.families["meeting.kind"].mode, Mode::Active);
        assert_eq!(cfg.families["meeting.kind"].tau_top, 0.7);
        assert_eq!(cfg.families["meeting.kind"].budget, Duration::from_secs(1));
        assert_eq!(cfg.families["meeting.brief"].mode, Mode::Shadow);
        assert_eq!(cfg.families["new.family"].tau_top, 0.5);
        // `checkpoint`, `none`, `cut_ok`, `temperature` patch like the rest; `checkpoint: default` clears.
        let cfg: DecisionConfig = serde_yaml::from_str(
            "families:\n  meeting.kind: { checkpoint: browser, none: false, cut_ok: true, temperature: 1.7 }\n  browser.target: { checkpoint: default }\ntool: { max_rows: 10, batch: 4 }\n",
        )
        .unwrap();
        let k = &cfg.families["meeting.kind"];
        assert_eq!(
            (k.checkpoint(), k.none, k.cut_ok, k.temperature),
            ("browser", false, true, 1.7)
        );
        assert_eq!(cfg.families["browser.target"].checkpoint, None);
        assert_eq!(
            (cfg.tool.max_rows, cfg.tool.max_state_bytes, cfg.tool.batch),
            (10, 4096, 4)
        );
    }

    /// 92 §2: `tool.<task>` derives from `tool` with any subset of fields; a sub-family of a
    /// non-family (`meeting.topic`) is still a no-op prefix.
    #[test]
    fn tool_sub_family_derives_from_its_parent() {
        let cfg: DecisionConfig = serde_yaml::from_str(
            "families:\n  tool: { checkpoint: default }\n  tool.mail: { train: true, temperature: 2.0 }\n  tool.files: { mode: off }\n  meeting.topic: { train: true }\n",
        )
        .unwrap();
        assert_eq!(cfg.families.len(), 10);
        let mail = &cfg.families["tool.mail"];
        assert!(mail.train && mail.temperature == 2.0 && mail.mode == Mode::Active);
        assert_eq!(
            mail.checkpoint(),
            DEFAULT_CHECKPOINT,
            "derived from the patched parent"
        );
        assert!(mail.cut_ok && mail.none && mail.tau_top == 0.0);
        assert_eq!(cfg.families["tool.files"].mode, Mode::Off);
        assert!(!cfg.families.contains_key("meeting.topic"));
    }

    #[test]
    fn checkpoint_dirs_default_browser_and_overrides() {
        let data = std::path::Path::new("/d");
        let cfg = DecisionConfig::default();
        let default = cfg.model_dir_in(data);
        assert_eq!(default, data.join("models/decision"));
        let dirs = cfg.checkpoint_dirs(&default);
        assert_eq!(dirs.len(), 3);
        assert_eq!(dirs["gliner"], data.join("models/decision-gliner"));
        assert_eq!(dirs["browser"], data.join("models/decision-browser"));
        assert_eq!(cfg.checkpoint_dir(&default, "browser"), dirs["browser"]);
        // `checkpoints.default` wins over `model_dir`; `checkpoints.<x>` overrides the sibling
        // convention; a family on an unknown checkpoint gets the convention's dir.
        let cfg: DecisionConfig = serde_yaml::from_str(
            "model_dir: /old\ncheckpoints: { default: /new, browser: /b, x: /x }\nfamilies:\n  agent.readable: { checkpoint: y }\n",
        )
        .unwrap();
        let default = cfg.model_dir_in(data);
        assert_eq!(default, std::path::Path::new("/new"));
        let dirs = cfg.checkpoint_dirs(&default);
        assert_eq!(dirs["browser"], std::path::Path::new("/b"));
        assert_eq!(dirs["x"], std::path::Path::new("/x"));
        assert_eq!(dirs["y"], std::path::Path::new("/decision-y"));
        assert_eq!(
            (
                actor_name("default").as_str(),
                actor_name("browser").as_str()
            ),
            ("decision", "decision/browser")
        );
        // 57 §2.1 item 2: `{ dir, idle_unload }` entries; `~` keeps the convention; the
        // browser default is 2m unless overridden, everything else follows `idle_unload`.
        let cfg: DecisionConfig = serde_yaml::from_str(
            "idle_unload: 5m\ncheckpoints:\n  browser: { idle_unload: 30s }\n  x: { dir: /x }\n  y: ~\n",
        )
        .unwrap();
        assert_eq!(cfg.idle_unload_for("default"), Duration::from_secs(300));
        assert_eq!(cfg.idle_unload_for("browser"), Duration::from_secs(30));
        assert_eq!(cfg.idle_unload_for("x"), Duration::from_secs(300));
        let dirs = cfg.checkpoint_dirs(&default);
        assert_eq!(dirs["browser"], std::path::Path::new("/decision-browser"));
        assert_eq!(dirs["x"], std::path::Path::new("/x"));
        assert_eq!(dirs["y"], std::path::Path::new("/decision-y"));
        assert_eq!(
            DecisionConfig::default().idle_unload_for("browser"),
            BROWSER_IDLE_UNLOAD
        );
        assert_eq!(DecisionConfig::default().idle_unload, IDLE_UNLOAD);
    }

    #[test]
    fn checkpoint_kind_defaults_to_laya_in_every_entry_form() {
        let cfg: DecisionConfig = serde_yaml::from_str(
            "checkpoints:\n  a: /a\n  b: { dir: /b }\n  c: { idle_unload: 10s }\n  d: ~\n",
        )
        .unwrap();
        for name in ["default", "browser", "a", "b", "c", "d"] {
            assert_eq!(cfg.kind_for(name), Kind::Laya, "{name}");
        }
        assert_eq!(cfg.checkpoints["a"].dir.as_deref(), Some(Path::new("/a")));
        assert_eq!(
            cfg.checkpoints["c"].idle_unload,
            Some(Duration::from_secs(10))
        );
        assert_eq!(DecisionConfig::default().kind_for("default"), Kind::Laya);
    }

    #[test]
    fn gliner_kind_parses_and_mode_for_checks_its_files() {
        let dir = tempfile::tempdir().unwrap();
        let cfg: DecisionConfig = serde_yaml::from_str(
            "checkpoints:\n  guard: { kind: gliner }\n  default: { kind: laya, dir: /l }\nfamilies:\n  guard.inject: { mode: active, tau_top: 0.5, tau_margin: 0.1, checkpoint: guard }\n",
        )
        .unwrap();
        assert_eq!(cfg.kind_for("guard"), Kind::Gliner);
        assert_eq!(cfg.kind_for("default"), Kind::Laya);
        assert_eq!(
            engine::model_files(Kind::Gliner),
            [
                "model.safetensors",
                "tokenizer.json",
                "encoder_config/config.json"
            ]
        );
        // The gliner checkpoint lives at the sibling convention and is checked for its own list.
        let models = dir.path();
        let default = models.join("decision");
        let m = mode_for(&cfg, &default, "guard");
        assert_eq!(m.effective, Mode::Off);
        assert!(!m.installed);
        assert_eq!(
            m.reason.as_deref().unwrap(),
            format!(
                "checkpoint missing: decision-guard — model.safetensors, tokenizer.json, encoder_config/config.json not in {}",
                models.join("decision-guard").display()
            )
        );
        // laya's `encoder/config.json` etc. are not asked of it: the three files alone install it.
        for f in engine::model_files(Kind::Gliner) {
            let p = models.join("decision-guard").join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
        let m = mode_for(&cfg, &default, "guard");
        assert_eq!((m.effective, m.reason), (Mode::Active, None));
        assert!(m.installed);
        // An unknown kind is a config error, not a silent laya.
        assert!(
            serde_yaml::from_str::<DecisionConfig>("checkpoints:\n  x: { kind: bert }\n").is_err()
        );
    }

    #[test]
    fn prefix_entry_sets_every_family_under_it_and_wins() {
        let cfg: DecisionConfig = serde_yaml::from_str(
            "families:\n  meeting.kind: { mode: active }\n  meeting: { mode: off }\n",
        )
        .unwrap();
        assert_eq!(cfg.families.len(), 8);
        assert_eq!(cfg.families["meeting.kind"].mode, Mode::Off);
        assert_eq!(cfg.families["meeting.brief"].mode, Mode::Off);
        assert_eq!(cfg.families["meeting.subject"].mode, Mode::Off);
        // Sky's `config.yaml` line from step 4 (`meeting: { mode: shadow }`) still lands on every family.
        let sky: DecisionConfig =
            serde_yaml::from_str("families:\n  meeting: { mode: shadow }\n").unwrap();
        assert!(
            sky.families
                .iter()
                .filter(|(k, _)| k.starts_with("meeting."))
                .all(|(_, g)| g.mode == Mode::Shadow)
        );
        assert_eq!(cfg.families["browser.target"].mode, Mode::Shadow);
        // A partial unknown family is neither inserted nor an error.
        let cfg: DecisionConfig =
            serde_yaml::from_str("families:\n  nobody: { mode: active }\n").unwrap();
        assert_eq!(cfg.families.len(), 8);
        // 57 §11: `train` patches like `mode`, default false everywhere.
        assert!(cfg.families.values().all(|g| !g.train));
        let train: DecisionConfig =
            serde_yaml::from_str("families:\n  meeting: { train: true }\n").unwrap();
        assert!(
            train
                .families
                .iter()
                .all(|(k, g)| g.train == k.starts_with("meeting.")),
            "{:?}",
            train.families
        );
        assert_eq!(train.families["meeting.kind"].mode, Mode::Shadow);
        // 57 §7.1: `share` patches like `train` and needs it — `train: false` (explicit or default) clears it.
        let share: DecisionConfig =
            serde_yaml::from_str("families:\n  meeting: { train: true, share: true }\n  browser: { share: true }\n  agent.readable: { train: false, share: true }\n")
                .unwrap();
        assert!(share.families["meeting.kind"].share && share.families["meeting.brief"].train);
        assert!(
            !share.families["browser.target"].share,
            "share without train is dropped"
        );
        assert!(!share.families["agent.readable"].share);
    }

    #[test]
    fn mode_resolution_forces_off_with_a_reason() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = DecisionConfig::default();
        let m = mode_for(&cfg, dir.path(), "meeting");
        assert_eq!(m.mode, Mode::Shadow);
        assert_eq!(m.effective, Mode::Off);
        assert!(
            m.reason
                .as_deref()
                .unwrap()
                .starts_with("model missing: model.safetensors")
        );
        assert!(m.enabled && !m.installed);
        assert_eq!((m.budget_ms, m.escalate_budget_ms), (400, 3000));
        // A non-default checkpoint names itself (Labs shows the fetch script for it).
        let b = mode_for(&cfg, dir.path(), "browser");
        assert_eq!(b.effective, Mode::Off);
        assert!(
            b.reason
                .as_deref()
                .unwrap()
                .starts_with("checkpoint missing: decision-browser — model.safetensors"),
            "{:?}",
            b.reason
        );

        for f in engine::model_files(Kind::Laya) {
            let p = dir.path().join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
        let m = mode_for(&cfg, dir.path(), "meeting");
        assert_eq!(
            (m.mode, m.effective, m.reason),
            (Mode::Shadow, Mode::Shadow, None)
        );
        assert!(m.installed);

        let off: DecisionConfig =
            serde_yaml::from_str("families:\n  meeting: { mode: off }\n").unwrap();
        let m = mode_for(&off, dir.path(), "meeting");
        assert_eq!(
            (m.mode, m.effective, m.reason),
            (Mode::Off, Mode::Off, None)
        );

        let disabled: DecisionConfig = serde_yaml::from_str("enabled: false\n").unwrap();
        let m = mode_for(&disabled, dir.path(), "meeting");
        assert_eq!(m.effective, Mode::Off);
        assert!(!m.enabled);
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
