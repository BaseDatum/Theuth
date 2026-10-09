//! `<data_dir>/autoresearch/autoresearch.sqlite` (design doc 140 §12.6): the
//! ledger every module writes before and after each step (§8.5; WAL,
//! `synchronous=NORMAL`). The schema is `migrations/0001_init.sql`, run by
//! `sqlx::migrate!` (its `_sqlx_migrations` table is the migrations table).
//!
//! This file holds the typed accessors every worker needs. A worker that
//! needs another query writes it in its own module against [`Store::pool`]
//! (`&SqlitePool`); nobody edits this file after the skeleton commit.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};
use sqlx::{FromRow, Row};

use crate::error::{Error, Result};
use crate::types::{
    By, Decision, ExperimentRow, ExperimentState, Idea, IdeaSource, IdeaStatus, PauseReason,
    ReviewVerdict, Sample, SampleRole, Steer, StopReason, StudySpec, StudyState, Verdict,
};

/// File name under the root.
pub const FILE: &str = "autoresearch.sqlite";

/// The open ledger.
#[derive(Clone, Debug)]
pub struct Store {
    pool: SqlitePool,
}

/// RFC 3339 now (UTC, seconds).
pub fn now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn to_json<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "null".into())
}

fn from_json<T: DeserializeOwned>(s: Option<String>) -> Option<T> {
    serde_json::from_str(s.as_deref()?).ok()
}

/// serde's string form of a unit-like enum (`"crash_streak"`), without quotes.
fn tag<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn untag<T: DeserializeOwned>(s: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(s.to_owned())).ok()
}

// ───────────────────────────── rows ─────────────────────────────

/// One `studies` row (140 §12.6).
#[derive(Clone, Debug, PartialEq)]
pub struct StudyRecord {
    /// PK.
    pub slug: String,
    /// Lifecycle.
    pub state: StudyState,
    /// The approved spec.
    pub spec: StudySpec,
    /// Repo path (denormalised from the spec).
    pub repo: String,
    /// Resolved at Start.
    pub base_sha: Option<String>,
    /// `autoresearch/<slug>`.
    pub branch: Option<String>,
    /// The study worktree.
    pub worktree: Option<String>,
    /// Schedule file stem.
    pub schedule: Option<String>,
    /// The bound study session.
    pub session: Option<runtime::SessionId>,
    /// The chat the draft card lived in.
    pub origin: Option<runtime::SessionId>,
    /// Current incumbent.
    pub incumbent_sha: Option<String>,
    /// Its mean.
    pub incumbent_mean: Option<f64>,
    /// Pooled σ of the current segment.
    pub sigma: Option<f64>,
    /// Current segment.
    pub segment: u32,
    /// The playbook.
    pub playbook: String,
    /// RFC 3339.
    pub created_at: String,
    /// RFC 3339.
    pub started_at: Option<String>,
    /// Time paused, for `budget.hours`.
    pub paused_ms: u64,
    /// RFC 3339.
    pub finished_at: Option<String>,
    /// Why it stopped.
    pub stop_reason: Option<StopReason>,
    /// Report document id.
    pub report_doc: Option<String>,
    /// Who created it.
    pub created_by: Option<String>,
}

impl StudyRecord {
    /// A fresh `draft` row for `spec`.
    pub fn new(spec: StudySpec, origin: Option<runtime::SessionId>) -> Self {
        Self {
            slug: spec.slug.clone(),
            state: StudyState::Draft,
            repo: spec.repo.clone(),
            spec,
            base_sha: None,
            branch: None,
            worktree: None,
            schedule: None,
            session: None,
            origin,
            incumbent_sha: None,
            incumbent_mean: None,
            sigma: None,
            segment: 0,
            playbook: String::new(),
            created_at: now(),
            started_at: None,
            paused_ms: 0,
            finished_at: None,
            stop_reason: None,
            report_doc: None,
            created_by: None,
        }
    }
}

/// Column-scoped study update (R-4): off-mailbox handlers patch the fields
/// they own instead of writing back a whole row loaded minutes earlier.
/// `None` = leave the column alone; the `Option<Option<_>>` columns can be
/// set to NULL with `Some(None)`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StudyPatch {
    /// Lifecycle (with its reason).
    pub state: Option<StudyState>,
    /// Resolved base.
    pub base_sha: Option<String>,
    /// `autoresearch/<slug>`.
    pub branch: Option<String>,
    /// The study worktree.
    pub worktree: Option<String>,
    /// The bound session.
    pub session: Option<Option<runtime::SessionId>>,
    /// Current incumbent.
    pub incumbent_sha: Option<String>,
    /// Its mean.
    pub incumbent_mean: Option<Option<f64>>,
    /// Segment σ.
    pub sigma: Option<Option<f64>>,
    /// Current segment.
    pub segment: Option<u32>,
    /// The playbook.
    pub playbook: Option<String>,
    /// RFC 3339.
    pub started_at: Option<String>,
    /// Time paused.
    pub paused_ms: Option<u64>,
    /// RFC 3339.
    pub finished_at: Option<String>,
    /// Why it stopped.
    pub stop_reason: Option<StopReason>,
    /// Report document id.
    pub report_doc: Option<String>,
}

impl StudyPatch {
    /// Only the state.
    pub fn state(state: StudyState) -> Self {
        Self {
            state: Some(state),
            ..Self::default()
        }
    }

    /// Incumbent sha + mean (+ σ): the keep / revert / re-measure write.
    pub fn incumbent(sha: &str, mean: Option<f64>, sigma: Option<f64>) -> Self {
        Self {
            incumbent_sha: Some(sha.to_owned()),
            incumbent_mean: Some(mean),
            sigma: Some(sigma),
            ..Self::default()
        }
    }

    /// Apply the patch to an in-memory record (keeps a handler's copy in step with the row).
    pub fn apply_to(&self, s: &mut StudyRecord) {
        if let Some(v) = &self.state {
            s.state = v.clone();
        }
        if let Some(v) = &self.base_sha {
            s.base_sha = Some(v.clone());
        }
        if let Some(v) = &self.branch {
            s.branch = Some(v.clone());
        }
        if let Some(v) = &self.worktree {
            s.worktree = Some(v.clone());
        }
        if let Some(v) = self.session {
            s.session = v;
        }
        if let Some(v) = &self.incumbent_sha {
            s.incumbent_sha = Some(v.clone());
        }
        if let Some(v) = self.incumbent_mean {
            s.incumbent_mean = v;
        }
        if let Some(v) = self.sigma {
            s.sigma = v;
        }
        if let Some(v) = self.segment {
            s.segment = v;
        }
        if let Some(v) = &self.playbook {
            s.playbook = v.clone();
        }
        if let Some(v) = &self.started_at {
            s.started_at = Some(v.clone());
        }
        if let Some(v) = self.paused_ms {
            s.paused_ms = v;
        }
        if let Some(v) = &self.finished_at {
            s.finished_at = Some(v.clone());
        }
        if let Some(v) = &self.stop_reason {
            s.stop_reason = Some(v.clone());
        }
        if let Some(v) = &self.report_doc {
            s.report_doc = Some(v.clone());
        }
    }
}

#[derive(FromRow)]
struct StudyRaw {
    slug: String,
    state: String,
    state_reason: Option<String>,
    spec: String,
    repo: String,
    base_sha: Option<String>,
    branch: Option<String>,
    worktree: Option<String>,
    schedule: Option<String>,
    session: Option<String>,
    origin: Option<String>,
    incumbent_sha: Option<String>,
    incumbent_mean: Option<f64>,
    sigma: Option<f64>,
    segment: i64,
    playbook: String,
    created_at: String,
    started_at: Option<String>,
    paused_ms: i64,
    finished_at: Option<String>,
    stop_reason: Option<String>,
    report_doc: Option<String>,
    created_by: Option<String>,
}

fn split_state(state: &StudyState) -> (&'static str, Option<String>) {
    match state {
        StudyState::Paused { reason } => ("paused", Some(to_json(reason))),
        StudyState::Failed { reason } => ("failed", Some(reason.clone())),
        s => (s.tag(), None),
    }
}

fn join_state(tag: &str, reason: Option<String>) -> StudyState {
    match tag {
        "draft" => StudyState::Draft,
        "preparing" => StudyState::Preparing,
        "running" => StudyState::Running,
        "paused" => StudyState::Paused {
            reason: from_json::<PauseReason>(reason).unwrap_or(PauseReason::User),
        },
        "finalizing" => StudyState::Finalizing,
        "finished" => StudyState::Finished,
        "failed" => StudyState::Failed {
            reason: reason.unwrap_or_default(),
        },
        _ => StudyState::Discarded,
    }
}

fn session(s: Option<String>) -> Option<runtime::SessionId> {
    s.and_then(|s| s.parse().ok())
}

impl From<StudyRaw> for StudyRecord {
    fn from(r: StudyRaw) -> Self {
        Self {
            slug: r.slug,
            state: join_state(&r.state, r.state_reason),
            spec: serde_json::from_str(&r.spec).unwrap_or_default(),
            repo: r.repo,
            base_sha: r.base_sha,
            branch: r.branch,
            worktree: r.worktree,
            schedule: r.schedule,
            session: session(r.session),
            origin: session(r.origin),
            incumbent_sha: r.incumbent_sha,
            incumbent_mean: r.incumbent_mean,
            sigma: r.sigma,
            segment: r.segment as u32,
            playbook: r.playbook,
            created_at: r.created_at,
            started_at: r.started_at,
            paused_ms: r.paused_ms as u64,
            finished_at: r.finished_at,
            stop_reason: from_json(r.stop_reason),
            report_doc: r.report_doc,
            created_by: r.created_by,
        }
    }
}

/// One `segments` row (140 §5.3).
#[derive(Clone, Debug, PartialEq)]
pub struct SegmentRecord {
    /// The study.
    pub slug: String,
    /// Segment number (0 at Start).
    pub segment: u32,
    /// RFC 3339.
    pub opened_at: String,
    /// Who opened it.
    pub opened_by: By,
    /// Why.
    pub reason: Option<String>,
    /// `{path: sha256}` of the custody copies.
    pub custody: BTreeMap<String, String>,
    /// Baseline mean of this segment.
    pub baseline_mean: Option<f64>,
    /// Baseline samples.
    pub baseline_n: u32,
}

#[derive(FromRow)]
struct SegmentRaw {
    slug: String,
    segment: i64,
    opened_at: String,
    opened_by: String,
    reason: Option<String>,
    custody: String,
    baseline_mean: Option<f64>,
    baseline_n: i64,
}

impl From<SegmentRaw> for SegmentRecord {
    fn from(r: SegmentRaw) -> Self {
        Self {
            slug: r.slug,
            segment: r.segment as u32,
            opened_at: r.opened_at,
            opened_by: untag(&r.opened_by).unwrap_or_default(),
            reason: r.reason,
            custody: serde_json::from_str(&r.custody).unwrap_or_default(),
            baseline_mean: r.baseline_mean,
            baseline_n: r.baseline_n as u32,
        }
    }
}

/// One `experiments` row (140 §12.6) — the full column set; [`ExperimentRow`] is its wire view.
#[derive(Clone, Debug, PartialEq)]
pub struct ExperimentRecord {
    /// The study.
    pub slug: String,
    /// Number.
    pub n: u32,
    /// Wave (fire) number.
    pub wave: Option<u32>,
    /// Wave slot.
    pub slot: Option<u8>,
    /// Segment.
    pub segment: u32,
    /// Incumbent sha when it opened.
    pub parent_sha: String,
    /// State.
    pub state: ExperimentState,
    /// Verdict.
    pub verdict: Option<Verdict>,
    /// From `experiment_run`.
    pub idea: String,
    /// From `experiment_run`.
    pub category: String,
    /// From `experiment_run`.
    pub hypothesis: String,
    /// From `experiment_log`.
    pub description: String,
    /// From `experiment_log`.
    pub learned: String,
    /// From `experiment_log`.
    pub next: Option<String>,
    /// From `experiment_log`.
    pub decision: Option<Decision>,
    /// From `experiment_log`.
    pub justification: Option<String>,
    /// Paths outside scope.
    pub deviations: Vec<String>,
    /// Direction-normalised Δ.
    pub delta: Option<f64>,
    /// Δ %.
    pub delta_pct: Option<f64>,
    /// z.
    pub z: Option<f64>,
    /// Samples spent.
    pub n_samples: u32,
    /// Candidate mean.
    pub metric: Option<f64>,
    /// Keep commit.
    pub commit_sha: Option<String>,
    /// `runs/<n>/candidate.patch`.
    pub patch_path: Option<String>,
    /// Normalised patch hash.
    pub patch_hash: Option<String>,
    /// Review.
    pub review: Option<ReviewVerdict>,
    /// The verdict line.
    pub line: String,
    /// Model cost, nano-USD.
    pub cost_nanos: u64,
    /// RFC 3339.
    pub started_at: String,
    /// RFC 3339.
    pub finished_at: Option<String>,
}

impl ExperimentRecord {
    /// A fresh `open` row.
    pub fn open(slug: &str, n: u32, segment: u32, parent_sha: &str) -> Self {
        Self {
            slug: slug.to_owned(),
            n,
            wave: None,
            slot: None,
            segment,
            parent_sha: parent_sha.to_owned(),
            state: ExperimentState::Open,
            verdict: None,
            idea: String::new(),
            category: String::new(),
            hypothesis: String::new(),
            description: String::new(),
            learned: String::new(),
            next: None,
            decision: None,
            justification: None,
            deviations: Vec::new(),
            delta: None,
            delta_pct: None,
            z: None,
            n_samples: 0,
            metric: None,
            commit_sha: None,
            patch_path: None,
            patch_hash: None,
            review: None,
            line: String::new(),
            cost_nanos: 0,
            started_at: now(),
            finished_at: None,
        }
    }

    /// The wire row (140 §12.5 `autoresearch.experiments`).
    pub fn row(&self) -> ExperimentRow {
        ExperimentRow {
            slug: self.slug.clone(),
            n: self.n,
            wave: self.wave,
            slot: self.slot,
            segment: self.segment,
            state: Some(self.state),
            verdict: self.verdict.clone(),
            idea: self.idea.clone(),
            category: self.category.clone(),
            metric: self.metric,
            delta: self.delta,
            delta_pct: self.delta_pct,
            z: self.z,
            n_samples: self.n_samples,
            commit_sha: self.commit_sha.clone(),
            deviations: self.deviations.clone(),
            review: self.review.clone(),
            cost_usd: self.cost_nanos as f64 / 1e9,
            started_at: self.started_at.clone(),
            finished_at: self.finished_at.clone(),
        }
    }
}

#[derive(FromRow)]
struct ExperimentRaw {
    slug: String,
    n: i64,
    wave: Option<i64>,
    slot: Option<i64>,
    segment: i64,
    parent_sha: String,
    state: String,
    verdict: Option<String>,
    idea: String,
    category: String,
    hypothesis: String,
    description: String,
    learned: String,
    next: Option<String>,
    decision: Option<String>,
    justification: Option<String>,
    deviations: String,
    delta: Option<f64>,
    delta_pct: Option<f64>,
    z: Option<f64>,
    n_samples: i64,
    metric: Option<f64>,
    commit_sha: Option<String>,
    patch_path: Option<String>,
    patch_hash: Option<String>,
    review: Option<String>,
    line: String,
    cost_nanos: i64,
    started_at: String,
    finished_at: Option<String>,
}

impl From<ExperimentRaw> for ExperimentRecord {
    fn from(r: ExperimentRaw) -> Self {
        Self {
            slug: r.slug,
            n: r.n as u32,
            wave: r.wave.map(|w| w as u32),
            slot: r.slot.map(|s| s as u8),
            segment: r.segment as u32,
            parent_sha: r.parent_sha,
            state: untag(&r.state).unwrap_or(ExperimentState::Interrupted),
            verdict: from_json(r.verdict),
            idea: r.idea,
            category: r.category,
            hypothesis: r.hypothesis,
            description: r.description,
            learned: r.learned,
            next: r.next,
            decision: r.decision.as_deref().and_then(untag),
            justification: r.justification,
            deviations: serde_json::from_str(&r.deviations).unwrap_or_default(),
            delta: r.delta,
            delta_pct: r.delta_pct,
            z: r.z,
            n_samples: r.n_samples as u32,
            metric: r.metric,
            commit_sha: r.commit_sha,
            patch_path: r.patch_path,
            patch_hash: r.patch_hash,
            review: from_json(r.review),
            line: r.line,
            cost_nanos: r.cost_nanos as u64,
            started_at: r.started_at,
            finished_at: r.finished_at,
        }
    }
}

#[derive(FromRow)]
struct SampleRaw {
    n: i64,
    k: i64,
    tree_sha: String,
    role: Option<String>,
    value: Option<f64>,
    secondaries: String,
    cases: String,
    asi: String,
    exit: Option<i64>,
    wall_ms: i64,
    load: Option<f64>,
    noisy: i64,
    lost: i64,
    log_path: Option<String>,
    at: String,
    slot_holders: String,
    failure: Option<String>,
}

impl From<SampleRaw> for Sample {
    fn from(r: SampleRaw) -> Self {
        Self {
            n: r.n as u32,
            k: r.k as u32,
            tree_sha: r.tree_sha,
            role: r.role.as_deref().and_then(untag::<SampleRole>),
            value: r.value,
            secondaries: serde_json::from_str(&r.secondaries).unwrap_or_default(),
            cases: serde_json::from_str(&r.cases).unwrap_or_default(),
            asi: serde_json::from_str(&r.asi).unwrap_or_default(),
            exit: r.exit.map(|e| e as i32),
            wall_ms: r.wall_ms as u64,
            load: r.load,
            noisy: r.noisy != 0,
            lost: r.lost != 0,
            log_path: r.log_path,
            at: r.at,
            slot_holders: serde_json::from_str(&r.slot_holders).unwrap_or_default(),
            failure: r.failure,
        }
    }
}

#[derive(FromRow)]
struct IdeaRaw {
    id: i64,
    slug: String,
    text: String,
    category: String,
    source: String,
    priority: i64,
    status: String,
    tried_in: String,
    at: String,
}

impl From<IdeaRaw> for Idea {
    fn from(r: IdeaRaw) -> Self {
        Self {
            id: r.id,
            slug: r.slug,
            text: r.text,
            category: r.category,
            source: untag::<IdeaSource>(&r.source).unwrap_or_default(),
            priority: r.priority.clamp(0, 3) as u8,
            status: untag::<IdeaStatus>(&r.status).unwrap_or_default(),
            tried_in: serde_json::from_str(&r.tried_in).unwrap_or_default(),
            at: r.at,
        }
    }
}

#[derive(FromRow)]
struct SteerRaw {
    id: i64,
    slug: String,
    text: String,
    by: String,
    at: String,
    seen_in: Option<i64>,
}

impl From<SteerRaw> for Steer {
    fn from(r: SteerRaw) -> Self {
        Self {
            id: r.id,
            slug: r.slug,
            text: r.text,
            by: untag::<By>(&r.by).unwrap_or_default(),
            at: r.at,
            seen_in: r.seen_in.map(|n| n as u32),
        }
    }
}

/// One `events` row (incidents, drift, overfit, review, stop, finalize steps).
#[derive(Clone, Debug, PartialEq)]
pub struct EventRecord {
    /// Row id.
    pub id: i64,
    /// The study.
    pub slug: String,
    /// Kind.
    pub kind: String,
    /// JSON payload.
    pub payload: serde_json::Value,
    /// RFC 3339.
    pub at: String,
}

// ───────────────────────────── the store ─────────────────────────────

const STUDY_COLS: &str = "slug, state, state_reason, spec, repo, base_sha, branch, worktree, schedule, session, origin, \
    incumbent_sha, incumbent_mean, sigma, segment, playbook, created_at, started_at, paused_ms, finished_at, \
    stop_reason, report_doc, created_by";

const EXPERIMENT_COLS: &str = "slug, n, wave, slot, segment, parent_sha, state, verdict, idea, category, hypothesis, \
    description, learned, next, decision, justification, deviations, delta, delta_pct, z, n_samples, metric, \
    commit_sha, patch_path, patch_hash, review, line, cost_nanos, started_at, finished_at";

const SAMPLE_COLS: &str = "n, k, tree_sha, role, value, secondaries, cases, asi, exit, wall_ms, load, noisy, lost, log_path, at, slot_holders, failure";

type Q = sqlx::QueryBuilder<sqlx::Sqlite>;

/// `AND state IN (…)` when `from` is non-empty.
fn push_state_filter(q: &mut Q, from: &[ExperimentState]) {
    if !from.is_empty() {
        q.push(" AND state IN (");
        let mut sep = q.separated(", ");
        for s in from {
            sep.push_bind(tag(s));
        }
        q.push(")");
    }
}

/// `UPDATE studies SET <p's columns> WHERE slug = ? [AND state IN from]`.
fn study_update(slug: &str, from: &[&str], p: &StudyPatch) -> Q {
    let mut q = Q::new("UPDATE studies SET slug = slug");
    if let Some(state) = &p.state {
        let (tag, reason) = split_state(state);
        q.push(", state = ").push_bind(tag);
        q.push(", state_reason = ").push_bind(reason);
    }
    if let Some(v) = &p.base_sha {
        q.push(", base_sha = ").push_bind(v.clone());
    }
    if let Some(v) = &p.branch {
        q.push(", branch = ").push_bind(v.clone());
    }
    if let Some(v) = &p.worktree {
        q.push(", worktree = ").push_bind(v.clone());
    }
    if let Some(v) = p.session {
        q.push(", session = ").push_bind(v.map(|s| s.to_string()));
    }
    if let Some(v) = &p.incumbent_sha {
        q.push(", incumbent_sha = ").push_bind(v.clone());
    }
    if let Some(v) = p.incumbent_mean {
        q.push(", incumbent_mean = ").push_bind(v);
    }
    if let Some(v) = p.sigma {
        q.push(", sigma = ").push_bind(v);
    }
    if let Some(v) = p.segment {
        q.push(", segment = ").push_bind(i64::from(v));
    }
    if let Some(v) = &p.playbook {
        q.push(", playbook = ").push_bind(v.clone());
    }
    if let Some(v) = &p.started_at {
        q.push(", started_at = ").push_bind(v.clone());
    }
    if let Some(v) = p.paused_ms {
        q.push(", paused_ms = ").push_bind(v as i64);
    }
    if let Some(v) = &p.finished_at {
        q.push(", finished_at = ").push_bind(v.clone());
    }
    if let Some(v) = &p.stop_reason {
        q.push(", stop_reason = ").push_bind(to_json(v));
    }
    if let Some(v) = &p.report_doc {
        q.push(", report_doc = ").push_bind(v.clone());
    }
    q.push(" WHERE slug = ").push_bind(slug.to_owned());
    if !from.is_empty() {
        q.push(" AND state IN (");
        let mut sep = q.separated(", ");
        for s in from {
            sep.push_bind((*s).to_owned());
        }
        q.push(")");
    }
    q
}

/// `UPDATE experiments SET <every column> WHERE slug = ? AND n = ? [AND state IN from]`.
fn experiment_update(e: &ExperimentRecord, from: &[ExperimentState]) -> Q {
    let mut q = Q::new("UPDATE experiments SET wave = ");
    q.push_bind(e.wave.map(i64::from));
    q.push(", slot = ").push_bind(e.slot.map(i64::from));
    q.push(", segment = ").push_bind(i64::from(e.segment));
    q.push(", parent_sha = ").push_bind(e.parent_sha.clone());
    q.push(", state = ").push_bind(tag(&e.state));
    q.push(", verdict = ")
        .push_bind(e.verdict.as_ref().map(to_json));
    q.push(", idea = ").push_bind(e.idea.clone());
    q.push(", category = ").push_bind(e.category.clone());
    q.push(", hypothesis = ").push_bind(e.hypothesis.clone());
    q.push(", description = ").push_bind(e.description.clone());
    q.push(", learned = ").push_bind(e.learned.clone());
    q.push(", next = ").push_bind(e.next.clone());
    q.push(", decision = ")
        .push_bind(e.decision.as_ref().map(tag));
    q.push(", justification = ")
        .push_bind(e.justification.clone());
    q.push(", deviations = ").push_bind(to_json(&e.deviations));
    q.push(", delta = ").push_bind(e.delta);
    q.push(", delta_pct = ").push_bind(e.delta_pct);
    q.push(", z = ").push_bind(e.z);
    q.push(", n_samples = ").push_bind(i64::from(e.n_samples));
    q.push(", metric = ").push_bind(e.metric);
    q.push(", commit_sha = ").push_bind(e.commit_sha.clone());
    q.push(", patch_path = ").push_bind(e.patch_path.clone());
    q.push(", patch_hash = ").push_bind(e.patch_hash.clone());
    q.push(", review = ")
        .push_bind(e.review.as_ref().map(to_json));
    q.push(", line = ").push_bind(e.line.clone());
    q.push(", cost_nanos = ").push_bind(e.cost_nanos as i64);
    q.push(", started_at = ").push_bind(e.started_at.clone());
    q.push(", finished_at = ").push_bind(e.finished_at.clone());
    q.push(" WHERE slug = ").push_bind(e.slug.clone());
    q.push(" AND n = ").push_bind(i64::from(e.n));
    push_state_filter(&mut q, from);
    q
}

impl Store {
    /// Open or create `<root>/autoresearch.sqlite` and run the migrations.
    pub async fn open(root: &Path) -> Result<Self> {
        std::fs::create_dir_all(root)?;
        Self::open_file(&root.join(FILE)).await
    }

    /// Open a specific file (tests).
    pub async fn open_file(path: &Path) -> Result<Self> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .map_err(|e| Error::Other(anyhow::anyhow!("migrate autoresearch.sqlite: {e}")))?;
        Ok(Self { pool })
    }

    /// The pool, for module-local queries.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    // ── studies ──

    /// Insert a study; the slug must be new.
    pub async fn create_study(&self, s: &StudyRecord) -> Result<()> {
        let (state, reason) = split_state(&s.state);
        sqlx::query(
            "INSERT INTO studies (slug, state, state_reason, spec, repo, base_sha, branch, worktree, schedule, session, \
             origin, incumbent_sha, incumbent_mean, sigma, segment, playbook, created_at, started_at, paused_ms, \
             finished_at, stop_reason, report_doc, created_by) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
        )
        .bind(&s.slug)
        .bind(state)
        .bind(reason)
        .bind(to_json(&s.spec))
        .bind(&s.repo)
        .bind(&s.base_sha)
        .bind(&s.branch)
        .bind(&s.worktree)
        .bind(&s.schedule)
        .bind(s.session.map(|s| s.to_string()))
        .bind(s.origin.map(|s| s.to_string()))
        .bind(&s.incumbent_sha)
        .bind(s.incumbent_mean)
        .bind(s.sigma)
        .bind(i64::from(s.segment))
        .bind(&s.playbook)
        .bind(&s.created_at)
        .bind(&s.started_at)
        .bind(s.paused_ms as i64)
        .bind(&s.finished_at)
        .bind(s.stop_reason.as_ref().map(to_json))
        .bind(&s.report_doc)
        .bind(&s.created_by)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Rewrite every column of a study by slug.
    pub async fn update_study(&self, s: &StudyRecord) -> Result<()> {
        let (state, reason) = split_state(&s.state);
        let n = sqlx::query(
            "UPDATE studies SET state = ?2, state_reason = ?3, spec = ?4, repo = ?5, base_sha = ?6, branch = ?7, \
             worktree = ?8, schedule = ?9, session = ?10, origin = ?11, incumbent_sha = ?12, incumbent_mean = ?13, \
             sigma = ?14, segment = ?15, playbook = ?16, created_at = ?17, started_at = ?18, paused_ms = ?19, \
             finished_at = ?20, stop_reason = ?21, report_doc = ?22, created_by = ?23 WHERE slug = ?1",
        )
        .bind(&s.slug)
        .bind(state)
        .bind(reason)
        .bind(to_json(&s.spec))
        .bind(&s.repo)
        .bind(&s.base_sha)
        .bind(&s.branch)
        .bind(&s.worktree)
        .bind(&s.schedule)
        .bind(s.session.map(|s| s.to_string()))
        .bind(s.origin.map(|s| s.to_string()))
        .bind(&s.incumbent_sha)
        .bind(s.incumbent_mean)
        .bind(s.sigma)
        .bind(i64::from(s.segment))
        .bind(&s.playbook)
        .bind(&s.created_at)
        .bind(&s.started_at)
        .bind(s.paused_ms as i64)
        .bind(&s.finished_at)
        .bind(s.stop_reason.as_ref().map(to_json))
        .bind(&s.report_doc)
        .bind(&s.created_by)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if n == 0 {
            return Err(Error::refused(format!("no study `{}`", s.slug)));
        }
        Ok(())
    }

    /// One study.
    pub async fn get_study(&self, slug: &str) -> Result<Option<StudyRecord>> {
        let r: Option<StudyRaw> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {STUDY_COLS} FROM studies WHERE slug = ?1"
        )))
        .bind(slug)
        .fetch_optional(&self.pool)
        .await?;
        Ok(r.map(Into::into))
    }

    /// Every study, newest first.
    pub async fn list_studies(&self) -> Result<Vec<StudyRecord>> {
        let rows: Vec<StudyRaw> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {STUDY_COLS} FROM studies ORDER BY created_at DESC, slug"
        )))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// Only the state (and its reason) of a study.
    pub async fn set_study_state(&self, slug: &str, state: &StudyState) -> Result<()> {
        let (tag, reason) = split_state(state);
        sqlx::query("UPDATE studies SET state = ?2, state_reason = ?3 WHERE slug = ?1")
            .bind(slug)
            .bind(tag)
            .bind(reason)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Patch only the columns `p` sets (R-4). `Err` when the study does not exist.
    pub async fn patch_study(&self, slug: &str, p: &StudyPatch) -> Result<()> {
        if self.patch_study_if(slug, &[], p).await? {
            Ok(())
        } else {
            Err(Error::refused(format!("no study `{slug}`")))
        }
    }

    /// [`patch_study`](Self::patch_study) only while the row's state tag is one of
    /// `from` (empty = any); `Ok(false)` when it was not — the caller lost the race.
    pub async fn patch_study_if(&self, slug: &str, from: &[&str], p: &StudyPatch) -> Result<bool> {
        Ok(study_update(slug, from, p)
            .build()
            .execute(&self.pool)
            .await?
            .rows_affected()
            > 0)
    }

    /// The study a session is bound to (140 §8.2), if any.
    pub async fn study_of_session(
        &self,
        session: runtime::SessionId,
    ) -> Result<Option<StudyRecord>> {
        let r: Option<StudyRaw> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {STUDY_COLS} FROM studies WHERE session = ?1"
        )))
        .bind(session.to_string())
        .fetch_optional(&self.pool)
        .await?;
        Ok(r.map(Into::into))
    }

    // ── segments ──

    /// Insert (or replace) a segment row.
    pub async fn put_segment(&self, s: &SegmentRecord) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO segments (slug, segment, opened_at, opened_by, reason, custody, baseline_mean, baseline_n) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(&s.slug)
        .bind(i64::from(s.segment))
        .bind(&s.opened_at)
        .bind(tag(&s.opened_by))
        .bind(&s.reason)
        .bind(to_json(&s.custody))
        .bind(s.baseline_mean)
        .bind(i64::from(s.baseline_n))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// One segment.
    pub async fn get_segment(&self, slug: &str, segment: u32) -> Result<Option<SegmentRecord>> {
        let r: Option<SegmentRaw> = sqlx::query_as(
            "SELECT slug, segment, opened_at, opened_by, reason, custody, baseline_mean, baseline_n \
             FROM segments WHERE slug = ?1 AND segment = ?2",
        )
        .bind(slug)
        .bind(i64::from(segment))
        .fetch_optional(&self.pool)
        .await?;
        Ok(r.map(Into::into))
    }

    // ── experiments ──

    /// The next free experiment number (1-based).
    pub async fn next_experiment_n(&self, slug: &str) -> Result<u32> {
        let row =
            sqlx::query("SELECT COALESCE(MAX(n), 0) + 1 AS n FROM experiments WHERE slug = ?1")
                .bind(slug)
                .fetch_one(&self.pool)
                .await?;
        Ok(row.try_get::<i64, _>("n")? as u32)
    }

    /// Insert an experiment row (`(slug, n)` must be new).
    pub async fn insert_experiment(&self, e: &ExperimentRecord) -> Result<()> {
        sqlx::query(
            "INSERT INTO experiments (slug, n, wave, slot, segment, parent_sha, state, verdict, idea, category, hypothesis, \
             description, learned, next, decision, justification, deviations, delta, delta_pct, z, n_samples, metric, \
             commit_sha, patch_path, patch_hash, review, line, cost_nanos, started_at, finished_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, \
             ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30)",
        )
        .bind(&e.slug)
        .bind(i64::from(e.n))
        .bind(e.wave.map(i64::from))
        .bind(e.slot.map(i64::from))
        .bind(i64::from(e.segment))
        .bind(&e.parent_sha)
        .bind(tag(&e.state))
        .bind(e.verdict.as_ref().map(to_json))
        .bind(&e.idea)
        .bind(&e.category)
        .bind(&e.hypothesis)
        .bind(&e.description)
        .bind(&e.learned)
        .bind(&e.next)
        .bind(e.decision.as_ref().map(tag))
        .bind(&e.justification)
        .bind(to_json(&e.deviations))
        .bind(e.delta)
        .bind(e.delta_pct)
        .bind(e.z)
        .bind(i64::from(e.n_samples))
        .bind(e.metric)
        .bind(&e.commit_sha)
        .bind(&e.patch_path)
        .bind(&e.patch_hash)
        .bind(e.review.as_ref().map(to_json))
        .bind(&e.line)
        .bind(e.cost_nanos as i64)
        .bind(&e.started_at)
        .bind(&e.finished_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Rewrite every column of an experiment by `(slug, n)`.
    pub async fn update_experiment(&self, e: &ExperimentRecord) -> Result<()> {
        if self.update_experiment_if(e, &[]).await? {
            Ok(())
        } else {
            Err(Error::refused(format!(
                "no experiment #{} in `{}`",
                e.n, e.slug
            )))
        }
    }

    /// [`update_experiment`](Self::update_experiment) only while the row's state is one
    /// of `from` (empty = any): the close-outs of Begin / StopNow / recover never
    /// overwrite a row `run_env` already moved on (R-4). `Ok(false)` = lost the race.
    pub async fn update_experiment_if(
        &self,
        e: &ExperimentRecord,
        from: &[ExperimentState],
    ) -> Result<bool> {
        Ok(experiment_update(e, from)
            .build()
            .execute(&self.pool)
            .await?
            .rows_affected()
            > 0)
    }

    /// Move `(slug, n)` to `to` only from one of `from` (R-60: two parallel
    /// `experiment_run`s cannot both claim `measuring`). `Ok(false)` = not claimed.
    pub async fn claim_experiment(
        &self,
        slug: &str,
        n: u32,
        from: &[ExperimentState],
        to: ExperimentState,
    ) -> Result<bool> {
        let mut q = sqlx::QueryBuilder::<sqlx::Sqlite>::new("UPDATE experiments SET state = ");
        q.push_bind(tag(&to));
        q.push(" WHERE slug = ").push_bind(slug.to_owned());
        q.push(" AND n = ").push_bind(i64::from(n));
        push_state_filter(&mut q, from);
        Ok(q.build().execute(&self.pool).await?.rows_affected() > 0)
    }

    /// A keep's ledger side in one transaction (R-50 / R-109): the candidate's
    /// samples retagged to `sha`, the experiment row, the study's incumbent and a
    /// `keep {n, sha}` event. Either all land or none.
    pub async fn record_keep(
        &self,
        e: &ExperimentRecord,
        sha: &str,
        study: &StudyPatch,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE samples SET tree_sha = ?3 WHERE slug = ?1 AND n = ?2 AND role = 'candidate'",
        )
        .bind(&e.slug)
        .bind(i64::from(e.n))
        .bind(sha)
        .execute(&mut *tx)
        .await?;
        experiment_update(e, &[]).build().execute(&mut *tx).await?;
        study_update(&e.slug, &[], study)
            .build()
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO events (slug, kind, payload, at) VALUES (?1, 'keep', ?2, ?3)")
            .bind(&e.slug)
            .bind(serde_json::json!({ "n": e.n, "sha": sha }).to_string())
            .bind(now())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// One experiment.
    pub async fn get_experiment(&self, slug: &str, n: u32) -> Result<Option<ExperimentRecord>> {
        let r: Option<ExperimentRaw> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {EXPERIMENT_COLS} FROM experiments WHERE slug = ?1 AND n = ?2"
        )))
        .bind(slug)
        .bind(i64::from(n))
        .fetch_optional(&self.pool)
        .await?;
        Ok(r.map(Into::into))
    }

    /// Experiments newest first; `before` pages (`n < before`).
    pub async fn list_experiments(
        &self,
        slug: &str,
        before: Option<u32>,
        limit: u32,
    ) -> Result<Vec<ExperimentRecord>> {
        let rows: Vec<ExperimentRaw> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {EXPERIMENT_COLS} FROM experiments WHERE slug = ?1 AND n < ?2 ORDER BY n DESC LIMIT ?3"
        )))
        .bind(slug)
        .bind(before.map_or(i64::MAX, i64::from))
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// The experiment still `open|measuring|measured` (140 §4.3 invariant), per slot.
    pub async fn dangling_experiment(
        &self,
        slug: &str,
        slot: Option<u8>,
    ) -> Result<Option<ExperimentRecord>> {
        let r: Option<ExperimentRaw> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {EXPERIMENT_COLS} FROM experiments WHERE slug = ?1 AND slot IS ?2 \
             AND state IN ('open', 'measuring', 'measured') ORDER BY n DESC LIMIT 1"
        )))
        .bind(slug)
        .bind(slot.map(i64::from))
        .fetch_optional(&self.pool)
        .await?;
        Ok(r.map(Into::into))
    }

    // ── samples ──

    /// Append a sample; returns its row id. A pending row (`value` and `exit`
    /// both `None`) is inserted before the benchmark spawns so an engine crash
    /// leaves a row to mark `lost` (R-34); [`update_sample`](Self::update_sample) fills it.
    pub async fn insert_sample(&self, slug: &str, s: &Sample) -> Result<i64> {
        let r = sqlx::query(
            "INSERT INTO samples (slug, n, k, tree_sha, role, value, secondaries, cases, asi, exit, wall_ms, load, \
             noisy, lost, log_path, at, slot_holders, failure) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
        )
        .bind(slug)
        .bind(i64::from(s.n))
        .bind(i64::from(s.k))
        .bind(&s.tree_sha)
        .bind(s.role.as_ref().map(tag))
        .bind(s.value)
        .bind(to_json(&s.secondaries))
        .bind(to_json(&s.cases))
        .bind(to_json(&s.asi))
        .bind(s.exit.map(i64::from))
        .bind(s.wall_ms as i64)
        .bind(s.load)
        .bind(i64::from(s.noisy))
        .bind(i64::from(s.lost))
        .bind(&s.log_path)
        .bind(&s.at)
        .bind(to_json(&s.slot_holders))
        .bind(&s.failure)
        .execute(&self.pool)
        .await?;
        Ok(r.last_insert_rowid())
    }

    /// Fill a pending sample row (by id) with the finished sample.
    pub async fn update_sample(&self, id: i64, s: &Sample) -> Result<()> {
        sqlx::query(
            "UPDATE samples SET tree_sha = ?2, role = ?3, value = ?4, secondaries = ?5, cases = ?6, asi = ?7, \
             exit = ?8, wall_ms = ?9, load = ?10, noisy = ?11, lost = ?12, log_path = ?13, at = ?14, \
             slot_holders = ?15, failure = ?16 WHERE id = ?1",
        )
        .bind(id)
        .bind(&s.tree_sha)
        .bind(s.role.as_ref().map(tag))
        .bind(s.value)
        .bind(to_json(&s.secondaries))
        .bind(to_json(&s.cases))
        .bind(to_json(&s.asi))
        .bind(s.exit.map(i64::from))
        .bind(s.wall_ms as i64)
        .bind(s.load)
        .bind(i64::from(s.noisy))
        .bind(i64::from(s.lost))
        .bind(&s.log_path)
        .bind(&s.at)
        .bind(to_json(&s.slot_holders))
        .bind(&s.failure)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Samples of one experiment (`n`), or of the whole study, in order.
    pub async fn list_samples(&self, slug: &str, n: Option<u32>) -> Result<Vec<Sample>> {
        let rows: Vec<SampleRaw> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {SAMPLE_COLS} FROM samples WHERE slug = ?1 AND (?2 IS NULL OR n = ?2) ORDER BY id"
        )))
        .bind(slug)
        .bind(n.map(i64::from))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// Samples of one tree (the σ pool and the incumbent's mean, 140 §5.2).
    pub async fn samples_of_tree(&self, slug: &str, tree_sha: &str) -> Result<Vec<Sample>> {
        let rows: Vec<SampleRaw> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {SAMPLE_COLS} FROM samples WHERE slug = ?1 AND tree_sha = ?2 ORDER BY id"
        )))
        .bind(slug)
        .bind(tree_sha)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    // ── ideas ──

    /// Insert an idea; returns its id.
    pub async fn insert_idea(&self, i: &Idea) -> Result<i64> {
        let r = sqlx::query(
            "INSERT INTO ideas (slug, text, category, source, priority, status, tried_in, at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(&i.slug)
        .bind(&i.text)
        .bind(&i.category)
        .bind(tag(&i.source))
        .bind(i64::from(i.priority))
        .bind(tag(&i.status))
        .bind(to_json(&i.tried_in))
        .bind(if i.at.is_empty() { now() } else { i.at.clone() })
        .execute(&self.pool)
        .await?;
        Ok(r.last_insert_rowid())
    }

    /// Rewrite an idea's mutable columns by id.
    pub async fn update_idea(&self, i: &Idea) -> Result<()> {
        sqlx::query(
            "UPDATE ideas SET text = ?2, category = ?3, priority = ?4, status = ?5, tried_in = ?6 WHERE id = ?1",
        )
        .bind(i.id)
        .bind(&i.text)
        .bind(&i.category)
        .bind(i64::from(i.priority))
        .bind(tag(&i.status))
        .bind(to_json(&i.tried_in))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// One idea.
    pub async fn get_idea(&self, id: i64) -> Result<Option<Idea>> {
        let r: Option<IdeaRaw> = sqlx::query_as(
            "SELECT id, slug, text, category, source, priority, status, tried_in, at FROM ideas WHERE id = ?1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(r.map(Into::into))
    }

    /// Ideas of a study, highest priority first then oldest; `status` filters.
    pub async fn list_ideas(&self, slug: &str, status: Option<IdeaStatus>) -> Result<Vec<Idea>> {
        let rows: Vec<IdeaRaw> = sqlx::query_as(
            "SELECT id, slug, text, category, source, priority, status, tried_in, at FROM ideas \
             WHERE slug = ?1 AND (?2 IS NULL OR status = ?2) ORDER BY priority DESC, id",
        )
        .bind(slug)
        .bind(status.as_ref().map(tag))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    // ── steers ──

    /// Append a steer; returns its id.
    pub async fn insert_steer(&self, slug: &str, text: &str, by: By) -> Result<i64> {
        let r = sqlx::query("INSERT INTO steers (slug, text, by, at) VALUES (?1, ?2, ?3, ?4)")
            .bind(slug)
            .bind(text)
            .bind(tag(&by))
            .bind(now())
            .execute(&self.pool)
            .await?;
        Ok(r.last_insert_rowid())
    }

    /// Steers newest first; `unseen_only` = not yet carried by a brief.
    pub async fn list_steers(&self, slug: &str, unseen_only: bool) -> Result<Vec<Steer>> {
        let rows: Vec<SteerRaw> = sqlx::query_as(
            "SELECT id, slug, text, by, at, seen_in FROM steers WHERE slug = ?1 \
             AND (?2 = 0 OR seen_in IS NULL) ORDER BY id DESC",
        )
        .bind(slug)
        .bind(i64::from(unseen_only))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// Mark every unseen steer as carried by experiment `n`.
    pub async fn mark_steers_seen(&self, slug: &str, n: u32) -> Result<()> {
        sqlx::query("UPDATE steers SET seen_in = ?2 WHERE slug = ?1 AND seen_in IS NULL")
            .bind(slug)
            .bind(i64::from(n))
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ── events ──

    /// Append an event; returns its id.
    pub async fn append_event(
        &self,
        slug: &str,
        kind: &str,
        payload: &serde_json::Value,
    ) -> Result<i64> {
        let r = sqlx::query("INSERT INTO events (slug, kind, payload, at) VALUES (?1, ?2, ?3, ?4)")
            .bind(slug)
            .bind(kind)
            .bind(payload.to_string())
            .bind(now())
            .execute(&self.pool)
            .await?;
        Ok(r.last_insert_rowid())
    }

    /// Events newest first; `kind` filters; `limit` caps.
    pub async fn list_events(
        &self,
        slug: &str,
        kind: Option<&str>,
        limit: u32,
    ) -> Result<Vec<EventRecord>> {
        let rows = sqlx::query(
            "SELECT id, slug, kind, payload, at FROM events WHERE slug = ?1 AND (?2 IS NULL OR kind = ?2) \
             ORDER BY id DESC LIMIT ?3",
        )
        .bind(slug)
        .bind(kind)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(EventRecord {
                    id: r.try_get("id")?,
                    slug: r.try_get("slug")?,
                    kind: r.try_get("kind")?,
                    payload: serde_json::from_str(r.try_get::<String, _>("payload")?.as_str())
                        .unwrap_or(serde_json::Value::Null),
                    at: r.try_get("at")?,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).await.unwrap();
        (dir, s)
    }

    /// The migration runs (twice: reopening is a no-op), WAL is on, FTS5 exists.
    #[tokio::test]
    async fn migrates_and_reopens() {
        let (dir, s) = store().await;
        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(s.pool())
            .await
            .unwrap();
        assert_eq!(mode, "wal");
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
            .fetch_one(s.pool())
            .await
            .unwrap();
        assert_eq!(n, 2, "0001_init + 0002_sample_detail");
        drop(s);
        let s = Store::open(dir.path()).await.unwrap();
        let fts: i64 = sqlx::query_scalar("SELECT count(*) FROM experiments_fts")
            .fetch_one(s.pool())
            .await
            .unwrap();
        assert_eq!(fts, 0);
    }

    /// Studies, experiments (with the FTS trigger), samples, ideas, steers and events round-trip.
    #[tokio::test]
    async fn rows_round_trip() {
        let (_dir, s) = store().await;
        let spec = StudySpec {
            slug: "sort".into(),
            goal: "faster".into(),
            repo: "/r".into(),
            ..Default::default()
        };
        let mut study = StudyRecord::new(spec, None);
        s.create_study(&study).await.unwrap();
        assert!(s.create_study(&study).await.is_err(), "slug is unique");
        study.state = StudyState::Paused {
            reason: PauseReason::CrashStreak,
        };
        study.incumbent_sha = Some("abc".into());
        study.stop_reason = Some(StopReason::Plateau);
        s.update_study(&study).await.unwrap();
        assert_eq!(s.get_study("sort").await.unwrap().unwrap(), study);
        assert_eq!(s.list_studies().await.unwrap().len(), 1);
        s.set_study_state(
            "sort",
            &StudyState::Failed {
                reason: "gone".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            s.get_study("sort").await.unwrap().unwrap().state,
            StudyState::Failed {
                reason: "gone".into()
            }
        );

        assert_eq!(s.next_experiment_n("sort").await.unwrap(), 1);
        let mut e = ExperimentRecord::open("sort", 1, 0, "abc");
        s.insert_experiment(&e).await.unwrap();
        assert!(s.dangling_experiment("sort", None).await.unwrap().is_some());
        e.idea = "cache the parsed template".into();
        e.state = ExperimentState::Kept;
        e.verdict = Some(Verdict::Improved);
        e.review = Some(ReviewVerdict::Clean {
            reason: "fine".into(),
        });
        e.decision = Some(Decision::Auto);
        e.deviations = vec!["README.md".into()];
        s.update_experiment(&e).await.unwrap();
        assert_eq!(s.get_experiment("sort", 1).await.unwrap().unwrap(), e);
        assert!(s.dangling_experiment("sort", None).await.unwrap().is_none());
        assert_eq!(s.next_experiment_n("sort").await.unwrap(), 2);
        assert_eq!(s.list_experiments("sort", None, 10).await.unwrap().len(), 1);
        assert!(
            s.list_experiments("sort", Some(1), 10)
                .await
                .unwrap()
                .is_empty()
        );
        let hits: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM experiments_fts WHERE experiments_fts MATCH 'template'",
        )
        .fetch_one(s.pool())
        .await
        .unwrap();
        assert_eq!(hits, 1, "the FTS trigger indexed the idea");
        assert_eq!(e.row().verdict, Some(Verdict::Improved));

        let sample = Sample {
            n: 1,
            k: 0,
            tree_sha: "def".into(),
            role: Some(SampleRole::Candidate),
            value: Some(41.2),
            secondaries: [("alloc_mb".to_owned(), 3.0)].into(),
            exit: Some(0),
            wall_ms: 1200,
            at: now(),
            ..Default::default()
        };
        s.insert_sample("sort", &sample).await.unwrap();
        assert_eq!(
            s.list_samples("sort", Some(1)).await.unwrap(),
            vec![sample.clone()]
        );
        assert_eq!(s.samples_of_tree("sort", "def").await.unwrap().len(), 1);

        let id = s
            .insert_idea(&Idea {
                slug: "sort".into(),
                text: "try SIMD".into(),
                category: "algorithm".into(),
                source: IdeaSource::User,
                priority: 3,
                ..Default::default()
            })
            .await
            .unwrap();
        let mut idea = s.get_idea(id).await.unwrap().unwrap();
        assert_eq!((idea.priority, idea.source), (3, IdeaSource::User));
        idea.status = IdeaStatus::Tried;
        idea.tried_in = vec![1];
        s.update_idea(&idea).await.unwrap();
        assert!(
            s.list_ideas("sort", Some(IdeaStatus::Open))
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(s.list_ideas("sort", None).await.unwrap(), vec![idea]);

        let sid = s.insert_steer("sort", "try SIMD", By::User).await.unwrap();
        assert_eq!(s.list_steers("sort", true).await.unwrap()[0].id, sid);
        s.mark_steers_seen("sort", 2).await.unwrap();
        assert!(s.list_steers("sort", true).await.unwrap().is_empty());
        assert_eq!(
            s.list_steers("sort", false).await.unwrap()[0].seen_in,
            Some(2)
        );

        s.append_event("sort", "drift", &serde_json::json!({"n": 1}))
            .await
            .unwrap();
        let ev = s.list_events("sort", Some("drift"), 5).await.unwrap();
        assert_eq!(ev[0].payload["n"], 1);
        assert!(
            s.list_events("sort", Some("overfit"), 5)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
