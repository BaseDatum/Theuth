//! 159 W6 — the metric lives in the world: launch press, deploy/measure/rollback under custody,
//! `Waiting` across fires, mSPRT (159 §11). Owner: W6. Signatures frozen by C; `wait`, `resume`
//! and `rearm` are the waiting-and-timer API W7 consumes (159 §18.4).
//!
//! One experiment across fires:
//! 1. *Launch fire.* The session edits the artefact and calls `experiment_run`; `sample` reads
//!    the source once (the control arm's current value) and `step` records a `launches` row
//!    (state `requested`, token hashed) and answers `GateStep::Wait { until: "your approval" }`.
//!    The measure loop stores the row `measured` with `Verdict::Pending`; [`settle_launch`]
//!    then moves it to `Waiting`, pauses the study with `LaunchApproval { n }` and posts the
//!    launch card (W6-d: the flip follows the frozen measure loop instead of living in it).
//! 2. *The press.* [`launch`] approves (snapshot the candidate tree, two detached worktrees,
//!    `deploy` both arms at `max_fraction / 2`, the study runs again with its schedule file
//!    disabled) or refuses (`Discarded`, "refused at launch").
//! 3. *Each window end.* The timer ([`arm`], re-armed by [`rearm`]) runs [`resume`]: `measure`,
//!    one `observations` row per arm, then the ratio check, the guards, the harm test and the
//!    keep test ([`judge`]).
//! 4. *Close.* `rollback` for both arms, always; keep or discard; the schedule file is enabled
//!    again and the next fire starts the next experiment.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use parking_lot::Mutex;
use runtime::SessionId;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::actor::{self, Env};
use crate::error::{Error, Result};
use crate::finalize::Headline;
use crate::git::KeepMeta;
use crate::runner::{SampleRequest, parse_duration};
use crate::schedule_file;
use crate::setup::SetupRow;
use crate::store::{ExperimentRecord, Store, StudyPatch, StudyRecord, now};
use crate::types::{
    ArmWindow, Call, Direction, Effect, ExperimentState, GateCfg, GateDecision, GateInput,
    GateRule, GateStep, KIND_WORLD, KindView, LaunchRecord, ObjectiveDelta, ObjectiveRole,
    ObservationRow, PauseReason, Sample, StudyEvent, StudyInfo, StudySpec, StudyState,
    Verdict, WaitInfo, WaitWhy, WorldSpec,
};

/// The control arm (the incumbent tree).
pub const CONTROL: &str = "control";
/// The candidate arm (the experiment's tree).
pub const CANDIDATE: &str = "candidate";
/// `WaitInfo.until` while a launch waits for its press.
pub const APPROVAL: &str = "your approval";
/// The registry tool the plugin invokes to post the launch card (`crates/app/src/launch_card.rs`).
pub const CARD_TOOL: &str = "autoresearch_launch_card";
/// Optional `CASE total units=<n>` line: production's whole traffic, for the exposure check.
const TOTAL: &str = "total";
/// The ratio check's level (159 §11 "Ratio check").
const RATIO_P: f64 = 0.001;

// ───────────────────────────── statistics ─────────────────────────────

/// mSPRT likelihood ratio (159 §11): Λ = √(V / (V + τ²)) · exp(τ² Δ̂² / (2 V (V + τ²))).
/// `v` is the variance of `delta`; with no variance or no τ there is no evidence (Λ = 1).
pub fn lambda(delta: f64, v: f64, tau: f64) -> f64 {
    if !(v > 0.0) || !(tau > 0.0) || !delta.is_finite() {
        return 1.0;
    }
    let t2 = tau * tau;
    (v / (v + t2)).sqrt() * (t2 * delta * delta / (2.0 * v * (v + t2))).exp()
}

/// χ² (1 df) p-value of two arms' unit counts against a planned 50/50 split (the sample ratio
/// mismatch check).
pub fn ratio_p(a: u64, b: u64) -> f64 {
    let n = (a + b) as f64;
    if n == 0.0 {
        return 1.0;
    }
    let x = (a as f64 - b as f64).powi(2) / n;
    erfc((x / 2.0).sqrt())
}

/// Complementary error function (Numerical Recipes `erfcc`, |ε| < 1.2e-7).
fn erfc(x: f64) -> f64 {
    let z = x.abs();
    let t = 1.0 / (1.0 + 0.5 * z);
    let r = t * (-z * z - 1.265_512_23
        + t * (1.000_023_68
            + t * (0.374_091_96
                + t * (0.096_784_18
                    + t * (-0.186_288_06
                        + t * (0.278_868_07
                            + t * (-1.135_203_98
                                + t * (1.488_515_87
                                    + t * (-0.822_152_23 + t * 0.170_872_77)))))))))
    .exp();
    if x >= 0.0 { r } else { 2.0 - r }
}

/// One arm's cumulated windows: unit-weighted mean, the variance of that mean and the
/// unit-weighted means of the extra fields (guard metrics).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArmStat {
    /// Σ units.
    pub units: u64,
    /// Σ units · mean / Σ units.
    pub mean: f64,
    /// Σ units · var / (Σ units)²: the variance of `mean` over independent windows.
    pub var_of_mean: f64,
    /// Extra fields, unit-weighted.
    pub extras: BTreeMap<String, f64>,
}

/// Cumulate `obs` of one arm.
pub fn cumulate(obs: &[ObservationRow], arm: &str) -> ArmStat {
    let (mut units, mut sum, mut v_num) = (0u64, 0.0, 0.0);
    let mut extras: BTreeMap<String, (f64, u64)> = BTreeMap::new();
    for o in obs.iter().filter(|o| o.arm == arm) {
        let u = u64::from(o.units);
        units += u;
        sum += o.mean.unwrap_or(0.0) * u as f64;
        v_num += o.var.unwrap_or(0.0) * u as f64;
        if let Some(m) = o.raw.as_object() {
            for (k, v) in m {
                if let Some(x) = v.as_f64()
                    && !matches!(k.as_str(), "units" | "mean" | "var")
                {
                    let e = extras.entry(k.clone()).or_default();
                    e.0 += x * u as f64;
                    e.1 += u;
                }
            }
        }
    }
    let n = units as f64;
    ArmStat {
        units,
        mean: if units > 0 { sum / n } else { 0.0 },
        var_of_mean: if units > 0 { v_num / (n * n) } else { 0.0 },
        extras: extras
            .into_iter()
            .filter(|(_, (_, u))| *u > 0)
            .map(|(k, (s, u))| (k, s / u as f64))
            .collect(),
    }
}

/// What one look at the cumulated arms says (159 §11 "The sequential test").
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    /// Windows measured.
    pub windows: u32,
    /// Direction-normalised candidate − control (positive = better).
    pub delta: f64,
    /// Raw relative change of the candidate against the control, %.
    pub delta_pct: f64,
    /// Λ.
    pub lambda: f64,
    /// Always-valid p: min(1, 1/Λ).
    pub p: f64,
    /// Sample-ratio p.
    pub ratio_p: f64,
    /// (control, candidate).
    pub arms: (ArmStat, ArmStat),
    /// Guards past their limit.
    pub breaches: Vec<String>,
}

/// The rule's call after a look.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Λ ≥ 1/α, Δ̂ improving, enough windows and units.
    Keep,
    /// Λ ≥ 1/α_harm with Δ̂ worsening.
    Harm,
    /// A guard metric past its limit.
    Guard(String),
    /// The deploy or the measurement is broken.
    Ratio(String),
    /// `max_windows` without a decision.
    Inconclusive,
    /// Wait for the next window.
    Continue,
}

/// The look and the call on `obs` (every window so far) under `w`, with `tau` in metric units
/// and the guards as `gate_cfg` lists them. `total` is production's whole traffic when the
/// source reports it.
pub fn judge(
    obs: &[ObservationRow],
    direction: Direction,
    w: &WorldSpec,
    tau: f64,
    guards: &[(String, Effect, Direction)],
    total: Option<u64>,
) -> (Look, Outcome) {
    let ctrl = cumulate(obs, CONTROL);
    let cand = cumulate(obs, CANDIDATE);
    let windows = obs.iter().map(|o| o.win).max().unwrap_or(0);
    let raw = cand.mean - ctrl.mean;
    let delta = match direction {
        Direction::Higher => raw,
        Direction::Lower => -raw,
    };
    let delta_pct = if ctrl.mean.abs() > f64::EPSILON {
        raw / ctrl.mean.abs() * 100.0
    } else {
        0.0
    };
    let v = cand.var_of_mean + ctrl.var_of_mean;
    let lam = lambda(delta, v, tau);
    let rp = ratio_p(ctrl.units, cand.units);
    let breaches: Vec<String> = guards
        .iter()
        .filter_map(|(name, limit, dir)| {
            let (c, r) = (cand.extras.get(name)?, ctrl.extras.get(name)?);
            let worse = match dir {
                Direction::Lower => c - r,
                Direction::Higher => r - c,
            };
            (worse > limit.against(*r)).then(|| name.clone())
        })
        .collect();
    let look = Look {
        windows,
        delta,
        delta_pct,
        lambda: lam,
        p: (1.0 / lam).min(1.0),
        ratio_p: rp,
        arms: (ctrl.clone(), cand.clone()),
        breaches: breaches.clone(),
    };
    let min_units = u64::from(w.min_units);
    let outcome = if rp < RATIO_P {
        Outcome::Ratio(format!(
            "sample ratio mismatch: {} control vs {} candidate units (p = {rp:.2e})",
            ctrl.units, cand.units
        ))
    } else if let Some(t) = total.filter(|t| *t > 0)
        && (ctrl.units + cand.units) as f64 / t as f64 > w.exposure.max_fraction * 1.05
    {
        Outcome::Ratio(format!(
            "exposure {:.1} % above the {:.1} % cap",
            (ctrl.units + cand.units) as f64 / t as f64 * 100.0,
            w.exposure.max_fraction * 100.0
        ))
    } else if let Some(g) = breaches.first() {
        Outcome::Guard(g.clone())
    } else if lam >= 1.0 / w.alpha_harm.max(1e-9) && delta < 0.0 {
        Outcome::Harm
    } else if lam >= 1.0 / w.alpha.max(1e-9)
        && delta > 0.0
        && windows >= w.min_windows
        && ctrl.units >= min_units
        && cand.units >= min_units
    {
        Outcome::Keep
    } else if windows >= w.max_windows {
        Outcome::Inconclusive
    } else {
        Outcome::Continue
    };
    (look, outcome)
}

// ───────────────────────────── the spec ─────────────────────────────

fn world_of(spec: &StudySpec) -> Result<&WorldSpec> {
    spec.world
        .as_ref()
        .ok_or_else(|| Error::refused("a world study needs a `world` block"))
}

/// The user's τ, else `metric.min_effect` of the base mean (159 §11).
fn tau_of(spec: &StudySpec, w: &WorldSpec, base_mean: f64) -> f64 {
    w.tau.filter(|t| *t > 0.0).unwrap_or_else(|| {
        Effect::parse(&spec.metric.min_effect)
            .unwrap_or(Effect::Absolute(0.0))
            .against(base_mean)
    })
}

fn window_of(w: &WorldSpec) -> Duration {
    parse_duration(&w.window).unwrap_or(Duration::from_secs(24 * 3600))
}

fn timeout_of(w: &WorldSpec) -> Duration {
    parse_duration(&w.timeout).unwrap_or(Duration::from_secs(600))
}

fn window_span(w: &WorldSpec) -> chrono::Duration {
    chrono::Duration::from_std(window_of(w)).unwrap_or_else(|_| chrono::Duration::hours(24))
}

fn parse_at(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&chrono::Utc))
}

fn fmt_at(t: chrono::DateTime<chrono::Utc>) -> String {
    t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

fn sha256_hex(s: &str) -> String {
    let d = Sha256::digest(s.as_bytes());
    d.iter().map(|b| format!("{b:02x}")).collect()
}

fn new_token() -> String {
    SessionId::new().0.simple().to_string()
}

// ───────────────────────────── the engine handle for the press ─────────────────────────────

/// The engine's `Env`, remembered by every entry the actor calls (`rearm` at start, `sample`,
/// `step`, `wait`) so the `autoresearch.launch` RPC — which has no actor message carrying an
/// `Env` — can reach the store and the backend (W6-b).
static ENGINE: LazyLock<Mutex<Option<Env>>> = LazyLock::new(|| Mutex::new(None));

fn remember(env: &Env) {
    *ENGINE.lock() = Some(env.clone());
}

/// `autoresearch.launch {slug, n, token, approve}` from `rpc_impl` (the `// 159:W6` line).
pub async fn launch_rpc(
    slug: String,
    n: u32,
    token: String,
    approve: bool,
) -> std::result::Result<StudyInfo, String> {
    let env = ENGINE
        .lock()
        .clone()
        .ok_or_else(|| "the autoresearch engine is not running".to_owned())?;
    launch(&env, &slug, n, &token, approve)
        .await
        .map_err(|e| e.to_string())
}

// ───────────────────────────── timers ─────────────────────────────

type TimerKey = (String, u32);

static TIMERS: LazyLock<Mutex<HashMap<TimerKey, tokio::task::AbortHandle>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Arm the window timer for `(slug, n)` at `until` (RFC 3339); `"your approval"` arms nothing.
/// Replaces an earlier timer of the same experiment. Returns whether a timer was armed.
fn arm(env: &Env, slug: &str, n: u32, until: &str) -> bool {
    let Some(at) = parse_at(until) else {
        return false;
    };
    let delay = (at - chrono::Utc::now())
        .to_std()
        .unwrap_or(Duration::ZERO);
    disarm(slug, n);
    let (env, slug_owned) = (env.clone(), slug.to_owned());
    let task = tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        TIMERS.lock().remove(&(slug_owned.clone(), n));
        if let Err(e) = resume(&env, &slug_owned, n).await {
            tracing::warn!(slug = %slug_owned, n, "window end not applied: {e}");
        }
    });
    TIMERS
        .lock()
        .insert((slug.to_owned(), n), task.abort_handle());
    true
}

fn disarm(slug: &str, n: u32) {
    if let Some(h) = TIMERS.lock().remove(&(slug.to_owned(), n)) {
        h.abort();
    }
}

/// Timers currently armed (tests).
pub fn armed() -> usize {
    TIMERS.lock().len()
}

// ───────────────────────────── the waiting API (W7 consumes) ─────────────────────────────

/// Put `exp` into `ExperimentState::Waiting` with `info` (stored in `experiments.wait`), disable
/// the study's schedule file (W-i) and arm a timer for `info.until`.
pub async fn wait(
    env: &Env,
    study: &StudyRecord,
    exp: &mut ExperimentRecord,
    info: WaitInfo,
) -> Result<()> {
    remember(env);
    exp.state = ExperimentState::Waiting;
    exp.wait = Some(info.clone());
    env.store.update_experiment(exp).await?;
    set_file(&study.slug, false).await;
    arm(env, &study.slug, exp.n, &info.until);
    Ok(())
}

/// The timer fired (or W7's inner studies finished): run the kind's resume step; close the
/// experiment or extend the wait; re-enable the schedule file when it closes. Returns the new
/// state. For a kind other than `real world` the kind owns the decision: a row it already
/// closed gets its schedule file back; a row still waiting is returned as it is.
pub async fn resume(env: &Env, slug: &str, n: u32) -> Result<ExperimentState> {
    remember(env);
    let study = study_by_slug(&env.store, slug).await?;
    let Some(mut exp) = env.store.get_experiment(slug, n).await? else {
        return Err(Error::refused(format!("{slug}: no experiment #{n}")));
    };
    if exp.state != ExperimentState::Waiting {
        set_file(slug, true).await;
        return Ok(exp.state);
    }
    if study.spec.kind() != KIND_WORLD {
        return Ok(exp.state);
    }
    if actor::is_terminal(&study.state) || study.state == StudyState::Finalizing {
        close(
            env,
            &study,
            &mut exp,
            Verdict::Refused {
                reason: "the study stopped while the launch ran".into(),
            },
            ExperimentState::Abandoned,
            "abandoned: the study stopped while the launch ran; both arms rolled back".into(),
        )
        .await?;
        return Ok(exp.state);
    }
    match exp.wait.as_ref().map(|w| w.why) {
        Some(WaitWhy::Window) => window_end(env, &study, &mut exp).await,
        // A launch still waiting for its press: nothing fires; the card stands.
        _ => Ok(exp.state),
    }
}

/// At engine start: re-arm one timer per waiting experiment from `experiments.wait.until`.
/// Returns how many. A ledger with no waiting experiment (every 140 ledger) re-arms nothing.
pub async fn rearm(env: &Env) -> Result<u32> {
    remember(env);
    let mut n = 0;
    for exp in env.store.waiting_experiments().await? {
        let Some(info) = exp.wait.as_ref() else {
            continue;
        };
        if arm(env, &exp.slug, exp.n, &info.until) {
            n += 1;
        }
    }
    Ok(n)
}

/// `reconcile`: does a waiting experiment explain this study's disabled schedule file?
pub async fn explains_disabled(store: &Store, slug: &str) -> Result<bool> {
    Ok(store
        .waiting_experiments()
        .await?
        .iter()
        .any(|e| e.slug == slug))
}

async fn set_file(slug: &str, enabled: bool) {
    if let Err(e) = schedule_file::set_enabled(slug, enabled).await {
        tracing::debug!(%slug, enabled, "schedule file not switched: {e}");
    }
}

async fn study_by_slug(store: &Store, slug: &str) -> Result<StudyRecord> {
    store
        .get_study(slug)
        .await?
        .ok_or_else(|| Error::refused(format!("unknown study `{slug}`")))
}

// ───────────────────────────── measurement ─────────────────────────────

/// One arm as the source reports it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArmRead {
    /// Units observed.
    pub units: u32,
    /// Mean outcome.
    pub mean: f64,
    /// Variance per unit.
    pub var: f64,
    /// Every other `<name>=<v>` field (guard metrics).
    pub extras: BTreeMap<String, f64>,
}

/// What one read of the source gave: the arms, and production's whole traffic when reported.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Read {
    /// `control`, `candidate`, … .
    pub arms: BTreeMap<String, ArmRead>,
    /// `CASE total units=<n>`.
    pub total: Option<u64>,
    /// The command's exit, `None` for a tool read.
    pub exit: Option<i32>,
    /// Wall time.
    pub wall_ms: u64,
    /// Why the read is unusable, when it is.
    pub failure: Option<String>,
}

/// `CASE <arm> units=<n> mean=<x> var=<v> [<name>=<v> …]` lines as the v2 scanner stored them.
pub fn parse_arms(case_values: &BTreeMap<String, BTreeMap<String, f64>>) -> Read {
    let mut read = Read::default();
    for (id, fields) in case_values {
        if id == TOTAL {
            read.total = fields.get("units").map(|u| *u as u64);
            continue;
        }
        let (Some(units), Some(mean)) = (fields.get("units"), fields.get("mean")) else {
            continue;
        };
        let mut extras = fields.clone();
        extras.remove("units");
        extras.remove("mean");
        let var = extras.remove("var").unwrap_or(0.0);
        read.arms.insert(
            id.clone(),
            ArmRead {
                units: *units as u32,
                mean: *mean,
                var,
                extras,
            },
        );
    }
    read
}

/// The arms out of a tool's JSON answer by the spec's pointers (W-d).
pub fn parse_tool(v: &serde_json::Value, t: &crate::types::MeasureTool) -> Read {
    let mut read = Read::default();
    let Some(arms) = v.pointer(&t.arms).and_then(|a| a.as_array()) else {
        read.failure = Some(format!("no array at `{}`", t.arms));
        return read;
    };
    let num = |o: &serde_json::Value, p: &str| o.pointer(p).and_then(|x| x.as_f64());
    for o in arms {
        let Some(arm) = o.pointer(&t.arm).and_then(|a| a.as_str()) else {
            continue;
        };
        let (Some(units), Some(mean)) = (num(o, &t.units), num(o, &t.mean)) else {
            continue;
        };
        if arm == TOTAL {
            read.total = Some(units as u64);
            continue;
        }
        let extras = o
            .as_object()
            .map(|m| {
                m.iter()
                    .filter_map(|(k, x)| x.as_f64().map(|x| (k.clone(), x)))
                    .filter(|(k, _)| {
                        let p = format!("/{k}");
                        p != t.units && p != t.mean && p != t.var && p != t.arm
                    })
                    .collect()
            })
            .unwrap_or_default();
        read.arms.insert(
            arm.to_owned(),
            ArmRead {
                units: units as u32,
                mean,
                var: num(o, &t.var).unwrap_or(0.0),
                extras,
            },
        );
    }
    read
}

fn arm_env(arm: &str, n: u32, extra: &[(&str, String)]) -> Vec<(String, String)> {
    let mut v = vec![
        ("AUTORESEARCH_ARM".to_owned(), arm.to_owned()),
        ("AUTORESEARCH_N".to_owned(), n.to_string()),
    ];
    v.extend(extra.iter().map(|(k, x)| ((*k).to_owned(), x.clone())));
    v
}

/// Read the source once: the `measure` command (scanned) or the registry tool.
async fn measure(env: &Env, study: &StudyRecord, n: u32, win: u32) -> Result<Read> {
    let w = world_of(&study.spec)?;
    if let Some(t) = &w.measure_tool {
        let out = env
            .backend
            .invoke_tool(study, &t.name, t.args.clone(), timeout_of(w))
            .await?;
        if out.is_error {
            return Ok(Read {
                failure: Some(format!("`{}`: {}", t.name, out.content.trim())),
                ..Read::default()
            });
        }
        let v = match out.structured {
            Some(v) => v,
            None => serde_json::from_str(&out.content).map_err(|e| {
                Error::refused(format!("`{}` did not answer JSON: {e}", t.name))
            })?,
        };
        return Ok(parse_tool(&v, t));
    }
    if w.measure.trim().is_empty() {
        return Err(Error::refused("world.measure is empty"));
    }
    let extra = arm_env("", n, &[("AUTORESEARCH_WINDOW", win.to_string())]);
    let run = env
        .backend
        .plugin_command(study, "measure", &w.measure, timeout_of(w), &extra)
        .await?;
    let mut read = parse_arms(&run.scanned.case_values);
    read.exit = run.exit;
    read.wall_ms = run.wall_ms;
    if run.exit != Some(0) {
        read.failure = Some(format!("measure exit {:?}", run.exit));
    } else if let Some(bad) = &run.scanned.bad_number {
        read.failure = Some(format!("measure: bad number {bad}"));
    } else if !read.arms.contains_key(CONTROL) {
        read.failure = Some("measure printed no `CASE control units= mean=` line".into());
    }
    Ok(read)
}

/// Engine-side scoring for this kind (`Scoring` dispatch in actor.rs): one `Sample` per tree.
/// Every role reads the source once and takes the control arm — production as it is now: the
/// baseline at Start, the incumbent's re-measures, and the launch fire's one candidate sample
/// (the candidate's own numbers come from the windows after the press).
pub async fn sample(
    env: &Env,
    study: &StudyRecord,
    req: SampleRequest<'_>,
    _cancel: &CancellationToken,
) -> Result<Sample> {
    remember(env);
    let read = measure(env, study, req.n, 0).await?;
    let ctrl = read.arms.get(CONTROL);
    let mut secondaries: BTreeMap<String, f64> = BTreeMap::new();
    if let Some(c) = ctrl {
        secondaries.insert("units".into(), f64::from(c.units));
        secondaries.insert("var".into(), c.var);
        secondaries.extend(c.extras.clone());
    }
    Ok(Sample {
        n: req.n,
        k: req.k,
        tree_sha: req.tree_sha.to_owned(),
        role: Some(req.role),
        value: match (&read.failure, ctrl) {
            (None, Some(c)) => Some(c.mean),
            _ => None,
        },
        secondaries,
        exit: read.exit.or(Some(0)),
        wall_ms: read.wall_ms,
        log_path: Some(format!("{}/runs/plugin/measure-0.log", study.slug)),
        at: now(),
        failure: read.failure,
        ..Sample::default()
    })
}

// ───────────────────────────── the launch fire ─────────────────────────────

/// The kind's decision rule (`GateRule` dispatch in actor.rs). For a world study the launch
/// fire never decides: it records the launch request and waits for the press.
pub async fn step(
    env: &Env,
    study: &StudyRecord,
    _cfg: &GateCfg,
    _input: &GateInput,
    candidate: &[Sample],
    _reference: &[Sample],
) -> Result<GateStep> {
    remember(env);
    let w = world_of(&study.spec)?;
    let n = candidate
        .iter()
        .map(|s| s.n)
        .max()
        .ok_or_else(|| Error::refused("no candidate sample to launch"))?;
    if let Some(open) = env
        .store
        .waiting_experiments()
        .await?
        .iter()
        .find(|e| e.slug == study.slug && e.n != n)
    {
        return Err(Error::refused(format!(
            "launch #{} is still running; one world experiment at a time",
            open.n
        )));
    }
    let token = new_token();
    env.store
        .launches_put(&LaunchRecord {
            slug: study.slug.clone(),
            n,
            state: "requested".into(),
            token_hash: sha256_hex(&token),
            exposure: w.exposure.max_fraction,
            requested_at: now(),
            decided_at: None,
            by: None,
        })
        .await?;
    tokio::spawn(settle_launch(env.clone(), study.clone(), n, token));
    Ok(GateStep::Wait {
        until: APPROVAL.into(),
        why: format!("launch #{n} needs your press"),
    })
}

/// After `step`: the measure loop stores the row `measured` (`Verdict::Pending`); the moment it
/// does, the experiment goes `Waiting`, the study pauses for the press and the card is posted.
/// A row the session closed meanwhile (a crash, an abandon) is left alone.
async fn settle_launch(env: Env, study: StudyRecord, n: u32, token: String) {
    for _ in 0..1200 {
        match env.store.get_experiment(&study.slug, n).await {
            Ok(Some(e)) if e.state == ExperimentState::Measured => {
                if let Err(err) = request(&env, &study, e, &token).await {
                    tracing::warn!(slug = %study.slug, n, "launch request not recorded: {err}");
                }
                return;
            }
            Ok(Some(e)) if e.state == ExperimentState::Measuring => {}
            Ok(Some(_)) | Ok(None) => return,
            Err(err) => {
                tracing::warn!(slug = %study.slug, n, "launch request: {err}");
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tracing::warn!(slug = %study.slug, n, "launch request: the run never settled");
}

/// Record the launch request on a measured row (tests call it directly).
pub async fn request(
    env: &Env,
    study: &StudyRecord,
    mut exp: ExperimentRecord,
    token: &str,
) -> Result<()> {
    wait(
        env,
        study,
        &mut exp,
        WaitInfo {
            until: APPROVAL.into(),
            why: WaitWhy::Approval,
            windows: 0,
        },
    )
    .await?;
    env.store
        .append_event(
            &study.slug,
            "launch",
            &serde_json::json!({ "n": exp.n, "state": "requested" }),
        )
        .await?;
    let mut study = study.clone();
    pause_for_launch(env, &mut study, exp.n).await?;
    post_card(&study, &exp, token).await;
    Ok(())
}

/// `enabled: false` + a `paused` event + `Paused { LaunchApproval { n } }` (the actor's `pause`
/// is private; this is its body with the launch reason).
async fn pause_for_launch(env: &Env, study: &mut StudyRecord, n: u32) -> Result<()> {
    if matches!(study.state, StudyState::Paused { .. }) {
        return Ok(());
    }
    set_file(&study.slug, false).await;
    env.store
        .append_event(
            &study.slug,
            "paused",
            &serde_json::json!({ "reason": "launch_approval", "at": now() }),
        )
        .await?;
    let state = StudyState::Paused {
        reason: PauseReason::LaunchApproval { n },
    };
    if env
        .store
        .patch_study_if(&study.slug, &["running"], &StudyPatch::state(state.clone()))
        .await?
    {
        study.state = state.clone();
        env.rt
            .publish(StudyEvent::StudyChanged {
                slug: study.slug.clone(),
                state,
                reason: Some(format!("launch #{n} waits for your press")),
            })
            .await;
    }
    Ok(())
}

/// Back to `running` after the press (the actor's `resume` is private): the paused time is
/// added, a `resumed` event written; the schedule file is enabled only when `enable_file`
/// (after Approve it stays disabled while the windows run, W-i).
async fn unpause(env: &Env, study: &mut StudyRecord, enable_file: bool) -> Result<()> {
    let mut paused_ms = study.paused_ms;
    if let Some(e) = env
        .store
        .list_events(&study.slug, Some("paused"), 1)
        .await?
        .first()
        && let Some(at) = e.payload.get("at").and_then(|v| v.as_str())
        && let Some(t) = parse_at(at)
    {
        paused_ms += (chrono::Utc::now() - t).num_milliseconds().max(0) as u64;
    }
    env.store
        .append_event(
            &study.slug,
            "resumed",
            &serde_json::json!({ "after_n": last_closed(&env.store, &study.slug).await? }),
        )
        .await?;
    if enable_file {
        set_file(&study.slug, true).await;
    }
    let p = StudyPatch {
        state: Some(StudyState::Running),
        paused_ms: Some(paused_ms),
        ..StudyPatch::default()
    };
    if env
        .store
        .patch_study_if(&study.slug, &["paused"], &p)
        .await?
    {
        p.apply_to(study);
        env.rt
            .publish(StudyEvent::StudyChanged {
                slug: study.slug.clone(),
                state: StudyState::Running,
                reason: None,
            })
            .await;
    }
    Ok(())
}

async fn last_closed(store: &Store, slug: &str) -> Result<u32> {
    Ok(store
        .list_experiments(slug, None, u32::MAX)
        .await?
        .iter()
        .filter(|e| e.state.is_closed())
        .map(|e| e.n)
        .max()
        .unwrap_or(0))
}

/// The launch card: the app card through the registry tool `autoresearch_launch_card` (the
/// presses live there, with the token the model never sees) and a plain notice in the origin
/// chat. Both best-effort: a ledger without a desktop still has the Study view.
async fn post_card(study: &StudyRecord, exp: &ExperimentRecord, token: &str) {
    let w = match world_of(&study.spec) {
        Ok(w) => w,
        Err(_) => return,
    };
    let summary = if exp.hypothesis.trim().is_empty() {
        exp.idea.clone()
    } else {
        format!("{} — {}", exp.idea.trim(), exp.hypothesis.trim())
    };
    let Some(origin) = study.origin else {
        tracing::warn!(slug = %study.slug, n = exp.n, "launch card not posted: no origin session");
        return;
    };
    if let Ok(Some(reg)) = kameo::actor::ActorRef::<::tools::ToolRegistry>::lookup(::tools::REGISTRY_ACTOR)
    {
        let call = ::tools::ToolCall {
            id: format!("autoresearch-launch-{}-{}-{}", study.slug, exp.n, now()),
            name: CARD_TOOL.into(),
            args: serde_json::json!({
                "slug": study.slug, "n": exp.n, "summary": summary,
                "exposure": w.exposure.max_fraction, "window": w.window, "token": token,
                "session": origin,
            }),
            session: origin,
            quiet: true,
        };
        match tokio::time::timeout(Duration::from_secs(30), reg.ask(::tools::Invoke(call))).await {
            Ok(Ok(out)) if !out.is_error => {}
            Ok(Ok(out)) => {
                tracing::warn!(slug = %study.slug, n = exp.n, "launch card refused: {}", out.content)
            }
            Ok(Err(e)) => tracing::warn!(slug = %study.slug, n = exp.n, "launch card: {e}"),
            Err(_) => tracing::warn!(slug = %study.slug, n = exp.n, "launch card: timed out"),
        }
    }
    let text = format!(
        "**Launch #{} of `{}` needs your press** — {}\n\ntwo arms, {:.0} % of traffic together; windows of {}; rollback `{}`.\n\nApprove or refuse on the launch card (this chat's apps) — the study is paused until you do. Open study: [/autoresearch/{slug}](/autoresearch/{slug})",
        exp.n,
        study.slug,
        summary.trim(),
        w.exposure.max_fraction * 100.0,
        w.window,
        w.rollback.trim(),
        slug = study.slug
    );
    notify(origin, &study.slug, text);
}

/// The actor's `deliver` (private): a user channel first, else the origin session itself.
fn notify(origin: SessionId, slug: &str, text: String) {
    let slug = slug.to_owned();
    tokio::spawn(async move {
        let label = format!("autoresearch {slug}");
        if let Ok(Some(user)) =
            kameo::actor::ActorRef::<agent::UserChannels>::lookup(agent::USER_ACTOR)
            && user
                .ask(agent::Deliver {
                    m: agent::Outbound {
                        sessions: vec![origin],
                        origin: label,
                        text: text.clone(),
                        files: vec![],
                    },
                    via: None,
                })
                .await
                .is_ok()
        {
            return;
        }
        let Ok(Some(agents)) = kameo::actor::ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
        else {
            return;
        };
        if let Err(e) = agents
            .ask(agent::SendInput {
                session: Some(origin),
                text: actor::wrapped(&slug, "launch", &text),
                files: vec![],
                delivery: agent::Delivery::default(),
                id: None,
            })
            .await
        {
            tracing::warn!(%slug, "launch notice not delivered: {e}");
        }
    });
}

// ───────────────────────────── the press ─────────────────────────────

fn arm_dir(env: &Env, slug: &str, arm: &str) -> PathBuf {
    crate::study_dir(&env.root, slug).join("arms").join(arm)
}

/// Commit the worktree as it is now (index and all) off any branch: the candidate arm's sha.
fn snapshot(worktree: &Path, parent: &str, n: u32) -> Result<String> {
    crate::git::plumbing(worktree, &["add", "-A"])?;
    let tree = crate::git::plumbing(worktree, &["write-tree"])?;
    let msg = format!("autoresearch launch #{n}");
    let sha = crate::git::plumbing(
        worktree,
        &["commit-tree", tree.trim(), "-p", parent, "-m", &msg],
    )?;
    Ok(sha.trim().to_owned())
}

/// `autoresearch.launch {slug, n, token, approve}`: the press. Approve deploys both arms; refuse
/// closes the experiment as `Discarded`, "refused at launch".
pub async fn launch(env: &Env, slug: &str, n: u32, token: &str, approve: bool) -> Result<StudyInfo> {
    remember(env);
    let mut study = study_by_slug(&env.store, slug).await?;
    let w = world_of(&study.spec)?.clone();
    let Some(mut rec) = env.store.launches_get(slug, n).await? else {
        return Err(Error::refused(format!("{slug}: no launch #{n}")));
    };
    if rec.state != "requested" {
        return Err(Error::refused(format!(
            "launch #{n} was already {}",
            rec.state
        )));
    }
    if rec.token_hash != sha256_hex(token.trim()) {
        return Err(Error::refused(
            "not the launch card's press (token mismatch) — a launch starts from the card's Approve button, not from a tool call",
        ));
    }
    let Some(mut exp) = env.store.get_experiment(slug, n).await? else {
        return Err(Error::refused(format!("{slug}: no experiment #{n}")));
    };
    if exp.state != ExperimentState::Waiting {
        return Err(Error::refused(format!(
            "experiment #{n} is {}, not waiting for a press",
            exp.state.tag()
        )));
    }
    rec.decided_at = Some(now());
    rec.by = Some("user".into());
    if !approve {
        rec.state = "refused".into();
        env.store.launches_put(&rec).await?;
        env.store
            .append_event(slug, "launch", &serde_json::json!({ "n": n, "state": "refused" }))
            .await?;
        close(
            env,
            &study,
            &mut exp,
            Verdict::Refused {
                reason: "refused at launch".into(),
            },
            ExperimentState::Discarded,
            "refused at launch".into(),
        )
        .await?;
        unpause(env, &mut study, true).await?;
        return actor::info_full(env, &study).await;
    }
    // Approve: the arms' trees, two detached worktrees, deploy both at max_fraction / 2.
    let worktree = actor::worktree_of(&study)?;
    let incumbent = actor::incumbent_of(&study)?;
    let repo = PathBuf::from(&study.repo);
    let cand_sha = snapshot(&worktree, &incumbent, n)?;
    let share = (w.exposure.max_fraction / 2.0).to_string();
    for (arm, sha) in [(CONTROL, incumbent.as_str()), (CANDIDATE, cand_sha.as_str())] {
        let dir = arm_dir(env, slug, arm);
        let _ = env.backend.worktree_remove(&repo, &dir);
        env.backend.worktree_add_detached(&repo, &dir, sha)?;
        let extra = arm_env(
            arm,
            n,
            &[
                ("AUTORESEARCH_TREE", sha.to_owned()),
                ("AUTORESEARCH_ARM_WORKTREE", dir.display().to_string()),
                ("AUTORESEARCH_EXPOSURE", share.clone()),
            ],
        );
        let run = env
            .backend
            .plugin_command(&study, &format!("deploy-{arm}"), &w.deploy, timeout_of(&w), &extra)
            .await;
        let failed = match &run {
            Ok(r) if r.exit == Some(0) => None,
            Ok(r) => Some(format!("deploy {arm}: exit {:?}", r.exit)),
            Err(e) => Some(format!("deploy {arm}: {e}")),
        };
        if let Some(line) = failed {
            rec.state = "approved".into();
            env.store.launches_put(&rec).await?;
            close(
                env,
                &study,
                &mut exp,
                Verdict::Crash,
                ExperimentState::Crashed,
                format!("crash: {line}; both arms rolled back"),
            )
            .await?;
            unpause(env, &mut study, true).await?;
            return actor::info_full(env, &study).await;
        }
    }
    rec.state = "deployed".into();
    env.store.launches_put(&rec).await?;
    env.store
        .append_event(
            slug,
            "launch",
            &serde_json::json!({ "n": n, "state": "deployed", "candidate": cand_sha, "control": incumbent }),
        )
        .await?;
    let until = chrono::Utc::now() + window_span(&w);
    exp.line = format!("waiting · window 1 of at most {} · closes {}", w.max_windows, fmt_at(until));
    wait(
        env,
        &study,
        &mut exp,
        WaitInfo {
            until: fmt_at(until),
            why: WaitWhy::Window,
            windows: 0,
        },
    )
    .await?;
    unpause(env, &mut study, false).await?;
    actor::info_full(env, &study).await
}

// ───────────────────────────── window end and close ─────────────────────────────

/// One window ended: measure, store, decide (159 §11 step 3).
async fn window_end(
    env: &Env,
    study: &StudyRecord,
    exp: &mut ExperimentRecord,
) -> Result<ExperimentState> {
    let spec = &study.spec;
    let w = world_of(spec)?;
    let info = exp.wait.clone().unwrap_or_default();
    let win = info.windows + 1;
    let end = parse_at(&info.until).unwrap_or_else(chrono::Utc::now);
    let start = end - window_span(w);
    let read = match measure(env, study, exp.n, win).await {
        Ok(r) => r,
        Err(e) => Read {
            failure: Some(e.to_string()),
            ..Read::default()
        },
    };
    let both = read.arms.contains_key(CONTROL) && read.arms.contains_key(CANDIDATE);
    if read.failure.is_some() || !both {
        let why = read
            .failure
            .unwrap_or_else(|| "measure printed no `CASE candidate` line".into());
        close(
            env,
            study,
            exp,
            Verdict::Crash,
            ExperimentState::Crashed,
            format!("crash: window {win}: {why}; both arms rolled back"),
        )
        .await?;
        return Ok(exp.state);
    }
    for (arm, a) in &read.arms {
        let mut raw = serde_json::Map::new();
        raw.insert("units".into(), f64::from(a.units).into());
        raw.insert("mean".into(), a.mean.into());
        raw.insert("var".into(), a.var.into());
        for (k, v) in &a.extras {
            raw.insert(k.clone(), (*v).into());
        }
        env.store
            .observations_insert(&ObservationRow {
                id: 0,
                slug: study.slug.clone(),
                n: exp.n,
                arm: arm.clone(),
                win,
                start_at: fmt_at(start),
                end_at: fmt_at(end),
                units: a.units,
                mean: Some(a.mean),
                var: Some(a.var),
                raw: serde_json::Value::Object(raw),
                at: now(),
            })
            .await?;
    }
    let obs = env.store.observations_list(&study.slug, Some(exp.n)).await?;
    let base = study
        .incumbent_mean
        .or(env.store.base_mean(&study.slug).await?)
        .unwrap_or(0.0);
    let guards = actor::gate_cfg(&env.cfg, spec).map(|c| c.guards).unwrap_or_default();
    let (look, outcome) = judge(
        &obs,
        spec.metric.direction,
        w,
        tau_of(spec, w, base),
        &guards,
        read.total,
    );
    let (ctrl, cand) = &look.arms;
    exp.gate = Some(GateDecision {
        v: GateDecision::V,
        rule: GateRule::Msprt,
        objectives: vec![ObjectiveDelta {
            name: spec.metric.name.clone(),
            role: ObjectiveRole::Primary,
            candidate: Some(cand.mean),
            reference: Some(ctrl.mean),
            delta: Some(look.delta),
            delta_pct: Some(look.delta_pct),
            z: None,
            call: match &outcome {
                Outcome::Keep => Call::Better,
                Outcome::Harm => Call::Worse,
                Outcome::Guard(_) => Call::GuardFailed,
                _ => Call::Same,
            },
        }],
        stat: Some(look.lambda),
        threshold: Some(1.0 / w.alpha.max(1e-9)),
        n: u32::try_from(ctrl.units + cand.units).unwrap_or(u32::MAX),
    });
    exp.delta = Some(look.delta);
    exp.delta_pct = Some(look.delta_pct);
    exp.metric = Some(cand.mean);
    exp.n_samples = win;
    let unit = spec.metric.unit.as_str();
    let numbers = format!(
        "{} {:.4} vs {:.4} {unit} ({:+.1} %), Λ = {:.2} (p ≤ {:.3}), {} + {} units",
        spec.metric.name, cand.mean, ctrl.mean, look.delta_pct, look.lambda, look.p, ctrl.units, cand.units
    );
    env.store
        .append_event(
            &study.slug,
            "window",
            &serde_json::json!({ "n": exp.n, "win": win, "lambda": look.lambda, "delta": look.delta,
                "p": look.p, "ratio_p": look.ratio_p, "units": [ctrl.units, cand.units] }),
        )
        .await?;
    match outcome {
        Outcome::Continue => {
            let until = end + window_span(w);
            exp.line = format!(
                "waiting · window {} of at most {} · closes {} · {numbers}",
                win + 1,
                w.max_windows,
                fmt_at(until)
            );
            wait(
                env,
                study,
                exp,
                WaitInfo {
                    until: fmt_at(until),
                    why: WaitWhy::Window,
                    windows: win,
                },
            )
            .await?;
        }
        Outcome::Keep => {
            close(
                env,
                study,
                exp,
                Verdict::Improved,
                ExperimentState::Kept,
                format!("kept after {win} windows: {numbers}"),
            )
            .await?;
        }
        Outcome::Harm => {
            close(
                env,
                study,
                exp,
                Verdict::Worse,
                ExperimentState::Discarded,
                format!("worse after {win} windows (rolled back): {numbers}"),
            )
            .await?;
        }
        Outcome::Guard(g) => {
            close(
                env,
                study,
                exp,
                Verdict::GuardFailed,
                ExperimentState::Discarded,
                format!("guard `{g}` past its limit after {win} windows (rolled back): {numbers}"),
            )
            .await?;
        }
        Outcome::Inconclusive => {
            close(
                env,
                study,
                exp,
                Verdict::NoGain,
                ExperimentState::Discarded,
                format!("inconclusive after {win} windows (rolled back): {numbers}"),
            )
            .await?;
        }
        Outcome::Ratio(reason) => {
            close(
                env,
                study,
                exp,
                Verdict::Crash,
                ExperimentState::Crashed,
                format!("crash: {reason}; both arms rolled back"),
            )
            .await?;
            let mut s = study.clone();
            pause_other(env, &mut s, format!("launch #{}: {reason}", exp.n)).await?;
        }
    }
    Ok(exp.state)
}

/// `Paused { Other { reason } }` after a ratio failure (the deploy or the measurement is broken).
async fn pause_other(env: &Env, study: &mut StudyRecord, reason: String) -> Result<()> {
    if matches!(study.state, StudyState::Paused { .. }) {
        return Ok(());
    }
    set_file(&study.slug, false).await;
    env.store
        .append_event(
            &study.slug,
            "paused",
            &serde_json::json!({ "reason": "other", "at": now() }),
        )
        .await?;
    let state = StudyState::Paused {
        reason: PauseReason::Other {
            reason: reason.clone(),
        },
    };
    if env
        .store
        .patch_study_if(&study.slug, &["running"], &StudyPatch::state(state.clone()))
        .await?
    {
        study.state = state.clone();
        env.rt
            .publish(StudyEvent::StudyChanged {
                slug: study.slug.clone(),
                state,
                reason: Some(reason),
            })
            .await;
    }
    Ok(())
}

/// Roll back both arms (always), drop the arm worktrees, mark the launch `rolled_back`.
async fn rollback_all(env: &Env, study: &StudyRecord, n: u32) -> Result<()> {
    let w = world_of(&study.spec)?;
    let repo = PathBuf::from(&study.repo);
    let mut failures = Vec::new();
    for arm in [CONTROL, CANDIDATE] {
        if !w.rollback.trim().is_empty() {
            let dir = arm_dir(env, &study.slug, arm);
            let extra = arm_env(
                arm,
                n,
                &[("AUTORESEARCH_ARM_WORKTREE", dir.display().to_string())],
            );
            match env
                .backend
                .plugin_command(study, &format!("rollback-{arm}"), &w.rollback, timeout_of(w), &extra)
                .await
            {
                Ok(r) if r.exit == Some(0) => {}
                Ok(r) => failures.push(format!("{arm}: exit {:?}", r.exit)),
                Err(e) => failures.push(format!("{arm}: {e}")),
            }
        }
        let dir = arm_dir(env, &study.slug, arm);
        if dir.exists() {
            let _ = env.backend.worktree_remove(&repo, &dir);
        }
    }
    if let Some(mut rec) = env.store.launches_get(&study.slug, n).await?
        && rec.state != "requested"
        && rec.state != "refused"
    {
        rec.state = "rolled_back".into();
        env.store.launches_put(&rec).await?;
    }
    env.store
        .append_event(
            &study.slug,
            "launch",
            &serde_json::json!({ "n": n, "state": "rolled_back", "failures": failures }),
        )
        .await?;
    if !failures.is_empty() {
        tracing::warn!(slug = %study.slug, n, "rollback failed: {}", failures.join("; "));
    }
    Ok(())
}

/// Close a world experiment outside the session: rollback, keep or discard, the `closed` event,
/// the schedule file back on, the stop rules.
async fn close(
    env: &Env,
    study: &StudyRecord,
    exp: &mut ExperimentRecord,
    verdict: Verdict,
    state: ExperimentState,
    line: String,
) -> Result<()> {
    let slug = study.slug.clone();
    disarm(&slug, exp.n);
    rollback_all(env, study, exp.n).await?;
    let worktree = actor::worktree_of(study)?;
    let incumbent = actor::incumbent_of(study)?;
    if exp.description.trim().is_empty() {
        exp.description = if exp.hypothesis.trim().is_empty() {
            exp.idea.clone()
        } else {
            exp.hypothesis.clone()
        };
    }
    exp.verdict = Some(verdict.clone());
    exp.line = line;
    exp.finished_at = Some(now());
    if state == ExperimentState::Kept {
        let meta = KeepMeta {
            slug: slug.clone(),
            n: exp.n,
            description: exp.description.clone(),
            metric_line: actor::metric_line(study, exp),
            category: exp.category.clone(),
            deviations: exp.deviations.clone(),
        };
        let sha = env
            .backend
            .keep(&worktree, &study.spec, &meta, env.cfg.git.author.as_deref())?;
        exp.commit_sha = Some(sha.clone());
        exp.state = ExperimentState::Kept;
        env.store
            .record_keep(exp, &sha, &StudyPatch::incumbent(&sha, exp.metric, study.sigma))
            .await?;
    } else {
        let runs = env.runs_dir(&slug, exp.n);
        std::fs::create_dir_all(&runs)?;
        let patch = runs.join("candidate.patch");
        match env.backend.discard(&worktree, &incumbent, &patch) {
            Ok(hash) => {
                exp.patch_path = Some(format!("{slug}/runs/{}/candidate.patch", exp.n));
                if exp.patch_hash.as_deref().unwrap_or("").is_empty() {
                    exp.patch_hash = Some(hash);
                }
            }
            Err(e) => tracing::warn!(%slug, n = exp.n, "candidate not saved: {e}"),
        }
        exp.state = state;
        env.store.update_experiment(exp).await?;
    }
    let spend = env.backend.session_cost(study.session).await.ok();
    env.store
        .append_event(
            &slug,
            "closed",
            &serde_json::json!({ "n": exp.n, "spend_usd": spend }),
        )
        .await?;
    actor::clear_phase(&env.root, &slug);
    env.rt
        .publish(StudyEvent::ExperimentFinished {
            slug: slug.clone(),
            n: exp.n,
            verdict,
            metric: exp.metric,
            delta_pct: exp.delta_pct,
            z: None,
        })
        .await;
    if !actor::is_terminal(&study.state) && study.state != StudyState::Finalizing {
        set_file(&slug, true).await;
        let env2 = env.clone();
        tokio::spawn(async move {
            if let Err(e) = actor::after_experiment(&env2, &slug).await {
                tracing::warn!(%slug, "stop rules not applied: {e}");
            }
        });
    }
    Ok(())
}

// ───────────────────────────── the Study view, brief, headline, card ─────────────────────────────

/// Λ after each window of experiment `n`, from its observations.
fn lambdas(spec: &StudySpec, w: &WorldSpec, obs: &[ObservationRow], base: f64) -> Vec<f64> {
    let tau = tau_of(spec, w, base);
    let last = obs.iter().map(|o| o.win).max().unwrap_or(0);
    (1..=last)
        .map(|k| {
            let upto: Vec<ObservationRow> = obs.iter().filter(|o| o.win <= k).cloned().collect();
            judge(&upto, spec.metric.direction, w, tau, &[], None).0.lambda
        })
        .collect()
}

/// `KindView::World` for the Study view: every arm × window, the latest launch, Λ per window of
/// the latest launched experiment.
pub async fn view(store: &Store, study: &StudyRecord) -> Result<KindView> {
    let w = world_of(&study.spec)?;
    let obs = store.observations_list(&study.slug, None).await?;
    let windows: Vec<ArmWindow> = obs
        .iter()
        .map(|o| ArmWindow {
            n: o.n,
            arm: o.arm.clone(),
            win: o.win,
            start_at: o.start_at.clone(),
            end_at: o.end_at.clone(),
            units: o.units,
            mean: o.mean,
            var: o.var,
        })
        .collect();
    let mut launch = None;
    for e in store.list_experiments(&study.slug, None, 50).await? {
        if let Some(l) = store.launches_get(&study.slug, e.n).await? {
            launch = Some(l.row());
            break;
        }
    }
    let base = study
        .incumbent_mean
        .or(store.base_mean(&study.slug).await?)
        .unwrap_or(0.0);
    let lambda = match obs.iter().map(|o| o.n).max() {
        Some(n) => {
            let mine: Vec<ObservationRow> = obs.into_iter().filter(|o| o.n == n).collect();
            lambdas(&study.spec, w, &mine, base)
        }
        None => Vec::new(),
    };
    Ok(KindView::World {
        windows,
        launch,
        lambda,
    })
}

/// The brief's per-kind section: how a world experiment runs and what the launches so far did.
pub async fn brief_section(store: &Store, study: &StudyRecord) -> Result<String> {
    let w = world_of(&study.spec)?;
    let mut s = format!(
        "## World study\n\nThe metric `{}` lives in production, not in a benchmark. `experiment_run` does not measure \
         your change: it records a **launch request** and answers `pending` — the user must press Approve \
         before anything goes live. After a `pending` verdict, end your turn with one line; do not call \
         `experiment_log` (the harness closes the experiment itself when the windows end). You never run \
         deploy, measure or rollback.\n\nAfter the press both arms go live on {:.0} % of traffic together \
         (control = the incumbent, candidate = your tree). Windows of {}; at least {} windows and {} units per \
         arm before a keep; at most {} windows, then inconclusive. The decision is an always-valid sequential \
         test (mSPRT, α = {}); harm (α = {}) or a guard breach rolls the candidate back early. A keep moves the \
         study incumbent; production changes only when the user lands it.\n",
        study.spec.metric.name,
        w.exposure.max_fraction * 100.0,
        w.window,
        w.min_windows,
        w.min_units,
        w.max_windows,
        w.alpha,
        w.alpha_harm,
    );
    let mut lines = Vec::new();
    for e in store.list_experiments(&study.slug, None, 12).await? {
        if let Some(l) = store.launches_get(&study.slug, e.n).await? {
            let wins = store
                .observations_list(&study.slug, Some(e.n))
                .await?
                .iter()
                .map(|o| o.win)
                .max()
                .unwrap_or(0);
            lines.push(format!(
                "- #{}: launch {}, {} window{}, {} — {}",
                e.n,
                l.state,
                wins,
                if wins == 1 { "" } else { "s" },
                e.state.tag(),
                e.line.trim()
            ));
        }
    }
    if !lines.is_empty() {
        s.push_str("\nLaunches so far (newest first):\n");
        s.push_str(&lines.join("\n"));
        s.push('\n');
    }
    Ok(s)
}

/// Finalize's headline: base mean (the first windows' control arm, else the ledger baseline)
/// against the last kept candidate's cumulated mean; no confirmation samples are spent.
pub async fn headline(store: &Store, study: &StudyRecord) -> Result<Option<Headline>> {
    let base = store
        .base_mean(&study.slug)
        .await?
        .or(study.incumbent_mean)
        .unwrap_or(0.0);
    let kept: Vec<ExperimentRecord> = store
        .list_experiments(&study.slug, None, u32::MAX)
        .await?
        .into_iter()
        .filter(|e| e.state == ExperimentState::Kept)
        .collect();
    let m = &study.spec.metric;
    let Some(last) = kept.first() else {
        return Ok(Some(Headline {
            text: format!("{} {base:.4} {} — no launch kept", m.name, m.unit),
            base_mean: base,
            final_mean: base,
            delta_pct: 0.0,
            holdout: None,
            sigma: 0.0,
            n: (0, 0),
        }));
    };
    let obs = store.observations_list(&study.slug, Some(last.n)).await?;
    let (ctrl, cand) = (cumulate(&obs, CONTROL), cumulate(&obs, CANDIDATE));
    let final_mean = if cand.units > 0 { cand.mean } else { last.metric.unwrap_or(base) };
    let delta_pct = if base.abs() > f64::EPSILON {
        (final_mean - base) / base.abs() * 100.0
    } else {
        0.0
    };
    let interval = 1.96 * (cand.var_of_mean + ctrl.var_of_mean).sqrt();
    Ok(Some(Headline {
        text: format!(
            "{} {base:.4} → {final_mean:.4} {} ({delta_pct:+.1} %, {} launch{} kept, last Δ ±{interval:.4} at 95 %)",
            m.name,
            m.unit,
            kept.len(),
            if kept.len() == 1 { "" } else { "es" }
        ),
        base_mean: base,
        final_mean,
        delta_pct,
        holdout: None,
        sigma: (cand.var_of_mean + ctrl.var_of_mean).sqrt(),
        n: (
            u32::try_from(ctrl.units).unwrap_or(u32::MAX),
            u32::try_from(cand.units).unwrap_or(u32::MAX),
        ),
    }))
}

/// Draft card line 1 for a real-world study (159 §11 "Draft card").
pub fn card_line(spec: &StudySpec) -> String {
    let w = spec.world.clone().unwrap_or_default();
    let what = spec
        .scope
        .first()
        .map(|s| format!("`{s}`"))
        .unwrap_or_else(|| "the repository".to_owned());
    let rollback = if w.rollback.trim().is_empty() {
        "no rollback command".to_owned()
    } else {
        format!("rollback `{}`", w.rollback.trim())
    };
    format!(
        "Test {what} on {:.0} % of traffic in two arms; every launch needs your press; {} windows ({}–{}); {rollback}.",
        w.exposure.max_fraction * 100.0,
        w.window,
        w.min_windows,
        w.max_windows
    )
}

// ───────────────────────────── setup rows ─────────────────────────────

/// Setup rows this kind adds to the draft panel (159 §11 "Setup chat").
pub const SETUP_ROWS: &[SetupRow] = &[
    SetupRow {
        row: "deploy",
        label: "Deploy",
        leaves: &["world.deploy"],
    },
    SetupRow {
        row: "rollback",
        label: "Rollback",
        leaves: &["world.rollback"],
    },
    SetupRow {
        row: "measure",
        label: "Measure",
        leaves: &["world.measure", "world.measure_tool"],
    },
    SetupRow {
        row: "exposure",
        label: "Exposure",
        leaves: &["world.exposure.max_fraction"],
    },
    SetupRow {
        row: "windows",
        label: "Windows",
        leaves: &[
            "world.window",
            "world.min_windows",
            "world.max_windows",
            "world.alpha",
            "world.alpha_harm",
            "world.tau",
            "world.timeout",
        ],
    },
    SetupRow {
        row: "min_units",
        label: "Units per window",
        leaves: &["world.min_units"],
    },
];

/// One-line value of a kind row for the draft panel (empty = absent).
pub fn setup_value(spec: &StudySpec, row: &str) -> String {
    let Some(w) = spec.world.as_ref() else {
        return String::new();
    };
    match row {
        "deploy" => w.deploy.trim().to_owned(),
        "rollback" => w.rollback.trim().to_owned(),
        "measure" => match &w.measure_tool {
            Some(t) if !t.name.trim().is_empty() => format!("tool `{}`", t.name.trim()),
            _ => w.measure.trim().to_owned(),
        },
        "exposure" => format!(
            "{:.0} % of traffic, {:.0} % per arm",
            w.exposure.max_fraction * 100.0,
            w.exposure.max_fraction * 50.0
        ),
        "windows" => format!(
            "{} · {}–{} windows · α {} · harm α {}{}",
            w.window,
            w.min_windows,
            w.max_windows,
            w.alpha,
            w.alpha_harm,
            w.tau.map(|t| format!(" · τ {t}")).unwrap_or_default()
        ),
        "min_units" => format!("{} per arm per window", w.min_units),
        _ => String::new(),
    }
}

/// Set one kind leaf from the panel's text.
pub fn setup_set(spec: &mut StudySpec, leaf: &str, v: &str) -> std::result::Result<(), String> {
    let v = v.trim();
    let w = spec.world.get_or_insert_with(WorldSpec::default);
    let num = |what: &str| -> std::result::Result<f64, String> {
        v.parse::<f64>()
            .ok()
            .filter(|x| x.is_finite())
            .ok_or_else(|| format!("{what}: `{v}` is not a number"))
    };
    let count = |what: &str| -> std::result::Result<u32, String> {
        v.parse::<u32>()
            .map_err(|_| format!("{what}: `{v}` is not a whole number"))
    };
    match leaf {
        "world.deploy" => w.deploy = v.to_owned(),
        "world.rollback" => w.rollback = v.to_owned(),
        "world.measure" => w.measure = v.to_owned(),
        "world.measure_tool" => {
            w.measure_tool = if v.is_empty() {
                None
            } else {
                Some(serde_json::from_str(v).map_err(|e| format!("world.measure_tool: {e}"))?)
            }
        }
        "world.exposure.max_fraction" => {
            let f = num(leaf)?;
            let f = if f > 1.0 { f / 100.0 } else { f };
            if !(0.0 < f && f <= 1.0) {
                return Err(format!("{leaf}: `{v}` is not a share between 0 and 1"));
            }
            w.exposure.max_fraction = f;
        }
        "world.window" => {
            parse_duration(v).map_err(|e| e.to_string())?;
            w.window = v.to_owned();
        }
        "world.timeout" => {
            parse_duration(v).map_err(|e| e.to_string())?;
            w.timeout = v.to_owned();
        }
        "world.min_units" => w.min_units = count(leaf)?,
        "world.min_windows" => w.min_windows = count(leaf)?.max(1),
        "world.max_windows" => w.max_windows = count(leaf)?.max(1),
        "world.alpha" => {
            let a = num(leaf)?;
            if !(0.0 < a && a < 1.0) {
                return Err(format!("{leaf}: `{v}` is not a level between 0 and 1"));
            }
            w.alpha = a;
        }
        "world.alpha_harm" => {
            let a = num(leaf)?;
            if !(0.0 < a && a < 1.0) {
                return Err(format!("{leaf}: `{v}` is not a level between 0 and 1"));
            }
            w.alpha_harm = a;
        }
        "world.tau" => {
            w.tau = if v.is_empty() {
                None
            } else {
                Some(num(leaf)?).filter(|t| *t > 0.0)
            }
        }
        other => return Err(format!("{other}: not a world leaf")),
    }
    if w.max_windows < w.min_windows {
        w.max_windows = w.min_windows;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
}
