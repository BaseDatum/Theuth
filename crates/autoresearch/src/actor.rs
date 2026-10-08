//! W3 LOOP — the `Studies` actor's handler bodies (design doc 140 §12.3).
//! [`crate::Studies`] (lib.rs) owns the struct and the `Message` impls; each
//! impl calls the function here of the same name, so this file is the whole
//! of the actor's logic: Create (worktree, custody, warm, baseline, schedule
//! file), Begin (drift repair, dangling experiment, brief), Run (novelty,
//! custody, scope, samples, gate, checks), Log (keep/discard, review, stop
//! rules), Notes, Control, Steer, IdeaOp, Status, Detail, Experiments,
//! Experiment, StartDraft. The `slot` paths are W5's (hand-off).
//!
//! Long handlers (Begin, Run, Log, and the preparing / segment / finalize
//! pipelines) run off the mailbox on an [`Env`] clone so Status, Control and
//! a `StopNow` cancel keep flowing while a benchmark runs (lib.rs delegates
//! their replies). Everything the actor calls in W1/W2/W6 goes through
//! [`Backend`], whose default bodies are the real modules; tests override
//! the few they need.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use parking_lot::Mutex;
use std::time::Duration;

use kameo::actor::ActorRef;
use runtime::SessionId;
use tokio_util::sync::CancellationToken;

use crate::Studies;
use crate::config::{AgentDefaults, AutoresearchConfig, OnSuspicious};
use crate::custody::ScopeDiff;
use crate::error::{Error, Result};
use crate::finalize::Headline;
use crate::gate;
use crate::git::KeepMeta;
use crate::novelty::Novelty;
use crate::runner::SampleRequest;
use crate::store::{ExperimentRecord, SegmentRecord, Store, StudyRecord, now};
use crate::types::*;
use crate::{binding, schedule_file, stop};

// ───────────────────────────── transient process state ─────────────────────────────

/// Per-study cancellation of the bench group (`StopNow`, a dropped tool call). Not a row: it is process state.
static CANCELS: LazyLock<Mutex<HashMap<String, CancellationToken>>> =
    LazyLock::new(Default::default);
/// The experiment in flight and its phase, for `StudyInfo.current/phase` (derived display state).
static PHASES: LazyLock<Mutex<HashMap<String, (u32, Phase)>>> = LazyLock::new(Default::default);

/// Map key: the root makes two engines' (or two tests') studies distinct.
fn key(root: &Path, slug: &str) -> String {
    format!("{}/{slug}", root.display())
}

/// The live token for `slug` (created on first use).
pub fn cancel_token(root: &Path, slug: &str) -> CancellationToken {
    CANCELS.lock().entry(key(root, slug)).or_default().clone()
}

/// Cancel whatever `slug` is measuring; the next call gets a fresh token.
pub fn cancel(root: &Path, slug: &str) {
    if let Some(t) = CANCELS.lock().remove(&key(root, slug)) {
        t.cancel();
    }
}

fn set_phase(root: &Path, slug: &str, n: u32, phase: Phase) {
    PHASES.lock().insert(key(root, slug), (n, phase));
}

/// Forget the phase of `slug` (the experiment closed).
pub fn clear_phase(root: &Path, slug: &str) {
    PHASES.lock().remove(&key(root, slug));
}

fn phase_of(root: &Path, slug: &str) -> Option<(u32, Phase)> {
    PHASES.lock().get(&key(root, slug)).copied()
}

// ───────────────────────────── the environment and the backend ─────────────────────────────

/// What a handler needs; cloned off the actor so long work runs off the mailbox.
#[derive(Clone)]
pub struct Env {
    /// `<data_dir>`.
    pub data_dir: PathBuf,
    /// `<data_dir>/autoresearch` or `root:`.
    pub root: PathBuf,
    /// The section.
    pub cfg: AutoresearchConfig,
    /// The `agent:` model defaults.
    pub agent: AgentDefaults,
    /// Events, lookups.
    pub rt: runtime::Runtime,
    /// The ledger.
    pub store: Store,
    /// W1/W2/W6 seams.
    pub backend: Arc<dyn Backend>,
}

impl Studies {
    /// A clone of the actor's handles for off-mailbox work.
    pub fn env(&self) -> Env {
        Env {
            data_dir: self.data_dir.clone(),
            root: self.root.clone(),
            cfg: self.cfg.clone(),
            agent: self.agent.clone(),
            rt: self.rt.clone(),
            store: self.store.clone(),
            backend: self.backend.clone(),
        }
    }
}

impl Env {
    async fn publish(&self, ev: StudyEvent) {
        self.rt.publish(ev).await;
    }

    fn study_dir(&self, slug: &str) -> PathBuf {
        crate::study_dir(&self.root, slug)
    }

    fn runs_dir(&self, slug: &str, n: u32) -> PathBuf {
        self.study_dir(slug).join("runs").join(n.to_string())
    }

    fn evaluator_dir(&self, slug: &str, segment: u32) -> PathBuf {
        self.study_dir(slug)
            .join("evaluator")
            .join(segment.to_string())
    }
}

/// Every call the actor makes into W1 (measure), W2 (ratchet), W6 (memory)
/// and the engine's accounting. The default bodies are the production code;
/// a test double overrides what its scenario needs.
#[async_trait::async_trait]
pub trait Backend: Send + Sync {
    // ── W2 git ──
    /// A git repository (colocated jj counts)?
    fn is_git_repo(&self, repo: &Path) -> bool {
        crate::git::is_git_repo(repo)
    }
    /// `base` → sha; a dirty tree is refused.
    fn resolve_base(&self, repo: &Path, base: &str) -> Result<String> {
        crate::git::resolve_base(repo, base, false)
    }
    /// `git worktree add -b autoresearch/<slug>`; returns HEAD.
    fn worktree_add(
        &self,
        repo: &Path,
        spec: &StudySpec,
        worktree: &Path,
        base_sha: &str,
    ) -> Result<String> {
        crate::git::worktree_add(repo, spec, worktree, base_sha)
    }
    /// `git worktree remove --force` + prune.
    fn worktree_remove(&self, repo: &Path, worktree: &Path) -> Result<()> {
        crate::git::worktree_remove(repo, worktree)
    }
    /// `add -A` (evaluator paths excluded) + commit with trailers; the new sha.
    fn keep(
        &self,
        worktree: &Path,
        spec: &StudySpec,
        meta: &KeepMeta,
        author: Option<&str>,
    ) -> Result<String> {
        crate::git::keep(worktree, spec, meta, author)
    }
    /// Save the candidate patch, `reset --hard`, `clean -fd`; the patch's sha256.
    fn discard(&self, worktree: &Path, incumbent: &str, patch_path: &Path) -> Result<String> {
        crate::git::discard(worktree, incumbent, patch_path)
    }
    /// Drift repair at begin; `Some(what)` when something was wrong.
    fn repair(
        &self,
        worktree: &Path,
        spec: &StudySpec,
        incumbent: &str,
        stray: &Path,
    ) -> Result<Option<String>> {
        crate::git::repair(worktree, spec, incumbent, stray)
    }
    /// Revert a reviewed keep; the commit stays as a rejected ref.
    fn reject(&self, worktree: &Path, slug: &str, n: u32, previous: &str) -> Result<()> {
        crate::git::reject(worktree, slug, n, previous)
    }
    /// Re-apply a rejected keep to the working tree, uncommitted (the next candidate).
    fn restore_rejected(&self, worktree: &Path, slug: &str, n: u32) -> Result<()> {
        crate::git::restore_rejected(worktree, slug, n)
    }
    /// `git merge --no-ff <branch>` into the user's current branch (clean tree only); the merge sha.
    fn merge_into_checkout(
        &self,
        repo: &Path,
        branch: &str,
        author: Option<&str>,
    ) -> Result<String> {
        crate::git::merge_into_checkout(repo, branch, author)
    }
    /// Normalised patch hash of the worktree vs the incumbent.
    fn patch_hash(&self, worktree: &Path, incumbent: &str) -> Result<String> {
        crate::git::patch_hash(worktree, incumbent)
    }
    /// `git diff a b`, capped.
    fn diff_text(&self, worktree: &Path, a: &str, b: &str, max_bytes: usize) -> Result<String> {
        crate::git::diff_text(worktree, a, b, max_bytes)
    }
    // ── W1 measure ──
    /// One scored sample through the slot and the load check.
    async fn sample(
        &self,
        spec: &StudySpec,
        cfg: &crate::config::Bench,
        req: SampleRequest<'_>,
        cancel: &CancellationToken,
    ) -> Result<Sample> {
        crate::runner::sample(spec, cfg, req, cancel).await
    }
    /// Checks / smoke / warm / holdout: `(exit, tail, wall_ms)`.
    async fn command(
        &self,
        command: &str,
        timeout: Duration,
        req: SampleRequest<'_>,
        cancel: &CancellationToken,
    ) -> Result<(Option<i32>, String, u64)> {
        crate::runner::command(command, timeout, req, cancel).await
    }
    /// Last 30 lines / 4 KiB of a sample log.
    fn tail(&self, log: &Path) -> String {
        crate::runner::tail(log)
    }
    /// Copy the evaluator into custody; `{path: sha256}`.
    fn custody_take(
        &self,
        spec: &StudySpec,
        repo: &Path,
        evaluator_dir: &Path,
    ) -> Result<BTreeMap<String, String>> {
        crate::custody::take(spec, repo, evaluator_dir)
    }
    /// Changed custody paths in the worktree (empty = intact).
    fn custody_verify(
        &self,
        custody: &BTreeMap<String, String>,
        worktree: &Path,
    ) -> Result<Vec<String>> {
        crate::custody::verify(custody, worktree)
    }
    /// What the worktree changed vs the incumbent, classified.
    fn scope_diff(&self, spec: &StudySpec, worktree: &Path) -> Result<ScopeDiff> {
        crate::custody::scope_diff(spec, worktree)
    }
    /// Pooled within-tree σ.
    fn pooled_sigma(&self, trees: &[Vec<f64>]) -> f64 {
        gate::pooled_sigma(trees)
    }
    /// One step of the sequential decision.
    fn gate_step(&self, cfg: &GateCfg, input: &GateInput) -> GateStep {
        gate::step(cfg, input)
    }
    /// The keep-simpler criterion.
    fn keep_simpler(&self, cfg: &GateCfg, input: &GateInput, added: u32, removed: u32) -> bool {
        gate::keep_simpler(cfg, input, added, removed)
    }
    /// A re-measure outside 3σ.
    fn drifted(&self, sigma: f64, incumbent_mean: f64, sample: f64) -> bool {
        gate::drifted(sigma, incumbent_mean, sample)
    }
    /// The verdict in words and numbers.
    fn gate_line(
        &self,
        step: &GateStep,
        unit: &str,
        candidate_mean: f64,
        incumbent_mean: f64,
        n: usize,
    ) -> String {
        gate::line(step, unit, candidate_mean, incumbent_mean, n)
    }
    // ── W6 memory ──
    /// Render the §7.2 brief.
    async fn brief(
        &self,
        store: &Store,
        study: &StudyRecord,
        cfg: &crate::config::BriefConfig,
        cx: &crate::brief::Context,
    ) -> Result<Brief> {
        crate::brief::render(store, study, cfg, cx).await
    }
    /// The novelty check.
    async fn novelty(
        &self,
        store: &Store,
        slug: &str,
        args: &RunArgs,
        patch_hash: &str,
        threshold: f64,
        flagged_before: bool,
    ) -> Result<Novelty> {
        crate::novelty::check(store, slug, args, patch_hash, threshold, flagged_before).await
    }
    /// Mark matching backlog ideas `tried`.
    async fn mark_tried(&self, store: &Store, slug: &str, idea: &str, n: u32) -> Result<Vec<i64>> {
        crate::novelty::mark_tried(store, slug, idea, n).await
    }
    /// The reviewer's word on a keep.
    async fn review(
        &self,
        store: &Store,
        study: &StudyRecord,
        n: u32,
        model: &str,
        rt: &runtime::Runtime,
    ) -> Result<ReviewVerdict> {
        crate::review::review(store, study, n, model, rt).await
    }
    /// The plateau consult.
    async fn advise(
        &self,
        store: &Store,
        study: &StudyRecord,
        model: Option<&str>,
        rt: &runtime::Runtime,
    ) -> Result<Vec<IdeaDraft>> {
        crate::review::advise(store, study, model, rt).await
    }
    // ── W2 finalize / record ──
    /// Every §11 step; the headline and the report document id.
    async fn finalize(
        &self,
        store: &Store,
        study: &StudyRecord,
        reason: StopReason,
        rt: &runtime::Runtime,
    ) -> Result<(Headline, String)> {
        crate::finalize::run(store, study, reason, rt).await
    }
    /// The cross-study knowledge record.
    async fn record(
        &self,
        store: &Store,
        study: &StudyRecord,
        headline: &str,
        rt: &runtime::Runtime,
    ) -> Result<String> {
        crate::record::write(store, study, headline, rt).await
    }
    /// The playbook of the last record for this repo.
    async fn seed_playbook(&self, repo: &str, rt: &runtime::Runtime) -> Result<Option<String>> {
        crate::record::seed_playbook(repo, rt).await
    }
    // ── the engine ──
    /// Study spend: `model_calls.cost` over the session tree (128's ledger via `SessionCostQuery`).
    async fn spend_usd(&self, session: Option<SessionId>) -> f64 {
        let Some(session) = session else { return 0.0 };
        let Some(agents) = agents() else { return 0.0 };
        match agents.ask(agent::SessionCostQuery(session)).await {
            Ok(Some(cost)) => cost
                .total
                .returned
                .iter()
                .filter(|m| m.currency == "USD")
                .map(|m| m.nanos as f64 / 1e9)
                .sum(),
            _ => 0.0,
        }
    }
    /// `code_map(focus=scope, budget=…)` through the tool registry; `None` when unavailable.
    async fn code_map(
        &self,
        rt: &runtime::Runtime,
        repo: &str,
        scope: &[String],
        budget: u32,
    ) -> Option<String> {
        let _ = rt;
        let reg = ActorRef::<::tools::ToolRegistry>::lookup(::tools::REGISTRY_ACTOR)
            .ok()
            .flatten()?;
        let call = ::tools::ToolCall {
            id: format!("autoresearch-code-map-{}", now()),
            name: "code_map".into(),
            args: serde_json::json!({ "focus": scope, "root": repo, "budget": budget }),
            session: SessionId::new(),
            quiet: true,
        };
        let out = tokio::time::timeout(Duration::from_secs(60), reg.ask(::tools::Invoke(call)))
            .await
            .ok()?
            .ok()?;
        (!out.is_error && !out.content.trim().is_empty()).then_some(out.content)
    }
}

/// The production backend: every method is the trait's default.
pub struct Real;
impl Backend for Real {}

fn agents() -> Option<ActorRef<agent::Agents>> {
    ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
        .ok()
        .flatten()
}

// ───────────────────────────── small helpers ─────────────────────────────

/// The study bound to `session`, or the §8.2 refusal.
async fn study_of(env: &Env, session: SessionId) -> Result<StudyRecord> {
    env.store
        .study_of_session(session)
        .await?
        .ok_or_else(binding::not_a_study_session)
}

async fn study_by_slug(store: &Store, slug: &str) -> Result<StudyRecord> {
    store
        .get_study(slug)
        .await?
        .ok_or_else(|| Error::refused(format!("no study `{slug}`")))
}

/// Finished, failed or discarded: nothing moves any more.
pub fn is_terminal(state: &StudyState) -> bool {
    matches!(
        state,
        StudyState::Finished | StudyState::Failed { .. } | StudyState::Discarded
    )
}

/// The directories one experiment's samples and commands run in.
struct Paths<'a> {
    slug: &'a str,
    n: u32,
    worktree: &'a Path,
    evaluator: &'a Path,
    runs: &'a Path,
}

impl<'a> Paths<'a> {
    fn req(&self, k: u32, tree: &'a str, role: SampleRole) -> SampleRequest<'a> {
        SampleRequest {
            slug: self.slug,
            n: self.n,
            k,
            tree_sha: tree,
            role,
            worktree: self.worktree,
            evaluator: self.evaluator,
            runs_dir: self.runs,
        }
    }
}

fn worktree_of(study: &StudyRecord) -> Result<PathBuf> {
    study
        .worktree
        .as_deref()
        .map(PathBuf::from)
        .ok_or_else(|| Error::refused(format!("study `{}` has no worktree yet", study.slug)))
}

fn incumbent_of(study: &StudyRecord) -> Result<String> {
    study
        .incumbent_sha
        .clone()
        .ok_or_else(|| Error::refused(format!("study `{}` has no incumbent yet", study.slug)))
}

fn gate_cfg(cfg: &AutoresearchConfig, spec: &StudySpec) -> Result<GateCfg> {
    Ok(GateCfg {
        direction: spec.metric.direction,
        z: cfg.gate.z,
        z_fast: cfg.gate.z_fast,
        z_discard: cfg.gate.z_discard,
        z_suspicious: cfg.gate.z_suspicious,
        confirm_runs: cfg.gate.confirm_runs,
        remeasure_after: cfg.gate.remeasure_after,
        min_effect: Effect::parse(&spec.metric.min_effect).map_err(Error::refused)?,
        guards: spec
            .secondary_guards
            .iter()
            .map(|g| Effect::parse(&g.max_regression).map(|e| (g.name.clone(), e)))
            .collect::<std::result::Result<_, _>>()
            .map_err(Error::refused)?,
    })
}

fn mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

fn clean_values(samples: &[Sample]) -> Vec<f64> {
    samples
        .iter()
        .filter(|s| !s.noisy && !s.lost)
        .filter_map(|s| s.value)
        .collect()
}

/// Pooled σ of the current segment: every tree measured more than once since the segment opened (140 §5.2 step 1).
async fn segment_sigma(env: &Env, study: &StudyRecord) -> Result<f64> {
    let opened = env
        .store
        .get_segment(&study.slug, study.segment)
        .await?
        .map(|s| s.opened_at)
        .unwrap_or_default();
    let samples = env.store.list_samples(&study.slug, None).await?;
    let mut trees: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for s in samples
        .iter()
        .filter(|s| s.at >= opened && !s.noisy && !s.lost && s.role != Some(SampleRole::Holdout))
    {
        if let Some(v) = s.value {
            trees.entry(s.tree_sha.clone()).or_default().push(v);
        }
    }
    let trees: Vec<Vec<f64>> = trees.into_values().filter(|t| t.len() > 1).collect();
    Ok(env.backend.pooled_sigma(&trees))
}

/// Closed experiments, kept experiments.
async fn counts(store: &Store, slug: &str) -> Result<(u32, u32)> {
    let all = store.list_experiments(slug, None, u32::MAX).await?;
    let closed = all
        .iter()
        .filter(|e| {
            !matches!(
                e.state,
                ExperimentState::Open | ExperimentState::Measuring | ExperimentState::Measured
            )
        })
        .count() as u32;
    let kept = all
        .iter()
        .filter(|e| e.state == ExperimentState::Kept)
        .count() as u32;
    Ok((closed, kept))
}

/// Review reverts the user already acted on (`restore` or `kept_reverted` events): no row, no needs-eyes line.
async fn dismissed_reverts(store: &Store, slug: &str) -> std::collections::HashSet<u64> {
    let mut out = std::collections::HashSet::new();
    for kind in ["restore", "kept_reverted"] {
        out.extend(
            store
                .list_events(slug, Some(kind), 500)
                .await
                .unwrap_or_default()
                .iter()
                .filter_map(|e| e.payload.get("n").and_then(|v| v.as_u64())),
        );
    }
    out
}

/// The user's pending Restore (§9.1): a `restore {n}` event newer than every `restore_applied`.
async fn pending_restore(store: &Store, slug: &str) -> Result<Option<u32>> {
    let restores = store.list_events(slug, Some("restore"), 1).await?;
    let Some(r) = restores.first() else {
        return Ok(None);
    };
    let applied = store.list_events(slug, Some("restore_applied"), 1).await?;
    if applied.first().is_some_and(|a| a.id > r.id) {
        return Ok(None);
    }
    Ok(r.payload
        .get("n")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32))
}

/// Which rejected experiment `n` restores (the harness applied its patch at begin), if any.
async fn restoring(store: &Store, slug: &str, n: u32) -> Result<Option<u32>> {
    Ok(store
        .list_events(slug, Some("restore_applied"), 50)
        .await?
        .iter()
        .find(|e| e.payload.get("n").and_then(|v| v.as_u64()) == Some(u64::from(n)))
        .filter(|e| e.payload.get("failed").is_none())
        .and_then(|e| e.payload.get("of").and_then(|v| v.as_u64()))
        .map(|of| of as u32))
}

/// A `drift` event not yet answered by a doubled confirm (§5.2 step 4: the next confirm takes 2× the samples).
async fn drift_pending(store: &Store, slug: &str) -> Result<bool> {
    let drift = store.list_events(slug, Some("drift"), 1).await?;
    let Some(d) = drift.first() else {
        return Ok(false);
    };
    let doubled = store.list_events(slug, Some("confirm_doubled"), 1).await?;
    Ok(doubled.first().is_none_or(|c| c.id < d.id))
}

/// §5.4: `holdout.repeats` samples of `holdout.command` on `tree` (role `Holdout`,
/// through the slot). Clean values; a crashed sample is skipped, never fatal.
/// Holdout numbers reach the ledger, the Study view and the report — never the brief.
async fn holdout(
    env: &Env,
    study: &StudyRecord,
    tree: &str,
    n: u32,
    cancel: &CancellationToken,
) -> Result<Vec<f64>> {
    let Some(h) = study.spec.holdout.as_ref() else {
        return Ok(vec![]);
    };
    let mut spec = study.spec.clone();
    spec.bench.command = h.command.clone();
    let worktree = worktree_of(study)?;
    let evaluator = env.evaluator_dir(&study.slug, study.segment);
    let runs = env.runs_dir(&study.slug, n).join("holdout");
    std::fs::create_dir_all(&runs)?;
    let mut values = Vec::new();
    for k in 1..=u32::from(h.repeats.unwrap_or(spec.bench.repeats).max(1)) {
        let s = env
            .backend
            .sample(
                &spec,
                &env.cfg.bench,
                SampleRequest {
                    slug: &study.slug,
                    n,
                    k,
                    tree_sha: tree,
                    role: SampleRole::Holdout,
                    worktree: &worktree,
                    evaluator: &evaluator,
                    runs_dir: &runs,
                },
                cancel,
            )
            .await?;
        env.store.insert_sample(&study.slug, &s).await?;
        values.extend(s.value.filter(|_| !s.noisy && !s.lost));
    }
    Ok(values)
}

/// §5.4 overfit: the incumbent's holdout is worse than the base's beyond 2σ_holdout
/// while the search metric improved. Records `overfit` (the stop rules treat it as a plateau).
async fn check_overfit(
    env: &Env,
    study: &StudyRecord,
    n: u32,
    incumbent_holdout: &[f64],
) -> Result<()> {
    let Some(base) = study.base_sha.as_deref() else {
        return Ok(());
    };
    let base_holdout: Vec<f64> = env
        .store
        .samples_of_tree(&study.slug, base)
        .await?
        .into_iter()
        .filter(|s| s.role == Some(SampleRole::Holdout) && !s.noisy && !s.lost)
        .filter_map(|s| s.value)
        .collect();
    if base_holdout.is_empty() || incumbent_holdout.is_empty() {
        return Ok(());
    }
    let sigma = env
        .backend
        .pooled_sigma(&[base_holdout.clone(), incumbent_holdout.to_vec()]);
    let direction = study.spec.metric.direction;
    let holdout_delta = gate::normalise(direction, mean(incumbent_holdout), mean(&base_holdout));
    let search_delta = match (
        study.incumbent_mean,
        env.store.get_segment(&study.slug, study.segment).await?,
    ) {
        (Some(inc), Some(seg)) => seg
            .baseline_mean
            .map(|b| gate::normalise(direction, inc, b)),
        _ => None,
    };
    if holdout_delta < -2.0 * sigma && search_delta.is_some_and(|d| d > 0.0) {
        env.store
            .append_event(
                &study.slug,
                "overfit",
                &serde_json::json!({
                    "n": n,
                    "holdout_base": mean(&base_holdout),
                    "holdout_incumbent": mean(incumbent_holdout),
                    "sigma": sigma,
                }),
            )
            .await?;
    }
    Ok(())
}

fn budget_left(spec: &StudySpec, closed: u32, hours: f64, spend: f64) -> BudgetLeft {
    BudgetLeft {
        usd: (spec.budget.usd - spend).max(0.0),
        hours: (spec.budget.hours - hours).max(0.0),
        experiments: spec.budget.experiments.saturating_sub(closed),
    }
}

fn repo_name(repo: &str) -> String {
    Path::new(repo)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| repo.to_owned())
}

fn pct(delta: f64, reference: f64) -> Option<f64> {
    (reference != 0.0 && reference.is_finite()).then(|| delta / reference.abs() * 100.0)
}

fn fmt_pct(p: Option<f64>) -> String {
    match p {
        Some(p) => format!("{p:+.1} %"),
        None => "—".into(),
    }
}

fn state_word(state: &StudyState) -> String {
    match state {
        StudyState::Paused { reason } => format!("paused ({})", pause_word(reason)),
        StudyState::Failed { reason } => format!("failed: {reason}"),
        s => s.tag().to_owned(),
    }
}

fn pause_word(reason: &PauseReason) -> String {
    match reason {
        PauseReason::User => "by you".into(),
        PauseReason::CrashStreak => "the benchmark keeps failing".into(),
        PauseReason::CustodyRefusals => "evaluator files keep changing".into(),
        PauseReason::ReviewFlags => "the reviewer keeps flagging keeps".into(),
        PauseReason::BudgetConfirm => "budget needs your confirmation".into(),
        PauseReason::Other { reason } => reason.clone(),
    }
}

fn pause_tag(reason: &PauseReason) -> &'static str {
    match reason {
        PauseReason::User => "user",
        PauseReason::CrashStreak => "crash_streak",
        PauseReason::CustodyRefusals => "custody_refusals",
        PauseReason::ReviewFlags => "review_flags",
        PauseReason::BudgetConfirm => "budget_confirm",
        PauseReason::Other { .. } => "other",
    }
}

fn stop_word(reason: &StopReason) -> String {
    match reason {
        StopReason::User => "stopped by you".into(),
        StopReason::UserNow => "stopped now by you".into(),
        StopReason::Experiments => "experiment budget reached".into(),
        StopReason::Budget => "money budget reached".into(),
        StopReason::Hours => "time budget reached".into(),
        StopReason::Deadline => "deadline reached".into(),
        StopReason::Plateau => "plateau".into(),
        StopReason::Overfit => "holdout says overfit".into(),
        StopReason::Failed { reason } => format!("failed: {reason}"),
    }
}

/// `StudyInfo` from a ledger row (shared by status, control, create). Counts
/// and spend need queries: see [`info_full`].
pub fn info(a: &Studies, study: &StudyRecord) -> StudyInfo {
    let _ = a;
    info_from(study, &a.root, None, 0, 0.0)
}

fn info_from(
    study: &StudyRecord,
    root: &Path,
    base_mean: Option<f64>,
    closed: u32,
    spend: f64,
) -> StudyInfo {
    let delta_pct = match (study.incumbent_mean, base_mean) {
        (Some(i), Some(b)) => pct(gate::normalise(study.spec.metric.direction, i, b), b),
        _ => None,
    };
    let (current, phase) =
        phase_of(root, &study.slug).map_or((None, None), |(n, p)| (Some(n), Some(p)));
    let hours = stop::hours_running(study, chrono::Utc::now());
    StudyInfo {
        slug: study.slug.clone(),
        goal: study.spec.goal.clone(),
        repo: study.repo.clone(),
        state: study.state.clone(),
        metric: study.spec.metric.clone(),
        base_sha: study.base_sha.clone(),
        incumbent: Incumbent {
            sha: study.incumbent_sha.clone().unwrap_or_default(),
            mean: study.incumbent_mean,
            kept: 0,
        },
        baseline_mean: base_mean,
        sigma: study.sigma,
        segment: study.segment,
        experiments: closed,
        delta_pct,
        headline: format!(
            "Optimising {} in {} · {} experiments · {} · {}",
            study.spec.metric.name,
            repo_name(&study.repo),
            closed,
            fmt_pct(delta_pct),
            state_word(&study.state)
        ),
        budget_used: BudgetUsed {
            usd: spend,
            hours,
            experiments: closed,
        },
        budget: study.spec.budget.clone(),
        session: study.session,
        current,
        phase,
        started_at: study.started_at.clone(),
        finished_at: study.finished_at.clone(),
        stop_reason: study.stop_reason.clone(),
        report_doc: study.report_doc.clone(),
    }
}

/// `StudyInfo` with counts, base and spend filled from the ledger.
pub async fn info_full(env: &Env, study: &StudyRecord) -> Result<StudyInfo> {
    let (closed, kept) = counts(&env.store, &study.slug).await?;
    let base_mean = env
        .store
        .get_segment(&study.slug, study.segment)
        .await?
        .and_then(|s| s.baseline_mean);
    let spend = if is_terminal(&study.state) {
        0.0
    } else {
        env.backend.spend_usd(study.session).await
    };
    let mut i = info_from(study, &env.root, base_mean, closed, spend);
    i.incumbent.kept = kept;
    Ok(i)
}

async fn set_state(
    env: &Env,
    study: &mut StudyRecord,
    state: StudyState,
    reason: Option<String>,
) -> Result<()> {
    study.state = state.clone();
    env.store.update_study(study).await?;
    env.publish(StudyEvent::StudyChanged {
        slug: study.slug.clone(),
        state,
        reason,
    })
    .await;
    Ok(())
}

async fn progress(
    env: &Env,
    slug: &str,
    n: u32,
    started_at: &str,
    phase: Phase,
    sample: u8,
    of: u8,
) {
    set_phase(&env.root, slug, n, phase);
    let elapsed_ms = chrono::DateTime::parse_from_rfc3339(started_at)
        .map(|t| {
            (chrono::Utc::now() - t.with_timezone(&chrono::Utc))
                .num_milliseconds()
                .max(0) as u64
        })
        .unwrap_or(0);
    env.publish(StudyEvent::ExperimentProgress {
        slug: slug.to_owned(),
        n,
        phase,
        sample,
        of,
        elapsed_ms,
    })
    .await;
}

/// `Deliver` to the chat the draft card lived in, regardless of `quiet` (140 §13.1); off-mailbox.
fn deliver(origin: Option<SessionId>, slug: &str, text: String) {
    let origin_label = format!("autoresearch {slug}");
    tokio::spawn(async move {
        let Ok(Some(user)) = ActorRef::<agent::UserChannels>::lookup(agent::USER_ACTOR) else {
            tracing::debug!(%origin_label, "user actor not running; card not delivered");
            return;
        };
        let m = agent::Outbound {
            sessions: origin.into_iter().collect(),
            origin: origin_label.clone(),
            text,
            files: vec![],
        };
        if let Err(e) = user.ask(agent::Deliver { m, via: None }).await {
            tracing::warn!(%origin_label, "card not delivered: {e}");
        }
    });
}

fn card(elements: serde_json::Value, root: &str) -> String {
    format!(
        "```json-render\n{}\n```",
        serde_json::json!({ "root": root, "elements": elements })
    )
}

fn fmt_num(v: f64) -> String {
    if v.abs() >= 100.0 {
        format!("{v:.0}")
    } else if v.abs() >= 10.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    }
}

/// The start card (140 §13.1): goal, metric, base value, σ, budgets, "Open study".
pub fn start_card(study: &StudyRecord, base_mean: f64, sigma: f64, sched: &str) -> String {
    let m = &study.spec.metric;
    let b = &study.spec.budget;
    let items = serde_json::json!([
        { "label": format!("{} (base)", m.name), "value": format!("{} {}", fmt_num(base_mean), m.unit) },
        { "label": "noise σ", "value": format!("{} {} ({:.1} %)", fmt_num(sigma), m.unit, pct(sigma, base_mean).unwrap_or(0.0)) },
        { "label": "budget", "value": format!("{} exp · {} h · ${}", b.experiments, b.hours, b.usd) },
        { "label": "per experiment", "value": format!("${}", b.per_experiment_usd) },
    ]);
    let elements = serde_json::json!({
        "c": { "type": "Card", "props": { "title": format!("Study {} started", study.slug) }, "children": ["t", "g"] },
        "t": { "type": "Text", "props": { "text": study.spec.goal } },
        "g": { "type": "MetricGrid", "props": { "items": items } },
    });
    format!(
        "**Autoresearch study `{}` started** — optimising `{}` ({}) in {} on `{}`; schedule `{}`.\n\n{}\n\nOpen study: /autoresearch/{}",
        study.slug,
        m.name,
        match m.direction {
            Direction::Lower => "lower is better",
            Direction::Higher => "higher is better",
        },
        repo_name(&study.repo),
        study
            .base_sha
            .as_deref()
            .map(|s| &s[..s.len().min(10)])
            .unwrap_or("?"),
        sched,
        card(elements, "c"),
        study.slug
    )
}

/// The result card (140 §11.4): headline, MetricGrid, LineChart, changesets, report link.
pub fn result_card(
    study: &StudyRecord,
    headline: &Headline,
    series: &ChartSeries,
    changesets: &[Changeset],
    closed: u32,
    kept: u32,
) -> String {
    let m = &study.spec.metric;
    let items = serde_json::json!([
        { "label": m.name, "value": format!("{} → {} {}", fmt_num(headline.base_mean), fmt_num(headline.final_mean), m.unit),
          "delta": format!("{:+.1} %", headline.delta_pct),
          "trend": if headline.delta_pct < 0.0 { "down" } else { "up" } },
        { "label": "experiments", "value": format!("{closed} · {kept} kept") },
        { "label": "changesets", "value": changesets.len().to_string() },
        { "label": "stopped", "value": study.stop_reason.as_ref().map(stop_word).unwrap_or_default() },
    ]);
    // ≤ 60 labels: thin the experiment series evenly.
    let step = series.experiments.len().div_ceil(60).max(1);
    let pts: Vec<&ChartPoint> = series.experiments.iter().step_by(step).collect();
    let labels: Vec<String> = pts.iter().map(|p| format!("#{}", p.n)).collect();
    let incumbent_at = |n: u32| -> f64 {
        series
            .incumbent
            .iter()
            .take_while(|p| p.n <= n)
            .last()
            .and_then(|p| p.metric)
            .or(series.baseline_mean)
            .unwrap_or(0.0)
    };
    let chart = serde_json::json!({
        "type": "LineChart",
        "props": {
            "labels": labels,
            "series": [
                { "name": "experiment", "values": pts.iter().map(|p| p.metric.unwrap_or(f64::NAN)).filter(|v| v.is_finite()).collect::<Vec<_>>() },
                { "name": "incumbent", "values": pts.iter().map(|p| incumbent_at(p.n)).collect::<Vec<_>>(), "state": "success" },
            ],
            "unit": m.unit,
            "title": format!("{} per experiment", m.name),
        }
    });
    let rows: Vec<Vec<String>> = changesets
        .iter()
        .map(|c| {
            vec![
                format!("#{} {}", c.k, c.name),
                fmt_pct(c.delta_alone_pct),
                fmt_pct(c.delta_together_pct),
                c.files.len().to_string(),
                c.branch.clone(),
            ]
        })
        .collect();
    let mut children = vec!["g", "l"];
    let mut elements = serde_json::json!({
        "g": { "type": "MetricGrid", "props": { "items": items } },
        "l": chart,
    });
    if !rows.is_empty() {
        children.push("t");
        elements["t"] = serde_json::json!({ "type": "Table", "props": {
            "columns": ["changeset", "Δ alone", "Δ together", "files", "branch"], "rows": rows } });
    }
    elements["c"] = serde_json::json!({ "type": "Card", "props": { "title": format!("Study {} finished", study.slug) }, "children": children });
    let report = study
        .report_doc
        .as_deref()
        .map(|d| {
            format!(
                "\n\nReport: `{d}` · Study view: /autoresearch/{}",
                study.slug
            )
        })
        .unwrap_or_default();
    let holdout = headline
        .holdout
        .as_deref()
        .map(|h| format!(" · holdout {h}"))
        .unwrap_or_default();
    format!(
        "**Autoresearch study `{}` finished** — {}{}\n\n{}{}",
        study.slug,
        headline.text,
        holdout,
        card(elements, "c"),
        report
    )
}

// ───────────────────────────── Create ─────────────────────────────

fn validate(spec: &StudySpec, cfg: &AutoresearchConfig, backend: &dyn Backend) -> Result<()> {
    let slug_ok = (1..=32).contains(&spec.slug.len())
        && spec
            .slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !slug_ok {
        return Err(Error::refused(format!(
            "slug `{}`: use [a-z0-9-]{{1,32}}",
            spec.slug
        )));
    }
    if spec.goal.trim().is_empty() {
        return Err(Error::refused("goal is empty"));
    }
    let repo = Path::new(&spec.repo);
    if spec.repo.trim().is_empty() || !repo.is_dir() {
        return Err(Error::refused(format!(
            "repo `{}` is not a directory",
            spec.repo
        )));
    }
    if !backend.is_git_repo(repo) {
        return Err(Error::refused(format!(
            "repo `{}` is not a git repository (a pure jj repo needs git colocation)",
            spec.repo
        )));
    }
    if spec.metric.name.trim().is_empty() {
        return Err(Error::refused("metric.name is empty"));
    }
    if spec.bench.command.trim().is_empty() {
        return Err(Error::refused("bench.command is empty"));
    }
    if spec.bench.repeats == 0 {
        return Err(Error::refused("bench.repeats must be at least 1"));
    }
    humantime::parse_duration(spec.bench.timeout.trim())
        .map_err(|e| Error::refused(format!("bench.timeout `{}`: {e}", spec.bench.timeout)))?;
    if let Some(c) = &spec.checks {
        if c.command.trim().is_empty() {
            return Err(Error::refused("checks.command is empty"));
        }
        humantime::parse_duration(c.timeout.trim())
            .map_err(|e| Error::refused(format!("checks.timeout `{}`: {e}", c.timeout)))?;
    }
    Effect::parse(&spec.metric.min_effect)
        .map_err(|e| Error::refused(format!("metric.min_effect: {e}")))?;
    for g in &spec.secondary_guards {
        Effect::parse(&g.max_regression)
            .map_err(|e| Error::refused(format!("guard {}: {e}", g.name)))?;
    }
    if spec.parallel == 0 || spec.parallel > cfg.max_parallel.max(1) {
        return Err(Error::refused(format!(
            "parallel must be 1..={}",
            cfg.max_parallel.max(1)
        )));
    }
    let b = &spec.budget;
    if !(b.usd > 0.0 && b.per_experiment_usd > 0.0 && b.hours > 0.0 && b.experiments > 0) {
        return Err(Error::refused(
            "budget: usd, per_experiment_usd, hours and experiments must be positive",
        ));
    }
    Ok(())
}

/// `Create` → `StudyInfo` in state `preparing` (140 §4.2 preparing). The
/// pipeline (worktree → warm → custody → baseline → σ → schedule file →
/// running → start card) runs off the mailbox; failures end in `failed`.
pub async fn create(a: &mut Studies, m: Create) -> Result<StudyInfo> {
    let env = a.env();
    validate(&m.spec, &env.cfg, env.backend.as_ref())?;
    if let Some(existing) = env.store.get_study(&m.spec.slug).await? {
        return Err(Error::refused(format!(
            "slug `{}` is taken by a study that is {}",
            existing.slug,
            state_word(&existing.state)
        )));
    }
    let mut study = StudyRecord::new(m.spec, Some(m.origin));
    study.state = StudyState::Preparing;
    study.created_by = Some("user".into());
    study.schedule = Some(schedule_file::name(&study.slug));
    env.store.create_study(&study).await?;
    env.publish(StudyEvent::StudyChanged {
        slug: study.slug.clone(),
        state: StudyState::Preparing,
        reason: None,
    })
    .await;
    let info = info_from(&study, &env.root, None, 0, 0.0);
    let env2 = env.clone();
    tokio::spawn(async move { prepare(env2, study, m.start).await });
    Ok(info)
}

/// The preparing pipeline; `Err` → `failed` with the reason on the card.
pub async fn prepare(env: Env, mut study: StudyRecord, start: StartWhen) {
    match prepare_steps(&env, &mut study, &start).await {
        Ok(()) => {}
        Err(e) => {
            let reason = e.to_string();
            tracing::warn!(slug = %study.slug, "study did not start: {reason}");
            let _ = env
                .store
                .append_event(
                    &study.slug,
                    "failed",
                    &serde_json::json!({ "reason": reason }),
                )
                .await;
            if let Err(e2) = set_state(
                &env,
                &mut study,
                StudyState::Failed {
                    reason: reason.clone(),
                },
                Some(reason.clone()),
            )
            .await
            {
                tracing::warn!("failed state not recorded: {e2}");
            }
            deliver(
                study.origin,
                &study.slug,
                format!(
                    "**Autoresearch study `{}` did not start** — {reason}",
                    study.slug
                ),
            );
        }
    }
}

async fn prepare_steps(env: &Env, study: &mut StudyRecord, start: &StartWhen) -> Result<()> {
    let spec = study.spec.clone();
    let slug = study.slug.clone();
    let repo = PathBuf::from(&spec.repo);
    let cancel = cancel_token(&env.root, &slug);

    let base_sha = env.backend.resolve_base(&repo, &spec.base)?;
    let worktree = env.study_dir(&slug).join("worktree");
    std::fs::create_dir_all(env.study_dir(&slug))?;
    let head = env
        .backend
        .worktree_add(&repo, &spec, &worktree, &base_sha)?;
    study.base_sha = Some(base_sha.clone());
    study.branch = Some(format!("autoresearch/{slug}"));
    study.worktree = Some(worktree.display().to_string());
    study.incumbent_sha = Some(head.clone());
    env.store.update_study(study).await?;

    let evaluator = env.evaluator_dir(&slug, 0);
    let runs = env.runs_dir(&slug, 0);
    std::fs::create_dir_all(&runs)?;
    let paths = Paths {
        slug: &slug,
        n: 0,
        worktree: &worktree,
        evaluator: &evaluator,
        runs: &runs,
    };
    if let Some(warm) = spec
        .warm
        .as_deref()
        .map(str::trim)
        .filter(|w| !w.is_empty())
    {
        let (exit, tail, _) = env
            .backend
            .command(
                warm,
                Duration::from_secs(30 * 60),
                paths.req(0, &head, SampleRole::Baseline),
                &cancel,
            )
            .await?;
        if exit != Some(0) {
            return Err(Error::refused(format!(
                "warm `{warm}` failed (exit {exit:?}): {tail}"
            )));
        }
    }

    let custody = env.backend.custody_take(&spec, &repo, &evaluator)?;
    env.store
        .put_segment(&SegmentRecord {
            slug: slug.clone(),
            segment: 0,
            opened_at: now(),
            opened_by: By::User,
            reason: None,
            custody,
            baseline_mean: None,
            baseline_n: 0,
        })
        .await?;
    if let Ok(Some(p)) = env.backend.seed_playbook(&spec.repo, &env.rt).await {
        study.playbook = p;
    }

    let values = baseline(env, study, &head, 0, SampleRole::Baseline, &cancel).await?;
    let (m, sigma) = (
        mean(&values),
        env.backend.pooled_sigma(std::slice::from_ref(&values)),
    );
    let cv = if m != 0.0 { sigma / m.abs() } else { 0.0 };
    if cv > env.cfg.noise.max_cv {
        return Err(Error::refused(format!(
            "σ = {:.1} % of the metric: a keep would need ≥ {:.1} %; make the benchmark steadier or raise repeats",
            cv * 100.0,
            env.cfg.gate.z * cv * 100.0
        )));
    }
    if let Some(mut seg) = env.store.get_segment(&slug, 0).await? {
        seg.baseline_mean = Some(m);
        seg.baseline_n = values.len() as u32;
        env.store.put_segment(&seg).await?;
    }
    study.incumbent_mean = Some(m);
    study.sigma = Some(sigma);
    // §5.4: the base's holdout, the reference every later holdout is judged against.
    holdout(env, study, &head, 0, &cancel).await?;

    let model = env.cfg.executor(&spec.models, &env.agent);
    let text = schedule_file::text(&spec, &env.cfg, &worktree, &model, start);
    schedule_file::write(&env.data_dir, &slug, &text)?;
    study.started_at = Some(now());
    set_state(env, study, StudyState::Running, None).await?;
    let sched = schedule_file::name(&slug);
    deliver(study.origin, &slug, start_card(study, m, sigma, &sched));
    Ok(())
}

/// `repeats` samples of `tree` through the slot; a crash fails the whole thing.
async fn baseline(
    env: &Env,
    study: &StudyRecord,
    tree: &str,
    n: u32,
    role: SampleRole,
    cancel: &CancellationToken,
) -> Result<Vec<f64>> {
    let spec = &study.spec;
    let worktree = worktree_of(study)?;
    let evaluator = env.evaluator_dir(&study.slug, study.segment);
    let runs = env.runs_dir(&study.slug, n);
    std::fs::create_dir_all(&runs)?;
    let mut values = Vec::new();
    for k in 1..=u32::from(spec.bench.repeats.max(1)) {
        let s = env
            .backend
            .sample(
                spec,
                &env.cfg.bench,
                SampleRequest {
                    slug: &study.slug,
                    n,
                    k,
                    tree_sha: tree,
                    role,
                    worktree: &worktree,
                    evaluator: &evaluator,
                    runs_dir: &runs,
                },
                cancel,
            )
            .await?;
        env.store.insert_sample(&study.slug, &s).await?;
        match s.value {
            Some(v) => values.push(v),
            None => {
                let tail = s
                    .log_path
                    .as_deref()
                    .map(|p| env.backend.tail(&env.root.join(p)))
                    .unwrap_or_default();
                return Err(Error::refused(format!(
                    "baseline sample {k} crashed (exit {:?}): {tail}",
                    s.exit
                )));
            }
        }
    }
    Ok(values)
}

/// `StartDraft` → the draft card's press (140 §12.4 `autoresearch_draft` invariant).
/// The draft is `<root>/drafts/<draft_id>/spec.json` = `{"spec": StudySpec, "origin": SessionId}`
/// written by `autoresearch_draft`; `draft_id` is the spec's slug.
pub async fn start_draft(a: &mut Studies, m: StartDraft) -> Result<StudyInfo> {
    let d = crate::tools::read_draft(&a.root, &m.draft_id)?;
    create(
        a,
        Create {
            spec: d.spec,
            origin: d.origin.unwrap_or_default(),
            start: m.start,
        },
    )
    .await
}

// ───────────────────────────── Begin ─────────────────────────────

/// `Begin` → `Brief` (140 §4.3 open, §7).
pub async fn begin(a: &mut Studies, m: Begin) -> Result<Brief> {
    begin_env(&a.env(), m).await
}

/// [`begin`] on an [`Env`] (off-mailbox).
pub async fn begin_env(env: &Env, m: Begin) -> Result<Brief> {
    if m.slot.is_some() {
        return Err(Error::NotBuilt("W5"));
    }
    let mut study = study_of(env, m.session).await?;
    match &study.state {
        StudyState::Running | StudyState::Paused { .. } => {}
        s => {
            return Err(Error::refused(format!(
                "study `{}` is {}; nothing to run",
                study.slug,
                state_word(s)
            )));
        }
    }
    if study.spec.parallel > 1 {
        return Err(Error::NotBuilt("W5"));
    }
    let slug = study.slug.clone();
    let mut incidents = Vec::new();

    // §8.4: the money cap is checked again here so a fire after the cap ends at once.
    let spend = env.backend.spend_usd(study.session).await;
    if spend >= study.spec.budget.usd {
        begin_finalize(env, study, StopReason::Budget).await?;
        return Err(Error::refused("budget exhausted; the study is finalizing"));
    }

    // §4.3 invariant: close a dangling experiment instead of blocking.
    if let Some(mut dangling) = env.store.dangling_experiment(&slug, None).await? {
        let was_measuring = dangling.state == ExperimentState::Measuring;
        dangling.state = if was_measuring {
            ExperimentState::Interrupted
        } else {
            ExperimentState::Abandoned
        };
        dangling.finished_at = Some(now());
        env.store.update_experiment(&dangling).await?;
        env.store
            .append_event(
                &slug,
                "incident",
                &serde_json::json!({ "n": dangling.n, "what": dangling.state }),
            )
            .await?;
        incidents.push(format!(
            "experiment {} was not logged; its change was discarded.",
            dangling.n
        ));
    }

    // §6 drift repair.
    let worktree = worktree_of(&study)?;
    let incumbent = incumbent_of(&study)?;
    let n = env.store.next_experiment_n(&slug).await?;
    let runs = env.runs_dir(&slug, n);
    std::fs::create_dir_all(&runs)?;
    match env.backend.repair(
        &worktree,
        &study.spec,
        &incumbent,
        &runs.join("stray.patch"),
    ) {
        Ok(None) => {}
        Ok(Some(what)) => {
            env.store
                .append_event(
                    &slug,
                    "incident",
                    &serde_json::json!({ "n": n, "what": what }),
                )
                .await?;
            incidents.push(format!(
                "the worktree had {what} at the start of this experiment; it was saved and removed."
            ));
        }
        Err(e) => {
            return Err(Error::refused(format!(
                "worktree `{}` cannot be repaired: {e}",
                worktree.display()
            )));
        }
    }

    // §5.2 step 4: one incumbent sample every `remeasure_every` keeps.
    let (closed, kept) = counts(&env.store, &slug).await?;
    if let Some(due) = kept.checked_div(env.cfg.gate.remeasure_every) {
        let remeasures = env
            .store
            .list_samples(&slug, None)
            .await?
            .iter()
            .filter(|s| s.role == Some(SampleRole::Incumbent))
            .count() as u32;
        if due > remeasures {
            let evaluator = env.evaluator_dir(&slug, study.segment);
            let s = env
                .backend
                .sample(
                    &study.spec,
                    &env.cfg.bench,
                    SampleRequest {
                        slug: &slug,
                        n,
                        k: 0,
                        tree_sha: &incumbent,
                        role: SampleRole::Incumbent,
                        worktree: &worktree,
                        evaluator: &evaluator,
                        runs_dir: &runs,
                    },
                    &cancel_token(&env.root, &slug),
                )
                .await?;
            env.store.insert_sample(&slug, &s).await?;
            if let (Some(v), Some(mean_i), Some(sigma)) =
                (s.value, study.incumbent_mean, study.sigma)
                && env.backend.drifted(sigma, mean_i, v)
            {
                env.store
                    .append_event(
                        &slug,
                        "drift",
                        &serde_json::json!({ "n": n, "value": v, "mean": mean_i, "sigma": sigma }),
                    )
                    .await?;
                incidents.push(format!(
                    "an incumbent re-measure landed at {} {} vs mean {} (drift); the next confirm takes more samples.",
                    fmt_num(v),
                    study.spec.metric.unit,
                    fmt_num(mean_i)
                ));
            }
            let inc = clean_values(&env.store.samples_of_tree(&slug, &incumbent).await?);
            if !inc.is_empty() {
                study.incumbent_mean = Some(mean(&inc));
            }
            study.sigma = Some(segment_sigma(env, &study).await?);
            env.store.update_study(&study).await?;
        }
    }

    // Allocate n.
    let exp = ExperimentRecord::open(&slug, n, study.segment, &incumbent);
    env.store.insert_experiment(&exp).await?;
    // Study spend at open; `Log` subtracts it for the row's `cost_nanos`.
    env.store
        .append_event(
            &slug,
            "begin",
            &serde_json::json!({ "n": n, "spend_usd": spend }),
        )
        .await?;
    set_phase(&env.root, &slug, n, Phase::Novelty);

    // §9.1 Restore: the harness re-applies the rejected change as this experiment's candidate.
    if let Some(of) = pending_restore(&env.store, &slug).await? {
        let rejected = env.store.get_experiment(&slug, of).await?;
        let (idea, category) = rejected.map(|r| (r.idea, r.category)).unwrap_or_default();
        let payload = match env.backend.restore_rejected(&worktree, &slug, of) {
            Ok(()) => {
                incidents.push(format!(
                    "this experiment restores #{of} (\"{idea}\", reverted by review; the user wants it measured again): \
                     its change is already applied in the worktree — call experiment_run(idea: \"{idea}\", category: \"{category}\") \
                     without editing, then experiment_log."
                ));
                serde_json::json!({ "n": n, "of": of })
            }
            Err(e) => {
                incidents.push(format!("restore of #{of} failed: {e}"));
                serde_json::json!({ "n": n, "of": of, "failed": e.to_string() })
            }
        };
        env.store
            .append_event(&slug, "restore_applied", &payload)
            .await?;
    }

    // Explore mode after a plateau (§8.4), budget line, code map, incidents.
    let events = env.store.list_events(&slug, Some("explore"), 1).await?;
    let last_keep_n = env
        .store
        .list_experiments(&slug, None, u32::MAX)
        .await?
        .iter()
        .find(|e| e.state == ExperimentState::Kept)
        .map_or(0, |e| e.n);
    let explore = events
        .first()
        .is_some_and(|e| {
            e.payload
                .get("after_n")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32
                >= last_keep_n
        });
    let hours = stop::hours_running(&study, chrono::Utc::now());
    let left = budget_left(&study.spec, closed, hours, spend);
    let code_map = env
        .backend
        .code_map(
            &env.rt,
            &study.repo,
            &study.spec.scope,
            env.cfg.brief.code_map_tokens,
        )
        .await;
    let cx = crate::brief::Context {
        n,
        explore,
        incidents,
        budget_left: (left.experiments, left.hours, left.usd),
        code_map,
        gate: env.cfg.gate.clone(),
        crash_retries: env.cfg.defaults.crash_retries,
    };
    let mut brief = env
        .backend
        .brief(&env.store, &study, &env.cfg.brief, &cx)
        .await?;
    brief.n = n;
    brief.segment = study.segment;
    env.store.mark_steers_seen(&slug, n).await?;
    env.publish(StudyEvent::ExperimentStarted {
        slug: slug.clone(),
        n,
        slot: None,
    })
    .await;
    Ok(brief)
}

// ───────────────────────────── Run ─────────────────────────────

/// `Run` → `RunReport` (140 §4.3 measuring); blocks while measuring, cancel-safe.
pub async fn run(a: &mut Studies, m: Run) -> Result<RunReport> {
    run_env(&a.env(), m).await
}

/// The experiment the study session is working on (`open|measuring|measured`).
async fn current_experiment(env: &Env, study: &StudyRecord) -> Result<ExperimentRecord> {
    env.store
        .dangling_experiment(&study.slug, None)
        .await?
        .ok_or_else(|| Error::refused("no experiment is open: call experiment_begin first"))
}

/// Refused before a sample: record and return.
async fn refuse_run(
    env: &Env,
    exp: &mut ExperimentRecord,
    reason: String,
    deviations: Vec<String>,
    similar: Vec<SimilarExperiment>,
) -> Result<RunReport> {
    let verdict = Verdict::Refused {
        reason: reason.clone(),
    };
    exp.state = ExperimentState::Measured;
    exp.verdict = Some(verdict.clone());
    exp.line = format!("refused: {reason}");
    exp.deviations = deviations.clone();
    env.store.update_experiment(exp).await?;
    if stop::is_custody_refusal(&verdict) {
        env.store
            .append_event(
                &exp.slug,
                "custody_refusal",
                &serde_json::json!({ "n": exp.n }),
            )
            .await?;
    }
    Ok(RunReport {
        verdict,
        line: exp.line.clone(),
        deviations,
        similar,
        ..RunReport::default()
    })
}

/// [`run`] on an [`Env`] (off-mailbox).
pub async fn run_env(env: &Env, m: Run) -> Result<RunReport> {
    if m.slot.is_some() {
        return Err(Error::NotBuilt("W5"));
    }
    let mut study = study_of(env, m.session).await?;
    let slug = study.slug.clone();
    let mut exp = current_experiment(env, &study).await?;
    let retries = env.cfg.defaults.crash_retries;
    let runs_so_far = env
        .store
        .list_events(&slug, Some("run"), 10_000)
        .await?
        .iter()
        .filter(|e| e.payload.get("n").and_then(|v| v.as_u64()) == Some(u64::from(exp.n)))
        .count() as u8;
    match (exp.state, exp.verdict.as_ref()) {
        (ExperimentState::Open, _) => {}
        (ExperimentState::Measured, Some(Verdict::Crash)) if runs_so_far <= retries => {}
        (ExperimentState::Measured, Some(Verdict::Crash)) => {
            return Err(Error::refused(format!(
                "no retries left after {runs_so_far} crashed runs; call experiment_log"
            )));
        }
        (ExperimentState::Measured, _) => {
            return Err(Error::refused(
                "this experiment is already measured; call experiment_log",
            ));
        }
        (ExperimentState::Measuring, _) => return Err(Error::refused("already measuring")),
        _ => {
            return Err(Error::refused(
                "no experiment is open: call experiment_begin first",
            ));
        }
    }
    let a = &m.args;
    if a.idea.trim().is_empty() {
        return Err(Error::refused("idea is required"));
    }
    if a.force && a.reason.as_deref().map(str::trim).unwrap_or("").is_empty() {
        return Err(Error::refused("force needs a reason"));
    }
    env.store
        .append_event(&slug, "run", &serde_json::json!({ "n": exp.n }))
        .await?;
    exp.idea = a.idea.chars().take(300).collect();
    exp.category = a.category.chars().take(40).collect();
    exp.hypothesis = a.hypothesis.chars().take(300).collect();
    exp.state = ExperimentState::Measuring;
    env.store.update_experiment(&exp).await?;
    let n = exp.n;
    let started = exp.started_at.clone();
    let worktree = worktree_of(&study)?;
    let incumbent = incumbent_of(&study)?;
    let cancel = cancel_token(&env.root, &slug);
    let retries_left = retries.saturating_sub(runs_so_far);
    let _ = env
        .backend
        .mark_tried(&env.store, &slug, &exp.idea, n)
        .await;

    // Novelty (§7.3).
    progress(env, &slug, n, &started, Phase::Novelty, 0, 1).await;
    let patch_hash = env
        .backend
        .patch_hash(&worktree, &incumbent)
        .unwrap_or_default();
    exp.patch_hash = Some(patch_hash.clone());
    let flagged_before = env
        .store
        .list_events(&slug, Some("novelty_flag"), 1)
        .await?
        .first()
        .is_some_and(|e| {
            e.payload.get("n").and_then(|v| v.as_u64()) == Some(u64::from(n.saturating_sub(1)))
        });
    let novelty = env
        .backend
        .novelty(
            &env.store,
            &slug,
            a,
            &patch_hash,
            env.cfg.novelty.threshold,
            flagged_before,
        )
        .await?;
    if !novelty.similar.is_empty() {
        env.store
            .append_event(&slug, "novelty_flag", &serde_json::json!({ "n": n }))
            .await?;
    }
    // A harness-applied Restore is a deliberate re-measure of a dead end (§9.1): never a duplicate.
    let restore_of = restoring(&env.store, &slug, n).await?;
    if let Some(reason) = novelty.refused.filter(|_| !a.force && restore_of.is_none()) {
        return refuse_run(env, &mut exp, reason, vec![], novelty.similar).await;
    }

    // Custody (§5.3).
    progress(env, &slug, n, &started, Phase::Custody, 0, 1).await;
    let custody = env
        .store
        .get_segment(&slug, study.segment)
        .await?
        .map(|s| s.custody)
        .unwrap_or_default();
    let changed = env.backend.custody_verify(&custody, &worktree)?;
    if !changed.is_empty() {
        let reason = format!(
            "evaluator changed ({}) — revert with \"git checkout {} -- {}\" or ask the user for a new segment",
            changed.join(", "),
            incumbent,
            changed.join(" ")
        );
        return refuse_run(env, &mut exp, reason, vec![], novelty.similar).await;
    }

    // Scope (§5.3).
    let scope = env.backend.scope_diff(&study.spec, &worktree)?;
    if !scope.off_limits.is_empty() {
        let reason = format!("touches off-limits {}", scope.off_limits.join(", "));
        return refuse_run(
            env,
            &mut exp,
            reason,
            scope.deviations.clone(),
            novelty.similar,
        )
        .await;
    }
    exp.deviations = scope.deviations.clone();
    env.store.update_experiment(&exp).await?;

    // Samples with the sequential gate (§5.2).
    let evaluator = env.evaluator_dir(&slug, study.segment);
    let runs = env.runs_dir(&slug, n);
    std::fs::create_dir_all(&runs)?;
    let cand_tree = format!("exp:{n}");
    let paths = Paths {
        slug: &slug,
        n,
        worktree: &worktree,
        evaluator: &evaluator,
        runs: &runs,
    };
    let gcfg = gate_cfg(&env.cfg, &study.spec)?;
    let mut taken: Vec<Sample> = Vec::new();
    let mut report = RunReport {
        deviations: scope.deviations.clone(),
        similar: novelty.similar.clone(),
        retries_left,
        ..RunReport::default()
    };
    let crash = |env: &Env, exp: &mut ExperimentRecord, report: &mut RunReport, s: &Sample| {
        let tail = s
            .log_path
            .as_deref()
            .map(|p| env.backend.tail(&env.root.join(p)))
            .unwrap_or_default();
        exp.line = format!(
            "crash: exit {} (tail below)",
            s.exit.map_or("killed".to_owned(), |e| e.to_string())
        );
        report.verdict = Verdict::Crash;
        report.line = exp.line.clone();
        report.tail = Some(tail);
    };

    if let Some(smoke) = study
        .spec
        .bench
        .smoke
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        progress(env, &slug, n, &started, Phase::Sampling, 0, 1).await;
        let (exit, tail, _) = env
            .backend
            .command(
                smoke,
                Duration::from_secs(120),
                paths.req(0, &cand_tree, SampleRole::Candidate),
                &cancel,
            )
            .await?;
        if exit != Some(0) {
            exp.line = format!("crash: smoke `{smoke}` exit {exit:?}");
            report.verdict = Verdict::Crash;
            report.line = exp.line.clone();
            report.tail = Some(tail);
            return finish_measure(env, &mut exp, report, taken, None).await;
        }
    }

    let largest_accepted_delta = env
        .store
        .list_experiments(&slug, None, u32::MAX)
        .await?
        .iter()
        .filter(|e| e.state == ExperimentState::Kept && e.segment == study.segment)
        .filter_map(|e| e.delta)
        .fold(None, |m: Option<f64>, d| Some(m.map_or(d, |x| x.max(d))));
    let mut planned: u8 = 1;
    let mut k: u32 = taken.len() as u32;
    // Candidate samples before the gate is consulted again (`Confirm.more` is W1's count;
    // the drift rule's doubling is ours, §5.2 step 4).
    let mut need: u32 = 1;
    let drift_double = drift_pending(&env.store, &slug).await?;
    let mut doubled = false;
    let mut decision: Option<GateStep> = None;
    let mut last_input: Option<GateInput> = None;
    let max_samples = 1 + 2 * u32::from(env.cfg.gate.confirm_runs) + 1;
    while decision.is_none() && k < max_samples {
        if cancel.is_cancelled() {
            return interrupted(env, &mut exp, taken).await;
        }
        k += 1;
        progress(
            env,
            &slug,
            n,
            &started,
            Phase::Sampling,
            k as u8,
            planned.max(k as u8),
        )
        .await;
        let s = env
            .backend
            .sample(
                &study.spec,
                &env.cfg.bench,
                paths.req(k, &cand_tree, SampleRole::Candidate),
                &cancel,
            )
            .await?;
        env.store.insert_sample(&slug, &s).await?;
        if cancel.is_cancelled() || s.lost {
            taken.push(s);
            return interrupted(env, &mut exp, taken).await;
        }
        taken.push(s.clone());
        if s.value.is_none() {
            crash(env, &mut exp, &mut report, &s);
            return finish_measure(env, &mut exp, report, taken, None).await;
        }
        let cand_taken: Vec<Sample> = taken
            .iter()
            .filter(|s| s.role == Some(SampleRole::Candidate))
            .cloned()
            .collect();
        if (cand_taken.len() as u32) < need {
            continue;
        }
        let inc_samples = env.store.samples_of_tree(&slug, &incumbent).await?;
        let incumbent_age = inc_samples
            .iter()
            .filter(|s| s.role != Some(SampleRole::Holdout))
            .map(|s| s.n)
            .max()
            .map_or(n, |newest| n.saturating_sub(newest).saturating_sub(1));
        let candidate = clean_values(&cand_taken);
        let incumbent_vals = clean_values(&inc_samples);
        let secondaries = secondary_means(&cand_taken, &inc_samples);
        let input = GateInput {
            candidate,
            incumbent: incumbent_vals,
            sigma: study.sigma.unwrap_or(0.0),
            largest_accepted_delta,
            incumbent_age,
            keep_simpler: false,
            secondaries,
        };
        match env.backend.gate_step(&gcfg, &input) {
            GateStep::Confirm {
                more,
                incumbent_sample,
            } => {
                let mut more = u32::from(more.max(1));
                if drift_double && !doubled {
                    more *= 2;
                    doubled = true;
                    env.store
                        .append_event(&slug, "confirm_doubled", &serde_json::json!({ "n": n }))
                        .await?;
                }
                need = cand_taken.len() as u32 + more;
                planned = need.min(u32::from(u8::MAX)) as u8;
                if incumbent_sample {
                    let si = env
                        .backend
                        .sample(
                            &study.spec,
                            &env.cfg.bench,
                            paths.req(k, &incumbent, SampleRole::Incumbent),
                            &cancel,
                        )
                        .await?;
                    env.store.insert_sample(&slug, &si).await?;
                    taken.push(si);
                }
            }
            d @ GateStep::Decide { .. } => decision = Some(d),
        }
        last_input = Some(input);
    }
    // At the cap the gate decides on what was measured; with no measurable input it is no gain.
    let (verdict, delta, delta_pct, z) =
        match decision.or_else(|| last_input.map(|i| env.backend.gate_step(&gcfg, &i))) {
            Some(GateStep::Decide {
                verdict,
                delta,
                delta_pct,
                z,
            }) => (verdict, delta, delta_pct, z),
            _ => (Verdict::NoGain, 0.0, 0.0, None),
        };
    let cand_vals = clean_values(
        &taken
            .iter()
            .filter(|s| s.role == Some(SampleRole::Candidate))
            .cloned()
            .collect::<Vec<_>>(),
    );
    let inc_vals = clean_values(&env.store.samples_of_tree(&slug, &incumbent).await?);
    let cand_mean = mean(&cand_vals);
    let inc_mean = if inc_vals.is_empty() {
        study.incumbent_mean.unwrap_or(0.0)
    } else {
        mean(&inc_vals)
    };
    let step = GateStep::Decide {
        verdict: verdict.clone(),
        delta,
        delta_pct,
        z,
    };
    report.verdict = verdict.clone();
    report.delta = Some(delta);
    report.delta_pct = Some(delta_pct);
    report.z = z;
    report.line = env.backend.gate_line(
        &step,
        &study.spec.metric.unit,
        cand_mean,
        inc_mean,
        cand_vals.len(),
    );
    if report.line.is_empty() {
        report.line = format!(
            "{}: {:+.2} % ({} samples)",
            verdict.tag(),
            delta_pct,
            cand_vals.len()
        );
    }
    report.cases_moved = cases_moved(&taken, &env.store.samples_of_tree(&slug, &incumbent).await?);
    exp.metric = Some(cand_mean);

    // Checks only on a would-be keep (§5.1).
    if verdict.is_keep_candidate()
        && let Some(c) = study.spec.checks.as_ref()
    {
        progress(env, &slug, n, &started, Phase::Checks, 0, 1).await;
        let timeout = schedule_file::duration(&c.timeout, Duration::from_secs(300));
        let (exit, tail, wall_ms) = env
            .backend
            .command(
                &c.command,
                timeout,
                paths.req(0, &cand_tree, SampleRole::Candidate),
                &cancel,
            )
            .await?;
        let passed = exit == Some(0);
        report.checks = Some(CheckResult {
            passed,
            tail: (!passed).then_some(tail),
            wall_ms,
        });
        if passed {
            report.line.push_str(", checks passed");
        } else {
            report.verdict = Verdict::ChecksFailed;
            report.line = format!("checks failed (exit {exit:?}); measured {}", report.line);
        }
    }
    // σ absorbs this experiment's repeats.
    study.sigma = Some(segment_sigma(env, &study).await?);
    env.store.update_study(&study).await?;
    finish_measure(env, &mut exp, report, taken, Some(cand_mean)).await
}

/// Mean secondaries `(name, candidate, incumbent)` for the guards.
fn secondary_means(candidate: &[Sample], incumbent: &[Sample]) -> Vec<(String, f64, f64)> {
    let mut out = Vec::new();
    let cands: Vec<&Sample> = candidate
        .iter()
        .filter(|s| s.role == Some(SampleRole::Candidate) && s.value.is_some())
        .collect();
    let incs: Vec<&Sample> = incumbent.iter().filter(|s| s.value.is_some()).collect();
    let names: std::collections::BTreeSet<&String> =
        cands.iter().flat_map(|s| s.secondaries.keys()).collect();
    for name in names {
        let c: Vec<f64> = cands
            .iter()
            .filter_map(|s| s.secondaries.get(name).copied())
            .collect();
        let i: Vec<f64> = incs
            .iter()
            .filter_map(|s| s.secondaries.get(name).copied())
            .collect();
        if !c.is_empty() && !i.is_empty() {
            out.push((name.clone(), mean(&c), mean(&i)));
        }
    }
    out
}

/// Top 5 `CASE` movements between the candidate's last sample and the incumbent's last sample.
fn cases_moved(candidate: &[Sample], incumbent: &[Sample]) -> Vec<CaseDelta> {
    let Some(c) = candidate
        .iter()
        .rev()
        .find(|s| s.role == Some(SampleRole::Candidate) && s.value.is_some())
    else {
        return vec![];
    };
    let i = incumbent.iter().rev().find(|s| s.value.is_some());
    let mut out: Vec<CaseDelta> = c
        .cases
        .iter()
        .map(|(id, &after)| {
            let before = i.and_then(|s| s.cases.get(id).copied());
            CaseDelta {
                id: id.clone(),
                before,
                after,
                delta_pct: before.and_then(|b| pct(after - b, b)),
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.delta_pct
            .map(f64::abs)
            .unwrap_or(0.0)
            .partial_cmp(&a.delta_pct.map(f64::abs).unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out.truncate(5);
    out
}

async fn finish_measure(
    env: &Env,
    exp: &mut ExperimentRecord,
    mut report: RunReport,
    taken: Vec<Sample>,
    metric: Option<f64>,
) -> Result<RunReport> {
    exp.state = ExperimentState::Measured;
    exp.verdict = Some(report.verdict.clone());
    exp.delta = report.delta;
    exp.delta_pct = report.delta_pct;
    exp.z = report.z;
    exp.n_samples += taken
        .iter()
        .filter(|s| s.role == Some(SampleRole::Candidate))
        .count() as u32;
    exp.metric = metric.or(exp.metric);
    exp.line = report.line.clone();
    env.store.update_experiment(exp).await?;
    report.samples = taken;
    Ok(report)
}

async fn interrupted(
    env: &Env,
    exp: &mut ExperimentRecord,
    taken: Vec<Sample>,
) -> Result<RunReport> {
    exp.state = ExperimentState::Interrupted;
    exp.finished_at = Some(now());
    exp.line = "interrupted".into();
    exp.n_samples += taken.len() as u32;
    env.store.update_experiment(exp).await?;
    mark_lost(&env.store, &exp.slug, exp.n).await?;
    clear_phase(&env.root, &exp.slug);
    Err(Error::refused("cancelled: the study was stopped"))
}

/// Samples of `(slug, n)` that never finished → `lost` (§8.5).
async fn mark_lost(store: &Store, slug: &str, n: u32) -> Result<()> {
    sqlx::query(
        "UPDATE samples SET lost = 1 WHERE slug = ?1 AND n = ?2 AND value IS NULL AND exit IS NULL",
    )
    .bind(slug)
    .bind(i64::from(n))
    .execute(store.pool())
    .await?;
    Ok(())
}

/// After a keep the candidate's samples belong to the new incumbent tree.
async fn retag_samples(store: &Store, slug: &str, n: u32, sha: &str) -> Result<()> {
    sqlx::query(
        "UPDATE samples SET tree_sha = ?3 WHERE slug = ?1 AND n = ?2 AND role = 'candidate'",
    )
    .bind(slug)
    .bind(i64::from(n))
    .bind(sha)
    .execute(store.pool())
    .await?;
    Ok(())
}

// ───────────────────────────── Log ─────────────────────────────

/// `Log` → `LogReport` (140 §4.3 kept/discarded…, §9.1 review, §8.4 stop rules).
pub async fn log(a: &mut Studies, m: Log) -> Result<LogReport> {
    log_env(&a.env(), m).await
}

/// [`log`] on an [`Env`] (off-mailbox).
pub async fn log_env(env: &Env, m: Log) -> Result<LogReport> {
    if m.slot.is_some() {
        return Err(Error::NotBuilt("W5"));
    }
    let mut study = study_of(env, m.session).await?;
    let slug = study.slug.clone();
    let mut exp = current_experiment(env, &study).await?;
    if exp.state == ExperimentState::Measuring {
        return Err(Error::refused(
            "still measuring; wait for experiment_run to return",
        ));
    }
    let a = m.args;
    let worktree = worktree_of(&study)?;
    let incumbent = incumbent_of(&study)?;
    let n = exp.n;
    exp.description = a.description.chars().take(500).collect();
    exp.learned = a.learned.chars().take(500).collect();
    exp.next = a.next.map(|s| s.chars().take(300).collect());
    exp.decision = Some(a.decision);
    exp.justification = a.justification.clone();

    // What the verdict and the decision make of it.
    let verdict = exp.verdict.clone().unwrap_or(Verdict::Refused {
        reason: "not measured".into(),
    });
    let mut keep = verdict.is_keep_candidate() && a.decision != Decision::Discard;
    if a.decision == Decision::KeepSimpler && !verdict.is_keep_candidate() {
        keep = keep_simpler_granted(env, &study, &exp, &worktree, &incumbent).await?;
        if keep {
            exp.verdict = Some(Verdict::KeepSimpler);
            exp.line = format!("keep-simpler granted; {}", exp.line);
        } else {
            exp.line = format!("keep-simpler refused; {}", exp.line);
        }
    }
    if keep
        && !exp.deviations.is_empty()
        && a.justification
            .as_deref()
            .map(str::trim)
            .unwrap_or("")
            .is_empty()
    {
        return Err(Error::refused(format!(
            "a keep that touches paths outside scope needs a justification: {}",
            exp.deviations.join(", ")
        )));
    }

    let mut report = LogReport::default();
    let runs = env.runs_dir(&slug, n);
    std::fs::create_dir_all(&runs)?;
    if keep {
        let meta = KeepMeta {
            slug: slug.clone(),
            n,
            description: exp.description.clone(),
            metric_line: metric_line(&study, &exp),
            category: exp.category.clone(),
            deviations: exp.deviations.clone(),
        };
        let previous_mean = study.incumbent_mean;
        let sha = env
            .backend
            .keep(&worktree, &study.spec, &meta, env.cfg.git.author.as_deref())?;
        retag_samples(&env.store, &slug, n, &sha).await?;
        exp.commit_sha = Some(sha.clone());
        exp.state = ExperimentState::Kept;
        study.incumbent_sha = Some(sha.clone());
        study.incumbent_mean = exp.metric.or(study.incumbent_mean);
        env.store.update_experiment(&exp).await?;
        env.store.update_study(&study).await?;
        report.commit = Some(sha.clone());

        // §9.1: review inside the call, before the next fire can build on it.
        if let Some(model) = env.cfg.reviewer(&study.spec.models, &env.agent) {
            progress(env, &slug, n, &exp.started_at, Phase::Review, 0, 1).await;
            let rv = match env
                .backend
                .review(&env.store, &study, n, &model, &env.rt)
                .await
            {
                Ok(v) => v,
                Err(e) => ReviewVerdict::Error {
                    reason: e.to_string(),
                },
            };
            let rv = match rv {
                ReviewVerdict::Flagged { reason }
                    if env.cfg.review.on_suspicious == OnSuspicious::Revert =>
                {
                    // Revert: incumbent ← previous, the commit stays as a rejected ref, the patch is kept.
                    let patch = runs.join("candidate.patch");
                    if let Ok(diff) = env.backend.diff_text(&worktree, &incumbent, &sha, 4 << 20) {
                        let _ = std::fs::write(&patch, diff);
                        exp.patch_path = Some(format!("{slug}/runs/{n}/candidate.patch"));
                    }
                    env.backend.reject(&worktree, &slug, n, &incumbent)?;
                    study.incumbent_sha = Some(incumbent.clone());
                    study.incumbent_mean = previous_mean;
                    exp.state = ExperimentState::RejectedByReview;
                    report.commit = None;
                    env.store
                        .append_event(
                            &slug,
                            "review",
                            &serde_json::json!({ "n": n, "reason": reason, "reverted": true }),
                        )
                        .await?;
                    ReviewVerdict::Reverted { reason }
                }
                other => other,
            };
            exp.review = Some(rv.clone());
            report.review = Some(rv);
            env.store.update_experiment(&exp).await?;
            env.store.update_study(&study).await?;
        }
    } else {
        let patch = runs.join("candidate.patch");
        match env.backend.discard(&worktree, &incumbent, &patch) {
            Ok(hash) => {
                exp.patch_path = Some(format!("{slug}/runs/{n}/candidate.patch"));
                if exp.patch_hash.as_deref().unwrap_or("").is_empty() {
                    exp.patch_hash = Some(hash);
                }
            }
            Err(e) => {
                tracing::warn!(%slug, n, "discard failed: {e}");
                env.store
                    .append_event(
                        &slug,
                        "incident",
                        &serde_json::json!({ "n": n, "what": format!("discard failed: {e}") }),
                    )
                    .await?;
            }
        }
        exp.state = match &verdict {
            Verdict::Crash => ExperimentState::Crashed,
            Verdict::ChecksFailed => ExperimentState::ChecksFailed,
            Verdict::Refused { .. } => ExperimentState::Refused,
            _ => ExperimentState::Discarded,
        };
    }
    exp.finished_at = Some(now());
    // Cost of this experiment = study spend now − spend at `Begin` (one ledger query each; already paid for).
    let spend = env.backend.spend_usd(study.session).await;
    let spend_at_begin = env
        .store
        .list_events(&slug, Some("begin"), 50)
        .await?
        .iter()
        .find(|e| e.payload.get("n").and_then(|v| v.as_u64()) == Some(u64::from(n)))
        .and_then(|e| e.payload.get("spend_usd").and_then(|v| v.as_f64()));
    if let Some(at_begin) = spend_at_begin {
        exp.cost_nanos = ((spend - at_begin).max(0.0) * 1e9) as u64;
    }
    env.store.update_experiment(&exp).await?;
    for id in &a.closes {
        if let Some(mut idea) = env.store.get_idea(*id).await?
            && idea.slug == slug
        {
            idea.status = IdeaStatus::Closed;
            env.store.update_idea(&idea).await?;
        }
    }
    clear_phase(&env.root, &slug);
    env.publish(StudyEvent::ExperimentFinished {
        slug: slug.clone(),
        n,
        verdict: exp.verdict.clone().unwrap_or(verdict),
        metric: exp.metric,
        delta_pct: exp.delta_pct,
        z: exp.z,
    })
    .await;

    let (closed, kept) = counts(&env.store, &slug).await?;
    // §5.4: the holdout on the incumbent every `holdout.every` keeps (never in the reply).
    if exp.state == ExperimentState::Kept
        && let Some(h) = study.spec.holdout.as_ref()
        && kept > 0
        && kept % h.every.max(1) == 0
        && let Some(sha) = study.incumbent_sha.as_deref()
    {
        match holdout(env, &study, sha, n, &cancel_token(&env.root, &slug)).await {
            Ok(values) => check_overfit(env, &study, n, &values).await?,
            Err(e) => tracing::warn!(%slug, n, "holdout failed: {e}"),
        }
    }
    report.applied = exp.state;
    report.incumbent = Incumbent {
        sha: study.incumbent_sha.clone().unwrap_or_default(),
        mean: study.incumbent_mean,
        kept,
    };
    report.budget_left = budget_left(
        &study.spec,
        closed,
        stop::hours_running(&study, chrono::Utc::now()),
        spend,
    );

    // §8.4 stop rules after the experiment closed.
    report.stop = apply_rules(env, &slug, spend).await?;
    Ok(report)
}

/// `Autoresearch-Metric: time_ms 42.3 -> 41.2 (-2.6%, 3.1 sigma, n=3)`.
fn metric_line(study: &StudyRecord, exp: &ExperimentRecord) -> String {
    format!(
        "{} {} -> {} ({:+.1}%, {} sigma, n={})",
        study.spec.metric.name,
        study
            .incumbent_mean
            .map(fmt_num)
            .unwrap_or_else(|| "?".into()),
        exp.metric.map(fmt_num).unwrap_or_else(|| "?".into()),
        exp.delta_pct.unwrap_or(0.0),
        exp.z
            .map(|z| format!("{z:.1}"))
            .unwrap_or_else(|| "—".into()),
        exp.n_samples
    )
}

/// §5.2 keep-simpler: not significantly worse, fewer lines, guards hold, checks pass.
async fn keep_simpler_granted(
    env: &Env,
    study: &StudyRecord,
    exp: &ExperimentRecord,
    worktree: &Path,
    incumbent: &str,
) -> Result<bool> {
    let gcfg = gate_cfg(&env.cfg, &study.spec)?;
    let cand: Vec<Sample> = env
        .store
        .list_samples(&study.slug, Some(exp.n))
        .await?
        .into_iter()
        .filter(|s| s.role == Some(SampleRole::Candidate))
        .collect();
    let inc = env.store.samples_of_tree(&study.slug, incumbent).await?;
    if clean_values(&cand).is_empty() {
        return Ok(false);
    }
    let input = GateInput {
        candidate: clean_values(&cand),
        incumbent: clean_values(&inc),
        sigma: study.sigma.unwrap_or(0.0),
        largest_accepted_delta: None,
        incumbent_age: 0,
        keep_simpler: true,
        secondaries: secondary_means(&cand, &inc),
    };
    let scope = env.backend.scope_diff(&study.spec, worktree)?;
    if !env
        .backend
        .keep_simpler(&gcfg, &input, scope.added, scope.removed)
    {
        return Ok(false);
    }
    let Some(c) = study.spec.checks.as_ref() else {
        return Ok(true);
    };
    let evaluator = env.evaluator_dir(&study.slug, study.segment);
    let runs = env.runs_dir(&study.slug, exp.n);
    let (exit, _, _) = env
        .backend
        .command(
            &c.command,
            schedule_file::duration(&c.timeout, Duration::from_secs(300)),
            SampleRequest {
                slug: &study.slug,
                n: exp.n,
                k: 0,
                tree_sha: incumbent,
                role: SampleRole::Candidate,
                worktree,
                evaluator: &evaluator,
                runs_dir: &runs,
            },
            &cancel_token(&env.root, &study.slug),
        )
        .await?;
    Ok(exit == Some(0))
}

// ───────────────────────────── stop rules, pause, finalize ─────────────────────────────

/// Evaluate §8.4 for `slug` and act; returns the stop reason when the study is finalizing.
async fn apply_rules(env: &Env, slug: &str, spend: f64) -> Result<Option<StopReason>> {
    let mut study = study_by_slug(&env.store, slug).await?;
    if !matches!(study.state, StudyState::Running | StudyState::Paused { .. }) {
        return Ok(study.stop_reason.clone());
    }
    match stop::evaluate(&env.store, &study, &env.cfg, spend).await? {
        stop::Rule::Continue => Ok(None),
        stop::Rule::Explore => {
            let (closed, _) = counts(&env.store, slug).await?;
            env.store
                .append_event(slug, "explore", &serde_json::json!({ "after_n": closed }))
                .await?;
            // §9.2: one advisor consult; its ideas enter the backlog as `advisor`, priority 2.
            let model = env.cfg.advisor(&study.spec.models);
            match env
                .backend
                .advise(&env.store, &study, model.as_deref(), &env.rt)
                .await
            {
                Ok(ideas) => {
                    for d in ideas {
                        let _ = env
                            .store
                            .insert_idea(&Idea {
                                slug: slug.to_owned(),
                                text: d.text,
                                category: d.category,
                                source: IdeaSource::Advisor,
                                priority: d.priority.unwrap_or(2).min(3),
                                ..Idea::default()
                            })
                            .await;
                    }
                }
                Err(e) => tracing::warn!(%slug, "advisor consult failed: {e}"),
            }
            Ok(None)
        }
        stop::Rule::Pause(reason) => {
            pause(env, &mut study, reason).await?;
            Ok(None)
        }
        stop::Rule::Stop(reason) => {
            begin_finalize(env, study, reason.clone()).await?;
            Ok(Some(reason))
        }
    }
}

/// After an experiment closed outside `Log` (abandoned, interrupted): the stop rules.
pub async fn after_experiment(env: &Env, slug: &str) -> Result<()> {
    let study = study_by_slug(&env.store, slug).await?;
    let spend = env.backend.spend_usd(study.session).await;
    apply_rules(env, slug, spend).await.map(|_| ())
}

/// Is a user `stop_after` waiting (recorded after the last resume)?
pub async fn stop_after_pending(env: &Env, study: &StudyRecord) -> Result<bool> {
    Ok(stop::view(&env.store, study, 0.0).await?.stop_after)
}

async fn pause(env: &Env, study: &mut StudyRecord, reason: PauseReason) -> Result<()> {
    if let Err(e) = schedule_file::set_enabled(&study.slug, false).await {
        tracing::warn!(slug = %study.slug, "schedule not disabled (the ledger state still refuses fires): {e}");
    }
    env.store
        .append_event(
            &study.slug,
            "paused",
            &serde_json::json!({ "reason": pause_tag(&reason), "at": now() }),
        )
        .await?;
    let word = pause_word(&reason);
    set_state(env, study, StudyState::Paused { reason }, Some(word)).await
}

async fn resume(env: &Env, study: &mut StudyRecord) -> Result<()> {
    if let Some(e) = env
        .store
        .list_events(&study.slug, Some("paused"), 1)
        .await?
        .first()
        && let Some(at) = e.payload.get("at").and_then(|v| v.as_str())
        && let Ok(t) = chrono::DateTime::parse_from_rfc3339(at)
    {
        study.paused_ms += (chrono::Utc::now() - t.with_timezone(&chrono::Utc))
            .num_milliseconds()
            .max(0) as u64;
    }
    let (closed, _) = counts(&env.store, &study.slug).await?;
    env.store
        .append_event(
            &study.slug,
            "resumed",
            &serde_json::json!({ "after_n": closed }),
        )
        .await?;
    if let Err(e) = schedule_file::set_enabled(&study.slug, true).await {
        tracing::warn!(slug = %study.slug, "schedule not enabled: {e}");
    }
    set_state(env, study, StudyState::Running, None).await
}

/// Enter `finalizing` (140 §4.2) and run §11 off the mailbox.
pub async fn begin_finalize(env: &Env, mut study: StudyRecord, reason: StopReason) -> Result<()> {
    if matches!(study.state, StudyState::Finalizing) || is_terminal(&study.state) {
        return Ok(());
    }
    cancel(&env.root, &study.slug);
    study.stop_reason = Some(reason.clone());
    env.store
        .append_event(
            &study.slug,
            "stop",
            &serde_json::json!({ "reason": reason }),
        )
        .await?;
    set_state(
        env,
        &mut study,
        StudyState::Finalizing,
        Some(stop_word(&reason)),
    )
    .await?;
    // End the loop: `Done` from inside the run when it is still open, else delete the file.
    let ended = match study.session {
        Some(s) => schedule_file::done_run(s, &stop_word(&reason))
            .await
            .is_ok(),
        None => false,
    };
    if !ended && let Err(e) = schedule_file::done(&study.slug, &stop_word(&reason)).await {
        tracing::warn!(slug = %study.slug, "schedule file not removed: {e}");
    }
    let env2 = env.clone();
    tokio::spawn(async move { finalize_task(env2, study.slug).await });
    Ok(())
}

/// §11 steps through W2's `finalize::run`, then the record, the result card, the event.
pub async fn finalize_task(env: Env, slug: String) {
    let Ok(Some(mut study)) = env.store.get_study(&slug).await else {
        return;
    };
    let reason = study.stop_reason.clone().unwrap_or(StopReason::User);
    match env
        .backend
        .finalize(&env.store, &study, reason.clone(), &env.rt)
        .await
    {
        Ok((headline, doc)) => {
            study.finished_at = Some(now());
            study.report_doc = Some(doc.clone());
            if let Err(e) = set_state(
                &env,
                &mut study,
                StudyState::Finished,
                Some(headline.text.clone()),
            )
            .await
            {
                tracing::warn!(%slug, "finished state not recorded: {e}");
            }
            if let Err(e) = env
                .backend
                .record(&env.store, &study, &headline.text, &env.rt)
                .await
            {
                tracing::warn!(%slug, "knowledge record not written: {e}");
            }
            let series = chart_series(&env, &study).await.unwrap_or_default();
            let (closed, kept) = counts(&env.store, &slug).await.unwrap_or((0, 0));
            let changesets = changesets_of(&env, &slug).await;
            deliver(
                study.origin,
                &slug,
                result_card(&study, &headline, &series, &changesets, closed, kept),
            );
            env.publish(StudyEvent::StudyFinished {
                slug: slug.clone(),
                reason,
                headline: headline.text,
                report_doc: doc,
            })
            .await;
        }
        Err(e) => {
            let reason = format!("finalize: {e}");
            tracing::warn!(%slug, "{reason}");
            let _ = set_state(
                &env,
                &mut study,
                StudyState::Failed {
                    reason: reason.clone(),
                },
                Some(reason.clone()),
            )
            .await;
            deliver(
                study.origin,
                &slug,
                format!("**Autoresearch study `{slug}` could not finalize** — {reason}"),
            );
        }
    }
    clear_phase(&env.root, &slug);
}

/// Finalize's changesets, as W2 records them (`events.kind = "changeset"`, payload = `Changeset`).
async fn changesets_of(env: &Env, slug: &str) -> Vec<Changeset> {
    let mut out: Vec<Changeset> = env
        .store
        .list_events(slug, Some("changeset"), 200)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| serde_json::from_value(e.payload).ok())
        .collect();
    out.sort_by_key(|c: &Changeset| c.k);
    out.dedup_by_key(|c| c.k);
    out
}

// ───────────────────────────── Notes, Steer, IdeaOp ─────────────────────────────

/// `Notes` → `NotesReport` (140 §7.3 playbook and ideas).
pub async fn notes(a: &mut Studies, m: Notes) -> Result<NotesReport> {
    let env = a.env();
    let mut study = study_of(&env, m.session).await?;
    if let Some(p) = m.args.playbook {
        let cap = env.cfg.brief.playbook_bytes as usize;
        if p.len() > cap {
            return Err(Error::refused(format!(
                "playbook is {} bytes; the cap is {cap}. Shorten it (it replaces the whole text).",
                p.len()
            )));
        }
        study.playbook = p;
        env.store.update_study(&study).await?;
    }
    let mut added = 0;
    for d in m.args.add_ideas {
        if d.text.trim().is_empty() {
            continue;
        }
        env.store
            .insert_idea(&Idea {
                slug: study.slug.clone(),
                text: d.text,
                category: d.category,
                source: IdeaSource::Model,
                priority: d.priority.unwrap_or(1).min(3),
                ..Idea::default()
            })
            .await?;
        added += 1;
    }
    Ok(NotesReport {
        playbook_bytes: study.playbook.len() as u32,
        ideas_added: added,
    })
}

/// `Steer` → the steer id (140 §7.3 Steers). Never pauses the loop.
pub async fn steer(a: &mut Studies, m: SteerMsg) -> Result<SteerId> {
    let text = m.text.trim();
    if text.is_empty() {
        return Err(Error::refused("steer text is empty"));
    }
    study_by_slug(&a.store, &m.slug).await?;
    a.store.insert_steer(&m.slug, text, m.by).await
}

/// `IdeaOp` → `Idea` (140 §7.3 Ideas).
pub async fn idea(a: &mut Studies, m: IdeaOp) -> Result<Idea> {
    study_by_slug(&a.store, &m.slug).await?;
    let fetch = |id: IdeaId, store: Store, slug: String| async move {
        store
            .get_idea(id)
            .await?
            .filter(|i| i.slug == slug)
            .ok_or_else(|| Error::refused(format!("no idea {id} in `{slug}`")))
    };
    match m.op {
        IdeaChange::Add {
            text,
            category,
            priority,
        } => {
            if text.trim().is_empty() {
                return Err(Error::refused("idea text is empty"));
            }
            let (source, priority) = match m.by {
                By::User | By::Setup => (
                    if m.by == By::User {
                        IdeaSource::User
                    } else {
                        IdeaSource::Setup
                    },
                    3,
                ),
                By::Advisor => (IdeaSource::Advisor, priority.unwrap_or(2).min(3)),
                By::Model | By::Plugin => (IdeaSource::Model, priority.unwrap_or(1).min(3)),
            };
            let idea = Idea {
                slug: m.slug.clone(),
                text: text.trim().to_owned(),
                category,
                source,
                priority,
                ..Idea::default()
            };
            let id = a.store.insert_idea(&idea).await?;
            Ok(Idea {
                id,
                at: now(),
                ..idea
            })
        }
        IdeaChange::Priority { id, priority } => {
            let mut i = fetch(id, a.store.clone(), m.slug).await?;
            i.priority = priority.min(3);
            a.store.update_idea(&i).await?;
            Ok(i)
        }
        IdeaChange::Close { id } => {
            let mut i = fetch(id, a.store.clone(), m.slug).await?;
            i.status = IdeaStatus::Closed;
            a.store.update_idea(&i).await?;
            Ok(i)
        }
        IdeaChange::Reopen { id } => {
            let mut i = fetch(id, a.store.clone(), m.slug).await?;
            i.status = IdeaStatus::Open;
            a.store.update_idea(&i).await?;
            Ok(i)
        }
    }
}

// ───────────────────────────── Control ─────────────────────────────

/// `Control` → `StudyInfo` (140 §8.6).
pub async fn control(a: &mut Studies, m: Control) -> Result<StudyInfo> {
    let env = a.env();
    let mut study = study_by_slug(&env.store, &m.slug).await?;
    let slug = study.slug.clone();
    match m.op {
        ControlOp::Pause => match study.state {
            StudyState::Running => pause(&env, &mut study, PauseReason::User).await?,
            StudyState::Paused { .. } => {}
            ref s => {
                return Err(Error::refused(format!(
                    "cannot pause a study that is {}",
                    state_word(s)
                )));
            }
        },
        ControlOp::Resume => match study.state {
            StudyState::Paused { .. } => resume(&env, &mut study).await?,
            StudyState::Running => {}
            ref s => {
                return Err(Error::refused(format!(
                    "cannot resume a study that is {}",
                    state_word(s)
                )));
            }
        },
        ControlOp::StopAfter => {
            if !matches!(study.state, StudyState::Running | StudyState::Paused { .. }) {
                return Err(Error::refused(format!(
                    "cannot stop a study that is {}",
                    state_word(&study.state)
                )));
            }
            env.store
                .append_event(&slug, "stop_after", &serde_json::json!({ "by": m.by }))
                .await?;
            if env.store.dangling_experiment(&slug, None).await?.is_none() {
                begin_finalize(&env, study.clone(), StopReason::User).await?;
            }
        }
        ControlOp::StopNow => {
            if !matches!(study.state, StudyState::Running | StudyState::Paused { .. }) {
                return Err(Error::refused(format!(
                    "cannot stop a study that is {}",
                    state_word(&study.state)
                )));
            }
            cancel(&env.root, &slug);
            if let Some(session) = study.session
                && let Some(agents) = agents()
            {
                let _ = agents
                    .ask(agent::Cancel {
                        session,
                        keep: None,
                        scope: agent::CancelScope::Tree,
                    })
                    .await;
            }
            if let Some(mut exp) = env.store.dangling_experiment(&slug, None).await? {
                exp.state = ExperimentState::Interrupted;
                exp.finished_at = Some(now());
                exp.line = "interrupted: stop now".into();
                env.store.update_experiment(&exp).await?;
                mark_lost(&env.store, &slug, exp.n).await?;
                // Reset the worktree to the incumbent now: finalize starts from a clean tree.
                if let (Ok(w), Ok(i)) = (worktree_of(&study), incumbent_of(&study)) {
                    let patch = env.runs_dir(&slug, exp.n).join("candidate.patch");
                    let _ = std::fs::create_dir_all(patch.parent().unwrap());
                    if let Err(e) = env.backend.discard(&w, &i, &patch) {
                        tracing::warn!(%slug, "worktree not reset at stop now: {e}");
                    }
                }
                clear_phase(&env.root, &slug);
            }
            begin_finalize(&env, study.clone(), StopReason::UserNow).await?;
        }
        ControlOp::RunOnce => {
            let paused = matches!(study.state, StudyState::Paused { .. });
            if !paused && study.state != StudyState::Running {
                return Err(Error::refused(format!(
                    "cannot run a study that is {}",
                    state_word(&study.state)
                )));
            }
            schedule_file::fire(&slug, paused).await?;
        }
        ControlOp::Segment { reason } => {
            if !matches!(study.state, StudyState::Running | StudyState::Paused { .. }) {
                return Err(Error::refused(format!(
                    "cannot re-baseline a study that is {}",
                    state_word(&study.state)
                )));
            }
            if env.store.dangling_experiment(&slug, None).await?.is_some() {
                return Err(Error::refused(
                    "an experiment is in flight; pause or wait for it, then open the segment",
                ));
            }
            let env2 = env.clone();
            let study2 = study.clone();
            tokio::spawn(async move { segment_task(env2, study2, reason, m.by).await });
        }
        ControlOp::Finalize => {
            if !matches!(study.state, StudyState::Running | StudyState::Paused { .. }) {
                return Err(Error::refused(format!(
                    "cannot finalize a study that is {}",
                    state_word(&study.state)
                )));
            }
            begin_finalize(&env, study.clone(), StopReason::User).await?;
        }
        ControlOp::Restore { n } => {
            let exp = env
                .store
                .get_experiment(&slug, n)
                .await?
                .ok_or_else(|| Error::refused(format!("no experiment #{n}")))?;
            if exp.state != ExperimentState::RejectedByReview {
                return Err(Error::refused(format!(
                    "experiment #{n} was not reverted by review"
                )));
            }
            if is_terminal(&study.state) {
                return Err(Error::refused(format!(
                    "study `{slug}` is {}; nothing left to restore into",
                    state_word(&study.state)
                )));
            }
            // The ★ idea is the brief's explanation; the harness re-applies the change at the
            // next experiment_begin (`restore_rejected`) and the model only measures it.
            env.store
                .insert_idea(&Idea {
                    slug: slug.clone(),
                    text: format!(
                        "Restore experiment #{n} ({}): the user wants it measured again. The harness re-applies \
                         its change at the next experiment_begin; experiment_run it as is, without editing.",
                        exp.idea
                    ),
                    category: exp.category.clone(),
                    source: IdeaSource::User,
                    priority: 3,
                    ..Idea::default()
                })
                .await?;
            env.store
                .append_event(&slug, "restore", &serde_json::json!({ "n": n }))
                .await?;
        }
        ControlOp::KeepReverted { n } => {
            let exp = env
                .store
                .get_experiment(&slug, n)
                .await?
                .ok_or_else(|| Error::refused(format!("no experiment #{n}")))?;
            if exp.state != ExperimentState::RejectedByReview {
                return Err(Error::refused(format!(
                    "experiment #{n} was not reverted by review"
                )));
            }
            env.store
                .append_event(
                    &slug,
                    "kept_reverted",
                    &serde_json::json!({ "n": n, "by": m.by }),
                )
                .await?;
        }
        ControlOp::Merge { changeset } => {
            if study.state != StudyState::Finished {
                return Err(Error::refused(format!(
                    "changesets exist once the study is finished (it is {})",
                    state_word(&study.state)
                )));
            }
            let cs = changesets_of(&env, &slug)
                .await
                .into_iter()
                .find(|c| c.k == changeset)
                .ok_or_else(|| Error::refused(format!("no changeset #{changeset}")))?;
            let sha = env.backend.merge_into_checkout(
                Path::new(&study.repo),
                &cs.branch,
                env.cfg.git.author.as_deref(),
            )?;
            env.store
                .append_event(
                    &slug,
                    "merged",
                    &serde_json::json!({ "k": changeset, "branch": cs.branch, "sha": sha, "by": m.by }),
                )
                .await?;
        }
        ControlOp::Discard => {
            if matches!(
                study.state,
                StudyState::Running | StudyState::Preparing | StudyState::Finalizing
            ) {
                return Err(Error::refused(format!(
                    "stop the study before discarding it (it is {})",
                    state_word(&study.state)
                )));
            }
            cancel(&env.root, &slug);
            if let Err(e) = schedule_file::done(&slug, "discarded").await {
                tracing::debug!(%slug, "schedule file not removed on discard: {e}");
            }
            if let Some(w) = study.worktree.as_deref().map(PathBuf::from)
                && w.exists()
                && let Err(e) = env.backend.worktree_remove(Path::new(&study.repo), &w)
            {
                tracing::warn!(%slug, "worktree not removed: {e}");
            }
            set_state(&env, &mut study, StudyState::Discarded, None).await?;
        }
    }
    let study = study_by_slug(&env.store, &slug).await?;
    info_full(&env, &study).await
}

/// `Segment`: new custody + baseline on the incumbent; σ restarts (140 §5.3).
async fn segment_task(env: Env, mut study: StudyRecord, reason: String, by: By) {
    let slug = study.slug.clone();
    let r: Result<()> = async {
        let segment = study.segment + 1;
        let evaluator = env.evaluator_dir(&slug, segment);
        let custody = env
            .backend
            .custody_take(&study.spec, Path::new(&study.repo), &evaluator)?;
        let mut seg = SegmentRecord {
            slug: slug.clone(),
            segment,
            opened_at: now(),
            opened_by: by,
            reason: Some(reason),
            custody,
            baseline_mean: None,
            baseline_n: 0,
        };
        env.store.put_segment(&seg).await?;
        study.segment = segment;
        env.store.update_study(&study).await?;
        let incumbent = incumbent_of(&study)?;
        let values = baseline(
            &env,
            &study,
            &incumbent,
            0,
            SampleRole::Baseline,
            &cancel_token(&env.root, &slug),
        )
        .await?;
        seg.baseline_mean = Some(mean(&values));
        seg.baseline_n = values.len() as u32;
        env.store.put_segment(&seg).await?;
        study.incumbent_mean = Some(mean(&values));
        study.sigma = Some(segment_sigma(&env, &study).await?);
        env.store.update_study(&study).await?;
        env.store
            .append_event(&slug, "segment", &serde_json::json!({ "segment": segment }))
            .await?;
        env.publish(StudyEvent::StudyChanged {
            slug: slug.clone(),
            state: study.state.clone(),
            reason: Some(format!("segment {segment} opened")),
        })
        .await;
        Ok(())
    }
    .await;
    if let Err(e) = r {
        tracing::warn!(%slug, "segment not opened: {e}");
        let _ = env
            .store
            .append_event(
                &slug,
                "incident",
                &serde_json::json!({ "what": format!("segment failed: {e}") }),
            )
            .await;
    }
}

// ───────────────────────────── reads ─────────────────────────────

/// `Status` → one or every `StudyInfo` (140 §12.5 `autoresearch.list`).
pub async fn status(a: &mut Studies, m: Status) -> Result<Vec<StudyInfo>> {
    let env = a.env();
    let studies = match m.slug {
        Some(slug) => vec![study_by_slug(&env.store, &slug).await?],
        None => env.store.list_studies().await?,
    };
    let mut out = Vec::with_capacity(studies.len());
    for s in &studies {
        out.push(info_full(&env, s).await?);
    }
    Ok(out)
}

async fn chart_series(env: &Env, study: &StudyRecord) -> Result<ChartSeries> {
    let mut exps = env
        .store
        .list_experiments(&study.slug, None, u32::MAX)
        .await?;
    exps.reverse();
    let seg = env.store.get_segment(&study.slug, study.segment).await?;
    let mut series = ChartSeries {
        baseline_mean: seg.as_ref().and_then(|s| s.baseline_mean),
        sigma: study.sigma,
        ..ChartSeries::default()
    };
    if let Some(b) = series.baseline_mean {
        series.incumbent.push(ChartPoint {
            n: 0,
            verdict: String::new(),
            metric: Some(b),
            at: seg
                .as_ref()
                .map(|s| s.opened_at.clone())
                .unwrap_or_default(),
        });
    }
    for e in &exps {
        let at = e
            .finished_at
            .clone()
            .unwrap_or_else(|| e.started_at.clone());
        series.experiments.push(ChartPoint {
            n: e.n,
            // The ledger state colours the dot; a discard says why (worse / guard failed / no gain).
            verdict: match (e.state, e.verdict.as_ref()) {
                (ExperimentState::Discarded, Some(v)) => v.tag(),
                (s, _) => s.tag(),
            }
            .to_owned(),
            metric: e.metric,
            at: at.clone(),
        });
        if e.state == ExperimentState::Kept {
            series.incumbent.push(ChartPoint {
                n: e.n,
                verdict: String::new(),
                metric: e.metric,
                at,
            });
        }
    }
    for s in env.store.list_samples(&study.slug, None).await? {
        if s.role == Some(SampleRole::Holdout) && s.value.is_some() {
            series.holdout.push(ChartPoint {
                n: s.n,
                verdict: String::new(),
                metric: s.value,
                at: s.at,
            });
        }
    }
    for e in env
        .store
        .list_events(&study.slug, Some("drift"), 100)
        .await?
    {
        series.drift.push(ChartPoint {
            n: e.payload.get("n").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            verdict: String::new(),
            metric: e.payload.get("value").and_then(|v| v.as_f64()),
            at: e.at,
        });
    }
    Ok(series)
}

/// `Detail` → `StudyDetail` with the chart series (140 §12.5 `autoresearch.get`).
pub async fn detail(a: &mut Studies, m: Detail) -> Result<StudyDetail> {
    let env = a.env();
    let study = study_by_slug(&env.store, &m.slug).await?;
    let info = info_full(&env, &study).await?;
    let series = chart_series(&env, &study).await?;
    let mut ideas = env.store.list_ideas(&study.slug, None).await?;
    ideas.sort_by_key(|i| {
        (
            i.status != IdeaStatus::Open,
            std::cmp::Reverse(i.priority),
            i.id,
        )
    });
    let steers = env.store.list_steers(&study.slug, false).await?;
    let changesets = changesets_of(&env, &study.slug).await;
    let mut needs_eyes = Vec::new();
    let dismissed = dismissed_reverts(&env.store, &study.slug).await;
    for e in env
        .store
        .list_experiments(&study.slug, None, u32::MAX)
        .await?
    {
        match (&e.state, &e.review) {
            (ExperimentState::RejectedByReview, Some(ReviewVerdict::Reverted { reason }))
                if !dismissed.contains(&u64::from(e.n)) =>
            {
                needs_eyes.push(format!("#{} reverted by review — {reason} · Restore", e.n));
            }
            (_, Some(ReviewVerdict::Flagged { reason })) => {
                needs_eyes.push(format!("#{} flagged by review — {reason}", e.n))
            }
            (_, Some(ReviewVerdict::Error { reason })) if e.state == ExperimentState::Kept => {
                needs_eyes.push(format!("#{} kept unreviewed — {reason}", e.n));
            }
            _ => {}
        }
        if e.state == ExperimentState::Kept && !e.deviations.is_empty() {
            needs_eyes.push(format!(
                "#{} kept with edits outside scope: {} — {}",
                e.n,
                e.deviations.join(", "),
                e.justification.as_deref().unwrap_or("no justification")
            ));
        }
    }
    for e in env.store.list_events(&study.slug, None, 500).await? {
        match e.kind.as_str() {
            "overfit" => needs_eyes.push(format!("holdout says overfit ({})", e.at)),
            "drift" => needs_eyes.push(format!(
                "incumbent drift at experiment {} ({})",
                e.payload.get("n").and_then(|v| v.as_u64()).unwrap_or(0),
                e.at
            )),
            _ => {}
        }
    }
    // The live tail: the newest log under runs/<n>/ while an experiment is measuring.
    let tail = phase_of(&env.root, &study.slug).and_then(|(n, _)| {
        let dir = env.runs_dir(&study.slug, n);
        let newest = std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "log"))
            .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())?;
        let text = env.backend.tail(&newest.path());
        (!text.is_empty()).then_some(text)
    });
    Ok(StudyDetail {
        info,
        spec: study.spec.clone(),
        playbook: study.playbook.clone(),
        series,
        ideas,
        steers,
        changesets,
        needs_eyes,
        tail,
    })
}

/// `Experiments` → rows newest first (140 §12.5).
pub async fn experiments(a: &mut Studies, m: Experiments) -> Result<Vec<ExperimentRow>> {
    study_by_slug(&a.store, &m.slug).await?;
    Ok(a.store
        .list_experiments(&m.slug, m.before, m.limit.clamp(1, 1000))
        .await?
        .iter()
        .map(ExperimentRecord::row)
        .collect())
}

/// `Experiment` → `ExperimentDetail` (140 §12.5).
pub async fn experiment(a: &mut Studies, m: Experiment) -> Result<ExperimentDetail> {
    let env = a.env();
    let study = study_by_slug(&env.store, &m.slug).await?;
    let e = env
        .store
        .get_experiment(&m.slug, m.n)
        .await?
        .ok_or_else(|| Error::refused(format!("no experiment #{} in `{}`", m.n, m.slug)))?;
    let samples = env.store.list_samples(&m.slug, Some(m.n)).await?;
    const MAX_DIFF: usize = 256 * 1024;
    let diff = match (&e.commit_sha, &e.patch_path, study.worktree.as_deref()) {
        (Some(sha), _, Some(w)) if e.state == ExperimentState::Kept => env
            .backend
            .diff_text(Path::new(w), &e.parent_sha, sha, MAX_DIFF)
            .ok(),
        (_, Some(p), _) => std::fs::read_to_string(env.root.join(p)).ok().map(|mut s| {
            if s.len() > MAX_DIFF {
                let mut cut = MAX_DIFF;
                while !s.is_char_boundary(cut) {
                    cut -= 1;
                }
                s.truncate(cut);
                s.push_str("\n… (truncated)");
            }
            s
        }),
        _ => None,
    };
    let tails = samples
        .iter()
        .filter_map(|s| s.log_path.as_deref())
        .map(|p| env.backend.tail(&env.root.join(p)))
        .filter(|t| !t.is_empty())
        .collect();
    let inc = env.store.samples_of_tree(&m.slug, &e.parent_sha).await?;
    Ok(ExperimentDetail {
        row: e.row(),
        hypothesis: e.hypothesis.clone(),
        description: e.description.clone(),
        learned: e.learned.clone(),
        next: e.next.clone(),
        decision: e.decision,
        justification: e.justification.clone(),
        parent_sha: e.parent_sha.clone(),
        patch_hash: e.patch_hash.clone(),
        cases_moved: cases_moved(&samples, &inc),
        samples,
        line: e.line.clone(),
        diff,
        tails,
    })
}

// ───────────────────────────── restart recovery (§8.5) ─────────────────────────────

/// At plugin start: experiments left open/measuring → `interrupted` (their
/// unfinished samples `lost`), a study left `preparing` → `failed`, a study
/// left `finalizing` resumes its steps. The worktree is reset at the next begin.
pub async fn recover(env: &Env) -> Result<()> {
    for mut study in env.store.list_studies().await? {
        let slug = study.slug.clone();
        match study.state {
            StudyState::Preparing => {
                let reason =
                    "engine restarted while preparing; discard the study and start it again"
                        .to_owned();
                set_state(
                    env,
                    &mut study,
                    StudyState::Failed {
                        reason: reason.clone(),
                    },
                    Some(reason),
                )
                .await?;
            }
            StudyState::Running | StudyState::Paused { .. } => {
                if let Some(mut exp) = env.store.dangling_experiment(&slug, None).await? {
                    exp.state = ExperimentState::Interrupted;
                    exp.finished_at = Some(now());
                    exp.line = "interrupted: engine restart".into();
                    env.store.update_experiment(&exp).await?;
                    mark_lost(&env.store, &slug, exp.n).await?;
                    env.store
                        .append_event(&slug, "incident", &serde_json::json!({ "n": exp.n, "what": "interrupted by an engine restart" }))
                        .await?;
                }
            }
            StudyState::Finalizing => {
                let env2 = env.clone();
                tokio::spawn(async move { finalize_task(env2, slug).await });
            }
            _ => {}
        }
    }
    Ok(())
}

// ───────────────────────────── attention rows (§13.1) ─────────────────────────────

/// The inbox source: `review` rows for finished studies and keeps reverted by
/// review (Restore / Keep reverted are the row's actions), `blocked` rows for
/// pauses that need the user. Pulled on every `ListAttention`; dismiss applies by id.
pub struct AttentionSource(pub Store);

fn ms_of(at: &str) -> u64 {
    chrono::DateTime::parse_from_rfc3339(at)
        .map(|t| t.timestamp_millis().max(0) as u64)
        .unwrap_or(0)
}

#[async_trait::async_trait]
impl roster::attention::Source for AttentionSource {
    async fn items(&self) -> Vec<roster::attention::AttentionItem> {
        use roster::attention::{AttentionItem, AttentionKind};
        let item = |id: String,
                    kind: AttentionKind,
                    title: String,
                    detail: Option<String>,
                    at: &str,
                    session: Option<SessionId>| AttentionItem {
            id,
            kind,
            title,
            detail,
            at_ms: ms_of(at),
            task: None,
            agent: None,
            session: session.map(|s| s.to_string()),
            approval: None,
            decision: None,
            packet: None,
            ask: None,
            turn: None,
            waiting_since_ms: None,
            summary: None,
            next_steps: Vec::new(),
            priority: None,
            rank: 5,
        };
        let mut out = Vec::new();
        let Ok(studies) = self.0.list_studies().await else {
            return out;
        };
        for s in studies {
            match &s.state {
                StudyState::Finished => {
                    let delta = match (
                        s.incumbent_mean,
                        self.0
                            .get_segment(&s.slug, 0)
                            .await
                            .ok()
                            .flatten()
                            .and_then(|g| g.baseline_mean),
                    ) {
                        (Some(i), Some(b)) => {
                            fmt_pct(pct(gate::normalise(s.spec.metric.direction, i, b), b))
                        }
                        _ => "—".into(),
                    };
                    out.push(item(
                        format!("autoresearch:finished:{}", s.slug),
                        AttentionKind::Review,
                        format!("Study {} finished: {delta} · Review", s.slug),
                        s.report_doc.as_ref().map(|d| format!("report {d}")),
                        s.finished_at.as_deref().unwrap_or(&s.created_at),
                        s.origin,
                    ));
                }
                StudyState::Paused { reason } if *reason != PauseReason::User => {
                    out.push(item(
                        format!("autoresearch:paused:{}:{}", s.slug, pause_tag(reason)),
                        AttentionKind::Blocked,
                        format!("Study {} paused: {}", s.slug, pause_word(reason)),
                        None,
                        s.started_at.as_deref().unwrap_or(&s.created_at),
                        s.origin,
                    ));
                }
                _ => {}
            }
            if is_terminal(&s.state) && s.state != StudyState::Finished {
                continue;
            }
            let restored = dismissed_reverts(&self.0, &s.slug).await;
            for e in self
                .0
                .list_experiments(&s.slug, None, u32::MAX)
                .await
                .unwrap_or_default()
            {
                if let (ExperimentState::RejectedByReview, Some(ReviewVerdict::Reverted { reason })) =
                    (&e.state, &e.review)
                    && !restored.contains(&u64::from(e.n))
                {
                    out.push(item(
                        format!("autoresearch:reverted:{}:{}", s.slug, e.n),
                        AttentionKind::Review,
                        format!(
                            "Study {}: experiment {} was reverted as possible gaming — {reason} · Restore · Keep reverted",
                            s.slug, e.n
                        ),
                        Some(e.idea.clone()),
                        e.finished_at.as_deref().unwrap_or(&e.started_at),
                        s.origin,
                    ));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests;
