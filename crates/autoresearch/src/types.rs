//! The wire types of design doc 140 (§4.1 the spec, §4.2–4.3 states, §5.2 the
//! gate, §7 the brief, §12.3 messages and events, §12.4 tool arguments and
//! reports, §12.5 RPC shapes). Every type here derives Serialize /
//! Deserialize / JsonSchema and `ts_rs::TS` behind `feature = "ts"`; the
//! actor messages live at the bottom. Other modules add nothing public to
//! the wire: a new field is a change here, announced to every worker.

use std::collections::BTreeMap;

use runtime::SessionId;
use serde::{Deserialize, Serialize};

/// `#[derive]` set every wire type carries (`Default` added per type).
macro_rules! wire {
    ($(#[$m:meta])* pub struct $name:ident $(as $ts:literal)? { $($body:tt)* }) => {
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
        #[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export $(, rename = $ts)?))]
        $(#[$m])*
        pub struct $name { $($body)* }
    };
    ($(#[$m:meta])* pub enum $name:ident $(as $ts:literal)? { $($body:tt)* }) => {
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
        #[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export $(, rename = $ts)?))]
        $(#[$m])*
        pub enum $name { $($body)* }
    };
}

/// Idea row id (`ideas.id`).
pub type IdeaId = i64;
/// Steer row id (`steers.id`).
pub type SteerId = i64;

// ───────────────────────────── §4.1 the specification ─────────────────────────────

wire! {
    /// The study specification the user approves on the draft card (140 §4.1).
    /// Durations are humantime strings (`600s`, `5m`) as in the YAML of §4.1.
    #[serde(default)]
    pub struct StudySpec {
        /// `[a-z0-9-]{1,32}`; schedule = `autoresearch-<slug>`, branch `autoresearch/<slug>`.
        pub slug: String,
        /// What the user wants, one sentence.
        pub goal: String,
        /// Absolute path of the repository.
        pub repo: String,
        /// Ref resolved to a sha at Start (`HEAD` default).
        pub base: String,
        /// The primary metric.
        pub metric: MetricSpec,
        /// Secondaries a keep may not regress past their limit.
        pub secondary_guards: Vec<Guard>,
        /// The benchmark.
        pub bench: BenchSpec,
        /// Pass/fail correctness check, run only on would-be keeps.
        pub checks: Option<CommandSpec>,
        /// The holdout the study session never sees (140 §5.4).
        pub holdout: Option<HoldoutSpec>,
        /// Repo files the evaluator reads: custody + hashes (140 §5.3).
        pub evaluator_paths: Vec<String>,
        /// Globs where edits are expected.
        pub scope: Vec<String>,
        /// Globs that can never be kept.
        pub off_limits: Vec<String>,
        /// Run once at Start, unscored.
        pub warm: Option<String>,
        /// Money, time and count limits.
        pub budget: BudgetSpec,
        /// Candidates per fire; `> 1` = waves (140 §10).
        pub parallel: u8,
        /// Per-study model overrides; `None` = defaults (140 §9.3, R1-D5).
        pub models: ModelsSpec,
        /// A 40 `when:` sensor confining the study to an idle window (140 §5.5).
        pub window: Option<String>,
    }
}

impl Default for StudySpec {
    fn default() -> Self {
        Self {
            slug: String::new(),
            goal: String::new(),
            repo: String::new(),
            base: "HEAD".into(),
            metric: MetricSpec::default(),
            secondary_guards: Vec::new(),
            bench: BenchSpec::default(),
            checks: None,
            holdout: None,
            evaluator_paths: Vec::new(),
            scope: Vec::new(),
            off_limits: Vec::new(),
            warm: None,
            budget: BudgetSpec::default(),
            parallel: 1,
            models: ModelsSpec::default(),
            window: None,
        }
    }
}

wire! {
    /// `metric:` of §4.1.
    #[derive(Default)]
    #[serde(default)]
    pub struct MetricSpec {
        /// The `METRIC <name>=` line that is the primary.
        pub name: String,
        /// Unit for display (`ms`, `MB`, `tokens`); a time or rate unit makes `exclusive` default on.
        pub unit: String,
        /// Which way is better.
        pub direction: Direction,
        /// Smallest effect worth keeping: relative (`0.5%`) or absolute in metric units (`0.3`). See [`Effect`].
        pub min_effect: String,
    }
}

wire! {
    /// Which way a metric improves.
    #[derive(Copy, Default, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum Direction as "MetricDirection" {
        /// Smaller is better (time, size, loss).
        #[default]
        Lower,
        /// Larger is better (throughput, score).
        Higher,
    }
}

wire! {
    /// One `secondary_guards:` entry.
    #[serde(default)]
    #[derive(Default)]
    pub struct Guard as "MetricGuard" {
        /// The secondary's `METRIC` name.
        pub name: String,
        /// Largest allowed regression, relative (`5%`) or absolute; see [`Effect`].
        pub max_regression: String,
    }
}

wire! {
    /// `bench:` of §4.1 (140 §5.1).
    #[serde(default)]
    pub struct BenchSpec {
        /// Custody script or repo command; cwd = the worktree, `/bin/bash -lc`.
        pub command: String,
        /// One sample's limit (humantime); kill = crash.
        pub timeout: String,
        /// Baseline samples (and final confirmation).
        pub repeats: u8,
        /// Take the machine bench slot per scored sample.
        pub exclusive: bool,
        /// Seconds-long pre-check before the first scored sample; non-zero = crash.
        pub smoke: Option<String>,
    }
}

impl Default for BenchSpec {
    fn default() -> Self {
        Self {
            command: String::new(),
            timeout: "600s".into(),
            repeats: 3,
            exclusive: true,
            smoke: None,
        }
    }
}

wire! {
    /// `checks:` of §4.1: a command and its timeout.
    #[serde(default)]
    pub struct CommandSpec {
        /// Same environment as the benchmark; exit 0 = pass.
        pub command: String,
        /// humantime.
        pub timeout: String,
    }
}

impl Default for CommandSpec {
    fn default() -> Self {
        Self {
            command: String::new(),
            timeout: "300s".into(),
        }
    }
}

wire! {
    /// `holdout:` of §4.1 (140 §5.4).
    #[derive(Default)]
    #[serde(default)]
    pub struct HoldoutSpec {
        /// Same contract as the benchmark, other inputs.
        pub command: String,
        /// Run on the incumbent every this many keeps (and at finalize).
        pub every: u32,
        /// Samples per holdout run; `None` = `bench.repeats`.
        pub repeats: Option<u8>,
    }
}

wire! {
    /// `budget:` of §4.1 (R1-D1 defaults).
    #[serde(default)]
    pub struct BudgetSpec {
        /// Study total, USD.
        pub usd: f64,
        /// Per fire (136 `budget:` → `SetBudget`).
        pub per_experiment_usd: f64,
        /// Wall hours since Start, excluding paused time.
        pub hours: f64,
        /// Experiment count.
        pub experiments: u32,
    }
}

impl Default for BudgetSpec {
    fn default() -> Self {
        Self {
            usd: 20.0,
            per_experiment_usd: 1.5,
            hours: 10.0,
            experiments: 120,
        }
    }
}

wire! {
    /// `models:` of §4.1; `None` = defaults (R1-D5).
    #[derive(Default, Eq)]
    #[serde(default)]
    pub struct ModelsSpec {
        /// The study session; default `agent.model`.
        pub executor: Option<String>,
        /// The judge; `"off"` disables; default another vendor.
        pub review: Option<String>,
        /// The plateau consult.
        pub advisor: Option<String>,
    }
}

/// A parsed `min_effect` / `max_regression` (140 §5.2 step 2).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    /// A fraction of the incumbent's mean (`0.5%` → `0.005`).
    Relative(f64),
    /// Metric units.
    Absolute(f64),
}

impl Effect {
    /// `"0.5%"` → `Relative(0.005)`, `"0.3"` → `Absolute(0.3)`, `""` → `Absolute(0.0)`.
    pub fn parse(s: &str) -> std::result::Result<Self, String> {
        let s = s.trim();
        if s.is_empty() {
            return Ok(Self::Absolute(0.0));
        }
        let (num, rel) = match s.strip_suffix('%') {
            Some(n) => (n.trim(), true),
            None => (s, false),
        };
        let v: f64 = num
            .parse()
            .map_err(|_| format!("effect `{s}`: not a number"))?;
        if !v.is_finite() || v < 0.0 {
            return Err(format!(
                "effect `{s}`: must be a finite non-negative number"
            ));
        }
        Ok(if rel {
            Self::Relative(v / 100.0)
        } else {
            Self::Absolute(v)
        })
    }

    /// The threshold in metric units against `reference` (the incumbent's mean).
    pub fn against(self, reference: f64) -> f64 {
        match self {
            Self::Relative(f) => f * reference.abs(),
            Self::Absolute(v) => v,
        }
    }
}

// ───────────────────────────── §4.2 lifecycle ─────────────────────────────

wire! {
    /// Study lifecycle (140 §4.2).
    #[derive(Default, Eq)]
    #[serde(tag = "state", rename_all = "snake_case")]
    pub enum StudyState {
        /// The card is shown; nothing on disk but the draft.
        #[default]
        Draft,
        /// Worktree, custody, warm, baseline.
        Preparing,
        /// Fires back to back.
        Running,
        /// `enabled: false` on the schedule file; the experiment in flight finishes.
        Paused {
            /// Why.
            reason: PauseReason,
        },
        /// §11 in progress.
        Finalizing,
        /// Report delivered; worktree kept until landed or swept.
        Finished,
        /// Could not start or continue.
        Failed {
            /// Why.
            reason: String,
        },
        /// The user discarded it (draft or finished).
        Discarded,
    }
}

impl StudyState {
    /// The serde tag (`"paused"`), the `studies.state` column.
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Preparing => "preparing",
            Self::Running => "running",
            Self::Paused { .. } => "paused",
            Self::Finalizing => "finalizing",
            Self::Finished => "finished",
            Self::Failed { .. } => "failed",
            Self::Discarded => "discarded",
        }
    }
}

wire! {
    /// Why a study paused (140 §4.2, §8.4).
    #[derive(Eq)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    pub enum PauseReason {
        /// A button, slash command or `autoresearch_control`.
        User,
        /// `pause.crash_streak` crashes in a row.
        CrashStreak,
        /// `pause.custody_refusals` refusals in a row.
        CustodyRefusals,
        /// Review flags ≥ `review.pause_after`.
        ReviewFlags,
        /// The draft's budget is above `confirm_budget_usd`.
        BudgetConfirm,
        /// Anything else, in words.
        Other {
            /// Why.
            reason: String,
        },
    }
}

wire! {
    /// Why a study stopped (140 §8.4).
    #[derive(Eq)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    pub enum StopReason {
        /// Stop after this experiment.
        User,
        /// Stop now (the experiment in flight was discarded).
        UserNow,
        /// `budget.experiments` reached.
        Experiments,
        /// Study spend ≥ `budget.usd`.
        Budget,
        /// `budget.hours` elapsed.
        Hours,
        /// `deadline:` reached.
        Deadline,
        /// Two plateau windows in a row.
        Plateau,
        /// The holdout says the search overfit.
        Overfit,
        /// Unrecoverable (worktree gone, …).
        Failed {
            /// Why.
            reason: String,
        },
    }
}

// ───────────────────────────── §4.3 one experiment ─────────────────────────────

wire! {
    /// Experiment lifecycle (140 §4.3, §8.5).
    #[derive(Copy, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum ExperimentState {
        /// `experiment_begin` ran.
        Open,
        /// `experiment_run` is measuring.
        Measuring,
        /// A verdict was returned; waiting for `experiment_log`.
        Measured,
        /// Committed; incumbent moved.
        Kept,
        /// Reverted after `no_gain` / `worse` / a model `discard`.
        Discarded,
        /// The benchmark failed.
        Crashed,
        /// Would have kept, but the check failed.
        ChecksFailed,
        /// Refused before a sample (custody, duplicate, off-limits).
        Refused,
        /// The reviewer called it suspicious; reverted.
        RejectedByReview,
        /// The turn ended without `experiment_log`.
        Abandoned,
        /// Engine restart or stop-now mid-experiment.
        Interrupted,
        /// The run hit `run_timeout`.
        Timeout,
    }
}

wire! {
    /// The gate's verdict (140 §5.2 step 5).
    #[serde(tag = "kind", rename_all = "snake_case")]
    pub enum Verdict as "ExperimentVerdict" {
        /// Cleared the gate.
        Improved,
        /// Not significantly better.
        NoGain,
        /// Significantly worse.
        Worse,
        /// Non-zero exit, timeout, missing primary, NaN.
        Crash,
        /// Would keep, check failed.
        ChecksFailed,
        /// A secondary regressed past its limit.
        GuardFailed,
        /// Refused before a sample was spent.
        Refused {
            /// The refusal text the model sees.
            reason: String,
        },
        /// A keep-candidate flagged for review (jump above `z_suspicious` or 3× the largest step).
        Suspicious,
        /// Granted keep-simpler (not worse, fewer lines, checks pass).
        KeepSimpler,
    }
}

impl Verdict {
    /// The serde tag, for the ledger column and the chart colour.
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Improved => "improved",
            Self::NoGain => "no_gain",
            Self::Worse => "worse",
            Self::Crash => "crash",
            Self::ChecksFailed => "checks_failed",
            Self::GuardFailed => "guard_failed",
            Self::Refused { .. } => "refused",
            Self::Suspicious => "suspicious",
            Self::KeepSimpler => "keep_simpler",
        }
    }

    /// Would the harness keep this (before checks and review)?
    pub fn is_keep_candidate(&self) -> bool {
        matches!(self, Self::Improved | Self::Suspicious | Self::KeepSimpler)
    }
}

wire! {
    /// Where a sample came from (`samples.role`).
    #[derive(Copy, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum SampleRole {
        /// The experiment's tree.
        Candidate,
        /// A re-measure of the incumbent.
        Incumbent,
        /// A segment's baseline.
        Baseline,
        /// The holdout command.
        Holdout,
        /// Finalize's interleaved confirmation.
        Final,
    }
}

wire! {
    /// One benchmark run (`samples` row, 140 §5.1, §12.6).
    #[derive(Default)]
    #[serde(default)]
    pub struct Sample as "BenchSample" {
        /// Experiment number (0 = a baseline/holdout outside an experiment).
        pub n: u32,
        /// Sample index within the experiment.
        pub k: u32,
        /// The tree measured.
        pub tree_sha: String,
        /// Role.
        pub role: Option<SampleRole>,
        /// The primary metric; `None` on crash/lost.
        pub value: Option<f64>,
        /// Other `METRIC` lines.
        pub secondaries: BTreeMap<String, f64>,
        /// `CASE <id> <name>=<v>` lines, keyed `id`.
        pub cases: BTreeMap<String, f64>,
        /// `ASI <key>=<text>` lines (≤ 2 KiB).
        pub asi: BTreeMap<String, String>,
        /// Process exit code; `None` when killed/lost.
        pub exit: Option<i32>,
        /// Wall time.
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        pub wall_ms: u64,
        /// 1-minute load ÷ cores at start.
        pub load: Option<f64>,
        /// Load precondition missed: excluded from σ.
        pub noisy: bool,
        /// The engine died mid-sample.
        pub lost: bool,
        /// `runs/<n>/sample-<k>.log`, root-relative.
        pub log_path: Option<String>,
        /// RFC 3339.
        pub at: String,
    }
}

// ───────────────────────────── §5.2 the gate ─────────────────────────────

/// The gate's parameters for one study (140 §5.2): config `gate:` plus the spec's effect sizes.
#[derive(Clone, Debug, PartialEq)]
pub struct GateCfg {
    /// Direction of the primary.
    pub direction: Direction,
    /// Confirmed keep threshold on means.
    pub z: f64,
    /// One-sample keep.
    pub z_fast: f64,
    /// One-sample discard below this.
    pub z_discard: f64,
    /// Flag for review at or above this.
    pub z_suspicious: f64,
    /// Extra samples in the band.
    pub confirm_runs: u8,
    /// Incumbent sample when its newest is older than this many experiments.
    pub remeasure_after: u32,
    /// Smallest effect worth keeping.
    pub min_effect: Effect,
    /// Secondary limits by name.
    pub guards: Vec<(String, Effect)>,
}

/// What the gate sees after each sample (140 §5.2 steps 1–3).
#[derive(Clone, Debug, PartialEq)]
pub struct GateInput {
    /// Candidate samples so far (primary values, raw units).
    pub candidate: Vec<f64>,
    /// The incumbent's samples since it became incumbent (plus re-measures).
    pub incumbent: Vec<f64>,
    /// Pooled within-tree σ of the segment (0 = deterministic).
    pub sigma: f64,
    /// Largest accepted Δ earlier in the segment (the 3× rule).
    pub largest_accepted_delta: Option<f64>,
    /// Experiments since the incumbent's newest sample.
    pub incumbent_age: u32,
    /// The model asked for `keep_simpler`.
    pub keep_simpler: bool,
    /// Secondary means: `(name, candidate_mean, incumbent_mean)`.
    pub secondaries: Vec<(String, f64, f64)>,
}

/// One step of the sequential decision (140 §5.2 step 3).
#[derive(Clone, Debug, PartialEq)]
pub enum GateStep {
    /// Take more samples before deciding.
    Confirm {
        /// Candidate samples still to take.
        more: u8,
        /// Take one incumbent sample too (drift).
        incumbent_sample: bool,
    },
    /// Decided.
    Decide {
        /// The verdict.
        verdict: Verdict,
        /// Direction-normalised Δ in metric units (positive = better).
        delta: f64,
        /// Δ as a percentage of the incumbent's mean.
        delta_pct: f64,
        /// The z-score behind it (`None` when σ = 0).
        z: Option<f64>,
    },
}

// ───────────────────────────── §12.4 tool arguments and reports ─────────────────────────────

wire! {
    /// `experiment_run` arguments (140 §12.4).
    #[derive(Default)]
    pub struct RunArgs {
        /// The idea in one line (≤ 300).
        pub idea: String,
        /// A short category (`caching`, `algorithm`, `allocation`; ≤ 40).
        pub category: String,
        /// Why it should help (≤ 300).
        pub hypothesis: String,
        /// Override the novelty refusal (needs `reason`).
        #[serde(default)]
        pub force: bool,
        /// Why a retry of a similar idea is legitimate.
        #[serde(default)]
        pub reason: Option<String>,
    }
}

wire! {
    /// `experiment_run` result (140 §12.4).
    #[serde(default)]
    pub struct RunReport {
        /// The decision.
        pub verdict: Verdict,
        /// The verdict in words and numbers (140 §5.2 step 5).
        pub line: String,
        /// Samples taken this call.
        pub samples: Vec<Sample>,
        /// Direction-normalised Δ in metric units.
        pub delta: Option<f64>,
        /// Δ as a percentage.
        pub delta_pct: Option<f64>,
        /// z-score.
        pub z: Option<f64>,
        /// The check, when run.
        pub checks: Option<CheckResult>,
        /// Paths touched outside `scope`.
        pub deviations: Vec<String>,
        /// Past experiments the novelty check found similar.
        pub similar: Vec<SimilarExperiment>,
        /// Last 30 lines / 4 KiB of a crashed sample.
        pub tail: Option<String>,
        /// Per-case deltas that moved most.
        pub cases_moved: Vec<CaseDelta>,
        /// `experiment_run` calls left after a crash.
        pub retries_left: u8,
    }
}

impl Default for RunReport {
    fn default() -> Self {
        Self {
            verdict: Verdict::Crash,
            line: String::new(),
            samples: Vec::new(),
            delta: None,
            delta_pct: None,
            z: None,
            checks: None,
            deviations: Vec::new(),
            similar: Vec::new(),
            tail: None,
            cases_moved: Vec::new(),
            retries_left: 0,
        }
    }
}

wire! {
    /// The correctness check's outcome.
    #[derive(Default)]
    #[serde(default)]
    pub struct CheckResult {
        /// Exit 0.
        pub passed: bool,
        /// Tail of the output when it failed.
        pub tail: Option<String>,
        /// Wall time.
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        pub wall_ms: u64,
    }
}

wire! {
    /// A past experiment the novelty check matched (140 §7.3).
    #[derive(Default)]
    #[serde(default)]
    pub struct SimilarExperiment {
        /// Its number.
        pub n: u32,
        /// Its verdict tag.
        pub verdict: String,
        /// Its Δ %.
        pub delta_pct: Option<f64>,
        /// Its idea.
        pub idea: String,
    }
}

wire! {
    /// One `CASE` id's movement between the incumbent and the candidate.
    #[derive(Default)]
    #[serde(default)]
    pub struct CaseDelta {
        /// The case id.
        pub id: String,
        /// Incumbent value.
        pub before: Option<f64>,
        /// Candidate value.
        pub after: f64,
        /// Relative change, %.
        pub delta_pct: Option<f64>,
    }
}

wire! {
    /// `experiment_log` arguments (140 §12.4).
    #[derive(Default)]
    #[serde(default)]
    pub struct LogArgs {
        /// What you did (≤ 500).
        pub description: String,
        /// What the result teaches (≤ 500).
        pub learned: String,
        /// What to try next (≤ 300).
        pub next: Option<String>,
        /// Downgrade or keep-simpler; `auto` applies the verdict.
        pub decision: Decision,
        /// Required when a keep touches paths outside `scope`.
        pub justification: Option<String>,
        /// Ideas this experiment closes.
        #[cfg_attr(feature = "ts", ts(type = "number[]"))]
        pub closes: Vec<IdeaId>,
    }
}

wire! {
    /// `experiment_log.decision`.
    #[derive(Copy, Default, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum Decision as "LogDecision" {
        /// Apply the verdict.
        #[default]
        Auto,
        /// Throw away an improvement you distrust.
        Discard,
        /// Keep a simplification at equal performance (the harness verifies).
        KeepSimpler,
    }
}

wire! {
    /// `experiment_log` result (140 §12.4).
    #[serde(default)]
    pub struct LogReport {
        /// The experiment's final state.
        pub applied: ExperimentState,
        /// The keep's commit.
        pub commit: Option<String>,
        /// The reviewer's word on a keep.
        pub review: Option<ReviewVerdict>,
        /// The incumbent after this experiment.
        pub incumbent: Incumbent,
        /// What is left.
        pub budget_left: BudgetLeft,
        /// Set when this experiment was the last.
        pub stop: Option<StopReason>,
    }
}

impl Default for LogReport {
    fn default() -> Self {
        Self {
            applied: ExperimentState::Abandoned,
            commit: None,
            review: None,
            incumbent: Incumbent::default(),
            budget_left: BudgetLeft::default(),
            stop: None,
        }
    }
}

wire! {
    /// The incumbent, in short.
    #[derive(Default)]
    #[serde(default)]
    pub struct Incumbent as "StudyIncumbent" {
        /// Its commit.
        pub sha: String,
        /// Its mean of the primary.
        pub mean: Option<f64>,
        /// Experiments kept so far.
        pub kept: u32,
    }
}

wire! {
    /// Budget left (140 §7.2 "Budget left" line).
    #[derive(Default)]
    #[serde(default)]
    pub struct BudgetLeft {
        /// USD.
        pub usd: f64,
        /// Hours.
        pub hours: f64,
        /// Experiments.
        pub experiments: u32,
    }
}

wire! {
    /// Budget used so far.
    #[derive(Default)]
    #[serde(default)]
    pub struct BudgetUsed {
        /// USD (sum of `model_calls.cost` over the study session tree).
        pub usd: f64,
        /// Hours since Start, excluding paused time.
        pub hours: f64,
        /// Experiments closed.
        pub experiments: u32,
    }
}

wire! {
    /// The reviewer's word (140 §9.1).
    #[derive(Eq)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    pub enum ReviewVerdict {
        /// `CLEAN: <reason>`; nothing visible.
        Clean {
            /// The reviewer's sentence.
            reason: String,
        },
        /// `SUSPICIOUS: <reason>` with `on_suspicious: revert` — the keep was undone.
        Reverted {
            /// The reviewer's sentence.
            reason: String,
        },
        /// `SUSPICIOUS: <reason>` with `on_suspicious: flag` — kept, marked.
        Flagged {
            /// The reviewer's sentence.
            reason: String,
        },
        /// Two malformed replies; the keep stands unreviewed.
        Error {
            /// What went wrong.
            reason: String,
        },
    }
}

wire! {
    /// `experiment_notes` arguments (140 §12.4).
    #[derive(Default)]
    #[serde(default)]
    pub struct NotesArgs {
        /// Replaces the playbook whole (≤ `brief.playbook_bytes`).
        pub playbook: Option<String>,
        /// New backlog ideas.
        pub add_ideas: Vec<IdeaDraft>,
    }
}

wire! {
    /// One idea to add.
    #[derive(Default)]
    #[serde(default)]
    pub struct IdeaDraft {
        /// The idea.
        pub text: String,
        /// Its category.
        pub category: String,
        /// 0–3; model ideas default 1, user ideas are always 3.
        pub priority: Option<u8>,
    }
}

wire! {
    /// `experiment_notes` result.
    #[derive(Default)]
    #[serde(default)]
    pub struct NotesReport {
        /// Playbook size after the call.
        pub playbook_bytes: u32,
        /// Ideas added.
        pub ideas_added: u32,
    }
}

wire! {
    /// `experiment_begin` result (140 §7.2, §10 step 1).
    #[derive(Default)]
    #[serde(default)]
    pub struct Brief as "StudyBrief" {
        /// The rendered `<study …>…</study>` block.
        pub text: String,
        /// This experiment's number.
        pub n: u32,
        /// The segment.
        pub segment: u32,
        /// Wave slots (`parallel > 1` only).
        pub slots: Option<Vec<Slot>>,
    }
}

wire! {
    /// One wave slot (140 §10 step 1).
    #[derive(Default)]
    #[serde(default)]
    pub struct Slot as "WaveSlot" {
        /// Slot number.
        pub k: u8,
        /// Candidate worktree path.
        pub worktree: String,
        /// The category assigned to this slot.
        pub category: String,
        /// A backlog idea assigned to it.
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        pub idea_id: Option<IdeaId>,
        /// The experiment number allocated to it.
        pub n: u32,
    }
}

// ───────────────────────────── §12.3 control, ideas, steers ─────────────────────────────

wire! {
    /// `autoresearch_control` / `autoresearch.control` operations (140 §12.3).
    #[derive(Eq)]
    #[serde(tag = "op", rename_all = "snake_case")]
    pub enum ControlOp {
        /// `enabled: false`; the experiment in flight finishes.
        Pause,
        /// `enabled: true`.
        Resume,
        /// Finalize after this experiment.
        StopAfter,
        /// Cancel the tree, reset the worktree, finalize.
        StopNow,
        /// Fire once while paused.
        RunOnce,
        /// New custody + baseline; σ restarts (140 §5.3).
        Segment {
            /// Why the user re-baselined.
            reason: String,
        },
        /// Finalize now.
        Finalize,
        /// Re-apply a reverted keep as the next experiment (140 §9.1).
        Restore {
            /// The reverted experiment.
            n: u32,
        },
        /// Remove worktree and branches.
        Discard,
    }
}

wire! {
    /// When a created study starts (the draft card's two Start buttons).
    #[derive(Default, Eq)]
    #[serde(tag = "when", rename_all = "snake_case")]
    pub enum StartWhen {
        /// Now.
        #[default]
        Now,
        /// Only when the sensor says so (`window:`).
        Window {
            /// A 40 `when:` sensor (`sensors/idle.py`).
            sensor: String,
        },
    }
}

wire! {
    /// Who did it (140 §12.3 `by`).
    #[derive(Copy, Default, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum By as "StudyActor" {
        /// The user: a button, a slash command, a root chat.
        #[default]
        User,
        /// The study session (model).
        Model,
        /// The advisor consult.
        Advisor,
        /// The setup conversation.
        Setup,
        /// The plugin itself (stop rules, pauses).
        Plugin,
    }
}

wire! {
    /// `autoresearch.idea` operations.
    #[derive(Eq)]
    #[serde(tag = "op", rename_all = "snake_case")]
    pub enum IdeaChange {
        /// Add one (user ideas get priority 3 and the ★).
        Add {
            /// The idea.
            text: String,
            /// Its category.
            category: String,
            /// 0–3.
            priority: Option<u8>,
        },
        /// Reprioritise.
        Priority {
            /// Which.
            #[cfg_attr(feature = "ts", ts(type = "number"))]
            id: IdeaId,
            /// 0–3.
            priority: u8,
        },
        /// Close without trying.
        Close {
            /// Which.
            #[cfg_attr(feature = "ts", ts(type = "number"))]
            id: IdeaId,
        },
        /// Reopen.
        Reopen {
            /// Which.
            #[cfg_attr(feature = "ts", ts(type = "number"))]
            id: IdeaId,
        },
    }
}

wire! {
    /// Where an idea came from (`ideas.source`).
    #[derive(Copy, Default, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum IdeaSource {
        /// The user.
        User,
        /// The study session.
        #[default]
        Model,
        /// The plateau consult.
        Advisor,
        /// The setup conversation.
        Setup,
    }
}

wire! {
    /// `ideas.status`.
    #[derive(Copy, Default, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum IdeaStatus {
        /// In the backlog.
        #[default]
        Open,
        /// An experiment matched it.
        Tried,
        /// Closed by the model or the user.
        Closed,
    }
}

wire! {
    /// One `ideas` row (140 §7.3).
    #[derive(Default)]
    #[serde(default)]
    pub struct Idea as "StudyIdea" {
        /// Row id.
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        pub id: IdeaId,
        /// The study.
        pub slug: String,
        /// The idea.
        pub text: String,
        /// Its category.
        pub category: String,
        /// Who added it.
        pub source: IdeaSource,
        /// 0–3.
        pub priority: u8,
        /// Status.
        pub status: IdeaStatus,
        /// Experiments that tried it.
        pub tried_in: Vec<u32>,
        /// RFC 3339.
        pub at: String,
    }
}

wire! {
    /// One `steers` row (140 §7.3).
    #[derive(Default)]
    #[serde(default)]
    pub struct Steer as "StudySteer" {
        /// Row id.
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        pub id: SteerId,
        /// The study.
        pub slug: String,
        /// The text.
        pub text: String,
        /// Who.
        pub by: By,
        /// RFC 3339.
        pub at: String,
        /// The first experiment whose brief carried it.
        pub seen_in: Option<u32>,
    }
}

wire! {
    /// Where an experiment is (`ExperimentProgress.phase`, 140 §12.3).
    #[derive(Copy, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum Phase as "ExperimentPhase" {
        /// Duplicate check.
        Novelty,
        /// Evaluator hashes.
        Custody,
        /// Bench slot or load precondition.
        Waiting,
        /// A benchmark sample is running.
        Sampling,
        /// The correctness check.
        Checks,
        /// The reviewer call.
        Review,
    }
}

// ───────────────────────────── §12.5 RPC shapes ─────────────────────────────

wire! {
    /// One study in the list (140 §12.5 `autoresearch.list`, the TUI line, the routines badge).
    #[derive(Default)]
    #[serde(default)]
    pub struct StudyInfo {
        /// The slug.
        pub slug: String,
        /// The goal.
        pub goal: String,
        /// The repo path.
        pub repo: String,
        /// State.
        pub state: StudyState,
        /// The primary metric.
        pub metric: MetricSpec,
        /// Base sha (after Start).
        pub base_sha: Option<String>,
        /// The incumbent.
        pub incumbent: Incumbent,
        /// Baseline mean of the current segment.
        pub baseline_mean: Option<f64>,
        /// Pooled σ of the current segment.
        pub sigma: Option<f64>,
        /// Current segment.
        pub segment: u32,
        /// Experiments closed.
        pub experiments: u32,
        /// Incumbent vs base, %.
        pub delta_pct: Option<f64>,
        /// One line: "Optimising time_ms in liquid · 34 experiments · −14.2 % · running".
        pub headline: String,
        /// Budget spent.
        pub budget_used: BudgetUsed,
        /// Budget limits.
        pub budget: BudgetSpec,
        /// The study session once bound.
        pub session: Option<SessionId>,
        /// The experiment in flight.
        pub current: Option<u32>,
        /// Its phase.
        pub phase: Option<Phase>,
        /// RFC 3339.
        pub started_at: Option<String>,
        /// RFC 3339.
        pub finished_at: Option<String>,
        /// Why it stopped.
        pub stop_reason: Option<StopReason>,
        /// The report document id once finished.
        pub report_doc: Option<String>,
    }
}

wire! {
    /// `autoresearch.get` (140 §12.5): everything the Study view needs.
    #[derive(Default)]
    #[serde(default)]
    pub struct StudyDetail {
        /// The list row.
        pub info: StudyInfo,
        /// The approved spec.
        pub spec: StudySpec,
        /// The playbook text.
        pub playbook: String,
        /// Chart series.
        pub series: ChartSeries,
        /// Ideas, open first.
        pub ideas: Vec<Idea>,
        /// Steers, newest first.
        pub steers: Vec<Steer>,
        /// Finalize's changesets (finished studies).
        pub changesets: Vec<Changeset>,
        /// Attention items: reverted, deviations, overfit, drift (one line each).
        pub needs_eyes: Vec<String>,
    }
}

wire! {
    /// The chart (140 §13.1): dots, staircase, holdout diamonds, drift markers.
    #[derive(Default)]
    #[serde(default)]
    pub struct ChartSeries as "StudyChartSeries" {
        /// One per experiment.
        pub experiments: Vec<ChartPoint>,
        /// The incumbent staircase.
        pub incumbent: Vec<ChartPoint>,
        /// Holdout points (second axis).
        pub holdout: Vec<ChartPoint>,
        /// Drift events.
        pub drift: Vec<ChartPoint>,
        /// Baseline mean of the current segment.
        pub baseline_mean: Option<f64>,
        /// σ of the current segment (the band).
        pub sigma: Option<f64>,
    }
}

wire! {
    /// One chart point.
    #[derive(Default)]
    #[serde(default)]
    pub struct ChartPoint as "StudyChartPoint" {
        /// Experiment number.
        pub n: u32,
        /// Verdict tag (colour); empty for staircase/holdout/drift points.
        pub verdict: String,
        /// Metric value.
        pub metric: Option<f64>,
        /// RFC 3339.
        pub at: String,
    }
}

wire! {
    /// One finalize changeset (140 §11.2).
    #[derive(Default)]
    #[serde(default)]
    pub struct Changeset as "StudyChangeset" {
        /// Index.
        pub k: u32,
        /// Short name.
        pub name: String,
        /// `autoresearch/<slug>/<k>-<name>`.
        pub branch: String,
        /// Files touched.
        pub files: Vec<String>,
        /// Experiments in it.
        pub experiments: Vec<u32>,
        /// Δ % measured alone on the base.
        pub delta_alone_pct: Option<f64>,
        /// Δ % in the final incumbent.
        pub delta_together_pct: Option<f64>,
        /// Changesets it only helps together with.
        pub only_with: Vec<u32>,
        /// Review verdicts of its keeps (worst first).
        pub review: Option<ReviewVerdict>,
    }
}

wire! {
    /// One run-table row (140 §12.5 `autoresearch.experiments`, §13.1).
    #[derive(Default)]
    #[serde(default)]
    pub struct ExperimentRow {
        /// The study.
        pub slug: String,
        /// Number.
        pub n: u32,
        /// Wave (fire) number; `None` sequential.
        pub wave: Option<u32>,
        /// Slot in the wave.
        pub slot: Option<u8>,
        /// Segment.
        pub segment: u32,
        /// State.
        pub state: Option<ExperimentState>,
        /// Verdict.
        pub verdict: Option<Verdict>,
        /// The idea.
        pub idea: String,
        /// Category.
        pub category: String,
        /// Candidate mean.
        pub metric: Option<f64>,
        /// Direction-normalised Δ.
        pub delta: Option<f64>,
        /// Δ %.
        pub delta_pct: Option<f64>,
        /// z.
        pub z: Option<f64>,
        /// Samples spent.
        pub n_samples: u32,
        /// Keep commit.
        pub commit_sha: Option<String>,
        /// Paths outside scope.
        pub deviations: Vec<String>,
        /// Review.
        pub review: Option<ReviewVerdict>,
        /// Model cost, USD.
        pub cost_usd: f64,
        /// RFC 3339.
        pub started_at: String,
        /// RFC 3339.
        pub finished_at: Option<String>,
    }
}

wire! {
    /// `autoresearch.experiment` (140 §12.5): the row plus everything behind it.
    #[derive(Default)]
    #[serde(default)]
    pub struct ExperimentDetail {
        /// The row.
        pub row: ExperimentRow,
        /// Hypothesis.
        pub hypothesis: String,
        /// Description from `experiment_log`.
        pub description: String,
        /// Learned.
        pub learned: String,
        /// Next.
        pub next: Option<String>,
        /// Decision asked.
        pub decision: Option<Decision>,
        /// Justification for deviations.
        pub justification: Option<String>,
        /// Incumbent sha at the start.
        pub parent_sha: String,
        /// Normalised patch hash.
        pub patch_hash: Option<String>,
        /// Samples.
        pub samples: Vec<Sample>,
        /// The verdict line.
        pub line: String,
        /// The diff / patch text (≤ 256 KiB).
        pub diff: Option<String>,
        /// Log tails per sample.
        pub tails: Vec<String>,
        /// Cases that moved most.
        pub cases_moved: Vec<CaseDelta>,
    }
}

// ───────────────────────────── §12.3 events ─────────────────────────────

wire! {
    /// The plugin's bus events (140 §12.3), bridged to RPC clients.
    #[serde(tag = "type")]
    pub enum StudyEvent {
        /// A study changed state.
        StudyChanged {
            /// Which.
            slug: String,
            /// New state.
            state: StudyState,
            /// Why, when there is more to say.
            reason: Option<String>,
        },
        /// `experiment_begin` ran.
        ExperimentStarted {
            /// Which study.
            slug: String,
            /// Number.
            n: u32,
            /// Wave slot.
            slot: Option<u8>,
        },
        /// Measuring progress.
        ExperimentProgress {
            /// Which study.
            slug: String,
            /// Number.
            n: u32,
            /// Where.
            phase: Phase,
            /// Sample index.
            sample: u8,
            /// Samples planned.
            of: u8,
            /// Since `experiment_begin`.
            #[cfg_attr(feature = "ts", ts(type = "number"))]
            elapsed_ms: u64,
        },
        /// `experiment_log` applied.
        ExperimentFinished {
            /// Which study.
            slug: String,
            /// Number.
            n: u32,
            /// The verdict.
            verdict: Verdict,
            /// Candidate mean.
            metric: Option<f64>,
            /// Δ %.
            delta_pct: Option<f64>,
            /// z.
            z: Option<f64>,
        },
        /// Finalize delivered.
        StudyFinished {
            /// Which study.
            slug: String,
            /// Why.
            reason: StopReason,
            /// `time_ms 52.1 → 41.2 (−20.9 %)`.
            headline: String,
            /// The report document id.
            report_doc: String,
        },
    }
}

// ───────────────────────────── §12.3 actor messages ─────────────────────────────

/// `Create` → `StudyInfo` (state `preparing`).
#[derive(Clone, Debug)]
pub struct Create {
    /// The approved spec.
    pub spec: StudySpec,
    /// The chat the draft card lives in (start and result cards go there).
    pub origin: SessionId,
    /// Now or in the idle window.
    pub start: StartWhen,
}

/// `StartDraft` → `StudyInfo`: the draft card's press (`autoresearch.create {draft_id, start}`).
#[derive(Clone, Debug)]
pub struct StartDraft {
    /// The `app` draft id.
    pub draft_id: String,
    /// Now or in the idle window.
    pub start: StartWhen,
}

/// `Begin` → `Brief`.
#[derive(Clone, Debug)]
pub struct Begin {
    /// The calling session (study session, or a wave child's parent).
    pub session: SessionId,
    /// Wave slot.
    pub slot: Option<u8>,
}

/// `Run` → `RunReport` (blocks while measuring; cancel-safe).
#[derive(Clone, Debug)]
pub struct Run {
    /// The calling session.
    pub session: SessionId,
    /// Wave slot.
    pub slot: Option<u8>,
    /// Arguments.
    pub args: RunArgs,
}

/// `Log` → `LogReport`.
#[derive(Clone, Debug)]
pub struct Log {
    /// The calling session.
    pub session: SessionId,
    /// Wave slot.
    pub slot: Option<u8>,
    /// Arguments.
    pub args: LogArgs,
}

/// `Notes` → `NotesReport`.
#[derive(Clone, Debug)]
pub struct Notes {
    /// The calling session.
    pub session: SessionId,
    /// Arguments.
    pub args: NotesArgs,
}

/// `Control` → `StudyInfo`.
#[derive(Clone, Debug)]
pub struct Control {
    /// Which study.
    pub slug: String,
    /// What.
    pub op: ControlOp,
    /// Who.
    pub by: By,
}

/// `Steer` → `SteerId`.
#[derive(Clone, Debug)]
pub struct SteerMsg {
    /// Which study.
    pub slug: String,
    /// The text.
    pub text: String,
    /// Who.
    pub by: By,
}

/// `IdeaOp` → `Idea`.
#[derive(Clone, Debug)]
pub struct IdeaOp {
    /// Which study.
    pub slug: String,
    /// What.
    pub op: IdeaChange,
    /// Who.
    pub by: By,
}

/// `Status` → `Vec<StudyInfo>` (one, or all newest first).
#[derive(Clone, Debug, Default)]
pub struct Status {
    /// One study, or every study.
    pub slug: Option<String>,
}

/// `Detail` → `StudyDetail` (`autoresearch.get`).
#[derive(Clone, Debug)]
pub struct Detail {
    /// Which study.
    pub slug: String,
}

/// `Experiments` → `Vec<ExperimentRow>` newest first.
#[derive(Clone, Debug)]
pub struct Experiments {
    /// Which study.
    pub slug: String,
    /// Rows with `n < before`.
    pub before: Option<u32>,
    /// Page size.
    pub limit: u32,
}

/// `Experiment` → `ExperimentDetail`.
#[derive(Clone, Debug)]
pub struct Experiment {
    /// Which study.
    pub slug: String,
    /// Number.
    pub n: u32,
}

/// `Settings` → `AutoresearchConfig` (`autoresearch.settings.get`).
#[derive(Clone, Debug, Default)]
pub struct Settings;

/// `SetSettings` → `AutoresearchConfig` persisted and applied (`autoresearch.settings.set`).
#[derive(Clone, Debug)]
pub struct SetSettings(pub crate::config::AutoresearchConfig);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_parses_relative_and_absolute() {
        assert_eq!(Effect::parse("0.5%"), Ok(Effect::Relative(0.005)));
        assert_eq!(Effect::parse(" 5 % "), Ok(Effect::Relative(0.05)));
        assert_eq!(Effect::parse("0.3"), Ok(Effect::Absolute(0.3)));
        assert_eq!(Effect::parse(""), Ok(Effect::Absolute(0.0)));
        assert!(Effect::parse("abc").is_err());
        assert!(Effect::parse("-1%").is_err());
        assert_eq!(Effect::Relative(0.01).against(-200.0), 2.0);
        assert_eq!(Effect::Absolute(0.7).against(42.0), 0.7);
    }

    /// The tagged enums round-trip and carry the design's tags.
    #[test]
    fn tagged_enums_round_trip() {
        let s = StudyState::Paused {
            reason: PauseReason::CrashStreak,
        };
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["state"], "paused");
        assert_eq!(j["reason"]["kind"], "crash_streak");
        assert_eq!(serde_json::from_value::<StudyState>(j).unwrap(), s);
        let v = Verdict::Refused {
            reason: "duplicate of #12".into(),
        };
        let j = serde_json::to_value(&v).unwrap();
        assert_eq!(j["kind"], "refused");
        assert_eq!(v.tag(), "refused");
        assert_eq!(serde_json::from_value::<Verdict>(j).unwrap(), v);
        let e = StudyEvent::ExperimentProgress {
            slug: "s".into(),
            n: 3,
            phase: Phase::Sampling,
            sample: 2,
            of: 3,
            elapsed_ms: 10,
        };
        let j = serde_json::to_value(&e).unwrap();
        assert_eq!(j["type"], "ExperimentProgress");
        assert_eq!(j["phase"], "sampling");
        assert_eq!(serde_json::from_value::<StudyEvent>(j).unwrap(), e);
        let op: ControlOp = serde_json::from_str(r#"{"op":"restore","n":4}"#).unwrap();
        assert_eq!(op, ControlOp::Restore { n: 4 });
    }

    /// A spec with only the required keys fills the R1 defaults; the §4.1 YAML parses.
    #[test]
    fn spec_defaults_and_the_design_yaml() {
        let s: StudySpec = serde_json::from_str(r#"{"slug":"x","goal":"g","repo":"/r"}"#).unwrap();
        assert_eq!(s.budget, BudgetSpec::default());
        assert_eq!(s.bench.timeout, "600s");
        assert!(s.bench.exclusive);
        let yaml = r#"
slug: sort-speed
goal: "Make parse+render faster."
repo: /Users/x/liquid
base: HEAD
metric: {name: time_ms, unit: ms, direction: lower, min_effect: 0.5%}
secondary_guards:
  - {name: alloc_mb, max_regression: 5%}
bench: {command: bench.sh, timeout: 600s, repeats: 3, exclusive: true}
checks: {command: "bundle exec rake test", timeout: 300s}
holdout: {command: bench-holdout.sh, every: 5}
evaluator_paths: [fixtures/, test/, Rakefile]
scope: [lib/liquid/**]
off_limits: [fixtures/**, test/**]
warm: "bundle install"
budget: {usd: 20, per_experiment_usd: 1.5, hours: 10, experiments: 120}
parallel: 1
models: {executor: null, review: null, advisor: null}
window: null
"#;
        let s: StudySpec = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(s.metric.direction, Direction::Lower);
        assert_eq!(
            Effect::parse(&s.metric.min_effect),
            Ok(Effect::Relative(0.005))
        );
        assert_eq!(s.holdout.as_ref().unwrap().every, 5);
        let back: StudySpec = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }
}
