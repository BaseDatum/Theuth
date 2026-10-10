//! W3 LOOP — the `Studies` actor's handler bodies (design doc 140 §12.3).
//! [`crate::Studies`] (lib.rs) owns the struct and the `Message` impls; each
//! impl calls the function here of the same name, so this file is the whole
//! of the actor's logic: Create (worktree, custody, warm, baseline, schedule
//! file), Begin (drift repair, dangling experiment, brief), Run (novelty,
//! custody, scope, samples, gate, checks), Log (keep/discard, review, stop
//! rules), Notes, Control, Steer, IdeaOp, Status, Detail, Experiments,
//! Experiment, StartDraft. The `slot` / `parallel > 1` branches of Begin, Run
//! and Log go through [`crate::wave`] (W5).
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
use crate::store::{ExperimentRecord, SegmentRecord, Store, StudyPatch, StudyRecord, now};
use crate::types::*;
use crate::{binding, schedule_file, stop};

// ───────────────────────────── transient process state ─────────────────────────────

/// Per-study cancellation of the bench group (`StopNow`, a dropped tool call). Not a row: it is process state.
static CANCELS: LazyLock<Mutex<HashMap<String, CancellationToken>>> =
    LazyLock::new(Default::default);
/// The experiment in flight and its phase, for `StudyInfo.current/phase` (derived display state).
static PHASES: LazyLock<Mutex<HashMap<String, (u32, Phase)>>> = LazyLock::new(Default::default);
/// Finalize tasks in flight (one per study; a deferred finalize and its fallback timer never run twice).
static FINALIZING: LazyLock<Mutex<std::collections::HashSet<String>>> =
    LazyLock::new(Default::default);
/// Wall-clock budget of the §11.3 report side call.
const REPORT_CALL_BUDGET: Duration = Duration::from_secs(300);
/// A finalize deferred to the study turn's end runs anyway after this long (a missed `TurnFinished`).
const FINALIZE_DEFER_MAX: Duration = Duration::from_secs(10 * 60);
/// Live-tail cadence while a sample runs (the UI refetches the detail on every progress event).
const SAMPLE_TICK: Duration = Duration::from_secs(3);

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

    pub(crate) fn runs_dir(&self, slug: &str, n: u32) -> PathBuf {
        self.study_dir(slug).join("runs").join(n.to_string())
    }

    pub(crate) fn evaluator_dir(&self, slug: &str, segment: u32) -> PathBuf {
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
    /// `base` → sha; a dirty tree is refused unless the spec says `include_dirty` (140 §6).
    fn resolve_base(&self, repo: &Path, base: &str, include_dirty: bool) -> Result<String> {
        crate::git::resolve_base(repo, base, include_dirty)
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
    /// Re-apply a saved patch as uncommitted changes (the candidate put back after an incumbent sample).
    fn apply_patch(&self, worktree: &Path, patch_path: &Path) -> Result<()> {
        crate::git::apply_patch(worktree, patch_path)
    }
    /// Discard study: every worktree under the study dir, `autoresearch/<slug>`, `+k` branches, rejected refs (140 §11.5).
    fn discard_study(&self, repo: &Path, slug: &str, study_dir: &Path) -> Result<Vec<String>> {
        crate::git::discard_study(repo, slug, study_dir)
    }
    /// First parent of `sha` (the keep-crash adoption check, R-50). Read-only.
    fn commit_parent(&self, worktree: &Path, sha: &str) -> Result<String> {
        // ponytail: one read-only `git` call here; F2's git.rs read helper is private (R-18 hand-off).
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(worktree)
            .args(["rev-parse", "--verify", "-q", &format!("{sha}^")])
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()?;
        if !out.status.success() {
            return Err(Error::refused(format!("{sha} has no parent")));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
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
    /// `git worktree add --detach` (wave candidates).
    fn worktree_add_detached(&self, repo: &Path, worktree: &Path, sha: &str) -> Result<()> {
        crate::git::worktree_add_detached(repo, worktree, sha)
    }
    /// `git cherry-pick <sha>` onto the study branch; `Ok(false)` = conflict (aborted).
    fn cherry_pick(&self, worktree: &Path, sha: &str) -> Result<bool> {
        crate::git::cherry_pick(worktree, sha)
    }
    /// `HEAD` of a worktree.
    fn head(&self, worktree: &Path) -> Result<String> {
        crate::git::head(worktree)
    }
    /// Files touched by a commit (wave promotion's disjointness).
    fn files_of(&self, worktree: &Path, sha: &str) -> Result<Vec<String>> {
        crate::git::files_of(worktree, sha)
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
    /// Every §11 step; the headline and the report document id. `cfg`/`root` are the
    /// actor's live values, not the startup snapshot (R-106).
    async fn finalize(
        &self,
        store: &Store,
        study: &StudyRecord,
        reason: StopReason,
        cfg: &AutoresearchConfig,
        root: &Path,
        rt: &runtime::Runtime,
    ) -> Result<(Headline, Option<String>)> {
        crate::finalize::run_with(store, study, reason, cfg.clone(), root, rt.shutdown.clone())
            .await
    }
    /// The playbook of the last record for this repo.
    async fn seed_playbook(&self, repo: &str, rt: &runtime::Runtime) -> Result<Option<String>> {
        crate::record::seed_playbook(repo, rt).await
    }
    /// §11.3's report turn as a side call charged to the study session: the
    /// executor writes the prose around the numbers (the tables are inserted by
    /// the plugin afterwards, so a number can never change).
    async fn report_prose(
        &self,
        study: &StudyRecord,
        model: &str,
        system: &str,
        user: String,
    ) -> Result<String> {
        let session = study.session.ok_or_else(|| {
            Error::refused("the study has no bound session to charge the call to")
        })?;
        let agents = agents().ok_or_else(|| Error::refused("agent actor is not running"))?;
        agents
            .ask(runtime::SideCall {
                session,
                role: runtime::ModelRole::Named(model.to_owned()),
                system: system.to_owned(),
                user,
                budget: REPORT_CALL_BUDGET,
                kind: runtime::CallKind::Autoresearch,
            })
            .await
            .map(|r| r.text)
            .map_err(|e| Error::refused(e.to_string()))
    }
    /// Replace the report document's body (`doc` is a document id); `Ok(false)` when there is no documents actor.
    async fn replace_report(&self, doc: &str, markdown: &str) -> Result<bool> {
        let Some(docs) = ActorRef::<document::DocumentActor>::lookup(document::DOCUMENT_ACTOR)
            .ok()
            .flatten()
        else {
            return Ok(false);
        };
        docs.ask(document::ImportMarkdown {
            doc: document::DocId(doc.to_owned()),
            markdown: markdown.to_owned(),
            by: document::Principal::Helper,
            mode: document::ImportMode::Replace,
        })
        .await
        .map_err(|e| Error::refused(e.to_string()))?;
        Ok(true)
    }
    // ── the engine ──
    /// The session tree's spend in USD (128's ledger via `SessionCostQuery`):
    /// provider-returned money plus the estimates for calls without one. `Err`
    /// when the query fails or the session is unknown — the money stop must not fail
    /// open (R-14). Without an agent actor no model call can happen at all, so that is `0`.
    async fn session_cost(&self, session: Option<SessionId>) -> Result<f64> {
        let (Some(session), Some(agents)) = (session, agents()) else {
            return Ok(0.0);
        };
        let cost = agents
            .ask(agent::SessionCostQuery(session))
            .await
            .map_err(|e| Error::refused(format!("session cost: {e}")))?
            .ok_or_else(|| Error::refused(format!("session cost: {session} is unknown")))?;
        Ok(cost
            .total
            .returned
            .iter()
            .chain(cost.total.estimated_for_unavailable.iter())
            .filter(|m| m.currency == "USD")
            .map(|m| m.nanos as f64 / 1e9)
            .sum())
    }
    /// 159 Z (live proof): an unattended root session for the study before its schedule has
    /// fired — engine-side scoring (cases, pairwise) charges its baseline model calls to it.
    /// The schedule's own session replaces it at the first fire (`on_fired`); the baseline's
    /// spend still counts through the Bench channel (Σ `samples.spend_nanos`).
    async fn open_session(&self, slug: &str, worktree: &Path) -> Result<SessionId> {
        let agents = agents().ok_or_else(|| Error::refused("agent actor is not running"))?;
        let session = agents
            .ask(agent::NewSession {
                cwd: Some(worktree.to_path_buf()),
                unattended: true,
            })
            .await
            .map_err(|e| Error::refused(format!("baseline session: {e}")))?;
        if let Err(e) = agents
            .ask(agent::RenameSession {
                session,
                title: format!("{}{slug} · baseline", crate::SCHEDULE_PREFIX),
            })
            .await
        {
            tracing::warn!(%slug, "baseline session title not set: {e}");
        }
        Ok(session)
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
    // ── 159 §5.10 plugin-side calls (C implements; W2, W3, W5, W6, W7 consume) ──
    /// A registry tool called for a study (W5 `search_web`/`read_url`, W6 MCP reads).
    /// `ToolCall.session` is the study session, so cost and policy attach to the study;
    /// refused unless `name` is in `spec.calls` and its registry risk is read-only (W-d);
    /// output capped at 64 KiB and scrubbed of control characters; never shown to the
    /// session raw.
    async fn invoke_tool(
        &self,
        study: &StudyRecord,
        name: &str,
        args: serde_json::Value,
        timeout: Duration,
    ) -> Result<::tools::ToolOutput> {
        if !study.spec.calls.iter().any(|c| c == name) {
            return Err(Error::refused(format!(
                "`{name}` is not in the study's `calls` list"
            )));
        }
        let session = study.session.ok_or_else(|| {
            Error::refused("the study has no bound session to charge the call to")
        })?;
        let reg = ActorRef::<::tools::ToolRegistry>::lookup(::tools::REGISTRY_ACTOR)
            .ok()
            .flatten()
            .ok_or_else(|| Error::refused("the tool registry is not running"))?;
        let specs = reg
            .ask(::tools::ListTools(None))
            .await
            .map_err(|e| Error::refused(format!("list tools: {e}")))?;
        let spec = specs
            .iter()
            .find(|t| t.name == name)
            .ok_or_else(|| Error::refused(format!("`{name}` is not a registered tool")))?;
        if spec.risk != ::tools::Risk::ReadOnly {
            return Err(Error::refused(format!(
                "`{name}` is {:?}, not read-only (159 W-d)",
                spec.risk
            )));
        }
        let call = ::tools::ToolCall {
            id: format!("autoresearch-{}-{name}-{}", study.slug, now()),
            name: name.to_owned(),
            args,
            session,
            quiet: true,
        };
        let mut out = tokio::time::timeout(timeout, reg.ask(::tools::Invoke(call)))
            .await
            .map_err(|_| Error::refused(format!("`{name}` timed out after {timeout:?}")))?
            .map_err(|e| Error::refused(format!("`{name}`: {e}")))?;
        out.content = scrub(&out.content, INVOKE_CAP);
        out.files.clear();
        Ok(out)
    }
    /// One SideCall charged to the study session (`CallKind::Autoresearch`): the case runner,
    /// the judge, literature extraction. `review::call` shares the body.
    async fn side_call(
        &self,
        study: &StudyRecord,
        role: runtime::ModelRole,
        system: &str,
        user: String,
        budget: Duration,
    ) -> Result<runtime::SideReply> {
        crate::review::side_call(study, role, system, user, budget)
            .await
            .map_err(Error::refused)
    }
    /// A custody command the plugin runs (hooks, deploy, measure, rollback): `runner::command`
    /// with the study environment plus `extra_env`, scanned with the v2 grammar. `what` names
    /// the log (`runs/plugin/<what>-0.log`).
    async fn plugin_command(
        &self,
        study: &StudyRecord,
        what: &str,
        command: &str,
        timeout: Duration,
        extra_env: &[(String, String)],
    ) -> Result<crate::runner::CommandRun> {
        let worktree = worktree_of(study)?;
        let study_dir = worktree
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| worktree.clone());
        let evaluator = study_dir.join("evaluator").join(study.segment.to_string());
        let runs = study_dir.join("runs").join("plugin");
        let req = SampleRequest {
            slug: &study.slug,
            n: 0,
            k: 0,
            tree_sha: study.incumbent_sha.as_deref().unwrap_or(""),
            role: SampleRole::Candidate,
            worktree: &worktree,
            evaluator: &evaluator,
            runs_dir: &runs,
        };
        let cancel = CancellationToken::new();
        crate::runner::command_run(what, command, timeout, req, &cancel, extra_env).await
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

/// 159 §5.10: the most of a plugin-side tool output the plugin keeps.
const INVOKE_CAP: usize = 64 * 1024;

/// Drop control characters (except `\n` and `\t`) and cut at `cap` bytes on a char boundary.
fn scrub(s: &str, cap: usize) -> String {
    let mut out: String = s
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect();
    if out.len() > cap {
        let mut cut = cap;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
    }
    out
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

pub(crate) fn worktree_of(study: &StudyRecord) -> Result<PathBuf> {
    study
        .worktree
        .as_deref()
        .map(PathBuf::from)
        .ok_or_else(|| Error::refused(format!("study `{}` has no worktree yet", study.slug)))
}

pub(crate) fn incumbent_of(study: &StudyRecord) -> Result<String> {
    study
        .incumbent_sha
        .clone()
        .ok_or_else(|| Error::refused(format!("study `{}` has no incumbent yet", study.slug)))
}

/// 159 §18.3 `parent_of`: what the next experiment is cut from, by `axes.search`. The ratchet
/// is the incumbent (140); the front (W1) and the population (W4) pick their own parent.
pub(crate) async fn parent_of(env: &Env, study: &StudyRecord, slot: Option<u8>) -> Result<String> {
    match study.spec.axes.search {
        Search::Ratchet => incumbent_of(study),
        Search::Front => crate::pareto::parent_of(env, study, slot).await, // 159:W1
        Search::Population => crate::population::parent_of(env, study, slot).await, // 159:W4
    }
}

pub(crate) fn gate_cfg(cfg: &AutoresearchConfig, spec: &StudySpec) -> Result<GateCfg> {
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
            .map(|g| Effect::parse(&g.max_regression).map(|e| (g.name.clone(), e, g.direction)))
            .collect::<std::result::Result<_, _>>()
            .map_err(Error::refused)?,
    })
}

/// 159 §18.3 `kind_rule`: the decision rule of the §5.6 table. `z_gate` is `Backend::gate_step`
/// (140, unchanged); every other rule lives in its wave's module and gets the samples too.
/// A combination with no row is refused (`validate_spec` already refuses it at draft time).
pub(crate) async fn kind_rule(
    env: &Env,
    study: &StudyRecord,
    cfg: &GateCfg,
    input: &GateInput,
    candidate: &[Sample],
    reference: &[Sample],
) -> Result<GateStep> {
    let rule = study.spec.rule().ok_or_else(|| {
        Error::refused(format!(
            "no decision rule for objectives `{:?}` with scoring `{:?}`",
            study.spec.axes.objectives, study.spec.axes.scoring
        ))
    })?;
    match rule {
        GateRule::ZGate => Ok(env.backend.gate_step(cfg, input)),
        GateRule::Dominance => {
            crate::pareto::step(env, study, cfg, input, candidate, reference).await
        } // 159:W1
        GateRule::Paired if study.spec.axes.scoring == Scoring::Suite => {
            crate::suite::step(env, study, cfg, input, candidate, reference).await // 159:W7
        }
        GateRule::Paired => crate::cases::step(env, study, cfg, input, candidate, reference).await, // 159:W2
        GateRule::WinBound => {
            crate::judge::step(env, study, cfg, input, candidate, reference).await
        } // 159:W3
        GateRule::Msprt => crate::world::step(env, study, cfg, input, candidate, reference).await, // 159:W6
    }
}

pub(crate) fn mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

pub(crate) fn clean_values(samples: &[Sample]) -> Vec<f64> {
    samples
        .iter()
        .filter(|s| !s.noisy && !s.lost)
        .filter_map(|s| s.value)
        .collect()
}

/// Clean values of the *benchmark* only: holdout rows never enter x̄_i, σ, the
/// guards, a verdict line or anything the study session reads (140 §5.4, R-3).
pub(crate) fn bench_values(samples: &[Sample]) -> Vec<f64> {
    samples
        .iter()
        .filter(|s| !s.noisy && !s.lost && s.role != Some(SampleRole::Holdout))
        .filter_map(|s| s.value)
        .collect()
}

/// The incumbent's own benchmark samples in the current segment (its baseline /
/// candidate samples since it became incumbent plus re-measures; never holdout
/// rows, never rows from before the segment opened — R-3, R-28).
pub(crate) async fn incumbent_samples(
    env: &Env,
    study: &StudyRecord,
    incumbent: &str,
) -> Result<Vec<Sample>> {
    let opened = env
        .store
        .get_segment(&study.slug, study.segment)
        .await?
        .map(|s| s.opened_at)
        .unwrap_or_default();
    Ok(env
        .store
        .samples_of_tree(&study.slug, incumbent)
        .await?
        .into_iter()
        .filter(|s| s.role != Some(SampleRole::Holdout) && s.at >= opened)
        .collect())
}

/// Pooled σ of the current segment: every tree measured more than once since the
/// segment opened (140 §5.2 step 1). Holdout rows and the repeats of `checks_failed`
/// candidates (§5.1 "never counts toward σ") are left out.
pub(crate) async fn segment_sigma(env: &Env, study: &StudyRecord) -> Result<f64> {
    let opened = env
        .store
        .get_segment(&study.slug, study.segment)
        .await?
        .map(|s| s.opened_at)
        .unwrap_or_default();
    let checks_failed: std::collections::HashSet<u32> = env
        .store
        .list_experiments(&study.slug, None, u32::MAX)
        .await?
        .iter()
        .filter(|e| e.state == ExperimentState::ChecksFailed)
        .map(|e| e.n)
        .collect();
    let samples = env.store.list_samples(&study.slug, None).await?;
    let mut trees: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for s in samples.iter().filter(|s| {
        s.at >= opened
            && !s.noisy
            && !s.lost
            && s.role != Some(SampleRole::Holdout)
            && !(s.role == Some(SampleRole::Candidate) && checks_failed.contains(&s.n))
    }) {
        if let Some(v) = s.value {
            trees.entry(s.tree_sha.clone()).or_default().push(v);
        }
    }
    let trees: Vec<Vec<f64>> = trees.into_values().filter(|t| t.len() > 1).collect();
    Ok(env.backend.pooled_sigma(&trees))
}

fn is_closed(e: &ExperimentRecord) -> bool {
    e.state.is_closed()
}

/// Closed experiments, kept experiments.
async fn counts(store: &Store, slug: &str) -> Result<(u32, u32)> {
    let all = store.list_experiments(slug, None, u32::MAX).await?;
    let closed = all.iter().filter(|e| is_closed(e)).count() as u32;
    let kept = all
        .iter()
        .filter(|e| e.state == ExperimentState::Kept)
        .count() as u32;
    Ok((closed, kept))
}

/// The highest closed experiment number (the `after_n` scale the stop rules compare `n` against, R-64).
async fn last_closed_n(store: &Store, slug: &str) -> Result<u32> {
    Ok(store
        .list_experiments(slug, None, u32::MAX)
        .await?
        .iter()
        .filter(|e| is_closed(e))
        .map(|e| e.n)
        .max()
        .unwrap_or(0))
}

/// Study spend (140 §8.4, R-14): Σ `cost_nanos` of the closed experiments plus what
/// the live session tree has spent since its last attribution mark — the `begin` of
/// the open experiment or the `closed` mark of the last one (both store the tree's
/// total at that moment). A re-created session reads below its mark and adds
/// nothing. `Err` when the live tree cannot be read: the money stop never fails open.
pub(crate) async fn study_spend(env: &Env, study: &StudyRecord) -> Result<f64> {
    let all = env
        .store
        .list_experiments(&study.slug, None, u32::MAX)
        .await?;
    let closed: f64 = all
        .iter()
        .filter(|e| is_closed(e))
        .map(|e| e.cost_nanos as f64 / 1e9)
        .sum();
    let live = env.backend.session_cost(study.session).await?;
    let mark = env
        .store
        .list_events(&study.slug, None, 200)
        .await?
        .iter()
        .filter(|e| e.kind == "begin" || e.kind == "closed")
        .find_map(|e| e.payload.get("spend_usd").and_then(|v| v.as_f64()))
        .unwrap_or(0.0);
    // 159 §5.8: study spend = Session + Bench (Σ `samples.spend_nanos`; 0 for a 140 study).
    let bench = env.store.bench_spend(&study.slug).await? as f64 / 1e9;
    Ok(closed + (live - mark).max(0.0) + bench)
}

/// 159 §5.8: the spend per channel for `BudgetUsed.by_channel` — `session` is the study's
/// spend minus the bench channel, `bench` is Σ `samples.spend_nanos`. `world` stays unset
/// (always 0 under D4).
async fn spend_channels(env: &Env, slug: &str, spend: f64) -> BTreeMap<String, f64> {
    let bench = env.store.bench_spend(slug).await.unwrap_or(0) as f64 / 1e9;
    BTreeMap::from([
        (
            SpendChannel::Session.key().to_owned(),
            (spend - bench).max(0.0),
        ),
        (SpendChannel::Bench.key().to_owned(), bench),
    ])
}

/// [`study_spend`] for displays that cannot refuse: the closed sum on error.
async fn study_spend_or_closed(env: &Env, study: &StudyRecord) -> f64 {
    match study_spend(env, study).await {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(slug = %study.slug, "live spend unavailable: {e}");
            closed_spend(env, &study.slug).await
        }
    }
}

/// Σ `cost_nanos` of the closed experiments, in USD (no agent ask).
async fn closed_spend(env: &Env, slug: &str) -> f64 {
    env.store
        .list_experiments(slug, None, u32::MAX)
        .await
        .unwrap_or_default()
        .iter()
        .filter(|e| is_closed(e))
        .map(|e| e.cost_nanos as f64 / 1e9)
        .sum()
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

/// A user `run_once` newer than the last `begin` (R-51): the one fire a paused study accepts.
async fn run_once_pending(store: &Store, slug: &str) -> Result<bool> {
    let once = store.list_events(slug, Some("run_once"), 1).await?;
    let Some(o) = once.first() else {
        return Ok(false);
    };
    let begun = store.list_events(slug, Some("begin"), 1).await?;
    Ok(begun.first().is_none_or(|b| b.id < o.id))
}

/// Fold a fresh incumbent sample into x̄_i and σ and run the §5.2 step-4 drift
/// check on it (every incumbent sample, scheduled or confirm-phase — R-20). The
/// study row is patched column-wise (R-4). Returns the brief's drift note.
async fn absorb_incumbent_sample(
    env: &Env,
    study: &mut StudyRecord,
    incumbent: &str,
    n: u32,
    s: &Sample,
) -> Result<Option<String>> {
    let mut note = None;
    if let (Some(v), Some(mean_i), Some(sigma)) = (s.value, study.incumbent_mean, study.sigma)
        && !s.noisy
        && sigma > 0.0
        && env.backend.drifted(sigma, mean_i, v)
    {
        env.store
            .append_event(
                &study.slug,
                "drift",
                &serde_json::json!({ "n": n, "value": v, "mean": mean_i, "sigma": sigma }),
            )
            .await?;
        note = Some(format!(
            "an incumbent re-measure landed at {} {} vs mean {} (drift); the next confirm takes more samples.",
            fmt_num(v),
            study.spec.metric.unit,
            fmt_num(mean_i)
        ));
    }
    let inc = bench_values(&incumbent_samples(env, study, incumbent).await?);
    let mean_i = if inc.is_empty() {
        study.incumbent_mean
    } else {
        Some(mean(&inc))
    };
    let sigma = segment_sigma(env, study).await?;
    patch(
        env,
        study,
        StudyPatch {
            incumbent_mean: Some(mean_i),
            sigma: Some(Some(sigma)),
            ..StudyPatch::default()
        },
    )
    .await?;
    Ok(note)
}

/// R-50: `experiment_log` records `keep_pending {n}` before `git commit` and the
/// ledger side (`record_keep`) after it. A crash in between leaves the experiment
/// `measured` while HEAD is a commit whose parent is the incumbent — adopt it as
/// the keep instead of letting the drift repair reset it off the branch.
async fn adopt_pending_keep(
    env: &Env,
    study: &mut StudyRecord,
    worktree: &Path,
) -> Result<Option<String>> {
    let pending = env
        .store
        .list_events(&study.slug, Some("keep_pending"), 1)
        .await?;
    let Some(p) = pending.first() else {
        return Ok(None);
    };
    let kept = env.store.list_events(&study.slug, Some("keep"), 1).await?;
    if kept.first().is_some_and(|k| k.id > p.id) {
        return Ok(None);
    }
    let Some(n) = p
        .payload
        .get("n")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32)
    else {
        return Ok(None);
    };
    let Some(mut exp) = env.store.get_experiment(&study.slug, n).await? else {
        return Ok(None);
    };
    let incumbent = incumbent_of(study)?;
    if exp.state != ExperimentState::Measured || exp.parent_sha != incumbent {
        return Ok(None);
    }
    let head = env.backend.head(worktree)?;
    if head == incumbent || env.backend.commit_parent(worktree, &head)? != incumbent {
        return Ok(None);
    }
    exp.state = ExperimentState::Kept;
    exp.commit_sha = Some(head.clone());
    exp.finished_at = Some(now());
    exp.line = format!("{} (ledger recovered after a crash)", exp.line);
    let mean_c = exp.metric;
    env.store
        .record_keep(
            &exp,
            &head,
            &StudyPatch::incumbent(&head, mean_c.or(study.incumbent_mean), study.sigma),
        )
        .await?;
    study.incumbent_sha = Some(head.clone());
    study.incumbent_mean = mean_c.or(study.incumbent_mean);
    env.store
        .append_event(
            &study.slug,
            "incident",
            &serde_json::json!({ "n": n, "what": format!("keep #{n} recovered: commit {} was on the branch, the ledger was not written", &head[..head.len().min(10)]) }),
        )
        .await?;
    Ok(Some(format!(
        "experiment {n} was kept (commit {}) but the engine stopped before the ledger recorded it; the ledger is now consistent.",
        &head[..head.len().min(10)]
    )))
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
        let s = scored_sample(
            env,
            &spec,
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
            None,
        )
        .await?;
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
    let mut sigma = env
        .backend
        .pooled_sigma(&[base_holdout.clone(), incumbent_holdout.to_vec()]);
    if sigma <= 0.0 {
        // Single-sample holdouts on both sides (R-43): judge against the search σ, never σ = 0.
        sigma = study.sigma.unwrap_or(0.0);
        if sigma <= 0.0 {
            return Ok(());
        }
    }
    let direction = study.spec.metric.direction;
    let holdout_delta = gate::normalise(direction, mean(incumbent_holdout), mean(&base_holdout));
    let search_delta = match (
        study.incumbent_mean,
        env.store.base_mean(&study.slug).await?,
    ) {
        (Some(inc), Some(b)) => Some(gate::normalise(direction, inc, b)),
        _ => None,
    };
    if holdout_delta < -2.0 * sigma && search_delta.is_some_and(|d| d > 0.0) {
        env.store
            .append_event(
                &study.slug,
                "overfit",
                &serde_json::json!({
                    "n": n,
                    // `after_n` is the scale the stop rules read (R-44): overfit after keep `n`.
                    "after_n": n,
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
        PauseReason::LaunchApproval { n } => format!("launch #{n} waits for your press"),
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
        PauseReason::LaunchApproval { .. } => "launch_approval",
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
pub fn info(root: &Path, study: &StudyRecord) -> StudyInfo {
    info_from(
        study,
        root,
        None,
        0,
        0.0,
        stop::hours_running(study, chrono::Utc::now()),
    )
}

fn info_from(
    study: &StudyRecord,
    root: &Path,
    base_mean: Option<f64>,
    closed: u32,
    spend: f64,
    hours: f64,
) -> StudyInfo {
    // Revision 2: the raw relative change (negative = the value went down), like every
    // other Δ; the UI words/colours it from the metric's direction.
    let delta_pct = match (study.incumbent_mean, base_mean) {
        (Some(i), Some(b)) => pct(i - b, b),
        _ => None,
    };
    let (current, phase) =
        phase_of(root, &study.slug).map_or((None, None), |(n, p)| (Some(n), Some(p)));
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
            by_channel: BTreeMap::new(),
        },
        budget: study.spec.budget.clone(),
        session: study.session,
        origin: study.origin,
        current,
        phase,
        started_at: study.started_at.clone(),
        finished_at: study.finished_at.clone(),
        stop_reason: study.stop_reason.clone(),
        report_doc: study.report_doc.clone(),
        kind: study.spec.kind().to_owned(),
        created_by: study.created_by.clone(),
    }
}

/// `StudyInfo` with counts, base and spend filled from the ledger.
pub async fn info_full(env: &Env, study: &StudyRecord) -> Result<StudyInfo> {
    let (closed, kept) = counts(&env.store, &study.slug).await?;
    let base_mean = env.store.base_mean(&study.slug).await?;
    // A finished study has no live tree to ask (R-115: no agent ask per `autoresearch.list`
    // row), but its closed experiments' cost is a ledger sum — $0 was wrong (140-P live).
    let spend = if is_terminal(&study.state) {
        closed_spend(env, &study.slug).await
    } else {
        study_spend_or_closed(env, study).await
    };
    let hours = hours_live(env, study).await;
    let mut i = info_from(study, &env.root, base_mean, closed, spend, hours);
    i.incumbent.kept = kept;
    i.budget_used.by_channel = spend_channels(env, &study.slug, spend).await;
    Ok(i)
}

/// Hours since Start excluding paused time, the open pause included (R-46: `stop::hours_running_live`).
async fn hours_live(env: &Env, study: &StudyRecord) -> f64 {
    match stop::hours_running_live(&env.store, study).await {
        Ok(h) => h,
        Err(e) => {
            tracing::debug!(slug = %study.slug, "live hours unavailable: {e}");
            stop::hours_running(study, chrono::Utc::now())
        }
    }
}

/// Patch `study`'s state column (never the whole row, R-4) and publish `StudyChanged`.
async fn set_state(
    env: &Env,
    study: &mut StudyRecord,
    state: StudyState,
    reason: Option<String>,
) -> Result<()> {
    set_state_if(env, study, &[], state, reason).await.map(drop)
}

/// [`set_state`] only while the row is still in one of `from` (state tags);
/// `Ok(false)` = another handler moved it first and nothing was written.
async fn set_state_if(
    env: &Env,
    study: &mut StudyRecord,
    from: &[&str],
    state: StudyState,
    reason: Option<String>,
) -> Result<bool> {
    let p = StudyPatch::state(state.clone());
    if !env.store.patch_study_if(&study.slug, from, &p).await? {
        return Ok(false);
    }
    p.apply_to(study);
    // F4E: a study that is over releases its session's seat (write root, cwd).
    if is_terminal(&state)
        && let Some(s) = study.session
    {
        crate::binding::unseat(s);
    }
    env.publish(StudyEvent::StudyChanged {
        slug: study.slug.clone(),
        state,
        reason,
    })
    .await;
    Ok(true)
}

/// Patch a few columns and keep the handler's copy in step (R-4).
async fn patch(env: &Env, study: &mut StudyRecord, p: StudyPatch) -> Result<()> {
    env.store.patch_study(&study.slug, &p).await?;
    p.apply_to(study);
    Ok(())
}

pub(crate) async fn progress(
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

/// §5.5 / §10 step 3: in-process bench queue. Scored samples of concurrent
/// wave slots (and of two studies in one engine) take it one at a time when
/// `bench.exclusive`; W1's machine slot is still taken inside. Smoke runs,
/// checks and edits never queue here.
static BENCH_QUEUE: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(Default::default);

/// One scored sample through the bench queue (when exclusive) and the backend.
pub(crate) async fn bench_sample(
    env: &Env,
    spec: &StudySpec,
    req: SampleRequest<'_>,
    cancel: &CancellationToken,
) -> Result<Sample> {
    let _turn = if spec.bench.exclusive {
        Some(BENCH_QUEUE.lock().await)
    } else {
        None
    };
    env.backend.sample(spec, &env.cfg.bench, req, cancel).await
}

/// 159 §18.3 `kind_sample`: how a tree becomes numbers, by `axes.scoring`. `bench` is
/// [`bench_sample`] (140, no ledger read); the engine-side kinds look the study up and score
/// in their own module.
pub(crate) async fn kind_sample(
    env: &Env,
    spec: &StudySpec,
    req: SampleRequest<'_>,
    cancel: &CancellationToken,
) -> Result<Sample> {
    if spec.axes.scoring == Scoring::Bench {
        return bench_sample(env, spec, req, cancel).await;
    }
    let study = study_by_slug(&env.store, req.slug).await?;
    match spec.axes.scoring {
        Scoring::Bench => unreachable!("handled above"),
        Scoring::Cases => crate::cases::sample(env, &study, req, cancel).await, // 159:W2
        Scoring::Pairwise => crate::judge::sample(env, &study, req, cancel).await, // 159:W3
        Scoring::World => crate::world::sample(env, &study, req, cancel).await, // 159:W6
        Scoring::Suite => crate::suite::sample(env, &study, req, cancel).await, // 159:W7
    }
}

/// What the live-tail ticks of a sample publish (`ExperimentProgress` every [`SAMPLE_TICK`]).
#[derive(Clone, Copy)]
pub(crate) struct Ticks<'a> {
    started_at: &'a str,
    sample: u8,
    of: u8,
}

/// One scored sample with its ledger row: a pending row is inserted before the
/// benchmark spawns (an engine crash leaves it to be marked `lost`, §8.5 / R-34)
/// and filled when the sample returns; a cancelled or failed spawn marks it lost
/// itself. With `ticks`, progress is re-published every few seconds so the Study
/// view's tail stays live (R-87).
pub(crate) async fn scored_sample(
    env: &Env,
    spec: &StudySpec,
    req: SampleRequest<'_>,
    cancel: &CancellationToken,
    ticks: Option<Ticks<'_>>,
) -> Result<Sample> {
    let pending = Sample {
        n: req.n,
        k: req.k,
        tree_sha: req.tree_sha.to_owned(),
        role: Some(req.role),
        at: now(),
        ..Sample::default()
    };
    let id = env.store.insert_sample(req.slug, &pending).await?;
    let (slug, n) = (req.slug.to_owned(), req.n);
    // `runner::sample` writes `runs/<n>/sample-<k>[-<role>].log` (candidates bare); its tail is
    // the Study view's live tail.
    let log = req.runs_dir.join(match req.role {
        SampleRole::Candidate => format!("sample-{}.log", req.k),
        SampleRole::Incumbent => format!("sample-{}-incumbent.log", req.k),
        SampleRole::Baseline => format!("sample-{}-baseline.log", req.k),
        SampleRole::Holdout => format!("sample-{}-holdout.log", req.k),
        SampleRole::Final => format!("sample-{}-final.log", req.k),
    });
    let fut = kind_sample(env, spec, req, cancel);
    tokio::pin!(fut);
    let mut tick = tokio::time::interval(SAMPLE_TICK);
    tick.tick().await;
    let mut last_tail = String::new();
    let r = loop {
        tokio::select! {
            r = &mut fut => break r,
            _ = tick.tick() => {
                // R-87: push only when the tail moved (the UI refetches the detail on progress).
                if let Some(t) = ticks {
                    let tail = env.backend.tail(&log);
                    if tail != last_tail {
                        last_tail = tail;
                        progress(env, &slug, n, t.started_at, Phase::Sampling, t.sample, t.of).await;
                    }
                }
            }
        }
    };
    match r {
        Ok(s) => {
            env.store.update_sample(id, &s).await?;
            Ok(s)
        }
        Err(e) => {
            let lost = Sample {
                lost: true,
                ..pending
            };
            env.store.update_sample(id, &lost).await?;
            Err(e)
        }
    }
}

/// R-2: a confirm-phase incumbent sample measures the *incumbent tree*. The
/// candidate's edits (tracked and untracked) are saved as `runs/<n>/stash-<k>.patch`,
/// the worktree is reset to the incumbent, the sample runs in the same worktree
/// (its build caches stay warm, unlike a fresh detached worktree), then the patch
/// is re-applied. The sample is folded into x̄_i / σ with the drift check (R-20).
#[allow(clippy::too_many_arguments)]
async fn incumbent_sample_in_place(
    env: &Env,
    study: &mut StudyRecord,
    paths: &Paths<'_>,
    incumbent: &str,
    k: u32,
    cancel: &CancellationToken,
    ticks: Ticks<'_>,
) -> Result<Sample> {
    let stash = paths.runs.join(format!("stash-{k}.patch"));
    env.backend.discard(paths.worktree, incumbent, &stash)?;
    let r = scored_sample(
        env,
        &study.spec,
        paths.req(k, incumbent, SampleRole::Incumbent),
        cancel,
        Some(ticks),
    )
    .await;
    env.backend
        .apply_patch(paths.worktree, &stash)
        .map_err(|e| {
            Error::refused(format!(
                "the candidate could not be re-applied after the incumbent sample ({e}); it is saved at {}",
                stash.display()
            ))
        })?;
    let _ = std::fs::remove_file(&stash);
    let s = r?;
    absorb_incumbent_sample(env, study, incumbent, paths.n, &s).await?;
    Ok(s)
}

/// `Deliver` to the chat the draft card lived in, regardless of `quiet` (140 §13.1); off-mailbox.
/// 151 R1 (D7): when no user channel renders the origin — the desktop's setup chat is not a
/// Slack/Signal/Telegram session — the card goes into the origin session itself as an
/// `<autoresearch slug event>` input through `agent::SendInput`, the path reminders and desk
/// notices already use; a channel-bound session keeps the channel path alone (never twice).
fn deliver(origin: Option<SessionId>, slug: &str, event: &'static str, text: String) {
    let origin_label = format!("autoresearch {slug}");
    let slug = slug.to_owned();
    tokio::spawn(async move {
        let via_channel = match ActorRef::<agent::UserChannels>::lookup(agent::USER_ACTOR) {
            Ok(Some(user)) => {
                let m = agent::Outbound {
                    sessions: origin.into_iter().collect(),
                    origin: origin_label.clone(),
                    text: text.clone(),
                    files: vec![],
                };
                match user.ask(agent::Deliver { m, via: None }).await {
                    Ok(_) => true,
                    Err(e) => {
                        tracing::debug!(%origin_label, "no channel took the card: {e}");
                        false
                    }
                }
            }
            _ => false,
        };
        if via_channel {
            return;
        }
        let (Some(origin), Some(agents)) = (origin, agents()) else {
            tracing::warn!(%origin_label, "card not delivered: no channel and no origin session");
            return;
        };
        let r = agents
            .ask(agent::SendInput {
                session: Some(origin),
                text: wrapped(&slug, event, &text),
                files: vec![],
                delivery: agent::Delivery::default(),
                id: None,
            })
            .await;
        if let Err(e) = r {
            tracing::warn!(%origin_label, "card not delivered to the origin session: {e}");
        }
    });
}

/// The `<autoresearch slug="…" event="…">` wrapper a desktop/TUI client renders as the study's
/// card (`injected.ts`, `tui::model::injected`); `text` is the card markdown.
pub(crate) fn wrapped(slug: &str, event: &str, text: &str) -> String {
    let attr = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    format!(
        "<autoresearch slug=\"{}\" event=\"{}\">\n{text}\n</autoresearch>",
        attr(slug),
        attr(event)
    )
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
        "**Autoresearch study `{}` started** — optimising `{}` ({}) in {} on `{}`; schedule `{}`.\n\n{}\n\nOpen study: [/autoresearch/{slug}](/autoresearch/{slug})",
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
        slug = study.slug
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
    // `report_doc` is a document id (R-98: `None` when finalize had no documents actor).
    let report = study
        .report_doc
        .as_deref()
        .map(|d| {
            format!(
                "\n\nReport: [/notes/{d}](/notes/{d}) · Study view: [/autoresearch/{slug}](/autoresearch/{slug})",
                slug = study.slug
            )
        })
        .unwrap_or_else(|| {
            format!(
                "\n\nStudy view: [/autoresearch/{slug}](/autoresearch/{slug})",
                slug = study.slug
            )
        });
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

fn validate(spec: &StudySpec, cfg: &AutoresearchConfig) -> Result<()> {
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
    // 159 W2 H6: a `file` artefact with no repo gets a scratch repo in `prepare_steps`.
    if !scratch_repo(spec) && (spec.repo.trim().is_empty() || !repo.is_dir()) {
        return Err(Error::refused(format!(
            "repo `{}` is not a directory",
            spec.repo
        )));
    }
    if spec.metric.name.trim().is_empty() {
        return Err(Error::refused("metric.name is empty"));
    }
    // 159 W2 H1 / R-SEAMS-11: engine-side scoring (cases, pairwise, world, suite) runs no bench command.
    if spec.axes.scoring == Scoring::Bench && spec.bench.command.trim().is_empty() {
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
    // Windows (2026-10-09, found by the 134/149 session): every bench/check/holdout command
    // runs under `/bin/bash -lc`, which a stock Windows box does not have — refuse honestly
    // before anything is written. `cfg!` keeps the rest compiled (no Windows-only dead code).
    if cfg!(windows) {
        return Err(Error::refused(crate::WINDOWS_UNSUPPORTED));
    }
    let env = a.env();
    // 159 §5.2: `metric` mirrors the headline objective before `validate` reads it (a pareto
    // README spec carries `objectives` only — R-W6-3, every kind creatable).
    let mut spec = m.spec;
    spec.normalise();
    // 159 W7 H2: the mechanism repo exists before the draft is checked (`validate` reads
    // `repo.is_dir()`); `git init` is a process spawn, so off the mailbox thread (R-100).
    if spec.axes.artefact == Artefact::Mechanism {
        let root = env.root.clone();
        tokio::task::spawn_blocking(move || crate::mechanism::ensure(&root))
            .await
            .map_err(|e| Error::refused(format!("mechanism repo: {e}")))??;
    }
    validate(&spec, &env.cfg)?;
    // 140-P live: `spawn_subagent` is gated by `agent.communication` (50 §2.1); without it
    // every wave is abandoned ("spawn_subagent is not available in this session").
    if spec.parallel > 1 && !env.agent.communication {
        return Err(Error::refused(
            "parallel > 1 needs `agent.communication: true` in config.yaml (spawn_subagent is off — a \
             ceiling no session can override); start the study with parallel: 1 or switch it on",
        ));
    }
    // `git rev-parse` is a process spawn: never on the mailbox thread (R-100).
    let (backend, repo) = (env.backend.clone(), PathBuf::from(&spec.repo));
    let is_git = scratch_repo(&spec)
        || tokio::task::spawn_blocking(move || backend.is_git_repo(&repo))
            .await
            .unwrap_or(false);
    if !is_git {
        return Err(Error::refused(format!(
            "repo `{}` is not a git repository (a pure jj repo needs git colocation)",
            spec.repo
        )));
    }
    if let Some(existing) = env.store.get_study(&spec.slug).await? {
        return Err(Error::refused(format!(
            "slug `{}` is taken by a study that is {}",
            existing.slug,
            state_word(&existing.state)
        )));
    }
    let mut study = StudyRecord::new(spec, Some(m.origin));
    study.state = StudyState::Preparing;
    study.created_by = Some("user".into());
    study.mechanism = crate::mechanism::active(&env.root).map(|(_, sha)| sha); // 159:W7 pin
    study.schedule = Some(schedule_file::name(&study.slug));
    env.store.create_study(&study).await?;
    env.publish(StudyEvent::StudyChanged {
        slug: study.slug.clone(),
        state: StudyState::Preparing,
        reason: None,
    })
    .await;
    let info = info_from(&study, &env.root, None, 0, 0.0, 0.0);
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
                "failed",
                format!(
                    "**Autoresearch study `{}` did not start** — {reason}",
                    study.slug
                ),
            );
        }
    }
}

/// 159 W2 H6: a `file` artefact with no repo is edited in a plugin-made scratch repo
/// (`<root>/<slug>/artefact`, seeded in `prepare_steps`).
fn scratch_repo(spec: &StudySpec) -> bool {
    spec.axes.artefact == Artefact::File && spec.repo.trim().is_empty()
}

/// `git init` `<root>/<slug>/artefact`, copy the seed (`cases.artefact` under `data_dir`,
/// keeping its relative path), one commit; the study's `repo`/`spec.repo` point at it.
async fn seed_scratch_repo(env: &Env, study: &mut StudyRecord) -> Result<PathBuf> {
    let dir = env.study_dir(&study.slug).join("artefact");
    let rel = study
        .spec
        .cases
        .as_ref()
        .map(|c| c.artefact.trim())
        .filter(|a| !a.is_empty())
        .ok_or_else(|| Error::refused("a scratch repo needs `cases.artefact` (the seed file)"))?;
    let src = env.data_dir.join(rel);
    let dst = dir.join(rel);
    std::fs::create_dir_all(dst.parent().unwrap_or(&dir))?;
    std::fs::copy(&src, &dst)
        .map_err(|e| Error::refused(format!("seed `{}`: {e}", src.display())))?;
    crate::git::plumbing(&dir, &["init", "-q"])?;
    crate::git::plumbing(&dir, &["add", "-A"])?;
    crate::git::plumbing(&dir, &["commit", "-q", "-m", "seed"])?;
    let mut spec = study.spec.clone();
    spec.repo = dir.display().to_string();
    patch(
        env,
        study,
        StudyPatch {
            repo: Some(spec.repo.clone()),
            spec: Some(Box::new(spec)),
            ..StudyPatch::default()
        },
    )
    .await?;
    Ok(dir)
}

async fn prepare_steps(env: &Env, study: &mut StudyRecord, start: &StartWhen) -> Result<()> {
    let slug = study.slug.clone();
    let repo = if scratch_repo(&study.spec) {
        Box::pin(seed_scratch_repo(env, study)).await?
    } else {
        PathBuf::from(&study.spec.repo)
    };
    let spec = study.spec.clone();
    let cancel = cancel_token(&env.root, &slug);

    let base_sha = env
        .backend
        .resolve_base(&repo, &spec.base, spec.include_dirty)?;
    let worktree = env.study_dir(&slug).join("worktree");
    std::fs::create_dir_all(env.study_dir(&slug))?;
    let head = env
        .backend
        .worktree_add(&repo, &spec, &worktree, &base_sha)?;
    // The study's base is the worktree's HEAD after `worktree_add`: with `include_dirty`
    // that is the `base snapshot` commit — the tree the baseline measures, the overfit
    // check's base holdout and finalize's "base vs final" / cherry-pick target
    // (140-P live: the resolved HEAD left the dirty changes out of all three).
    patch(
        env,
        study,
        StudyPatch {
            base_sha: Some(head.clone()),
            branch: Some(format!("autoresearch/{slug}")),
            worktree: Some(worktree.display().to_string()),
            incumbent_sha: Some(head.clone()),
            ..StudyPatch::default()
        },
    )
    .await?;

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

    // Custody is taken from the study worktree — the tree `custody::verify` hashes later (R-7).
    let custody = env.backend.custody_take(&spec, &worktree, &evaluator)?;
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
        patch(
            env,
            study,
            StudyPatch {
                playbook: Some(p),
                ..StudyPatch::default()
            },
        )
        .await?;
    }

    // 159 Z: cases and pairwise scoring call the target / judge inside the baseline sample,
    // and every side call is charged to `study.session` — bound by the schedule's first fire,
    // which comes after this. Open the study's session now (live proof: toy-prompt failed
    // `the study has no bound session to charge the call to` at baseline sample 1).
    if matches!(spec.axes.scoring, Scoring::Cases | Scoring::Pairwise) && study.session.is_none() {
        let session = env.backend.open_session(&slug, &worktree).await?;
        patch(
            env,
            study,
            StudyPatch {
                session: Some(Some(session)),
                ..StudyPatch::default()
            },
        )
        .await?;
    }

    let values = baseline(env, study, &head, 0, SampleRole::Baseline, &cancel).await?;
    let (m, sigma) = (
        mean(&values),
        env.backend.pooled_sigma(std::slice::from_ref(&values)),
    );
    let cv = if m != 0.0 { sigma / m.abs() } else { 0.0 };
    // 159 W6 H8: a world study's baseline σ is production's day-to-day variation; the mSPRT
    // uses the arms' reported variance.
    if cv > env.cfg.noise.max_cv && spec.axes.scoring != Scoring::World {
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
    patch(
        env,
        study,
        StudyPatch {
            incumbent_mean: Some(Some(m)),
            sigma: Some(Some(sigma)),
            ..StudyPatch::default()
        },
    )
    .await?;
    // §5.4: the base's holdout, the reference every later holdout is judged against.
    holdout(env, study, &head, 0, &cancel).await?;

    // `running` first, then the file: a fire can follow the write at once (`every: 0s`
    // plus the Reconcile the write asks for), and the confinement resolver ends the
    // loop of a study that is not running or paused ("toolset gone", 140-P live).
    // A failed write still lands in `failed` through `prepare`'s error path.
    patch(
        env,
        study,
        StudyPatch {
            started_at: Some(now()),
            ..StudyPatch::default()
        },
    )
    .await?;
    set_state(env, study, StudyState::Running, None).await?;
    let model = env.cfg.executor(&spec.models, &env.agent);
    let text = schedule_file::text(&spec, &env.cfg, &worktree, &model, start);
    schedule_file::write(&env.data_dir, &slug, &text).await?;
    let sched = schedule_file::name(&slug);
    deliver(
        study.origin,
        &slug,
        "started",
        start_card(study, m, sigma, &sched),
    );
    Ok(())
}

/// `repeats` samples of `tree` through the slot; a crash fails the whole thing.
/// Samples flagged `noisy` (load precondition missed) are recorded but never
/// enter the baseline mean / σ (R-32); fewer than two clean ones is a refusal.
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
    let mut noisy = 0u32;
    for k in 1..=u32::from(spec.bench.repeats.max(1)) {
        let s = match scored_sample(
            env,
            spec,
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
            None,
        )
        .await
        {
            Ok(s) => s,
            // R-85: Stop now while the sample still waits for the slot or the load
            // precondition surfaces as the slot's error, not a lost sample.
            Err(_) if cancel.is_cancelled() => {
                return Err(Error::refused(
                    "stopped by you while the baseline was measured; nothing was kept",
                ));
            }
            Err(e) => return Err(e),
        };
        match s.value {
            Some(_) if s.noisy => noisy += 1,
            Some(v) => values.push(v),
            None if s.lost || cancel.is_cancelled() => {
                // R-85: Stop now while preparing cancels the baseline; the card says so.
                return Err(Error::refused(
                    "stopped by you while the baseline was measured; nothing was kept",
                ));
            }
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
    if values.len() < 2.min(usize::from(spec.bench.repeats.max(1))) {
        return Err(Error::refused(format!(
            "only {} clean baseline sample(s) ({noisy} taken under load above bench.max_load); \
             wait for a quieter machine or raise bench.wait",
            values.len()
        )));
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
    if let Some(slot) = m.slot {
        return begin_slot(env, m.session, slot).await;
    }
    let mut study = study_of(env, m.session).await?;
    match &study.state {
        StudyState::Running => {}
        // R-51: a paused study fires only for the user's RunOnce (a `run_once` event newer
        // than the last `begin`); a schedule that failed to disable does not keep it running.
        StudyState::Paused { .. } if run_once_pending(&env.store, &study.slug).await? => {}
        s => {
            return Err(Error::refused(format!(
                "study `{}` is {}; nothing to run",
                study.slug,
                state_word(s)
            )));
        }
    }
    let slug = study.slug.clone();
    // One fire = one experiment (§0 D1): a `begin` newer than the newest `turn` event
    // means this turn already ran an experiment — the model is told to end the turn
    // (140-P live: one fire ran seven). Without turn events (no agent) the rule is off.
    if second_begin_this_turn(&env.store, &slug).await? {
        return Err(Error::refused(
            "one experiment per fire: this turn already ran its experiment — end your turn now; \
             the loop starts the next experiment in a fresh session",
        ));
    }
    let mut incidents = Vec::new();
    // §10: a wave study promotes the previous wave first (unlogged slots count as discard).
    let wave = if study.spec.parallel > 1 {
        let (stop, notes) = crate::wave::settle(env, &study).await?;
        if stop.is_some() {
            return Err(Error::refused("the study is finalizing"));
        }
        incidents.extend(notes);
        study = study_by_slug(&env.store, &slug).await?;
        Some(crate::wave::next_wave(env, &slug).await?)
    } else {
        None
    };

    // §8.4: the money cap is checked again here so a fire after the cap ends at once.
    // The spend must be known (R-14): an unreadable ledger refuses the fire instead of failing open.
    let spend = study_spend(env, &study).await.map_err(|e| {
        Error::refused(format!(
            "study spend unknown ({e}); not starting an experiment"
        ))
    })?;
    if spend >= study.spec.budget.usd {
        begin_finalize(env, study, StopReason::Budget).await?;
        return Err(Error::refused("budget exhausted; the study is finalizing"));
    }

    let worktree = worktree_of(&study)?;
    // R-50: a keep whose commit landed but whose ledger write did not (crash in between) is
    // adopted here, before the drift repair could reset the kept commit off the branch.
    if let Some(note) = adopt_pending_keep(env, &mut study, &worktree).await? {
        incidents.push(note);
    }

    // §4.3 invariant: close a dangling experiment instead of blocking. A measuring one has
    // a bench group to kill and samples to mark lost (R-49); its change is saved as its own
    // candidate.patch before the repair resets the tree (R-63).
    if let Some(mut dangling) = env.store.dangling_experiment(&slug, None).await? {
        let was_measuring = dangling.state == ExperimentState::Measuring;
        if was_measuring {
            cancel(&env.root, &slug);
        }
        if let Ok(inc) = incumbent_of(&study) {
            let patch = env.runs_dir(&slug, dangling.n).join("candidate.patch");
            let _ = std::fs::create_dir_all(env.runs_dir(&slug, dangling.n));
            match env.backend.discard(&worktree, &inc, &patch) {
                Ok(_) => {
                    dangling.patch_path =
                        Some(format!("{slug}/runs/{}/candidate.patch", dangling.n));
                }
                Err(e) => tracing::warn!(%slug, n = dangling.n, "dangling change not saved: {e}"),
            }
        }
        let from = dangling.state;
        dangling.state = if was_measuring {
            ExperimentState::Interrupted
        } else {
            ExperimentState::Abandoned
        };
        dangling.finished_at = Some(now());
        if !env.store.update_experiment_if(&dangling, &[from]).await? {
            // The cancelled run closed its own row first; only the saved patch is ours to add.
            if let Some(mut cur) = env.store.get_experiment(&slug, dangling.n).await? {
                cur.patch_path = dangling.patch_path.clone();
                env.store.update_experiment(&cur).await?;
            }
        }
        if was_measuring {
            mark_lost(&env.store, &slug, dangling.n).await?;
        }
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

    // §5.2 step 4: one incumbent sample every `remeasure_every` keeps, counted by its
    // own `remeasure` event (confirm-phase incumbent samples share the role, R-20).
    let (closed, kept) = counts(&env.store, &slug).await?;
    if let Some(due) = kept.checked_div(env.cfg.gate.remeasure_every) {
        let remeasures = env
            .store
            .list_events(&slug, Some("remeasure"), 10_000)
            .await?
            .len() as u32;
        if due > remeasures {
            let evaluator = env.evaluator_dir(&slug, study.segment);
            // The tree is clean at the incumbent here (repair just ran): sample it in place.
            let s = scored_sample(
                env,
                &study.spec,
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
                None,
            )
            .await?;
            env.store
                .append_event(
                    &slug,
                    "remeasure",
                    &serde_json::json!({ "n": n, "value": s.value }),
                )
                .await?;
            if let Some(note) = absorb_incumbent_sample(env, &mut study, &incumbent, n, &s).await? {
                incidents.push(note);
            }
        }
    }

    // Allocate n (one experiment), or the wave's N slots (§10 step 1).
    let slots = match wave {
        Some(w) => Some(crate::wave::open_env(env, &study, w).await?),
        None => {
            // 159 §18.3: the ratchet's parent is the incumbent; a front (W1) or population (W4)
            // parent is picked, and checked out, by its module.
            let parent = parent_of(env, &study, None).await?;
            let mut exp = ExperimentRecord::open(&slug, n, study.segment, &parent);
            exp.parent_ref = crate::pareto::parent_ref(&env.store, &slug, &parent).await?; // 159:W1
            env.store.insert_experiment(&exp).await?;
            None
        }
    };
    // The live session tree's spend at open; `Log` subtracts it for the row's `cost_nanos`.
    let live = env.backend.session_cost(study.session).await.ok();
    env.store
        .append_event(
            &slug,
            "begin",
            &serde_json::json!({ "n": n, "spend_usd": live }),
        )
        .await?;
    set_phase(&env.root, &slug, n, Phase::Novelty);

    // §9.1 Restore: the harness re-applies the rejected change as this experiment's candidate
    // (single-experiment studies; a wave's candidates live in slot worktrees — the ★ idea still carries it).
    if wave.is_none()
        && let Some(of) = pending_restore(&env.store, &slug).await?
    {
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

    // The overlaid config lives only through the render (the future stays small).
    let (mut brief, max_bytes) = {
        let cfg = mechanism_cfg(env, &study);
        let cx = brief_context(env, &study, &cfg, n, incidents, closed, spend).await?;
        let brief = env
            .backend
            .brief(&env.store, &study, &cfg.brief, &cx)
            .await?;
        (brief, cfg.brief.max_bytes as usize)
    };
    brief.n = n;
    brief.segment = study.segment;
    env.store.mark_steers_seen(&slug, n).await?;
    match (wave, slots) {
        (Some(w), Some(slots)) => {
            let ideas = env.store.list_ideas(&slug, Some(IdeaStatus::Open)).await?;
            append_block(
                &mut brief,
                &crate::wave::supervisor_block(&study, w, &slots, &ideas),
                max_bytes,
            );
            for s in &slots {
                env.publish(StudyEvent::ExperimentStarted {
                    slug: slug.clone(),
                    n: s.n,
                    slot: Some(s.k),
                })
                .await;
            }
            brief.slots = Some(slots);
        }
        _ => {
            env.publish(StudyEvent::ExperimentStarted {
                slug: slug.clone(),
                n,
                slot: None,
            })
            .await;
        }
    }
    Ok(brief)
}

/// 159 W7 H6: `env.cfg` with the study's mechanism laid over it (brief caps, plateau window,
/// novelty threshold, crash retries): the pin when the study has one, else the active version,
/// else `env.cfg` byte for byte. One `git show` round per call; call it once per handler.
pub(crate) fn mechanism_cfg(env: &Env, study: &StudyRecord) -> AutoresearchConfig {
    crate::mechanism::overlay(&env.root, study.mechanism.as_deref(), &env.cfg)
}

/// The brief's context for experiment `n`: explore mode after a plateau (§8.4),
/// the budget line, the code map of the scope (focus = scope paths without their
/// glob tails, rooted at the worktree — R-76), incidents. Shared by the single
/// experiment and every wave slot (R-73).
async fn brief_context(
    env: &Env,
    study: &StudyRecord,
    cfg: &AutoresearchConfig,
    n: u32,
    incidents: Vec<String>,
    closed: u32,
    spend: f64,
) -> Result<crate::brief::Context> {
    let slug = &study.slug;
    let events = env.store.list_events(slug, Some("explore"), 1).await?;
    let last_keep_n = env
        .store
        .list_experiments(slug, None, u32::MAX)
        .await?
        .iter()
        .find(|e| e.state == ExperimentState::Kept)
        .map_or(0, |e| e.n);
    let explore = events.first().is_some_and(|e| {
        e.payload
            .get("after_n")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32
            >= last_keep_n
    });
    let hours = hours_live(env, study).await;
    let left = budget_left(&study.spec, closed, hours, spend);
    let focus: Vec<String> = study
        .spec
        .scope
        .iter()
        .map(|g| {
            g.split(['*', '?', '['])
                .next()
                .unwrap_or("")
                .trim_end_matches('/')
                .to_owned()
        })
        .filter(|f| !f.is_empty())
        .collect();
    let root = study.worktree.clone().unwrap_or_else(|| study.repo.clone());
    let code_map = env
        .backend
        .code_map(&env.rt, &root, &focus, cfg.brief.code_map_tokens)
        .await;
    Ok(crate::brief::Context {
        n,
        explore,
        incidents,
        budget_left: (left.experiments, left.hours, left.usd),
        code_map,
        gate: cfg.gate.clone(),
        crash_retries: cfg.defaults.crash_retries,
        worktree: None,
        hooks: crate::hooks::before(env, study, n).await.unwrap_or(None), // 159:W5
        protocol: crate::mechanism::protocol(&env.root, study.mechanism.as_deref()), // 159:W7
    })
}

/// Append a wave block to a rendered brief without exceeding `brief.max_bytes`
/// (R-74): the brief's own lists are trimmed first, never the block.
fn append_block(brief: &mut Brief, block: &str, max_bytes: usize) {
    if brief.text.len() + block.len() > max_bytes {
        brief.text = crate::brief::trim(&brief.text, max_bytes.saturating_sub(block.len()));
    }
    brief.text.push_str(block);
}

/// `Begin{slot}` from a wave child (140 §10 step 2): bind the child to its
/// slot, seat it in the slot worktree (F4E: its writes cannot reach the study
/// tree or the user's checkout), and return the study brief for the slot's
/// experiment plus its `<slot>` block.
async fn begin_slot(env: &Env, session: SessionId, slot: u8) -> Result<Brief> {
    crate::wave::bind_slot(session, slot);
    let (study, exp, dir) = crate::wave::resolve_env(env, session, slot).await?;
    if crate::wave::is_child(session) {
        crate::binding::seat(session, &dir);
    }
    let slug = study.slug.clone();
    let wave = exp.wave.unwrap_or(0);
    let slots = crate::wave::slots_of(env, &slug, wave).await?;
    let Some(mine) = slots.iter().find(|s| s.k == slot).cloned() else {
        return Err(Error::refused(format!("wave {wave} has no slot {slot}")));
    };
    let (closed, _) = counts(&env.store, &slug).await?;
    let spend = study_spend_or_closed(env, &study).await;
    let (mut brief, max_bytes) = {
        let cfg = mechanism_cfg(env, &study);
        let mut cx = brief_context(env, &study, &cfg, exp.n, Vec::new(), closed, spend).await?;
        // The child edits its slot tree, not the study worktree (140-P live: a slot that
        // followed the brief's Worktree line edited the study tree and measured nothing).
        cx.worktree = Some(mine.worktree.clone());
        let brief = env
            .backend
            .brief(&env.store, &study, &cfg.brief, &cx)
            .await?;
        (brief, cfg.brief.max_bytes as usize)
    };
    brief.n = exp.n;
    brief.segment = study.segment;
    let ideas = env.store.list_ideas(&slug, Some(IdeaStatus::Open)).await?;
    append_block(
        &mut brief,
        &crate::wave::child_block(wave, &mine, &ideas),
        max_bytes,
    );
    brief.slots = Some(vec![mine]);
    Ok(brief)
}

// ───────────────────────────── Run ─────────────────────────────

/// `Run` → `RunReport` (140 §4.3 measuring); blocks while measuring, cancel-safe.
pub async fn run(a: &mut Studies, m: Run) -> Result<RunReport> {
    run_env(&a.env(), m).await
}

/// The experiment the study session is working on (`open|measuring|measured`); with
/// `or_waiting` also the study's newest `waiting` launch (159 W6: `experiment_log` records its
/// text without closing it). Kept out of `log_env`'s generator: inlined there, the extra
/// sqlx future overflowed the 2 MiB test stack (golden ledger).
async fn current_experiment(
    env: &Env,
    study: &StudyRecord,
    or_waiting: bool,
) -> Result<ExperimentRecord> {
    if let Some(e) = env.store.dangling_experiment(&study.slug, None).await? {
        return Ok(e);
    }
    if or_waiting
        && let Some(e) = env
            .store
            .waiting_experiments()
            .await?
            .into_iter()
            .rfind(|e| e.slug == study.slug)
    {
        return Ok(e);
    }
    Err(Error::refused(
        "no experiment is open: call experiment_begin first",
    ))
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
    // A wave slot (§10 step 2) measures its own candidate worktree; everything else is the same path.
    let (mut study, mut exp, worktree) = match m.slot {
        Some(slot) => crate::wave::resolve_env(env, m.session, slot).await?,
        None => {
            let study = study_of(env, m.session).await?;
            let exp = current_experiment(env, &study, false).await?;
            let worktree = worktree_of(&study)?;
            (study, exp, worktree)
        }
    };
    let slug = study.slug.clone();
    let retries = mechanism_cfg(env, &study).defaults.crash_retries; // 159 W7 H6
    let runs_so_far = u8::try_from(
        env.store
            .list_events(&slug, Some("run"), 10_000)
            .await?
            .iter()
            .filter(|e| e.payload.get("n").and_then(|v| v.as_u64()) == Some(u64::from(exp.n)))
            .count(),
    )
    .unwrap_or(u8::MAX);
    match (exp.state, exp.verdict.as_ref()) {
        (ExperimentState::Open, _) => {}
        (ExperimentState::Measured, Some(Verdict::Crash)) if runs_so_far <= retries => {}
        (ExperimentState::Measured, Some(Verdict::Crash)) => {
            return Err(Error::refused(format!(
                "no retries left after {runs_so_far} crashed runs; call experiment_log"
            )));
        }
        // R-1: experiment_log found the tree changed since the measurement → measure again.
        (ExperimentState::Measured, _)
            if tree_changed_pending(&env.store, &slug, exp.n).await? => {}
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
    // R-60: the claim is conditional — two parallel experiment_run calls cannot both measure.
    let from = exp.state;
    if !env
        .store
        .claim_experiment(&slug, exp.n, &[from], ExperimentState::Measuring)
        .await?
    {
        return Err(Error::refused("already measuring"));
    }
    env.store
        .append_event(&slug, "run", &serde_json::json!({ "n": exp.n }))
        .await?;
    exp.idea = a.idea.chars().take(300).collect();
    exp.category = a.category.chars().take(40).collect();
    exp.hypothesis = a.hypothesis.chars().take(300).collect();
    exp.state = ExperimentState::Measuring;
    env.store.update_experiment(&exp).await?;
    let cancel = cancel_token(&env.root, &slug);
    // R-52: whatever fails inside the measurement (a spawn, the store, a cancelled slot
    // wait) must not leave the row `measuring`: a cancel closes it `interrupted`, any
    // other error hands the experiment back (`open`) so the model can run again.
    let r = measure_env(
        env,
        &mut study,
        &mut exp,
        &worktree,
        a,
        runs_so_far,
        &cancel,
    )
    .await;
    if let Err(e) = &r
        && exp.state == ExperimentState::Measuring
    {
        if cancel.is_cancelled() {
            let _ = interrupted(env, &mut exp, Vec::new()).await;
        } else {
            tracing::warn!(%slug, n = exp.n, "measurement failed, experiment reopened: {e}");
            exp.state = ExperimentState::Open;
            env.store
                .update_experiment_if(&exp, &[ExperimentState::Measuring])
                .await?;
            mark_lost(&env.store, &slug, exp.n).await?;
            clear_phase(&env.root, &slug);
        }
    }
    r
}

/// A `tree_changed {n}` event newer than the last `run {n}` (R-1): the keep was
/// refused because the worktree no longer matches the measured patch.
async fn tree_changed_pending(store: &Store, slug: &str, n: u32) -> Result<bool> {
    let events = store.list_events(slug, None, 10_000).await?;
    let of_n = |e: &&crate::store::EventRecord| {
        e.payload.get("n").and_then(|v| v.as_u64()) == Some(u64::from(n))
    };
    let changed = events
        .iter()
        .filter(of_n)
        .find(|e| e.kind == "tree_changed")
        .map(|e| e.id);
    let ran = events
        .iter()
        .filter(of_n)
        .find(|e| e.kind == "run")
        .map(|e| e.id);
    Ok(matches!((changed, ran), (Some(c), Some(r)) if c > r))
}

/// A `begin` event newer than the newest `turn` event: this turn already opened an
/// experiment. `false` when the study has no `turn` events (no agent, tests).
async fn second_begin_this_turn(store: &Store, slug: &str) -> Result<bool> {
    let Some(turn) = store.list_events(slug, Some("turn"), 1).await?.pop() else {
        return Ok(false);
    };
    Ok(store
        .list_events(slug, Some("begin"), 1)
        .await?
        .pop()
        .is_some_and(|b| b.id > turn.id))
}

/// The measurement body of [`run_env`]: novelty → custody → scope → samples →
/// gate → checks → verdict. `exp` is `measuring` on entry and `measured` on `Ok`.
async fn measure_env(
    env: &Env,
    study: &mut StudyRecord,
    exp: &mut ExperimentRecord,
    worktree: &Path,
    a: &RunArgs,
    runs_so_far: u8,
    cancel: &CancellationToken,
) -> Result<RunReport> {
    let slug = study.slug.clone();
    let mcfg = mechanism_cfg(env, study); // 159 W7 H6
    let retries = mcfg.defaults.crash_retries;
    let n = exp.n;
    let started = exp.started_at.clone();
    let incumbent = incumbent_of(study)?;
    let retries_left = retries.saturating_sub(runs_so_far);
    // Nothing to measure (140-P live): the model edited outside the worktree (file
    // tools resolve relative paths against the engine's cwd), so the tree equals the
    // incumbent. A named error, no sample, experiment still open — the model edits the
    // right tree and runs again (a duplicate-of-#1 refusal hid the cause).
    let scope = env.backend.scope_diff(&study.spec, worktree)?;
    if scope == ScopeDiff::default() {
        env.store
            .append_event(&slug, "no_change", &serde_json::json!({ "n": n }))
            .await?;
        return Err(Error::refused(format!(
            "no change in the worktree {} vs the incumbent — nothing to measure; edit files by \
             absolute path under the worktree (relative paths resolve elsewhere), then call \
             experiment_run again",
            worktree.display()
        )));
    }
    // Novelty (§7.3).
    progress(env, &slug, n, &started, Phase::Novelty, 0, 1).await;
    // A git failure here is an error the model sees, never an empty hash fed to the novelty check (R-102).
    let patch_hash = env.backend.patch_hash(worktree, &exp.parent_sha)?; // 159 R-SEAMS-7
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
            mcfg.novelty.threshold, // 159 W7 H6
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
        return refuse_run(env, exp, reason, vec![], novelty.similar).await;
    }

    // Custody (§5.3).
    progress(env, &slug, n, &started, Phase::Custody, 0, 1).await;
    let custody = env
        .store
        .get_segment(&slug, study.segment)
        .await?
        .map(|s| s.custody)
        .unwrap_or_default();
    let changed = env.backend.custody_verify(&custody, worktree)?;
    if !changed.is_empty() {
        let reason = format!(
            "evaluator changed ({}) — revert with \"git checkout {} -- {}\" or ask the user for a new segment",
            changed.join(", "),
            incumbent,
            changed.join(" ")
        );
        return refuse_run(env, exp, reason, vec![], novelty.similar).await;
    }

    // Scope (§5.3), computed before the novelty check above.
    if !scope.off_limits.is_empty() {
        let reason = format!("touches off-limits {}", scope.off_limits.join(", "));
        return refuse_run(env, exp, reason, scope.deviations.clone(), novelty.similar).await;
    }
    exp.deviations = scope.deviations.clone();
    env.store.update_experiment(exp).await?;
    // Only a run that will spend a sample marks the matching idea `tried` (R-69): a
    // refused one leaves the idea open for the next brief.
    let _ = env
        .backend
        .mark_tried(&env.store, &slug, &exp.idea, n)
        .await;

    // Samples with the sequential gate (§5.2). A re-run after a crash retry or a
    // tree change gets its own tree name so σ never pools two measurements of
    // different trees (R-1).
    let evaluator = env.evaluator_dir(&slug, study.segment);
    let runs = env.runs_dir(&slug, n);
    std::fs::create_dir_all(&runs)?;
    let cand_tree = if runs_so_far == 0 {
        format!("exp:{n}")
    } else {
        format!("exp:{n}r{runs_so_far}")
    };
    let paths = Paths {
        slug: &slug,
        n,
        worktree,
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
    let metric_name = study.spec.metric.name.clone();
    let crash = |env: &Env, exp: &mut ExperimentRecord, report: &mut RunReport, s: &Sample| {
        let tail = s
            .log_path
            .as_deref()
            .map(|p| env.backend.tail(&env.root.join(p)))
            .unwrap_or_default();
        // The runner's reason (R-41: `exit 1`, `bad number time_ms=nan`, `no METRIC time_ms line`).
        exp.line = match &s.failure {
            Some(f) => format!("crash: {f} (tail below)"),
            None => match s.exit {
                Some(0) => format!(
                    "crash: exit 0 but no usable `METRIC {metric_name}=<number>` line (missing, nan or inf; tail below)"
                ),
                Some(e) => format!("crash: exit {e} (tail below)"),
                None => "crash: killed (timeout or signal; tail below)".to_owned(),
            },
        };
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
                cancel,
            )
            .await?;
        if exit != Some(0) {
            exp.line = format!("crash: smoke `{smoke}` exit {exit:?}");
            report.verdict = Verdict::Crash;
            report.line = exp.line.clone();
            report.tail = Some(tail);
            return finish_measure(env, exp, report, taken, None).await;
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
    let mut k: u32 = 0;
    // Incumbent samples taken in this call get their own `k` (sample-<k>.log is never overwritten, R-23).
    let mut k_inc: u32 = 100;
    // Candidate samples before the gate is consulted again (`Confirm.more` is W1's count;
    // the drift rule's doubling is ours, §5.2 step 4). `more == 0` with `incumbent_sample`
    // is the incumbent-first step after a fast keep (R-25): re-judge on the same candidate
    // samples once the incumbent has one more.
    let mut need: u32 = 1;
    let drift_double = drift_pending(&env.store, &slug).await?;
    let mut doubled = false;
    let mut decision: Option<GateStep> = None;
    let mut last_input: Option<GateInput> = None;
    let max_samples = 1 + 2 * u32::from(env.cfg.gate.confirm_runs) + 1;
    let max_incumbent = 1 + u32::from(env.cfg.gate.confirm_runs);
    let (mut sample_no, mut of) = (1u8, 1u8);
    while decision.is_none() && k < max_samples {
        if cancel.is_cancelled() {
            return interrupted(env, exp, taken).await;
        }
        let cand_count = taken
            .iter()
            .filter(|s| s.role == Some(SampleRole::Candidate))
            .count() as u32;
        if cand_count < need {
            k += 1;
            sample_no = u8::try_from(k).unwrap_or(u8::MAX);
            of = planned.max(sample_no);
            progress(env, &slug, n, &started, Phase::Sampling, sample_no, of).await;
            let s = scored_sample(
                env,
                &study.spec,
                paths.req(k, &cand_tree, SampleRole::Candidate),
                cancel,
                Some(Ticks {
                    started_at: &started,
                    sample: sample_no,
                    of,
                }),
            )
            .await?;
            if cancel.is_cancelled() || s.lost {
                taken.push(s);
                return interrupted(env, exp, taken).await;
            }
            taken.push(s.clone());
            if s.value.is_none() {
                crash(env, exp, &mut report, &s);
                return finish_measure(env, exp, report, taken, None).await;
            }
            continue;
        }
        let cand_taken: Vec<Sample> = taken
            .iter()
            .filter(|s| s.role == Some(SampleRole::Candidate))
            .cloned()
            .collect();
        let inc_samples = incumbent_samples(env, study, &exp.parent_sha).await?;
        let incumbent_age = inc_samples
            .iter()
            .map(|s| s.n)
            .max()
            .map_or(n, |newest| n.saturating_sub(newest).saturating_sub(1));
        let candidate = clean_values(&cand_taken);
        let incumbent_vals = bench_values(&inc_samples);
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
        match kind_rule(env, study, &gcfg, &input, &cand_taken, &inc_samples).await? {
            GateStep::Confirm {
                more,
                incumbent_sample,
            } => {
                let mut more = u32::from(more);
                // An incumbent-first step is honoured only while the incumbent samples of this
                // call stay within `1 + confirm_runs`; past that (or with nothing asked at all)
                // a candidate sample is taken so the loop always advances.
                let take_incumbent = incumbent_sample && k_inc - 100 < max_incumbent;
                if more == 0 && !take_incumbent {
                    more = 1;
                }
                if drift_double && !doubled && more > 0 {
                    more *= 2;
                    doubled = true;
                    env.store
                        .append_event(&slug, "confirm_doubled", &serde_json::json!({ "n": n }))
                        .await?;
                }
                need = cand_taken.len() as u32 + more;
                planned = u8::try_from(need).unwrap_or(u8::MAX);
                if take_incumbent {
                    k_inc += 1;
                    let si = incumbent_sample_in_place(
                        env,
                        study,
                        &paths,
                        &incumbent,
                        k_inc,
                        cancel,
                        Ticks {
                            started_at: &started,
                            sample: sample_no,
                            of,
                        },
                    )
                    .await?;
                    if si.lost || cancel.is_cancelled() {
                        taken.push(si);
                        return interrupted(env, exp, taken).await;
                    }
                    taken.push(si);
                }
            }
            d @ (GateStep::Decide { .. } | GateStep::Wait { .. }) => decision = Some(d),
        }
        last_input = Some(input);
    }
    // At the cap the gate decides on what was measured; with no measurable input it is no gain.
    let (verdict, delta, delta_pct, z, detail) = match decision
        .or_else(|| last_input.as_ref().map(|i| env.backend.gate_step(&gcfg, i)))
    {
        Some(GateStep::Decide {
            verdict,
            delta,
            delta_pct,
            z,
            detail,
        }) => (verdict, delta, delta_pct, z, detail),
        Some(GateStep::Wait { until, .. }) => (Verdict::Pending { until }, 0.0, 0.0, None, None),
        _ => (Verdict::NoGain, 0.0, 0.0, None, None),
    };
    let cand_vals = clean_values(
        &taken
            .iter()
            .filter(|s| s.role == Some(SampleRole::Candidate))
            .cloned()
            .collect::<Vec<_>>(),
    );
    let inc_samples = incumbent_samples(env, study, &exp.parent_sha).await?;
    let inc_vals = bench_values(&inc_samples);
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
        detail: detail.clone(),
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
    // §5.2 step 5: name the guard that failed (`alloc_mb +7.2 % > 5 %`), R-35.
    if verdict == Verdict::GuardFailed
        && let Some(input) = last_input.as_ref()
    {
        let failed = gate::guard_failures(&gcfg, input);
        if !failed.is_empty() {
            report.line = format!("guard failed: {}; {}", failed.join(", "), report.line);
        }
    }
    report.cases_moved = cases_moved(&taken, &inc_samples);
    exp.metric = Some(cand_mean);
    exp.gate = detail;

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
                paths.req(1, &cand_tree, SampleRole::Candidate),
                cancel,
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
    // σ absorbs this experiment's repeats (a column patch, never the whole row: R-4).
    let sigma = segment_sigma(env, study).await?;
    patch(
        env,
        study,
        StudyPatch {
            sigma: Some(Some(sigma)),
            ..StudyPatch::default()
        },
    )
    .await?;
    finish_measure(env, exp, report, taken, Some(cand_mean)).await
}

/// Mean secondaries `(name, candidate, incumbent)` for the guards.
pub(crate) fn secondary_means(
    candidate: &[Sample],
    incumbent: &[Sample],
) -> Vec<(String, f64, f64)> {
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
    let i = incumbent
        .iter()
        .rev()
        .find(|s| s.value.is_some() && s.role != Some(SampleRole::Holdout));
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
    if !env
        .store
        .update_experiment_if(exp, &[ExperimentState::Measuring])
        .await?
    {
        // StopNow / recover closed the row while the last sample ran.
        clear_phase(&env.root, &exp.slug);
        return Err(Error::refused("cancelled: the study was stopped"));
    }
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
    // StopNow / recover may have closed the row first (R-4): never overwrite their close-out.
    env.store
        .update_experiment_if(exp, &[ExperimentState::Measuring])
        .await?;
    mark_lost(&env.store, &exp.slug, exp.n).await?;
    clear_phase(&env.root, &exp.slug);
    Err(Error::refused("cancelled: the study was stopped"))
}

/// Samples of `(slug, n)` that never finished → `lost` (§8.5).
pub(crate) async fn mark_lost(store: &Store, slug: &str, n: u32) -> Result<()> {
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
pub(crate) async fn retag_samples(store: &Store, slug: &str, n: u32, sha: &str) -> Result<()> {
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
    // A wave slot (§10 step 2) closes its own candidate worktree; everything else is the same path.
    let (mut study, mut exp, worktree) = match m.slot {
        Some(slot) => crate::wave::resolve_env(env, m.session, slot).await?,
        None => {
            let study = study_of(env, m.session).await?;
            // 159 W6: `experiment_log` may also find a waiting launch (its text is recorded).
            let exp = current_experiment(env, &study, true).await?;
            let worktree = worktree_of(&study)?;
            (study, exp, worktree)
        }
    };
    let slug = study.slug.clone();
    if exp.state == ExperimentState::Measuring {
        return Err(Error::refused(
            "still measuring; wait for experiment_run to return",
        ));
    }
    let a = m.args;
    // 159 W6: a world launch waiting for its press (or its window) — record the text, leave the
    // state; `world::close` ends the row when the world answers.
    if matches!(exp.verdict, Some(Verdict::Pending { .. })) || exp.state == ExperimentState::Waiting
    {
        exp.description = a.description.chars().take(500).collect();
        exp.learned = a.learned.chars().take(500).collect();
        env.store.update_experiment(&exp).await?;
        return Ok(LogReport {
            applied: exp.state,
            ..LogReport::default()
        });
    }
    let incumbent = incumbent_of(&study)?;
    // 159 W1 H8 / R-SEAMS-7: the tree the worktree rests on (a front member, a cell elite);
    // every git base and reference sample is the parent — the ratchet's parent is the incumbent.
    let base = exp.parent_sha.clone();
    let n = exp.n;
    exp.description = a.description.chars().take(500).collect();
    exp.learned = a.learned.chars().take(500).collect();
    exp.next = a.next.map(|s| s.chars().take(300).collect());
    exp.decision = Some(a.decision);
    exp.justification = a.justification.clone();

    // What the verdict and the decision make of it.
    let mut verdict = exp.verdict.clone().unwrap_or(Verdict::Refused {
        reason: "not measured".into(),
    });
    let mut keep = verdict.is_keep_candidate() && a.decision != Decision::Discard;
    let wants_simpler = a.decision == Decision::KeepSimpler && !verdict.is_keep_candidate();
    // R-1: a keep commits what is in the tree *now*. Before any keep (incl. keep-simpler)
    // the tree must still be the measured patch, custody intact, nothing off-limits.
    if (keep || wants_simpler) && exp.verdict.is_some() {
        match verify_tree_for_keep(env, &study, &exp, &worktree, &base).await? {
            TreeCheck::Same => {}
            TreeCheck::Changed => {
                env.store
                    .append_event(&slug, "tree_changed", &serde_json::json!({ "n": n }))
                    .await?;
                return Err(Error::refused(
                    "the worktree changed since experiment_run measured it; nothing was kept. \
                     Call experiment_run again to measure what is in the tree now (or undo the \
                     edit first), then experiment_log.",
                ));
            }
            TreeCheck::Refused(reason) => {
                // Custody / off-limits at keep: the keep is impossible (§5.3); it closes as refused.
                verdict = Verdict::Refused {
                    reason: reason.clone(),
                };
                exp.verdict = Some(verdict.clone());
                exp.line = format!("refused at keep: {reason}");
                keep = false;
                if stop::is_custody_refusal(&verdict) {
                    env.store
                        .append_event(&slug, "custody_refusal", &serde_json::json!({ "n": n }))
                        .await?;
                }
            }
        }
    }
    if wants_simpler && !matches!(verdict, Verdict::Refused { .. }) {
        keep = keep_simpler_granted(env, &mut study, &mut exp, &worktree, &base).await?;
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
    if keep && m.slot.is_some() {
        // §10: a slot keep is a candidate commit in its own worktree; promotion moves the incumbent.
        let meta = crate::wave::keep_meta(&study, &exp);
        let sha = env
            .backend
            .keep(&worktree, &study.spec, &meta, env.cfg.git.author.as_deref())?;
        retag_samples(&env.store, &slug, n, &sha).await?;
        exp.commit_sha = Some(sha.clone());
        exp.state = ExperimentState::Kept;
        exp.line = format!(
            "keep-candidate of wave {}; {}",
            exp.wave.unwrap_or(0),
            exp.line
        );
        report.commit = Some(sha);
    } else if keep {
        let meta = KeepMeta {
            slug: slug.clone(),
            n,
            description: exp.description.clone(),
            metric_line: metric_line(&study, &exp),
            category: exp.category.clone(),
            deviations: exp.deviations.clone(),
        };
        let previous_mean = study.incumbent_mean;
        // R-50: the intent is recorded before the commit; `record_keep` writes the ledger side
        // in one transaction after it. A crash in between is adopted by the next Begin.
        env.store
            .append_event(
                &slug,
                "keep_pending",
                &serde_json::json!({ "n": n, "parent": base }),
            )
            .await?;
        let sha = env
            .backend
            .keep(&worktree, &study.spec, &meta, env.cfg.git.author.as_deref())?;
        exp.commit_sha = Some(sha.clone());
        exp.state = ExperimentState::Kept;
        let new_mean = exp.metric.or(study.incumbent_mean);
        env.store
            .record_keep(
                &exp,
                &sha,
                &StudyPatch::incumbent(&sha, new_mean, study.sigma),
            )
            .await?;
        study.incumbent_sha = Some(sha.clone());
        study.incumbent_mean = new_mean;
        report.commit = Some(sha.clone());
        // 159 W1 H11: every keep stays reachable past a review revert or a front/population
        // branch move. The branch already holds the commit; a failed ref is a warning.
        if crate::git::is_git_repo(&worktree)
            && let Err(e) = crate::git::plumbing(
                &worktree,
                &[
                    "update-ref",
                    &format!("refs/autoresearch/{slug}/kept/{n}"),
                    &sha,
                ],
            )
        {
            tracing::warn!(%slug, n, "kept ref not written: {e}");
        }

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
            // A keep the user restored (§9.1 Restore) was already judged by the user: a
            // second SUSPICIOUS flags it (needs-eyes line) but cannot revert it again,
            // or the user and the reviewer would take turns forever (140-P live).
            let restored = restoring(&env.store, &slug, n).await?.is_some();
            let rv = match rv {
                ReviewVerdict::Flagged { reason }
                    if env.cfg.review.on_suspicious == OnSuspicious::Revert && !restored =>
                {
                    // Revert: incumbent ← previous, the commit stays as a rejected ref, the patch
                    // is kept — and Restore must find it, so both writes are checked (R-103).
                    let patch_file = runs.join("candidate.patch");
                    let diff = env.backend.diff_text(&worktree, &base, &sha, 4 << 20)?;
                    std::fs::write(&patch_file, diff)?;
                    exp.patch_path = Some(format!("{slug}/runs/{n}/candidate.patch"));
                    env.backend.reject(&worktree, &slug, n, &base)?;
                    let back = StudyPatch::incumbent(&incumbent, previous_mean, study.sigma);
                    patch(env, &mut study, back).await?;
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
        }
    } else {
        let patch = runs.join("candidate.patch");
        // R-104: a failed discard (patch not saved, tree not reset) keeps the experiment
        // `measured` and the candidate in the tree; the model can call experiment_log again.
        let hash = env.backend.discard(&worktree, &base, &patch).map_err(|e| {
            Error::refused(format!(
                "the candidate could not be saved and reverted ({e}); nothing was closed — call experiment_log again"
            ))
        })?;
        exp.patch_path = Some(format!("{slug}/runs/{n}/candidate.patch"));
        if exp.patch_hash.as_deref().unwrap_or("").is_empty() {
            exp.patch_hash = Some(hash);
        }
        exp.state = match &verdict {
            Verdict::Crash => ExperimentState::Crashed,
            Verdict::ChecksFailed => ExperimentState::ChecksFailed,
            Verdict::Refused { .. } => ExperimentState::Refused,
            _ => ExperimentState::Discarded,
        };
    }
    exp.finished_at = Some(now());
    // Cost of this experiment = the live session tree now − its mark at `Begin`; the
    // `closed` mark hands the rest of the tree's spend to `study_spend` (R-14).
    let spend_at_begin = env
        .store
        .list_events(&slug, Some("begin"), 50)
        .await?
        .iter()
        .find(|e| e.payload.get("n").and_then(|v| v.as_u64()) == Some(u64::from(n)))
        .and_then(|e| e.payload.get("spend_usd").and_then(|v| v.as_f64()));
    match env.backend.session_cost(study.session).await {
        Ok(live) => {
            if let Some(at_begin) = spend_at_begin {
                exp.cost_nanos = ((live - at_begin).max(0.0) * 1e9) as u64;
            }
            env.store
                .append_event(
                    &slug,
                    "closed",
                    &serde_json::json!({ "n": n, "spend_usd": live }),
                )
                .await?;
        }
        Err(e) => tracing::warn!(%slug, n, "experiment cost unknown: {e}"),
    }
    env.store.update_experiment(&exp).await?;
    let _ = crate::hooks::after(env, &study, &exp).await; // 159:W5
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
    // §5.3 Revision 2: a regime change restarts the segment at this keep (slot keeps
    // are judged at promotion, `wave::promote_env`).
    if exp.state == ExperimentState::Kept && m.slot.is_none() {
        regime_segment(env, &study, &exp).await;
    }
    let spend = study_spend_or_closed(env, &study).await;
    report.applied = exp.state;
    report.incumbent = Incumbent {
        sha: study.incumbent_sha.clone().unwrap_or_default(),
        mean: study.incumbent_mean,
        kept,
    };
    report.budget_left = budget_left(&study.spec, closed, hours_live(env, &study).await, spend);

    // §8.4 stop rules after the experiment closed; a wave waits for its last slot, then promotes (§10 step 4).
    // A stop from here defers finalize to the study turn's end (R-56): the model still has the turn.
    report.stop = match m.slot {
        None => apply_rules_from_turn(env, &slug, spend, true).await?,
        Some(_) => {
            let wave = exp.wave.unwrap_or(0);
            if crate::wave::wave_closed(env, &slug, wave).await? {
                crate::wave::promote_env(env, &slug, wave).await?;
                if let Some(e) = env.store.get_experiment(&slug, n).await? {
                    report.applied = e.state;
                    report.commit = e.commit_sha.clone();
                    report.review = e.review.clone();
                }
                let after = study_by_slug(&env.store, &slug).await?;
                report.incumbent = Incumbent {
                    sha: after.incumbent_sha.clone().unwrap_or_default(),
                    mean: after.incumbent_mean,
                    kept: counts(&env.store, &slug).await?.1,
                };
                apply_rules_from_turn(env, &slug, spend, true).await?
            } else {
                None
            }
        }
    };
    Ok(report)
}

/// `Autoresearch-Metric: time_ms 42.3 -> 41.2 (-2.6%, 3.1 sigma, n=3)`.
pub(crate) fn metric_line(study: &StudyRecord, exp: &ExperimentRecord) -> String {
    // §6: `42.3 -> 41.2 (-2.6%)` — the raw relative change, the one Δ convention (Revision 2;
    // 140-P live wrote `8996 -> 39.0 (+99.6%)` when the ledger was direction-normalised).
    let raw_pct = match (study.incumbent_mean, exp.metric) {
        (Some(a), Some(b)) if a != 0.0 => (b - a) / a.abs() * 100.0,
        _ => exp.delta_pct.unwrap_or(0.0),
    };
    format!(
        "{} {} -> {} ({raw_pct:+.1}%, {} sigma, n={})",
        study.spec.metric.name,
        study
            .incumbent_mean
            .map(fmt_num)
            .unwrap_or_else(|| "?".into()),
        exp.metric.map(fmt_num).unwrap_or_else(|| "?".into()),
        exp.z
            .map(|z| format!("{z:.1}"))
            .unwrap_or_else(|| "—".into()),
        exp.n_samples
    )
}

/// §5.2 keep-simpler: not significantly worse on means *after confirm*, fewer
/// lines, guards hold, checks pass. The candidate is brought up to
/// `1 + confirm_runs` samples first (R-21): one sample would pass a 1.5σ
/// regression ≈ 36 % of the time, the means ≈ 7 %.
async fn keep_simpler_granted(
    env: &Env,
    study: &mut StudyRecord,
    exp: &mut ExperimentRecord,
    worktree: &Path,
    incumbent: &str,
) -> Result<bool> {
    let gcfg = gate_cfg(&env.cfg, &study.spec)?;
    let cand_tree = env
        .store
        .list_samples(&study.slug, Some(exp.n))
        .await?
        .into_iter()
        .rev()
        .find(|s| s.role == Some(SampleRole::Candidate) && s.value.is_some())
        .map(|s| s.tree_sha);
    let Some(cand_tree) = cand_tree else {
        return Ok(false);
    };
    let scope = env.backend.scope_diff(&study.spec, worktree)?;
    if scope.removed <= scope.added {
        return Ok(false); // no samples spent on a change that is not simpler
    }
    let slug = study.slug.clone();
    let evaluator = env.evaluator_dir(&slug, study.segment);
    let runs = env.runs_dir(&slug, exp.n);
    std::fs::create_dir_all(&runs)?;
    let paths = Paths {
        slug: &slug,
        n: exp.n,
        worktree,
        evaluator: &evaluator,
        runs: &runs,
    };
    let cancel = cancel_token(&env.root, &study.slug);
    let want = 1 + usize::from(env.cfg.gate.confirm_runs);
    let mut cand: Vec<Sample> = env
        .store
        .list_samples(&study.slug, Some(exp.n))
        .await?
        .into_iter()
        .filter(|s| s.role == Some(SampleRole::Candidate) && s.tree_sha == cand_tree)
        .collect();
    let mut k = cand.iter().map(|s| s.k).max().unwrap_or(0);
    while clean_values(&cand).len() < want {
        k += 1;
        progress(
            env,
            &study.slug,
            exp.n,
            &exp.started_at,
            Phase::Sampling,
            u8::try_from(k).unwrap_or(u8::MAX),
            u8::try_from(want).unwrap_or(u8::MAX),
        )
        .await;
        let s = scored_sample(
            env,
            &study.spec,
            paths.req(k, &cand_tree, SampleRole::Candidate),
            &cancel,
            None,
        )
        .await?;
        exp.n_samples += 1;
        if s.value.is_none() || s.lost {
            return Ok(false);
        }
        cand.push(s);
    }
    let inc = incumbent_samples(env, study, incumbent).await?;
    let input = GateInput {
        candidate: clean_values(&cand),
        incumbent: bench_values(&inc),
        sigma: study.sigma.unwrap_or(0.0),
        largest_accepted_delta: None,
        incumbent_age: 0,
        keep_simpler: true,
        secondaries: secondary_means(&cand, &inc),
    };
    exp.metric = Some(mean(&input.candidate));
    let sigma = segment_sigma(env, study).await?;
    patch(
        env,
        study,
        StudyPatch {
            sigma: Some(Some(sigma)),
            ..StudyPatch::default()
        },
    )
    .await?;
    if !env
        .backend
        .keep_simpler(&gcfg, &input, scope.added, scope.removed)
    {
        return Ok(false);
    }
    let Some(c) = study.spec.checks.as_ref() else {
        return Ok(true);
    };
    let (exit, _, _) = env
        .backend
        .command(
            &c.command,
            schedule_file::duration(&c.timeout, Duration::from_secs(300)),
            paths.req(2, &cand_tree, SampleRole::Candidate),
            &cancel,
        )
        .await?;
    Ok(exit == Some(0))
}

/// What [`verify_tree_for_keep`] found.
enum TreeCheck {
    /// The tree is the measured patch, custody intact, nothing off-limits.
    Same,
    /// The normalised patch hash differs from `experiment_run`'s.
    Changed,
    /// Custody or off-limits violated: the keep is impossible (the refusal text).
    Refused(String),
}

/// R-1: the keep invariant re-checked at keep time. Between `experiment_run`
/// returning and `experiment_log` the model has a full turn with edit tools;
/// whatever is in the tree now is what `git add -A` would commit.
async fn verify_tree_for_keep(
    env: &Env,
    study: &StudyRecord,
    exp: &ExperimentRecord,
    worktree: &Path,
    incumbent: &str,
) -> Result<TreeCheck> {
    let custody = env
        .store
        .get_segment(&study.slug, study.segment)
        .await?
        .map(|s| s.custody)
        .unwrap_or_default();
    let changed = env.backend.custody_verify(&custody, worktree)?;
    if !changed.is_empty() {
        return Ok(TreeCheck::Refused(format!(
            "evaluator changed ({}) after the measurement",
            changed.join(", ")
        )));
    }
    let scope = env.backend.scope_diff(&study.spec, worktree)?;
    if !scope.off_limits.is_empty() {
        return Ok(TreeCheck::Refused(format!(
            "touches off-limits {}",
            scope.off_limits.join(", ")
        )));
    }
    let hash = env.backend.patch_hash(worktree, incumbent)?;
    Ok(match exp.patch_hash.as_deref() {
        Some(h) if !h.is_empty() && h != hash => TreeCheck::Changed,
        _ => TreeCheck::Same,
    })
}

// ───────────────────────────── stop rules, pause, finalize ─────────────────────────────

/// Evaluate §8.4 for `slug` and act; returns the stop reason when the study is finalizing.
/// Called from outside a study turn (abandoned / interrupted close-outs, wave settle).
pub(crate) async fn apply_rules(env: &Env, slug: &str, spend: f64) -> Result<Option<StopReason>> {
    apply_rules_from_turn(env, slug, spend, false).await
}

/// [`apply_rules`]; `in_turn` = the call comes from `experiment_log` while the model
/// still has the turn, so a stop defers §11 to the turn's end (R-56).
async fn apply_rules_from_turn(
    env: &Env,
    slug: &str,
    spend: f64,
    in_turn: bool,
) -> Result<Option<StopReason>> {
    let mut study = study_by_slug(&env.store, slug).await?;
    if !matches!(study.state, StudyState::Running | StudyState::Paused { .. }) {
        return Ok(study.stop_reason.clone());
    }
    // The overlaid config is a statement temporary: dropped before the branches' awaits.
    let rule = stop::evaluate(&env.store, &study, &mechanism_cfg(env, &study), spend).await?;
    match rule {
        stop::Rule::Continue => Ok(None),
        stop::Rule::Explore => {
            let after_n = last_closed_n(&env.store, slug).await?;
            env.store
                .append_event(slug, "explore", &serde_json::json!({ "after_n": after_n }))
                .await?;
            if study.spec.literature.is_some() {
                let _ = crate::literature::consult(env, &study).await; // 159:W5
            }
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
                Err(e) => {
                    // R-75: on the ledger, so the brief can drop "the advisor's suggestions".
                    tracing::warn!(%slug, "advisor consult failed: {e}");
                    env.store
                        .append_event(
                            slug,
                            "advisor_failed",
                            &serde_json::json!({ "after_n": after_n, "reason": e.to_string() }),
                        )
                        .await?;
                }
            }
            Ok(None)
        }
        stop::Rule::Pause(reason) => {
            pause(env, &mut study, reason).await?;
            Ok(None)
        }
        stop::Rule::Stop(reason) => {
            begin_finalize_with(env, study, reason.clone(), in_turn).await?;
            Ok(Some(reason))
        }
    }
}

/// After an experiment closed outside `Log` (abandoned, interrupted): the stop rules.
pub async fn after_experiment(env: &Env, slug: &str) -> Result<()> {
    let study = study_by_slug(&env.store, slug).await?;
    let spend = study_spend_or_closed(env, &study).await;
    apply_rules(env, slug, spend).await.map(|_| ())
}

/// Does the study turn's end have finalize work waiting (140 §8.6 / §11)? Either a
/// user `stop_after` recorded after the last resume, or a finalize that `experiment_log`
/// deferred to the turn's end (R-56). `binding::on_turn_finished` calls
/// [`begin_finalize`] when this is true.
pub async fn stop_after_pending(env: &Env, study: &StudyRecord) -> Result<bool> {
    if study.state == StudyState::Finalizing {
        return finalize_deferred(&env.store, &study.slug).await;
    }
    Ok(stop::view(&env.store, study, 0.0).await?.stop_after)
}

/// A `finalize_deferred` event newer than every `finalize_started`.
async fn finalize_deferred(store: &Store, slug: &str) -> Result<bool> {
    let deferred = store
        .list_events(slug, Some("finalize_deferred"), 1)
        .await?;
    let Some(d) = deferred.first() else {
        return Ok(false);
    };
    let started = store.list_events(slug, Some("finalize_started"), 1).await?;
    Ok(started.first().is_none_or(|s| s.id < d.id))
}

/// `enabled: false` + a `paused` event + the state. A no-op on an already paused
/// study (R-46: a rule re-pause must not re-stamp `paused.at`).
pub(crate) async fn pause(env: &Env, study: &mut StudyRecord, reason: PauseReason) -> Result<()> {
    if matches!(study.state, StudyState::Paused { .. }) {
        return Ok(());
    }
    if let Err(e) = schedule_file::set_enabled(&study.slug, false).await {
        tracing::warn!(slug = %study.slug, "schedule not disabled (Begin accepts only a RunOnce while paused): {e}");
    }
    env.store
        .append_event(
            &study.slug,
            "paused",
            &serde_json::json!({ "reason": pause_tag(&reason), "at": now() }),
        )
        .await?;
    let word = pause_word(&reason);
    // Only a running study pauses: a Finalizing / StopNow that landed meanwhile wins (R-4).
    set_state_if(
        env,
        study,
        &["running"],
        StudyState::Paused { reason },
        Some(word),
    )
    .await
    .map(drop)
}

async fn resume(env: &Env, study: &mut StudyRecord) -> Result<()> {
    let mut paused_ms = study.paused_ms;
    if let Some(e) = env
        .store
        .list_events(&study.slug, Some("paused"), 1)
        .await?
        .first()
        && let Some(at) = e.payload.get("at").and_then(|v| v.as_str())
        && let Ok(t) = chrono::DateTime::parse_from_rfc3339(at)
    {
        paused_ms += (chrono::Utc::now() - t.with_timezone(&chrono::Utc))
            .num_milliseconds()
            .max(0) as u64;
    }
    let after_n = last_closed_n(&env.store, &study.slug).await?;
    env.store
        .append_event(
            &study.slug,
            "resumed",
            &serde_json::json!({ "after_n": after_n }),
        )
        .await?;
    if let Err(e) = schedule_file::set_enabled(&study.slug, true).await {
        tracing::warn!(slug = %study.slug, "schedule not enabled: {e}");
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
        env.publish(StudyEvent::StudyChanged {
            slug: study.slug.clone(),
            state: StudyState::Running,
            reason: None,
        })
        .await;
    }
    Ok(())
}

/// Enter `finalizing` (140 §4.2) and run §11 off the mailbox. From outside a study
/// turn; `experiment_log` uses the deferring variant. On a study already
/// `finalizing` with a deferred §11 (the turn ended — `binding::on_turn_finished`),
/// this starts the deferred task.
pub async fn begin_finalize(env: &Env, study: StudyRecord, reason: StopReason) -> Result<()> {
    begin_finalize_with(env, study, reason, false).await
}

async fn begin_finalize_with(
    env: &Env,
    mut study: StudyRecord,
    reason: StopReason,
    defer: bool,
) -> Result<()> {
    if study.state == StudyState::Finalizing {
        if finalize_deferred(&env.store, &study.slug).await? {
            spawn_finalize(env, &study.slug).await?;
        }
        return Ok(());
    }
    if is_terminal(&study.state) {
        return Ok(());
    }
    cancel(&env.root, &study.slug);
    env.store
        .append_event(
            &study.slug,
            "stop",
            &serde_json::json!({ "reason": reason }),
        )
        .await?;
    let p = StudyPatch {
        state: Some(StudyState::Finalizing),
        stop_reason: Some(reason.clone()),
        ..StudyPatch::default()
    };
    // CAS from running/paused: two stop rules racing (Log's review vs StopNow) finalize once (R-4).
    if !env
        .store
        .patch_study_if(&study.slug, &["running", "paused"], &p)
        .await?
    {
        return Ok(());
    }
    p.apply_to(&mut study);
    env.publish(StudyEvent::StudyChanged {
        slug: study.slug.clone(),
        state: StudyState::Finalizing,
        reason: Some(stop_word(&reason)),
    })
    .await;
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
    if defer {
        // R-56: the model still has the turn; §11 measures the tree once the turn is over
        // (`on_turn_finished` → `begin_finalize`), or after FINALIZE_DEFER_MAX regardless.
        env.store
            .append_event(&study.slug, "finalize_deferred", &serde_json::json!({}))
            .await?;
        let (env2, slug) = (env.clone(), study.slug.clone());
        tokio::spawn(async move {
            tokio::time::sleep(FINALIZE_DEFER_MAX).await;
            if finalize_deferred(&env2.store, &slug).await.unwrap_or(false) {
                tracing::warn!(%slug, "study turn did not end; finalizing anyway");
                let _ = spawn_finalize(&env2, &slug).await;
            }
        });
        return Ok(());
    }
    spawn_finalize(env, &study.slug).await
}

/// Start [`finalize_task`] once per study (a `finalize_started` event + an in-process guard).
async fn spawn_finalize(env: &Env, slug: &str) -> Result<()> {
    if !FINALIZING.lock().insert(key(&env.root, slug)) {
        return Ok(());
    }
    env.store
        .append_event(slug, "finalize_started", &serde_json::json!({}))
        .await?;
    let (env2, slug) = (env.clone(), slug.to_owned());
    tokio::spawn(async move {
        finalize_task(env2.clone(), slug.clone()).await;
        FINALIZING.lock().remove(&key(&env2.root, &slug));
    });
    Ok(())
}

/// §11 steps through W2's `finalize::run` (confirm, changesets, tables, the
/// knowledge record), then §11.3's report turn, the result card and the event.
pub async fn finalize_task(env: Env, slug: String) {
    // R-105: a transient store error must not leave the study `finalizing` until a restart.
    let mut study = None;
    for attempt in 1..=3u32 {
        match env.store.get_study(&slug).await {
            Ok(Some(s)) => {
                study = Some(s);
                break;
            }
            Ok(None) => {
                tracing::warn!(%slug, "finalize: study row is gone");
                return;
            }
            Err(e) => {
                tracing::warn!(%slug, attempt, "finalize: study row unreadable: {e}");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
    let Some(mut study) = study else {
        return;
    };
    let reason = study.stop_reason.clone().unwrap_or(StopReason::User);
    // R-56: §11 measures the *final incumbent*. Whatever the model edited after its last
    // experiment_log is saved as finalize/stray.patch and the tree reset before confirm.
    if let (Ok(wt), Ok(inc)) = (worktree_of(&study), incumbent_of(&study)) {
        let stray = env.study_dir(&slug).join("finalize").join("stray.patch");
        let _ = std::fs::create_dir_all(stray.parent().expect("finalize dir"));
        match env.backend.repair(&wt, &study.spec, &inc, &stray) {
            Ok(Some(what)) => {
                let _ = env
                    .store
                    .append_event(&slug, "incident", &serde_json::json!({ "what": what }))
                    .await;
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(%slug, "finalize: worktree not reset: {e}"),
        }
    }
    match env
        .backend
        .finalize(
            &env.store,
            &study,
            reason.clone(),
            &env.cfg,
            &env.root,
            &env.rt,
        )
        .await
    {
        Ok((headline, report_doc)) => {
            // R-98: `report_doc` is a document id or nothing (no documents actor:
            // the tables sit in `<study_dir>/report.md`, no turn to wrap them).
            if let Some(doc) = &report_doc
                && let Err(e) = report_turn(&env, &study, doc).await
            {
                tracing::warn!(%slug, "report prose not written (tables stand): {e}");
            }
            let p = StudyPatch {
                state: Some(StudyState::Finished),
                finished_at: Some(now()),
                report_doc: report_doc.clone(),
                ..StudyPatch::default()
            };
            match env.store.patch_study(&slug, &p).await {
                Ok(()) => {
                    p.apply_to(&mut study);
                    if let Some(s) = study.session {
                        crate::binding::unseat(s);
                    }
                    env.publish(StudyEvent::StudyChanged {
                        slug: slug.clone(),
                        state: StudyState::Finished,
                        reason: Some(headline.text.clone()),
                    })
                    .await;
                }
                Err(e) => tracing::warn!(%slug, "finished state not recorded: {e}"),
            }
            let series = chart_series(&env, &study).await.unwrap_or_default();
            let (closed, kept) = counts(&env.store, &slug).await.unwrap_or((0, 0));
            let changesets = changesets_of(&env, &slug).await;
            deliver(
                study.origin,
                &slug,
                "finished",
                result_card(&study, &headline, &series, &changesets, closed, kept),
            );
            env.publish(StudyEvent::StudyFinished {
                slug: slug.clone(),
                reason,
                headline: headline.text,
                report_doc: report_doc.unwrap_or_default(),
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
                "failed",
                format!("**Autoresearch study `{slug}` could not finalize** — {reason}"),
            );
        }
    }
    clear_phase(&env.root, &slug);
}

const REPORT_SYSTEM: &str = "You write the closing report of an automated optimisation study for the person who \
    commissioned it. Plain prose, markdown headings allowed, no tables: the harness inserts the tables with the \
    numbers after your text. Never state a number that is not in the material you were given.";

/// §11.3's report turn (R-45): the executor writes the prose around the numbers — what
/// was asked, what the headline means, what the changesets do, what needs the user's
/// eyes, how to land — from the final brief and the finalize tables; the plugin then
/// stores prose + tables as the report document. Idempotent per study (`report_prose` step).
async fn report_turn(env: &Env, study: &StudyRecord, doc: &str) -> Result<()> {
    let done = env
        .store
        .list_events(&study.slug, Some("report_prose"), 1)
        .await?;
    if !done.is_empty() {
        return Ok(());
    }
    let Some(report) = crate::finalize::report(&env.store, &study.slug).await? else {
        return Err(Error::refused("no finalize report recorded"));
    };
    let tables = report.tables();
    let (closed, _) = counts(&env.store, &study.slug).await?;
    let n = env.store.next_experiment_n(&study.slug).await?;
    let cfg = mechanism_cfg(env, study);
    let cx = brief_context(env, study, &cfg, n, Vec::new(), closed, 0.0).await?;
    let brief = env
        .backend
        .brief(&env.store, study, &cfg.brief, &cx)
        .await?;
    let user = format!(
        "The study `{}` has finished. Below are the final brief (what the loop knew) and the report tables the \
         harness computed. Write the report's prose in this order: what was asked and how it was measured; what \
         the headline means for the goal (base → final, holdout when present); what the changesets do and which \
         are worth landing alone; what needs the user's eyes (reverted or suspicious keeps, deviations, unreviewed \
         keeps, holdout gap, drift); the dead ends worth remembering; how to land (the per-changeset branches). \
         Under 600 words. Do not repeat the tables.\n\n<brief>\n{}\n</brief>\n\n<tables>\n{}\n</tables>",
        study.slug, brief.text, tables
    );
    let model = env.cfg.executor(&study.spec.models, &env.agent);
    let prose = env
        .backend
        .report_prose(study, &model, REPORT_SYSTEM, user)
        .await?;
    let prose = prose.trim();
    if prose.is_empty() {
        return Err(Error::refused("empty report prose"));
    }
    let title = tables.lines().next().unwrap_or_default();
    let body = tables
        .split_once('\n')
        .map(|(_, rest)| rest.trim_start())
        .unwrap_or("");
    let text = format!("{title}\n\n{prose}\n\n---\n\n{body}");
    std::fs::write(env.study_dir(&study.slug).join("report.md"), &text)?;
    if !doc.starts_with('/') {
        env.backend.replace_report(doc, &text).await?;
    }
    env.store
        .append_event(
            &study.slug,
            "report_prose",
            &serde_json::json!({ "bytes": prose.len(), "model": model }),
        )
        .await?;
    Ok(())
}

/// Finalize's changesets as W2 records them: the `finalize` event whose `step` is
/// `changesets` (`finalize::changesets`). 140-P live: this read `events.kind =
/// "changeset"`, which nothing writes — the result card and `StudyDetail.changesets`
/// were always empty ("No changeset cleared the gate") while the report had them.
async fn changesets_of(env: &Env, slug: &str) -> Vec<Changeset> {
    let mut out = crate::finalize::changesets(&env.store, slug)
        .await
        .unwrap_or_default();
    out.sort_by_key(|c: &Changeset| c.k);
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
        patch(
            &env,
            &mut study,
            StudyPatch {
                playbook: Some(p),
                ..StudyPatch::default()
            },
        )
        .await?;
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
    control_env(&a.env(), m).await
}

/// [`control`] on an [`Env`] (off-mailbox, R-115: it asks the agent and schedule actors
/// and waits for a cancelled run; the git writes go through `spawn_blocking`, R-100).
pub async fn control_env(env: &Env, m: Control) -> Result<StudyInfo> {
    let env = env.clone();
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
            if study.state == StudyState::Preparing {
                // R-85: the Study view offers Stop now while preparing — cancel the baseline;
                // `prepare` then ends the study `failed` with the cancellation as its reason.
                cancel(&env.root, &slug);
                let study = study_by_slug(&env.store, &slug).await?;
                return info_full(&env, &study).await;
            }
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
                // R-58: a measuring run closes itself (`interrupted`) once the bench group is
                // dead; wait for that before the reset, so no sample is still writing the tree.
                if exp.state == ExperimentState::Measuring {
                    let until = tokio::time::Instant::now() + Duration::from_secs(15);
                    while tokio::time::Instant::now() < until
                        && env
                            .store
                            .get_experiment(&slug, exp.n)
                            .await?
                            .is_some_and(|e| e.state == ExperimentState::Measuring)
                    {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
                // §4.3: `interrupted` only mid-measurement, else `abandoned` (R-66).
                let from = exp.state;
                exp.state = if from == ExperimentState::Measuring {
                    ExperimentState::Interrupted
                } else {
                    ExperimentState::Abandoned
                };
                exp.finished_at = Some(now());
                exp.line = format!("{}: stop now", exp.state.tag());
                if env.store.update_experiment_if(&exp, &[from]).await? {
                    mark_lost(&env.store, &slug, exp.n).await?;
                }
                // Reset the worktree to the incumbent now: finalize starts from a clean tree.
                if let (Ok(w), Ok(i)) = (worktree_of(&study), incumbent_of(&study)) {
                    let patch = env.runs_dir(&slug, exp.n).join("candidate.patch");
                    let _ = std::fs::create_dir_all(patch.parent().expect("runs dir"));
                    let backend = env.backend.clone();
                    let r = tokio::task::spawn_blocking(move || backend.discard(&w, &i, &patch))
                        .await
                        .map_err(|e| Error::Other(anyhow::anyhow!("discard task: {e}")))?;
                    if let Err(e) = r {
                        tracing::warn!(%slug, "worktree not reset at stop now: {e}");
                    }
                }
                clear_phase(&env.root, &slug);
            }
            begin_finalize(&env, study.clone(), StopReason::UserNow).await?;
            // 159 W6 hand-off 3: a waiting launch is closed `Abandoned` (arms rolled back) now
            // that the study is finalizing; `world::resume` closes on a finalizing study.
            // (Not inside `begin_finalize`: `resume` → `close` → `after_experiment` →
            // `begin_finalize` is a cycle of async fns rustc cannot type.)
            for e in env
                .store
                .waiting_experiments()
                .await?
                .into_iter()
                .filter(|e| e.slug == slug)
            {
                // Boxed: `resume` → `window_end` → `close` is a large future; keep `control_env` small.
                if let Err(err) = Box::pin(crate::world::resume(&env, &slug, e.n)).await {
                    tracing::warn!(%slug, n = e.n, "launch not closed at stop: {err}");
                }
            }
        }
        ControlOp::RunOnce => {
            let paused = matches!(study.state, StudyState::Paused { .. });
            if !paused && study.state != StudyState::Running {
                return Err(Error::refused(format!(
                    "cannot run a study that is {}",
                    state_word(&study.state)
                )));
            }
            // R-51: the one fire Begin accepts while paused is this event.
            env.store
                .append_event(&slug, "run_once", &serde_json::json!({ "by": m.by }))
                .await?;
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
            let (backend, repo, branch, author) = (
                env.backend.clone(),
                PathBuf::from(&study.repo),
                cs.branch.clone(),
                env.cfg.git.author.clone(),
            );
            let sha = tokio::task::spawn_blocking(move || {
                backend.merge_into_checkout(&repo, &branch, author.as_deref())
            })
            .await
            .map_err(|e| Error::Other(anyhow::anyhow!("merge task: {e}")))??;
            env.store
                .append_event(
                    &slug,
                    "merged",
                    &serde_json::json!({ "k": changeset, "branch": cs.branch, "sha": sha, "by": m.by }),
                )
                .await?;
        }
        ControlOp::Discard => {
            // 159 W6 hand-off 3: a waiting launch has arms in production; stop rolls them back.
            if crate::world::explains_disabled(&env.store, &slug).await? {
                return Err(Error::refused("a launch is live; stop the study first"));
            }
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
            // R-9: the study worktree, scratch worktrees, `autoresearch/<slug>`, its `+k`
            // changeset branches and the rejected refs — everything the study put in the repo.
            let (backend, repo, slug2, dir) = (
                env.backend.clone(),
                PathBuf::from(&study.repo),
                slug.clone(),
                env.study_dir(&slug),
            );
            match tokio::task::spawn_blocking(move || backend.discard_study(&repo, &slug2, &dir))
                .await
                .map_err(|e| Error::Other(anyhow::anyhow!("discard task: {e}")))?
            {
                Ok(removed) => {
                    env.store
                        .append_event(
                            &slug,
                            "discarded",
                            &serde_json::json!({ "removed": removed }),
                        )
                        .await?;
                }
                Err(e) => tracing::warn!(%slug, "worktrees/branches not fully removed: {e}"),
            }
            set_state(&env, &mut study, StudyState::Discarded, None).await?;
        }
    }
    let study = study_by_slug(&env.store, &slug).await?;
    info_full(&env, &study).await
}

/// What a new segment's baseline is (140 §5.3): a user-opened segment measures the
/// incumbent afresh; a regime change (Revision 2) reuses the keep's own samples.
enum SegmentBaseline {
    Measure,
    /// The keep at experiment `n` whose |Δ| ≥ `regime_sigma` × σ; `sigma` and `delta`
    /// are recorded on the `segment_auto` event.
    Keep {
        n: u32,
        sigma: f64,
        delta: f64,
    },
}

/// `Segment`: new custody + baseline on the incumbent; σ restarts (140 §5.3).
async fn segment_task(env: Env, study: StudyRecord, reason: String, by: By) {
    let slug = study.slug.clone();
    let r = open_segment(&env, study, reason, by, SegmentBaseline::Measure).await;
    segment_outcome(&env, &slug, r).await;
}

/// A segment that failed to open is an incident, never a failed call.
async fn segment_outcome(env: &Env, slug: &str, r: Result<()>) {
    if let Err(e) = r {
        tracing::warn!(%slug, "segment not opened: {e}");
        let _ = env
            .store
            .append_event(
                slug,
                "incident",
                &serde_json::json!({ "what": format!("segment failed: {e}") }),
            )
            .await;
    }
}

/// §5.3 Revision 2: a keep whose |Δ| ≥ `gate.regime_sigma` × σ moved the metric to a
/// scale where the old absolute σ is blind (140-P live: 8996 → 39 ms with σ ≈ 500 ms),
/// so the segment restarts at the keep. σ comes from the keep's own samples plus later
/// incumbent samples; while no tree has two samples, σ is 0 and the gate's cold-start
/// path (one incumbent re-measure before any judgement, R-25) applies. A deterministic
/// metric (σ = 0) has no regime.
pub(crate) async fn regime_segment(env: &Env, study: &StudyRecord, exp: &ExperimentRecord) {
    let k = env.cfg.gate.regime_sigma;
    let (Some(sigma), Some(delta)) = (study.sigma, exp.delta) else {
        return;
    };
    if k <= 0.0 || sigma <= 0.0 || delta.abs() < k * sigma {
        return;
    }
    let reason = format!(
        "regime change at #{}: Δ {} {} ≥ {k}σ (σ {})",
        exp.n,
        fmt_num(delta),
        study.spec.metric.unit,
        fmt_num(sigma)
    );
    let r = open_segment(
        env,
        study.clone(),
        reason,
        By::Plugin,
        SegmentBaseline::Keep {
            n: exp.n,
            sigma,
            delta,
        },
    )
    .await;
    segment_outcome(env, &study.slug, r).await;
}

async fn open_segment(
    env: &Env,
    mut study: StudyRecord,
    reason: String,
    by: By,
    baseline_from: SegmentBaseline,
) -> Result<()> {
    let slug = study.slug.clone();
    let segment = study.segment + 1;
    let evaluator = env.evaluator_dir(&slug, segment);
    let worktree = worktree_of(&study)?;
    // The worktree is what `custody::verify` hashes (R-7), never the user's moving checkout.
    let custody = env
        .backend
        .custody_take(&study.spec, &worktree, &evaluator)?;
    let incumbent = incumbent_of(&study)?;
    // A regime segment opens at the keep's first sample so `incumbent_samples` and
    // `segment_sigma` (both cut at `opened_at`) see the keep's own samples.
    let opened_at = match &baseline_from {
        SegmentBaseline::Measure => now(),
        SegmentBaseline::Keep { n, .. } => env
            .store
            .list_samples(&slug, Some(*n))
            .await?
            .into_iter()
            .filter(|s| s.tree_sha == incumbent)
            .map(|s| s.at)
            .min()
            .unwrap_or_else(now),
    };
    let mut seg = SegmentRecord {
        slug: slug.clone(),
        segment,
        opened_at,
        opened_by: by,
        reason: Some(reason.clone()),
        custody,
        baseline_mean: None,
        baseline_n: 0,
    };
    env.store.put_segment(&seg).await?;
    patch(
        env,
        &mut study,
        StudyPatch {
            segment: Some(segment),
            ..StudyPatch::default()
        },
    )
    .await?;
    let mut values = match &baseline_from {
        SegmentBaseline::Measure => {
            baseline(
                env,
                &study,
                &incumbent,
                0,
                SampleRole::Baseline,
                &cancel_token(&env.root, &slug),
            )
            .await?
        }
        SegmentBaseline::Keep { .. } => {
            bench_values(&incumbent_samples(env, &study, &incumbent).await?)
        }
    };
    if let SegmentBaseline::Keep { n, .. } = &baseline_from
        && values.len() < 2
    {
        // A regime keep with one clean sample (possible only below `z_suspicious`, or
        // when its confirm samples were noisy) leaves σ inestimable, and a σ of 0 would
        // read as a deterministic metric; spend the R-25 re-measure now instead of at
        // the next experiment's start.
        let worktree = worktree_of(&study)?;
        let runs = env.runs_dir(&slug, *n);
        let s = scored_sample(
            env,
            &study.spec,
            SampleRequest {
                slug: &slug,
                n: *n,
                k: 150,
                tree_sha: &incumbent,
                role: SampleRole::Incumbent,
                worktree: &worktree,
                evaluator: &env.evaluator_dir(&slug, segment),
                runs_dir: &runs,
            },
            &cancel_token(&env.root, &slug),
            None,
        )
        .await?;
        if let Some(v) = s.value
            && !s.noisy
            && !s.lost
        {
            values.push(v);
        }
    }
    let baseline_mean = if values.is_empty() {
        study.incumbent_mean
    } else {
        Some(mean(&values))
    };
    seg.baseline_mean = baseline_mean;
    seg.baseline_n = values.len() as u32;
    env.store.put_segment(&seg).await?;
    let sigma = segment_sigma(env, &study).await?;
    patch(
        env,
        &mut study,
        StudyPatch {
            incumbent_mean: Some(baseline_mean),
            sigma: Some(Some(sigma)),
            ..StudyPatch::default()
        },
    )
    .await?;
    let (kind, payload) = match baseline_from {
        SegmentBaseline::Measure => ("segment", serde_json::json!({ "segment": segment })),
        SegmentBaseline::Keep { n, sigma, delta } => (
            "segment_auto",
            serde_json::json!({ "segment": segment, "n": n, "sigma": sigma, "delta": delta, "reason": reason }),
        ),
    };
    env.store.append_event(&slug, kind, &payload).await?;
    env.publish(StudyEvent::StudyChanged {
        slug: slug.clone(),
        state: study.state.clone(),
        reason: Some(format!("segment {segment} opened")),
    })
    .await;
    Ok(())
}

// ───────────────────────────── reads ─────────────────────────────

/// `Status` → one or every `StudyInfo` (140 §12.5 `autoresearch.list`).
pub async fn status(a: &mut Studies, m: Status) -> Result<Vec<StudyInfo>> {
    status_env(&a.env(), m).await
}

/// [`status`] on an [`Env`] (off-mailbox: the spend asks the agent actor, R-115).
pub async fn status_env(env: &Env, m: Status) -> Result<Vec<StudyInfo>> {
    let studies = match m.slug {
        Some(slug) => vec![study_by_slug(&env.store, &slug).await?],
        None => env.store.list_studies().await?,
    };
    let mut out = Vec::with_capacity(studies.len());
    for s in &studies {
        out.push(info_full(env, s).await?);
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
    // Holdout: one point per run (the mean of its clean samples), not one per sample (R-89).
    // Finalize writes its holdout samples at n = 0 too (§11.1); only the base tree's belong
    // to the base point — the final tree's are the headline's (`StudyDetail.final_holdout`).
    let base_sha = study.base_sha.as_deref().unwrap_or_default();
    let mut holdout: BTreeMap<u32, (Vec<f64>, String)> = BTreeMap::new();
    for s in env.store.list_samples(&study.slug, None).await? {
        if s.role == Some(SampleRole::Holdout)
            && !s.noisy
            && !s.lost
            && (s.n > 0 || s.tree_sha == base_sha)
            && let Some(v) = s.value
        {
            let e = holdout
                .entry(s.n)
                .or_insert_with(|| (Vec::new(), s.at.clone()));
            e.0.push(v);
            e.1 = s.at;
        }
    }
    for (n, (values, at)) in holdout {
        series.holdout.push(ChartPoint {
            n,
            verdict: String::new(),
            metric: Some(mean(&values)),
            at,
        });
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
    detail_env(&a.env(), m).await
}

/// [`detail`] on an [`Env`] (off-mailbox).
pub async fn detail_env(env: &Env, m: Detail) -> Result<StudyDetail> {
    let study = study_by_slug(&env.store, &m.slug).await?;
    let info = info_full(env, &study).await?;
    let series = chart_series(env, &study).await?;
    let mut ideas = env.store.list_ideas(&study.slug, None).await?;
    ideas.sort_by_key(|i| {
        (
            i.status != IdeaStatus::Open,
            std::cmp::Reverse(i.priority),
            i.id,
        )
    });
    let steers = env.store.list_steers(&study.slug, false).await?;
    let changesets = changesets_of(env, &study.slug).await;
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
                needs_eyes.push(format!("#{} reverted by review — {reason}", e.n));
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
        let p = |k: &str| e.payload.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        match e.kind.as_str() {
            "overfit" => needs_eyes.push(format!("holdout says overfit ({})", e.at)),
            "drift" => needs_eyes.push(format!(
                "incumbent drift at experiment {} ({})",
                p("n"),
                e.at
            )),
            // §5.3 Revision 2: the Study view says the segment opened itself and why.
            "segment_auto" => needs_eyes.push(format!(
                "segment {} opened automatically at #{} — {}; σ restarted from the keep's samples",
                p("segment"),
                p("n"),
                e.payload
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("regime change")
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
    // §11.1: finalize's holdout line (`61.0 → 49.7 (−18.5 %)`) for the Study view's summary
    // once the study is finished — its samples sit at n = 0 beside the base's (Revision 2).
    let final_holdout = if is_terminal(&study.state) {
        crate::finalize::report(&env.store, &study.slug)
            .await
            .ok()
            .flatten()
            .and_then(|r| r.headline.holdout)
    } else {
        None
    };
    let view = kind_view(env, &study).await?;
    // 159 W5: the citation of every literature idea.
    let mut citations = Vec::new();
    for i in ideas.iter().filter(|i| i.source == IdeaSource::Literature) {
        if let Some(c) = env.store.citations_get(i.id).await? {
            citations.push(c);
        }
    }
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
        final_holdout,
        view,
        citations,
    })
}

/// 159 §18.3 `kind_view`: the Study view's per-kind data, by `StudySpec::kind`. `one metric`
/// is `Scalar` (the chart series as today); every other kind asks its module.
async fn kind_view(env: &Env, study: &StudyRecord) -> Result<KindView> {
    let store = &env.store;
    match study.spec.kind() {
        KIND_SCALAR => Ok(KindView::Scalar),
        KIND_PARETO => crate::pareto::view(store, study).await, // 159:W1
        KIND_PROMPT => crate::cases::view(store, study).await,  // 159:W2
        KIND_JUDGED => crate::judge::view(store, study).await,  // 159:W3
        KIND_POPULATION => crate::population::view(store, study).await, // 159:W4
        KIND_WORLD => crate::world::view(store, study).await,   // 159:W6
        KIND_META => crate::suite::view(store, study).await,    // 159:W7
        _ => Ok(KindView::Scalar),
    }
}

/// `Experiments` → rows newest first (140 §12.5).
pub async fn experiments(a: &mut Studies, m: Experiments) -> Result<Vec<ExperimentRow>> {
    experiments_env(&a.env(), m).await
}

/// [`experiments`] on an [`Env`].
pub async fn experiments_env(env: &Env, m: Experiments) -> Result<Vec<ExperimentRow>> {
    study_by_slug(&env.store, &m.slug).await?;
    Ok(env
        .store
        .list_experiments(&m.slug, m.before, m.limit.clamp(1, 1000))
        .await?
        .iter()
        .map(ExperimentRecord::row)
        .collect())
}

/// `Experiment` → `ExperimentDetail` (140 §12.5).
pub async fn experiment(a: &mut Studies, m: Experiment) -> Result<ExperimentDetail> {
    experiment_env(&a.env(), m).await
}

/// [`experiment`] on an [`Env`] (off-mailbox: it reads the diff through git, R-100).
pub async fn experiment_env(env: &Env, m: Experiment) -> Result<ExperimentDetail> {
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
    let inc = incumbent_samples(env, &study, &e.parent_sha).await?;
    // 159 W5: the hook outputs of this experiment (`hook_before` then `hook_after`).
    let mut hooks = Vec::new();
    for kind in [crate::hooks::EVENT_BEFORE, crate::hooks::EVENT_AFTER] {
        for ev in env.store.list_events(&m.slug, Some(kind), 10_000).await? {
            if ev.payload.get("n").and_then(|v| v.as_u64()) == Some(u64::from(m.n))
                && let Some(out) = ev.payload.get("output").and_then(|v| v.as_str())
            {
                hooks.push(out.to_owned());
            }
        }
    }
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
        hooks,
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
                    let from = exp.state;
                    // §4.3: a restart mid-measurement is `interrupted`; an open/measured row
                    // whose turn died is `abandoned`.
                    exp.state = if from == ExperimentState::Measuring {
                        ExperimentState::Interrupted
                    } else {
                        ExperimentState::Abandoned
                    };
                    exp.finished_at = Some(now());
                    exp.line = format!("{}: engine restart", exp.state.tag());
                    env.store.update_experiment_if(&exp, &[from]).await?;
                    mark_lost(&env.store, &slug, exp.n).await?;
                    env.store
                        .append_event(&slug, "incident", &serde_json::json!({ "n": exp.n, "what": format!("{} by an engine restart", exp.state.tag()) }))
                        .await?;
                    // R-65: the same close-out as a turn end — event and stop rules.
                    env.publish(StudyEvent::ExperimentFinished {
                        slug: slug.clone(),
                        n: exp.n,
                        verdict: exp.verdict.clone().unwrap_or(Verdict::Refused {
                            reason: exp.state.tag().to_owned(),
                        }),
                        metric: exp.metric,
                        delta_pct: exp.delta_pct,
                        z: exp.z,
                    })
                    .await;
                    if let Err(e) = after_experiment(env, &slug).await {
                        tracing::warn!(%slug, "stop rules after recovery: {e}");
                    }
                }
                // 140-P live: 109's durability resumes the turn the crash interrupted — with
                // none of the fire's in-memory settings (toolset, approvals off): the model
                // went on unconfined (`python`, knowledge hints, 7 experiments in one turn).
                // §8.5 wants a fresh fire instead: re-apply the toolset (a resume that has not
                // started yet reads it at its rebase), then cancel the resumed turn; the
                // schedule closes its run and `every: 0s` fires again, confined.
                if let Some(session) = study.session
                    && let Some(agents) = agents()
                {
                    if let Some(w) = study.worktree.as_deref() {
                        crate::binding::seat(session, Path::new(w));
                    }
                    let _ = agents
                        .ask(agent::SetToolset {
                            session,
                            toolset: Some(crate::binding::toolset(study.spec.parallel)),
                        })
                        .await;
                    let _ = agents
                        .ask(agent::Cancel {
                            session,
                            keep: None,
                            scope: agent::CancelScope::Tree,
                        })
                        .await;
                }
                // 140-P live: a loop the schedule ended (or a file removed by hand) left a
                // live study with no file — nothing would ever fire again. Re-write it.
                let file = env
                    .data_dir
                    .join(schedule::DIR)
                    .join(format!("{}.md", schedule_file::name(&slug)));
                if !file.exists() {
                    if let Err(e) = rewrite_schedule_file(env, &study).await {
                        tracing::warn!(%slug, "schedule file not restored: {e}");
                    } else {
                        env.store
                            .append_event(&slug, "incident", &serde_json::json!({ "what": "schedule file missing at restart; re-written" }))
                            .await?;
                    }
                }
            }
            StudyState::Finalizing => spawn_finalize(env, &slug).await?,
            _ => {}
        }
    }
    crate::world::rearm(env).await?; // 159:W6
    Ok(())
}

/// The study's schedule file as `prepare` writes it, `enabled:` per the study state.
async fn rewrite_schedule_file(env: &Env, study: &StudyRecord) -> Result<()> {
    let worktree = worktree_of(study)?;
    let start = match &study.spec.window {
        Some(sensor) => StartWhen::Window {
            sensor: sensor.clone(),
        },
        None => StartWhen::Now,
    };
    let model = env.cfg.executor(&study.spec.models, &env.agent);
    let mut text = schedule_file::text(&study.spec, &env.cfg, &worktree, &model, &start);
    if matches!(study.state, StudyState::Paused { .. }) {
        text = schedule::file::set_enabled(&text, false);
    }
    schedule_file::write(&env.data_dir, &study.slug, &text).await
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
            context: None,
            priority: None,
            rank: 5,
            alert: None,
        };
        let mut out = Vec::new();
        let Ok(studies) = self.0.list_studies().await else {
            return out;
        };
        for s in studies {
            match &s.state {
                StudyState::Finished => {
                    // Revision 2's one Δ convention: raw change, never direction-normalised
                    // (140-P2 live: the row read `+100.0 %` for 7356 → 3.1 ms).
                    let delta = match (s.incumbent_mean, self.0.base_mean(&s.slug).await) {
                        (Some(i), Ok(Some(b))) => fmt_pct(pct(i - b, b)),
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
            if is_terminal(&s.state) {
                continue;
            }
            // 159 W3: a blind pair waiting for the user's answer (`autoresearch.human_pair`;
            // the Study view's PairViewer shows the two texts).
            for j in self
                .0
                .judgments_list(&s.slug, None)
                .await
                .unwrap_or_default()
                .iter()
                .filter(|j| j.model == "human" && !j.human)
            {
                out.push(item(
                    format!("autoresearch:pair:{}:{}", s.slug, j.id),
                    AttentionKind::Blocked,
                    format!("Study {}: which blurb is better? (pair {})", s.slug, j.id),
                    Some(format!("case {}", j.case_id)),
                    &j.at,
                    s.origin,
                ));
            }
            // 159 W6: a launch waiting for the press (`autoresearch.launch` with the card's token;
            // the row never carries it — `LaunchRecord.token_hash` stays out).
            for e in self
                .0
                .waiting_experiments()
                .await
                .unwrap_or_default()
                .iter()
                .filter(|e| e.slug == s.slug)
            {
                if let Ok(Some(l)) = self.0.launches_get(&s.slug, e.n).await
                    && l.state == "requested"
                {
                    out.push(item(
                        format!("autoresearch:launch:{}:{}", s.slug, l.n),
                        AttentionKind::Blocked,
                        format!(
                            "Study {}: launch #{} needs your press · exposure {:.0} %",
                            s.slug,
                            l.n,
                            l.exposure * 100.0
                        ),
                        None,
                        &l.requested_at,
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
