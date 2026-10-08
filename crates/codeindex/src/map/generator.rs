//! The generator — what the model decides (147 §6): an engine-owned pipeline
//! of no-tools side calls (`runtime::SideCall { kind: CallKind::CodeMap }`
//! asked of the agent actor through the registry, C8's shape), one job per
//! root, stages A → B → D → C, every reply JSON-validated by [`super::validate`]
//! with one repair round, then the structure fallback. Cost is charged to the
//! opening session (S1); progress is `CodeindexEvent::CodeMapProgress`.
//!
//! The one-job-per-root registry (§6.1 `Mutex<HashMap<PathBuf, JobHandle>>`)
//! lives in this file as a `static` — one `Codeindex` per process, and it keeps
//! `index.rs` untouched by the generator. So does the [`SideCaller`] seam: the
//! kameo registry is typed, so the crate that knows the agent actor's type
//! (`rpc`, the binary) installs one closure at start; tests install a fake.
//!
//! owner: W2 — design §6, §10.1 (CodeMapProgress), §10.3 (Generate/Cancel)

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::Instant;

use runtime::{CallKind, ModelRole, SideCall, SideReply};
use serde::Deserialize;
use serde_json::json;
use sqlx::SqlitePool;
use surface::{GraphDirection, GraphEdge, GraphEdgeStyle, GraphKind, GraphNode};

use super::detect::{Detection, FLOW_PERSPECTIVES, PERSPECTIVES};
use super::generator_facts::{Facts, Flow, cut};
use super::generator_write::{self as w, ElementRow};
use super::skeleton::Child;
use super::validate::{self, Located, check_description, check_diagram, check_edge_label};
use super::{
    CALL_BUDGET, Cancel, DIAGRAM_NODES, Generate, GraphProps, MAX_DEPTH, MapEdge, MapEvidence,
    MapFields, MapJob, MapOrigin, MapProvenance, cluster_id, element_id,
};
use crate::event::CodeindexEvent;
use crate::index::{Codeindex, RootIndex};

/// Stage-B calls in flight at once (§6.7).
pub const TOP_CONCURRENCY: usize = 3;
/// Elements described per stage-C call (§6.2).
pub const DESCRIBE_BATCH: usize = 10;
/// Entry points per flow perspective (§6.2 stage D).
pub const FLOW_ENTRY_POINTS: usize = 4;
/// `CodeMapProgress` gate per root, as `CodeReconciled` (§10.1).
pub const PROGRESS_GATE: std::time::Duration = std::time::Duration::from_millis(250);
/// Consecutive transport failures that stop a job (§6.8: one failed call
/// leaves its element for the next open; a dead provider should not burn
/// thirty calls).
pub const TRANSPORT_FAILURES: u32 = 3;
/// `MapProvenance.currency` when the ledger reports nothing.
const CURRENCY: &str = "USD";

/// Sends one [`SideCall`] and returns the reply or the agent plugin's error
/// text (`decision::SideCaller`'s shape).
pub type SideCaller = Arc<
    dyn Fn(SideCall) -> Pin<Box<dyn Future<Output = Result<SideReply, String>> + Send>>
        + Send
        + Sync,
>;

static SIDE_CALLER: OnceLock<SideCaller> = OnceLock::new();

/// Install the process-wide side caller once (the binary, after the agent
/// plugin starts). `false` when one was already installed.
pub fn install_side_caller(caller: SideCaller) -> bool {
    SIDE_CALLER.set(caller).is_ok()
}

/// A running job: its live state and the task to abort.
struct JobHandle {
    state: Arc<Mutex<MapJob>>,
    task: tokio::task::AbortHandle,
    pool: SqlitePool,
}

static JOBS: LazyLock<Mutex<HashMap<PathBuf, JobHandle>>> = LazyLock::new(Mutex::default);

fn jobs() -> std::sync::MutexGuard<'static, HashMap<PathBuf, JobHandle>> {
    JOBS.lock().unwrap_or_else(|e| e.into_inner())
}

/// `codemap_generate`: start the root's job or return the running one;
/// `element` set = regenerate that subtree only. `ASK_FAILED` with the
/// supervisor's text when `session` is not a live native session (C2) — the
/// first side call fails with it and the job ends `failed`.
pub async fn generate(ix: &Arc<Codeindex>, m: Generate) -> anyhow::Result<MapJob> {
    let ri = crate::lens::open_indexed(ix, &m.root).await?;
    let snap = ri.snapshot();
    anyhow::ensure!(
        !snap.suspect_root,
        "{}",
        snap.hint.unwrap_or("the index is suspect; reconcile first")
    );
    anyhow::ensure!(
        snap.pending == 0,
        "the index is still updating ({} files pending); open the CodeMap again when it settles",
        snap.pending
    );
    if let Some(h) = jobs().get(&ri.root)
        && !h.task.is_finished()
    {
        return Ok(h.state.lock().unwrap_or_else(|e| e.into_inner()).clone());
    }
    let caller = SIDE_CALLER.get().cloned().ok_or_else(|| {
        anyhow::anyhow!("codemap generation needs the agent plugin (no side caller installed)")
    })?;
    let pool = ix.map_pool(&ri).await?;
    let job = MapJob {
        id: new_id(),
        session: m.session,
        started_ms: u64::try_from(w::now_ms()).unwrap_or(0),
        stage: "select".to_owned(),
        done: 0,
        total: 0,
        calls: 0,
        cost_nanos: 0,
        currency: CURRENCY.to_owned(),
        error: None,
    };
    w::write_job(&pool, &job, false, None).await?;
    let state = Arc::new(Mutex::new(job.clone()));
    let role = if ix.cfg.codemap.model.trim().is_empty() {
        ModelRole::Session
    } else {
        ModelRole::Named(ix.cfg.codemap.model.trim().to_owned())
    };
    let cx = Arc::new(Job {
        ix: Arc::clone(ix),
        ri: Arc::clone(&ri),
        pool: pool.clone(),
        state: Arc::clone(&state),
        caller,
        role,
        only: m.element,
        log: Mutex::new(Vec::new()),
        last_progress: Mutex::new(None),
    });
    let root = ri.root.clone();
    let task = tokio::spawn(async move {
        let outcome = cx.run().await;
        cx.finish(outcome).await;
        jobs().remove(&cx.ri.root);
    });
    jobs().insert(
        root,
        JobHandle {
            state,
            task: task.abort_handle(),
            pool,
        },
    );
    Ok(job)
}

/// `codemap_cancel`: abort the root's running job (`stage: "cancelled"`);
/// stored elements stay. Errors when no job runs.
pub async fn cancel(ix: &Arc<Codeindex>, m: Cancel) -> anyhow::Result<MapJob> {
    let root = crate::root::discover(Some(&m.root))?;
    let Some(h) = jobs().remove(&root) else {
        anyhow::bail!("no generation job is running for {}", root.display());
    };
    h.task.abort();
    let job = {
        let mut s = h.state.lock().unwrap_or_else(|e| e.into_inner());
        s.stage = "cancelled".to_owned();
        s.clone()
    };
    w::write_job(&h.pool, &job, true, None).await?;
    if let Ok(ri) = ix.open_root(&root).await {
        ix.publish(progress_event(&ri, &job, None)).await;
    }
    Ok(job)
}

/// `jobs.id`: a UUIDv7 (the runtime's id newtypes wrap one; no `uuid` dep here).
fn new_id() -> String {
    runtime::TurnId::new().0.to_string()
}

fn progress_event(ri: &RootIndex, job: &MapJob, element: Option<&str>) -> CodeindexEvent {
    CodeindexEvent::CodeMapProgress {
        root: ri.root.clone(),
        job: job.id.clone(),
        session: job.session,
        stage: job.stage.clone(),
        done: job.done,
        total: job.total,
        element: element.map(str::to_owned),
        calls: job.calls,
        cost_nanos: job.cost_nanos,
        currency: job.currency.clone(),
        error: job.error.clone(),
        seq: ri.next_seq(),
    }
}

// --- the job ------------------------------------------------------------------

/// Why a job stopped before `done`.
#[derive(Clone, Debug)]
enum Stop {
    /// The opening session closed, or `codemap_cancel`.
    Cancelled,
    /// The session's `Budget` refused (W3) or the provider is down.
    Failed(String),
}

/// One side call that came back.
#[derive(Clone, Debug)]
struct CallOut {
    id: String,
    text: String,
    model: String,
    input: u32,
    output: u32,
}

/// What a validated call produced: the value, the calls it took, whether a
/// repair round ran.
struct Validated<T> {
    value: T,
    calls: Vec<CallOut>,
    repaired: bool,
}

/// How a validated call failed.
enum Failure<T> {
    Transport(Stop),
    /// Invalid twice; `last` is the second reply when it at least parsed.
    Invalid {
        errors: Vec<Located>,
        last: Option<T>,
        calls: Vec<CallOut>,
    },
}

struct Job {
    ix: Arc<Codeindex>,
    ri: Arc<RootIndex>,
    pool: SqlitePool,
    state: Arc<Mutex<MapJob>>,
    caller: SideCaller,
    role: ModelRole,
    only: Option<String>,
    log: Mutex<Vec<String>>,
    last_progress: Mutex<Option<Instant>>,
}

/// The planned perspective of one run.
#[derive(Clone, Debug)]
struct Planned {
    id: String,
    kind: String,
    title: String,
    why: String,
    order_no: u32,
    /// Run stage B (and D) for it.
    top: bool,
}

impl Job {
    fn state(&self) -> std::sync::MutexGuard<'_, MapJob> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn note(&self, line: String) {
        tracing::info!(root = %self.ri.root.display(), "codemap: {line}");
        self.log
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(line);
    }

    /// `CodeMapProgress`, gated at [`PROGRESS_GATE`] per root unless `force`
    /// (stage changes and the end always publish; a skipped per-element
    /// event is covered by the next forced one).
    async fn progress(&self, stage: &str, element: Option<&str>, force: bool) {
        let job = {
            let mut s = self.state();
            if s.stage != stage {
                s.stage = stage.to_owned();
            }
            s.clone()
        };
        {
            let mut last = self.last_progress.lock().unwrap_or_else(|e| e.into_inner());
            if !force && last.is_some_and(|t| t.elapsed() < PROGRESS_GATE) {
                return;
            }
            *last = Some(Instant::now());
        }
        let _ = w::write_job(&self.pool, &job, false, None).await;
        self.ix
            .publish(progress_event(&self.ri, &job, element))
            .await;
    }

    async fn changed(&self, ids: Vec<String>) {
        if ids.is_empty() {
            return;
        }
        let more = ids.len().saturating_sub(super::CHANGED_ELEMENTS);
        let mut elements = ids;
        elements.truncate(super::CHANGED_ELEMENTS);
        self.ix
            .publish(CodeindexEvent::CodeMapChanged {
                root: self.ri.root.clone(),
                elements,
                stale: Vec::new(),
                more,
                seq: self.ri.next_seq(),
            })
            .await;
    }

    async fn finish(&self, outcome: Result<(), Stop>) {
        let (stage, error) = match outcome {
            Ok(()) => ("done", None),
            Err(Stop::Cancelled) => ("cancelled", None),
            Err(Stop::Failed(e)) => ("failed", Some(e)),
        };
        let log = self
            .log
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .join("\n");
        let job = {
            let mut s = self.state();
            s.stage = stage.to_owned();
            s.error = error;
            s.clone()
        };
        if let Err(e) = w::write_job(
            &self.pool,
            &job,
            true,
            (!log.is_empty()).then_some(log.as_str()),
        )
        .await
        {
            tracing::warn!("codemap: jobs row: {e:#}");
        }
        self.ix.publish(progress_event(&self.ri, &job, None)).await;
    }

    /// One side call on the opening session's cost line.
    async fn call(&self, user: String) -> Result<CallOut, Stop> {
        let session = self.state().session;
        let r = (self.caller)(SideCall {
            session,
            role: self.role.clone(),
            system: system_prompt(),
            user,
            budget: CALL_BUDGET,
            kind: CallKind::CodeMap,
        })
        .await;
        self.state().calls += 1;
        match r {
            Ok(reply) => Ok(CallOut {
                id: new_id(),
                text: reply.text,
                model: reply.model,
                input: reply.input_tokens.unwrap_or(0),
                output: reply.output_tokens.unwrap_or(0),
            }),
            Err(e) if e.contains("cancelled") => Err(Stop::Cancelled),
            Err(e) if e.to_ascii_lowercase().contains("budget") => Err(Stop::Failed(format!(
                "budget exhausted · {} elements left",
                self.left()
            ))),
            Err(e) => Err(Stop::Failed(e)),
        }
    }

    fn left(&self) -> u32 {
        let s = self.state();
        s.total.saturating_sub(s.done)
    }

    /// Ask, parse as `T`, run `validate`; one repair round on any error
    /// (§6.6), then [`Failure::Invalid`]. Transport errors are never repaired.
    async fn ask<T, V>(
        &self,
        user: String,
        validate: impl Fn(&T) -> Result<V, Vec<Located>>,
    ) -> Result<Validated<V>, Failure<T>>
    where
        T: for<'de> Deserialize<'de>,
    {
        let mut calls = Vec::with_capacity(2);
        let first = self.call(user).await.map_err(Failure::Transport)?;
        calls.push(first.clone());
        let (errors, _) = match parse_reply::<T>(&first.text) {
            Ok(t) => match validate(&t) {
                Ok(v) => {
                    return Ok(Validated {
                        value: v,
                        calls,
                        repaired: false,
                    });
                }
                Err(errors) => (errors, Some(t)),
            },
            Err(e) => (vec![e], None),
        };
        let repair = validate::repair_message(&first.text, &errors).unwrap_or_default();
        let second = self.call(repair).await.map_err(Failure::Transport)?;
        calls.push(second.clone());
        match parse_reply::<T>(&second.text) {
            Ok(t) => match validate(&t) {
                Ok(v) => Ok(Validated {
                    value: v,
                    calls,
                    repaired: true,
                }),
                Err(errors) => Err(Failure::Invalid {
                    errors,
                    last: Some(t),
                    calls,
                }),
            },
            Err(e) => Err(Failure::Invalid {
                errors: vec![e],
                last: None,
                calls,
            }),
        }
    }

    fn provenance(
        &self,
        calls: &[CallOut],
        repaired: bool,
        facts: &Facts,
        facts_hash: String,
    ) -> MapProvenance {
        MapProvenance {
            model: calls.last().map(|c| c.model.clone()).unwrap_or_default(),
            git_head: facts.git_head.clone(),
            index_state: facts.index_state.clone(),
            generated_ms: u64::try_from(w::now_ms()).unwrap_or(0),
            call_ids: calls.iter().map(|c| c.id.clone()).collect(),
            cost_nanos: 0,
            currency: CURRENCY.to_owned(),
            input_tokens: calls.iter().map(|c| c.input).sum(),
            output_tokens: calls.iter().map(|c| c.output).sum(),
            repaired,
            facts_hash,
        }
    }

    // --- the pipeline ---------------------------------------------------------

    async fn run(self: &Arc<Self>) -> Result<(), Stop> {
        let facts = match Facts::load(&self.ri, &self.pool).await {
            Ok(f) => Arc::new(f),
            Err(e) => return Err(Stop::Failed(format!("facts: {e:#}"))),
        };
        let plan = self.plan(&facts).await?;
        {
            let mut s = self.state();
            s.total = s.total.max(u32::try_from(plan.len()).unwrap_or(u32::MAX));
        }
        // Stage B (+ D) for every planned perspective, ≤ TOP_CONCURRENCY in flight.
        let sem = Arc::new(tokio::sync::Semaphore::new(TOP_CONCURRENCY));
        let mut set = tokio::task::JoinSet::new();
        for p in plan.iter().filter(|p| p.top).cloned() {
            let (me, facts, sem) = (Arc::clone(self), Arc::clone(&facts), Arc::clone(&sem));
            set.spawn(async move {
                let _permit = sem.acquire().await;
                me.top(&facts, &p).await?;
                if FLOW_PERSPECTIVES.contains(&p.kind.as_str()) {
                    me.flows(&facts, &p).await?;
                }
                Ok::<(), Stop>(())
            });
        }
        let mut stop = None;
        while let Some(r) = set.join_next().await {
            match r {
                Ok(Ok(())) => {}
                Ok(Err(s)) => {
                    stop.get_or_insert(s);
                    set.abort_all();
                }
                Err(e) if e.is_cancelled() => {}
                Err(e) => {
                    stop.get_or_insert(Stop::Failed(format!("stage B task: {e}")));
                }
            }
        }
        if let Some(s) = stop {
            return Err(s);
        }
        // Stage C: prose, breadth-first, ten elements per call.
        self.progress("describe", None, true).await;
        let mut pending: Vec<String> = Vec::new();
        for p in &plan {
            let max_depth = if p.kind == PERSPECTIVES[0] {
                MAX_DEPTH - 1
            } else {
                1
            };
            let mut ids = w::pending_prose(&self.pool, &p.id, max_depth)
                .await
                .unwrap_or_default();
            if let Some(only) = &self.only
                && *only != p.id
            {
                // One subtree: the element and its direct children.
                let kids = w::children_of(&self.pool, only).await.unwrap_or_default();
                ids.retain(|id| id == only || kids.iter().any(|(k, _, _, _)| k == id));
            }
            pending.extend(ids);
        }
        {
            let mut s = self.state();
            s.total = s.done + u32::try_from(pending.len()).unwrap_or(u32::MAX);
        }
        let mut transport_failures = 0;
        for batch in pending.chunks(DESCRIBE_BATCH) {
            match self.describe(&facts, batch).await {
                Ok(()) => transport_failures = 0,
                Err(Stop::Cancelled) => return Err(Stop::Cancelled),
                Err(Stop::Failed(e)) if e.starts_with("budget") => return Err(Stop::Failed(e)),
                Err(Stop::Failed(e)) => {
                    transport_failures += 1;
                    self.note(format!("describe {}: {e}", batch.join(", ")));
                    if transport_failures >= TRANSPORT_FAILURES {
                        return Err(Stop::Failed(e));
                    }
                }
            }
        }
        Ok(())
    }

    /// Which perspectives this run touches and whether stage B runs for each
    /// (§6.1 resume: only what is stale or without prose; `only` = one subtree).
    async fn plan(&self, facts: &Facts) -> Result<Vec<Planned>, Stop> {
        let stored = w::generated_perspectives(&self.pool)
            .await
            .map_err(|e| Stop::Failed(format!("perspectives: {e:#}")))?;
        if let Some(only) = &self.only {
            let header = w::element_header(&self.pool, only)
                .await
                .map_err(|e| Stop::Failed(format!("element: {e:#}")))?;
            let Some(header) = header else {
                return Err(Stop::Failed(format!("unknown element {only}")));
            };
            let persp = header.perspective;
            let Some(p) = stored.iter().find(|p| p.id == persp) else {
                return Err(Stop::Failed(format!(
                    "{persp} is not a generated perspective"
                )));
            };
            return Ok(vec![Planned {
                id: p.id.clone(),
                kind: p.kind.clone(),
                title: p.title.clone(),
                why: p.why.clone(),
                order_no: p.order_no,
                top: *only == p.id,
            }]);
        }
        if !stored.is_empty() {
            return Ok(stored
                .into_iter()
                .map(|p| Planned {
                    id: p.id,
                    kind: p.kind,
                    title: p.title,
                    why: p.why,
                    order_no: p.order_no,
                    top: p.needs_top,
                })
                .collect());
        }
        self.select(facts).await
    }

    /// Stage A.
    async fn select(&self, facts: &Facts) -> Result<Vec<Planned>, Stop> {
        self.progress("select", None, true).await;

        let chosen = match self
            .ask::<SelectReply, Vec<SelectRow>>(select_message(facts), |r| {
                let mut errors = Vec::new();
                let mut seen = BTreeSet::new();
                for (i, p) in r.perspectives.iter().enumerate() {
                    if !PERSPECTIVES.contains(&p.kind.as_str()) {
                        errors.push(Located::new(
                            format!("/perspectives/{i}/kind"),
                            format!("unknown perspective {:?}", p.kind),
                            "a catalogue kind",
                        ));
                    }
                    if !seen.insert(p.kind.as_str()) {
                        errors.push(Located::new(
                            format!("/perspectives/{i}/kind"),
                            format!("{:?} listed twice", p.kind),
                            "each kind once",
                        ));
                    }
                }
                if errors.is_empty() {
                    Ok(r.perspectives.clone())
                } else {
                    Err(errors)
                }
            })
            .await
        {
            Ok(v) => v.value,
            Err(Failure::Transport(stop)) => return Err(stop),
            Err(Failure::Invalid { errors, .. }) => {
                self.note(format!(
                    "select: invalid twice ({}); overall-architecture only",
                    errors
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("; ")
                ));
                Vec::new()
            }
        };
        let (rows, dropped) = choose(chosen, &facts.detections);
        for kind in dropped {
            self.note(format!("select: {kind} is not eligible; dropped"));
        }
        let mut plan = Vec::with_capacity(rows.len());
        for (i, p) in rows.into_iter().enumerate() {
            let eligible = facts
                .detections
                .iter()
                .find(|d| d.kind == p.kind)
                .map(|d| serde_json::to_value(d).unwrap_or_default())
                .unwrap_or_default();
            let order_no = u32::try_from(i).unwrap_or(u32::MAX);
            let title = if p.title.trim().is_empty() {
                title_of(&p.kind)
            } else {
                p.title.trim().to_owned()
            };
            w::write_perspective(
                &self.pool, &p.kind, &p.kind, &title, &p.why, order_no, &eligible,
            )
            .await
            .map_err(|e| Stop::Failed(format!("perspectives: {e:#}")))?;
            plan.push(Planned {
                id: p.kind.clone(),
                kind: p.kind,
                title,
                why: p.why,
                order_no,
                top: true,
            });
        }
        Ok(plan)
    }

    /// Stage B for one perspective: partition, draw, describe the root.
    async fn top(&self, facts: &Facts, p: &Planned) -> Result<(), Stop> {
        self.progress("top", None, true).await;
        let handles = Handles::new(&facts.top, &facts.edges);
        let detection = facts.detections.iter().find(|d| d.kind == p.kind);
        let flows = if FLOW_PERSPECTIVES.contains(&p.kind.as_str()) {
            facts
                .flows(&self.ri.pool, detection, FLOW_ENTRY_POINTS)
                .await
        } else {
            Vec::new()
        };
        let user = top_message(facts, p, &handles, detection, &flows);
        let max_depth = if p.kind == PERSPECTIVES[0] {
            MAX_DEPTH
        } else {
            2
        };
        let (plan, calls, repaired, error) = match self
            .ask::<TopReply, TopPlan>(user, |r| validate_top(r, facts, &handles, &p.id))
            .await
        {
            Ok(v) => (v.value, v.calls, v.repaired, None),
            Err(Failure::Transport(stop)) => return Err(stop),
            Err(Failure::Invalid {
                errors,
                last,
                calls,
            }) => {
                let error = errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ");
                self.note(format!(
                    "{}: invalid twice; structure view kept ({error})",
                    p.id
                ));
                let fields = last
                    .map(|t| t.fields.into_fields())
                    .filter(|f| check_description("", &f.description).is_none())
                    .unwrap_or_default();
                (structure_plan(facts, fields), calls, true, Some(error))
            }
        };

        // The root's hash as `recompute` stamps it: every file, the skeleton's
        // edges between the model's groups and the bundle (a kept child there).
        let mut group_children: Vec<Child> = plan
            .groups
            .iter()
            .filter_map(|g| facts.group_child(&g.node, &g.name, &g.members))
            .collect();
        if !plan.uncovered.is_empty() {
            let bundle_node = format!("bundle:perspective:{}", p.id);
            group_children.extend(facts.group_child(&bundle_node, "", &plan.uncovered));
        }
        let (root_edges, _) = facts.sk.edges_between(&group_children);
        let root_hash = facts.facts_hash(&facts.all_files(), &root_edges);
        let mut prov = self.provenance(&calls, repaired, facts, root_hash);
        prov.repaired |= error.is_some();
        if let Some(warning) = &plan.warning {
            self.note(warning.clone());
        }
        let rows = build_tree(facts, p, &plan, prov, error, max_depth);
        w::clear_perspective(&self.pool, &p.id)
            .await
            .map_err(|e| Stop::Failed(format!("store: {e:#}")))?;
        let mut ids = Vec::with_capacity(rows.len());
        for row in &rows {
            w::write_element(&self.pool, row)
                .await
                .map_err(|e| Stop::Failed(format!("store: {e:#}")))?;
            ids.push(row.id.clone());
        }
        {
            let mut s = self.state();
            s.done += 1;
            let described = rows
                .iter()
                .filter(|r| r.depth >= 1 && r.depth < max_depth && r.kind != "bundle")
                .count();
            s.total += u32::try_from(described).unwrap_or(u32::MAX);
        }
        self.changed(ids).await;
        self.progress("top", Some(&p.id), true).await;
        Ok(())
    }

    /// Stage D for a flow perspective: one diagram per entry point.
    async fn flows(&self, facts: &Facts, p: &Planned) -> Result<(), Stop> {
        let detection = facts.detections.iter().find(|d| d.kind == p.kind);
        let flows = facts
            .flows(&self.ri.pool, detection, FLOW_ENTRY_POINTS)
            .await;
        if flows.is_empty() {
            return Ok(());
        }
        self.progress("flow", None, true).await;
        let base = w::children_of(&self.pool, &p.id)
            .await
            .unwrap_or_default()
            .len();
        for (i, flow) in flows.iter().enumerate() {
            let handles: Vec<String> = (1..=flow.symbols.len()).map(|n| format!("s{n}")).collect();
            let user = flow_message(facts, p, flow, &handles, detection);
            let (plan, calls, repaired, error) = match self
                .ask::<FlowReply, FlowPlan>(user, |r| validate_flow(r, flow, &handles))
                .await
            {
                Ok(v) => (v.value, v.calls, v.repaired, None),
                Err(Failure::Transport(Stop::Cancelled)) => return Err(Stop::Cancelled),
                Err(Failure::Transport(Stop::Failed(e))) if e.starts_with("budget") => {
                    return Err(Stop::Failed(e));
                }
                Err(Failure::Transport(Stop::Failed(e))) => {
                    self.note(format!(
                        "{}/{}: {e}; left for the next open",
                        p.id, flow.entry.node
                    ));
                    continue;
                }
                Err(Failure::Invalid {
                    errors,
                    last,
                    calls,
                }) => {
                    let error = errors
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("; ");
                    self.note(format!(
                        "{}/{}: invalid twice ({error})",
                        p.id, flow.entry.node
                    ));
                    let fields = last
                        .map(|t| t.fields.into_fields())
                        .filter(|f| check_description("", &f.description).is_none())
                        .unwrap_or_default();
                    (structure_flow(flow, fields), calls, true, Some(error))
                }
            };

            let mut files: Vec<usize> = flow
                .symbols
                .iter()
                .flat_map(|s| facts.sk.files_under(&s.path))
                .collect();
            files.sort_unstable();
            files.dedup();
            let hash = facts.facts_hash(&files, &plan.skeleton_edges);
            let mut prov = self.provenance(&calls, repaired, facts, hash);
            prov.repaired |= error.is_some();
            let rows = build_flow(
                p,
                flow,
                &plan,
                prov,
                error,
                u32::try_from(base + i).unwrap_or(u32::MAX),
            );
            let mut ids = Vec::with_capacity(rows.len());
            for row in &rows {
                w::write_element(&self.pool, row)
                    .await
                    .map_err(|e| Stop::Failed(format!("store: {e:#}")))?;
                ids.push(row.id.clone());
            }
            self.state().done += 1;
            self.changed(ids).await;
            self.progress("flow", Some(&rows[0].id), false).await;
        }
        Ok(())
    }

    /// Stage C for one batch: names, prose, edge labels.
    async fn describe(&self, facts: &Facts, batch: &[String]) -> Result<(), Stop> {
        let mut items = Vec::with_capacity(batch.len());
        for id in batch {
            let Some(header) = w::element_header(&self.pool, id)
                .await
                .map_err(|e| Stop::Failed(format!("store: {e:#}")))?
            else {
                continue;
            };
            let (kind, name, path, members) =
                (header.kind, header.name, header.path, header.members);
            let own = facts
                .group_child(&header.node, &name, &members)
                .map(|c| c.files)
                .unwrap_or_default();
            let sub = facts.sub_skeleton(
                facts.children_for(&kind, path.as_deref(), &members),
                Facts::cap(kind == "cluster" || kind == "bundle"),
            );
            let mut children = Vec::with_capacity(sub.children.len());
            for c in &sub.children {
                let top = facts.top_symbols(&self.ri.pool, &c.path).await;
                children.push((c.clone(), top));
            }
            let stored = w::read_diagram(&self.pool, id)
                .await
                .map_err(|e| Stop::Failed(format!("store: {e:#}")))?;
            let edges: Vec<MapEdge> = stored.as_ref().map(|(_, e)| e.clone()).unwrap_or_default();
            items.push(DescribeItem {
                id: id.clone(),
                kind: kind.clone(),
                name,
                path,
                members,
                anchors: header.anchors,
                // The row's own edges: none for a leaf — `recompute` hashes
                // a ≤ LEAF_FILES unit the same way.
                hash: facts.facts_hash(&own, &edges),
                triggers: facts.concern_triggers(&own, &header.node),
                children,
                edges,
            });
        }
        if items.is_empty() {
            return Ok(());
        }
        let user = describe_message(facts, &items);
        let (described, calls, repaired) = match self
            .ask::<DescribeReply, Vec<Described>>(user, |r| validate_describe(r, &items))
            .await
        {
            Ok(v) => (v.value, v.calls, v.repaired),
            Err(Failure::Transport(stop)) => return Err(stop),
            Err(Failure::Invalid {
                errors,
                last,
                calls,
            }) => {
                // Keep the rows that validated on their own; mark the rest.
                let error = errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ");
                self.note(format!("describe: invalid twice ({error})"));
                let ok = last
                    .map(|t| {
                        t.elements
                            .into_iter()
                            .filter(|d| {
                                items.iter().any(|i| i.id == d.id)
                                    && check_description("", &d.fields.description).is_none()
                                    && d.edge_labels
                                        .values()
                                        .all(|l| matches!(check_edge_label("", l), Ok(None)))
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                for item in &items {
                    if !ok.iter().any(|d| d.id == item.id) {
                        let _ = w::write_error(&self.pool, &item.id, &error).await;
                    }
                }
                (ok, calls, true)
            }
        };
        let mut ids = Vec::new();
        for d in described {
            let Some(item) = items.iter().find(|i| i.id == d.id) else {
                continue;
            };
            let hash = item.hash.clone();
            let prov = self.provenance(&calls, repaired, facts, hash);
            let name = d.name.as_deref().map(str::trim).filter(|n| !n.is_empty());
            w::write_prose(&self.pool, &d.id, name, &d.fields.into_fields(), &prov)
                .await
                .map_err(|e| Stop::Failed(format!("store: {e:#}")))?;
            // Edge labels on the element's own diagram.
            if let Ok(Some((mut g, mut edges))) = w::read_diagram(&self.pool, &d.id).await {
                for (handle, label) in &d.edge_labels {
                    let Some(k) = handle
                        .strip_prefix('e')
                        .and_then(|n| n.parse::<usize>().ok())
                        .and_then(|n| n.checked_sub(1))
                    else {
                        continue;
                    };
                    let Some(e) = item.edges.get(k) else { continue };
                    for ge in g
                        .edges
                        .iter_mut()
                        .filter(|x| x.from == e.from && x.to == e.to)
                    {
                        ge.label = Some(label.clone());
                    }
                    for me in edges
                        .iter_mut()
                        .filter(|x| x.from == e.from && x.to == e.to)
                    {
                        me.label = label.clone();
                    }
                }
                let _ = w::write_diagram(&self.pool, &d.id, &g, &edges).await;
            }
            // The new name on the parent's diagram node.
            if let Some(name) = name
                && let Ok(Some(h)) = w::element_header(&self.pool, &d.id).await
                && let Some(parent_id) = h.parent
                && let Ok(Some((mut g, edges))) = w::read_diagram(&self.pool, &parent_id).await
            {
                for n in g.nodes.iter_mut().filter(|n| n.id == h.node) {
                    n.label = name.to_owned();
                }
                let _ = w::write_diagram(&self.pool, &parent_id, &g, &edges).await;
                ids.push(parent_id);
            }
            ids.push(d.id.clone());
            self.state().done += 1;
            self.progress("describe", Some(&d.id), false).await;
        }
        self.changed(ids).await;
        Ok(())
    }
}

/// §6.6 rule 6 / S5: keep the chosen kinds that are eligible (no ceiling —
/// every eligible kind may be chosen), always include `overall-architecture`,
/// catalogue order. Returns `(kept, dropped kinds)`.
fn choose(chosen: Vec<SelectRow>, detections: &[Detection]) -> (Vec<SelectRow>, Vec<String>) {
    let eligible: BTreeSet<&str> = detections
        .iter()
        .filter(|d| d.eligible)
        .map(|d| d.kind.as_str())
        .collect();
    let mut dropped = Vec::new();
    let mut rows: Vec<SelectRow> = chosen
        .into_iter()
        .filter(|p| {
            let ok = eligible.contains(p.kind.as_str()) || p.kind == PERSPECTIVES[0];
            if !ok {
                dropped.push(p.kind.clone());
            }
            ok
        })
        .collect();
    if !rows.iter().any(|p| p.kind == PERSPECTIVES[0]) {
        rows.insert(
            0,
            SelectRow {
                kind: PERSPECTIVES[0].to_owned(),
                title: "Overall architecture".to_owned(),
                why: "always: what exists and how the pieces relate".to_owned(),
            },
        );
    }
    rows.sort_by_key(|p| PERSPECTIVES.iter().position(|k| *k == p.kind));
    (rows, dropped)
}

// --- handles ------------------------------------------------------------------

/// The `n1…`/`e1…` handles of one top-level call.
struct Handles {
    units: Vec<(String, String)>, // (handle, unit node)
    edges: Vec<(String, usize)>,  // (handle, index into facts.edges)
}

impl Handles {
    fn new(units: &[Child], edges: &[MapEdge]) -> Self {
        Self {
            units: units
                .iter()
                .enumerate()
                .map(|(i, u)| (format!("n{}", i + 1), u.node.clone()))
                .collect(),
            edges: (0..edges.len())
                .map(|i| (format!("e{}", i + 1), i))
                .collect(),
        }
    }

    fn unit_node(&self, handle: &str) -> Option<&str> {
        self.units
            .iter()
            .find(|(h, _)| h == handle)
            .map(|(_, n)| n.as_str())
    }

    fn unit_handle(&self, node: &str) -> Option<&str> {
        self.units
            .iter()
            .find(|(_, n)| n == node)
            .map(|(h, _)| h.as_str())
    }

    fn edge_index(&self, handle: &str) -> Option<usize> {
        self.edges
            .iter()
            .find(|(h, _)| h == handle)
            .map(|(_, i)| *i)
    }
}

// --- stage replies (deny_unknown_fields, §6.6 rule 1) -------------------------------

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectReply {
    perspectives: Vec<SelectRow>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectRow {
    kind: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    why: String,
}

/// omm's prose fields as the model writes them.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fields {
    #[serde(default)]
    description: String,
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    constraint: Option<String>,
    #[serde(default)]
    concern: Option<String>,
    #[serde(default)]
    todo: Option<String>,
    #[serde(default)]
    note: Option<String>,
}

impl Fields {
    fn into_fields(self) -> MapFields {
        let some = |s: Option<String>| s.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
        MapFields {
            description: self.description.trim().to_owned(),
            context: some(self.context),
            constraint: some(self.constraint),
            concern: some(self.concern),
            todo: some(self.todo),
            note: some(self.note),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopReply {
    fields: Fields,
    nodes: Vec<TopNode>,
    mermaid: String,
    #[serde(default)]
    edges: Vec<EdgeNote>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopNode {
    id: String,
    name: String,
    members: Vec<String>,
}

/// Per-edge annotation beside the mermaid: a label override, cited evidence
/// handles, or the reason for an edge the facts do not back.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct EdgeNote {
    from: String,
    to: String,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    evidence: Vec<String>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FlowReply {
    title: String,
    fields: Fields,
    steps: Vec<StepName>,
    mermaid: String,
    #[serde(default)]
    edges: Vec<EdgeNote>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StepName {
    id: String,
    name: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DescribeReply {
    elements: Vec<Described>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Described {
    id: String,
    #[serde(default)]
    name: Option<String>,
    fields: Fields,
    #[serde(default)]
    edge_labels: BTreeMap<String, String>,
}

/// Strict JSON after trimming a Markdown fence or chatter around the object.
fn parse_reply<T: for<'de> Deserialize<'de>>(text: &str) -> Result<T, Located> {
    let start = text.find('{');
    let end = text.rfind('}');
    let body = match (start, end) {
        (Some(s), Some(e)) if e > s => &text[s..=e],
        _ => text,
    };
    serde_json::from_str(body).map_err(|e| {
        Located::new(
            "",
            format!("invalid JSON: {e}"),
            "one JSON object matching the stage schema, nothing else",
        )
    })
}

// --- stage B validation and assembly -----------------------------------------------

/// A validated top level: groups over unit nodes, the drawn diagram, edges.
#[derive(Clone, Debug)]
struct TopPlan {
    fields: MapFields,
    groups: Vec<Group>,
    /// Unit nodes no group covers (the bundle).
    uncovered: Vec<String>,
    /// Model-drawn edges between group nodes.
    edges: Vec<MapEdge>,
    /// Groups the model drew (handle order); node ids resolved.
    nodes: Vec<String>,
    /// 13–20 nodes: accepted with this note in the job log (§5.3).
    warning: Option<String>,
}

/// One model-made group: a cluster of units or a single unit.
#[derive(Clone, Debug)]
struct Group {
    node: String,
    name: String,
    members: Vec<String>,
    /// Member unit paths (anchors).
    paths: Vec<String>,
    /// The group as the skeleton sees it (its files).
    child: Child,
}

fn validate_top(
    r: &TopReply,
    facts: &Facts,
    h: &Handles,
    persp: &str,
) -> Result<TopPlan, Vec<Located>> {
    let mut errors = Vec::new();
    let mut seen_members: BTreeMap<&str, &str> = BTreeMap::new();
    let mut groups: Vec<(String, Group)> = Vec::new();
    let mut handles: Vec<String> = Vec::new();
    for (i, n) in r.nodes.iter().enumerate() {
        let path = format!("/nodes/{i}");
        if n.id.trim().is_empty() || handles.contains(&n.id) {
            errors.push(Located::new(
                &path,
                format!("group id {:?} is empty or repeated", n.id),
                "unique ids g1, g2, …",
            ));
            continue;
        }
        if n.name.trim().is_empty() {
            errors.push(Located::new(
                format!("{path}/name"),
                "group has no name",
                "2–4 words",
            ));
        }
        if n.members.is_empty() {
            errors.push(Located::new(
                format!("{path}/members"),
                "group has no members",
                "at least one unit handle",
            ));
        }
        let mut members = Vec::with_capacity(n.members.len());
        let mut paths = Vec::with_capacity(n.members.len());
        for m in &n.members {
            let Some(node) = h.unit_node(m) else {
                errors.push(Located::new(
                    format!("{path}/members"),
                    format!("{m:?} is not a unit handle"),
                    format!("one of n1…n{}", h.units.len()),
                ));
                continue;
            };
            if let Some(other) = seen_members.insert(node, &n.id) {
                errors.push(Located::new(
                    format!("{path}/members"),
                    format!("{m} is already in {other}"),
                    "each unit in exactly one group",
                ));
                continue;
            }
            members.push(node.to_owned());
            if let Some(u) = facts.top_unit(node) {
                paths.push(u.path.clone());
            }
        }
        let node = if members.len() == 1 {
            members[0].clone()
        } else {
            cluster_id(&members)
        };
        let name = n.name.trim().to_owned();
        let Some(child) = facts.group_child(&node, &name, &members) else {
            continue; // members all invalid: reported above
        };
        handles.push(n.id.clone());
        groups.push((
            n.id.clone(),
            Group {
                node,
                name,
                members,
                paths,
                child,
            },
        ));
    }
    if let Some(l) = check_description("/fields/description", &r.fields.description) {
        errors.push(l);
    }
    let g = match surface::graph::from_mermaid(
        &r.mermaid,
        Some(&handles.iter().map(String::as_str).collect::<Vec<_>>()),
    ) {
        Ok(g) => g,
        Err(e) => {
            errors.push(e.into());
            return Err(errors);
        }
    };
    errors.extend(check_diagram(&g, &handles).unwrap_or_default());
    for handle in &handles {
        if !g.nodes.iter().any(|n| n.id == *handle) {
            errors.push(Located::new(
                "/mermaid",
                format!("group {handle} is not drawn"),
                "every group as a node",
            ));
        }
    }
    let group_of = |handle: &str| groups.iter().find(|(hh, _)| hh == handle).map(|(_, g)| g);
    let mut edges = Vec::with_capacity(g.edges.len());
    for (i, e) in g.edges.iter().enumerate() {
        let path = format!("/mermaid/edges/{i}");
        let (Some(from), Some(to)) = (group_of(&e.from), group_of(&e.to)) else {
            continue; // reported by check_diagram
        };
        let note = r.edges.iter().find(|n| n.from == e.from && n.to == e.to);
        let label = note
            .and_then(|n| n.label.clone())
            .or_else(|| e.label.clone())
            .unwrap_or_default();
        if let Ok(Some(l)) = check_edge_label(&path, &label) {
            errors.push(l);
        }
        let mut evidence = facts.evidence_between(&from.child, &to.child);
        if let Some(n) = note {
            for cited in &n.evidence {
                match h.edge_index(cited) {
                    Some(k) => {
                        let ue = &facts.edges[k];
                        if !(from.members.contains(&ue.from) && to.members.contains(&ue.to)) {
                            errors.push(Located::new(
                                format!("/edges/{cited}"),
                                format!("{cited} does not join {} → {}", e.from, e.to),
                                "evidence whose ends lie in the two groups",
                            ));
                        }
                    }
                    None => errors.push(Located::new(
                        format!("/edges/{cited}"),
                        format!("{cited:?} is not an edge handle"),
                        format!("one of e1…e{}", h.edges.len()),
                    )),
                }
            }
        }
        let backed = !evidence.is_empty();
        if !backed {
            match note
                .and_then(|n| n.reason.as_deref())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                Some(reason) => evidence.push(MapEvidence::Inferred {
                    reason: reason.to_owned(),
                }),
                None => errors.push(Located::new(
                    &path,
                    format!("{} → {} has no evidence in the facts", e.from, e.to),
                    "a `reason` in `edges`, or drop the edge",
                )),
            }
        }
        edges.push(MapEdge {
            from: from.node.clone(),
            to: to.node.clone(),
            label,
            backed,
            evidence,
        });
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let uncovered: Vec<String> = h
        .units
        .iter()
        .filter(|(_, node)| !seen_members.contains_key(node.as_str()))
        .map(|(_, node)| node.clone())
        .collect();
    let nodes: Vec<String> = g
        .nodes
        .iter()
        .filter_map(|n| group_of(&n.id).map(|g| g.node.clone()))
        .collect();
    let warning = (g.nodes.len() > DIAGRAM_NODES).then(|| {
        format!(
            "{persp}: {} nodes drawn (aim ≤ {DIAGRAM_NODES})",
            g.nodes.len()
        )
    });
    Ok(TopPlan {
        fields: r.fields.clone().into_fields(),
        groups: groups.into_iter().map(|(_, g)| g).collect(),
        uncovered,
        edges,
        nodes,
        warning,
    })
}

/// §5.6: the structure-only top level — the largest units by share and a
/// bundle, mechanical labels — used when the model's output failed twice.
fn structure_plan(facts: &Facts, fields: MapFields) -> TopPlan {
    let drawn: Vec<&Child> = facts.top.iter().take(DIAGRAM_NODES).collect();
    let groups: Vec<Group> = drawn
        .iter()
        .map(|u| Group {
            node: u.node.clone(),
            name: u.name.clone(),
            members: vec![u.node.clone()],
            paths: vec![u.path.clone()],
            child: (*u).clone(),
        })
        .collect();
    let drawn_nodes: BTreeSet<&str> = drawn.iter().map(|u| u.node.as_str()).collect();
    let edges = facts
        .edges
        .iter()
        .filter(|e| drawn_nodes.contains(e.from.as_str()) && drawn_nodes.contains(e.to.as_str()))
        .cloned()
        .collect();
    TopPlan {
        fields,
        nodes: groups.iter().map(|g| g.node.clone()).collect(),
        groups,
        uncovered: facts
            .top
            .iter()
            .skip(DIAGRAM_NODES)
            .map(|u| u.node.clone())
            .collect(),
        edges,
        warning: None,
    }
}

/// Abbreviated anchors for a node's second line: first two joined by ` · `,
/// then `+N`, cut to the spec's `sub` width.
fn sub_line(paths: &[String]) -> Option<String> {
    if paths.is_empty() {
        return None;
    }
    let mut s = paths
        .iter()
        .take(2)
        .cloned()
        .collect::<Vec<_>>()
        .join(" · ");
    if paths.len() > 2 {
        s.push_str(&format!(" +{}", paths.len() - 2));
    }
    Some(cut(&s, surface::MAX_SUB_CHARS))
}

fn graph_node(id: &str, label: &str, paths: &[String], tag: Option<String>) -> GraphNode {
    GraphNode {
        id: id.to_owned(),
        label: cut(label, surface::MAX_GRAPH_LABEL_CHARS),
        kind: Some(GraphKind::Box),
        group: None,
        state: None,
        r#ref: paths.first().cloned(),
        sub: sub_line(paths),
        tag: tag.map(|t| cut(&t, surface::MAX_TAG_CHARS)),
        icon: None,
    }
}

/// A stored edge as drawn: the model's *why* (or the skeleton's mechanical
/// label), dashed when inferred.
fn graph_edge(e: &MapEdge) -> GraphEdge {
    GraphEdge {
        from: e.from.clone(),
        to: e.to.clone(),
        label: (!e.label.is_empty()).then(|| e.label.clone()),
        style: Some(if e.backed {
            GraphEdgeStyle::Solid
        } else {
            GraphEdgeStyle::Dashed
        }),
        state: None,
        arrow: None,
    }
}

fn graph(nodes: Vec<GraphNode>, edges: &[MapEdge]) -> GraphProps {
    GraphProps {
        nodes,
        edges: edges.iter().map(graph_edge).collect(),
        groups: None,
        direction: Some(GraphDirection::LR),
        legend: None,
        title: None,
    }
}

/// The element rows of one perspective from a validated top level: the root
/// (depth 0, the model's diagram), the groups (depth 1, the model's — stored
/// `generated` so `recompute` keeps their names and members), and the
/// skeleton's structure below them to `max_depth` (§5.5).
fn build_tree(
    facts: &Facts,
    p: &Planned,
    plan: &TopPlan,
    prov: MapProvenance,
    error: Option<String>,
    max_depth: u8,
) -> Vec<ElementRow> {
    let root_node = format!("perspective:{}", p.id);
    let mut rows = Vec::new();
    let mut nodes: Vec<GraphNode> = plan
        .nodes
        .iter()
        .filter_map(|node| plan.groups.iter().find(|g| g.node == *node))
        .map(|g| graph_node(&g.node, &g.name, &g.paths, None))
        .collect();
    let bundle_node = format!("bundle:{root_node}");
    let bundle = (!plan.uncovered.is_empty())
        .then(|| {
            facts.group_child(
                &bundle_node,
                &format!("+{} more", plan.uncovered.len()),
                &plan.uncovered,
            )
        })
        .flatten();
    let mut root_edges = plan.edges.clone();
    if let Some(b) = &bundle {
        nodes.push(graph_node(
            &b.node,
            &b.name,
            &[],
            Some(format!("+{}", plan.uncovered.len())),
        ));
        // Edges the skeleton holds into the bundle, counted beside the model's.
        for g in &plan.groups {
            let to_bundle = facts.evidence_between(&g.child, b);
            if !to_bundle.is_empty() {
                root_edges.push(MapEdge {
                    from: g.node.clone(),
                    to: b.node.clone(),
                    label: String::new(),
                    backed: true,
                    evidence: to_bundle,
                });
            }
        }
    }
    rows.push(ElementRow {
        id: p.id.clone(),
        perspective: p.id.clone(),
        node: root_node.clone(),
        parent: None,
        depth: 0,
        order_no: p.order_no,
        name: p.title.clone(),
        path: None,
        kind: "perspective".to_owned(),
        members: Vec::new(),
        leaf: false,
        origin: if error.is_some() {
            MapOrigin::Structure
        } else {
            MapOrigin::Generated
        },
        facts_hash: prov.facts_hash.clone(),
        diagram: Some(graph(nodes, &root_edges)),
        fields: plan.fields.clone(),
        provenance: Some(prov),
        edges: root_edges,
        anchors: Vec::new(),
        error,
    });
    for (i, g) in plan.groups.iter().enumerate() {
        let kind = if g.members.len() == 1 {
            "unit"
        } else {
            "cluster"
        };
        let members = if kind == "cluster" {
            g.members.clone()
        } else {
            Vec::new()
        };
        let mut group_rows = structure_rows(
            facts,
            &p.id,
            &p.id,
            1,
            u32::try_from(i).unwrap_or(u32::MAX),
            &g.child,
            kind,
            members,
            g.paths.clone(),
            max_depth,
        );
        // The partition and its names are the model's: `generated` keeps them
        // through the skeleton's recompute (W1 rewrites `structure` rows' names).
        if let Some(group) = group_rows.first_mut() {
            group.origin = MapOrigin::Generated;
        }
        rows.extend(group_rows);
    }
    if let Some(b) = &bundle {
        let paths: Vec<String> = plan
            .uncovered
            .iter()
            .filter_map(|n| n.strip_prefix("unit:").map(str::to_owned))
            .collect();
        // One leaf row: the bundle's door is its member list (O4).
        let mut bundle_rows = structure_rows(
            facts,
            &p.id,
            &p.id,
            1,
            u32::try_from(plan.groups.len()).unwrap_or(u32::MAX),
            b,
            "bundle",
            plan.uncovered.clone(),
            paths,
            max_depth,
        );
        if let Some(row) = bundle_rows.first_mut() {
            row.origin = MapOrigin::Generated;
        }
        rows.extend(bundle_rows);
    }
    rows
}

/// One structure element (basenames, the skeleton's mechanical labels,
/// `origin: Structure`) and, while `depth < max_depth`, its children the same
/// way; children at `max_depth` are leaves.
#[allow(clippy::too_many_arguments)]
fn structure_rows(
    facts: &Facts,
    persp: &str,
    parent: &str,
    depth: u8,
    order_no: u32,
    child: &Child,
    kind: &str,
    members: Vec<String>,
    anchors: Vec<String>,
    max_depth: u8,
) -> Vec<ElementRow> {
    let id = element_id(persp, &child.node);
    let grouped = kind == "cluster" || kind == "bundle";
    // Doors (O4): a cluster or a unit with more than a leaf's files opens a
    // diagram; a bundle opens its member list — a leaf here, as in W1's
    // skeleton, so recompute never has rows to delete under it.
    let descend = depth < max_depth && (kind == "cluster" || (!grouped && !Facts::is_leaf(child)));
    let sub = if descend {
        facts.sub_skeleton(
            facts.children_for(kind, Some(&child.path), &members),
            Facts::cap(grouped),
        )
    } else {
        Default::default()
    };
    let leaf = !descend || sub.children.is_empty();
    let mut rows = Vec::new();
    let (diagram, edges) = if leaf {
        (None, Vec::new())
    } else {
        let mut nodes: Vec<GraphNode> = sub
            .children
            .iter()
            .map(|c| graph_node(&c.node, &c.name, std::slice::from_ref(&c.path), None))
            .collect();
        if !sub.bundled.is_empty() {
            let b = format!("bundle:{}", child.node);
            nodes.push(graph_node(
                &b,
                &format!("+{} more", sub.bundled.len()),
                &[],
                Some(format!("+{}", sub.bundled.len())),
            ));
        }
        (Some(graph(nodes, &sub.edges)), sub.edges.clone())
    };
    rows.push(ElementRow {
        id: id.clone(),
        perspective: persp.to_owned(),
        node: child.node.clone(),
        parent: Some(parent.to_owned()),
        depth,
        order_no,
        name: child.name.clone(),
        path: (!grouped || !child.path.is_empty()).then(|| child.path.clone()),
        kind: kind.to_owned(),
        members,
        leaf,
        origin: MapOrigin::Structure,
        facts_hash: facts.facts_hash(&child.files, &sub.edges),
        diagram,
        fields: MapFields::default(),
        provenance: None,
        edges,
        anchors: anchors.iter().map(|a| (a.clone(), None, None)).collect(),
        error: None,
    });
    if leaf {
        return rows;
    }
    for (i, c) in sub.children.iter().enumerate() {
        rows.extend(structure_rows(
            facts,
            persp,
            &id,
            depth + 1,
            u32::try_from(i).unwrap_or(u32::MAX),
            c,
            c.kind,
            Vec::new(),
            vec![c.path.clone()],
            max_depth,
        ));
    }
    if !sub.bundled.is_empty() {
        let b = format!("bundle:{}", child.node);
        let files: Vec<usize> = sub
            .bundled
            .iter()
            .flat_map(|c| c.files.iter().copied())
            .collect();
        rows.push(ElementRow {
            id: element_id(persp, &b),
            perspective: persp.to_owned(),
            node: b,
            parent: Some(id),
            depth: depth + 1,
            order_no: u32::try_from(sub.children.len()).unwrap_or(u32::MAX),
            name: format!("+{} more", sub.bundled.len()),
            path: None,
            kind: "bundle".to_owned(),
            members: sub.bundled.iter().map(|c| c.node.clone()).collect(),
            leaf: true,
            origin: MapOrigin::Structure,
            facts_hash: facts.facts_hash(&files, &[]),
            diagram: None,
            fields: MapFields::default(),
            provenance: None,
            edges: Vec::new(),
            anchors: sub
                .bundled
                .iter()
                .map(|c| (c.path.clone(), None, None))
                .collect(),
            error: None,
        });
    }
    rows
}

// --- stage D validation and assembly -----------------------------------------------

#[derive(Clone, Debug)]
struct FlowPlan {
    title: String,
    fields: MapFields,
    /// Symbol handle → name.
    names: BTreeMap<String, String>,
    /// Drawn symbol nodes, diagram order.
    nodes: Vec<String>,
    edges: Vec<MapEdge>,
    /// The chain's call edges as skeleton edges (facts hash).
    skeleton_edges: Vec<MapEdge>,
}

fn call_evidence(c: &super::generator_facts::CallSite) -> MapEvidence {
    MapEvidence::Call {
        site: c.site.clone(),
        from: c.from.trim_start_matches("sym:").to_owned(),
        to: c.to.trim_start_matches("sym:").to_owned(),
    }
}

fn flow_skeleton_edges(flow: &Flow) -> Vec<MapEdge> {
    flow.calls
        .iter()
        .map(|c| MapEdge {
            from: c.from.clone(),
            to: c.to.clone(),
            label: String::new(),
            backed: true,
            evidence: vec![call_evidence(c)],
        })
        .collect()
}

fn validate_flow(r: &FlowReply, flow: &Flow, handles: &[String]) -> Result<FlowPlan, Vec<Located>> {
    let mut errors = Vec::new();
    if r.title.trim().is_empty() {
        errors.push(Located::new(
            "/title",
            "flow has no title",
            "2–6 words naming the flow",
        ));
    }
    if let Some(l) = check_description("/fields/description", &r.fields.description) {
        errors.push(l);
    }
    let mut names = BTreeMap::new();
    for (i, s) in r.steps.iter().enumerate() {
        if !handles.contains(&s.id) {
            errors.push(Located::new(
                format!("/steps/{i}/id"),
                format!("{:?} is not a symbol handle", s.id),
                format!("one of s1…s{}", handles.len()),
            ));
        }
        names.insert(s.id.clone(), s.name.trim().to_owned());
    }
    let g = match surface::graph::from_mermaid(
        &r.mermaid,
        Some(&handles.iter().map(String::as_str).collect::<Vec<_>>()),
    ) {
        Ok(g) => g,
        Err(e) => {
            errors.push(e.into());
            return Err(errors);
        }
    };
    errors.extend(check_diagram(&g, handles).unwrap_or_default());
    let node_of = |h: &str| {
        handles
            .iter()
            .position(|x| x == h)
            .map(|i| flow.symbols[i].node.clone())
    };
    let mut edges = Vec::with_capacity(g.edges.len());
    for (i, e) in g.edges.iter().enumerate() {
        let (Some(from), Some(to)) = (node_of(&e.from), node_of(&e.to)) else {
            continue;
        };
        let path = format!("/mermaid/edges/{i}");
        let note = r.edges.iter().find(|n| n.from == e.from && n.to == e.to);
        let label = note
            .and_then(|n| n.label.clone())
            .or_else(|| e.label.clone())
            .unwrap_or_default();
        if let Ok(Some(l)) = check_edge_label(&path, &label) {
            errors.push(l);
        }
        let mut evidence: Vec<MapEvidence> = flow
            .calls
            .iter()
            .filter(|c| c.from == from && c.to == to)
            .map(call_evidence)
            .collect();
        let backed = !evidence.is_empty();
        if !backed {
            match note
                .and_then(|n| n.reason.as_deref())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                Some(reason) => evidence.push(MapEvidence::Inferred {
                    reason: reason.to_owned(),
                }),
                None => errors.push(Located::new(
                    &path,
                    format!("{} → {} is not a call in the chain", e.from, e.to),
                    "a `reason` in `edges` (a return path, an event), or drop the edge",
                )),
            }
        }
        edges.push(MapEdge {
            from,
            to,
            label,
            backed,
            evidence,
        });
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(FlowPlan {
        title: r.title.trim().to_owned(),
        fields: r.fields.clone().into_fields(),
        names,
        nodes: g.nodes.iter().filter_map(|n| node_of(&n.id)).collect(),
        edges,
        skeleton_edges: flow_skeleton_edges(flow),
    })
}

/// The chain as the index saw it, when the model's flow failed twice.
fn structure_flow(flow: &Flow, fields: MapFields) -> FlowPlan {
    let skeleton_edges = flow_skeleton_edges(flow);
    FlowPlan {
        title: format!("{} flow", flow.entry.name),
        fields,
        names: BTreeMap::new(),
        nodes: flow
            .symbols
            .iter()
            .take(DIAGRAM_NODES)
            .map(|s| s.node.clone())
            .collect(),
        edges: skeleton_edges.clone(),
        skeleton_edges,
    }
}

fn build_flow(
    p: &Planned,
    flow: &Flow,
    plan: &FlowPlan,
    prov: MapProvenance,
    error: Option<String>,
    order_no: u32,
) -> Vec<ElementRow> {
    let id = element_id(&p.id, &flow.entry.node);
    let handles: Vec<String> = (1..=flow.symbols.len()).map(|n| format!("s{n}")).collect();
    let name_of = |s: &super::generator_facts::FlowSymbol| -> String {
        flow.symbols
            .iter()
            .position(|x| x.node == s.node)
            .and_then(|i| handles.get(i))
            .and_then(|h| plan.names.get(h))
            .cloned()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| s.name.clone())
    };
    let drawn: Vec<&super::generator_facts::FlowSymbol> = plan
        .nodes
        .iter()
        .filter(|n| {
            plan.edges.is_empty() || plan.edges.iter().any(|e| e.from == **n || e.to == **n)
        })
        .filter_map(|n| flow.symbols.iter().find(|s| s.node == *n))
        .collect();
    let nodes: Vec<GraphNode> = drawn
        .iter()
        .map(|s| {
            let mut n = graph_node(&s.node, &name_of(s), std::slice::from_ref(&s.path), None);
            n.r#ref = Some(format!("{}#L{}-L{}", s.path, s.line_start, s.line_end));
            n.kind = Some(GraphKind::Rounded);
            n
        })
        .collect();
    let mut rows = vec![ElementRow {
        id: id.clone(),
        perspective: p.id.clone(),
        node: flow.entry.node.clone(),
        parent: Some(p.id.clone()),
        depth: 1,
        order_no,
        name: plan.title.clone(),
        path: Some(flow.entry.path.clone()),
        kind: "symbol".to_owned(),
        members: drawn.iter().map(|s| s.node.clone()).collect(),
        leaf: false,
        origin: if error.is_some() {
            MapOrigin::Structure
        } else {
            MapOrigin::Generated
        },
        facts_hash: prov.facts_hash.clone(),
        diagram: Some(graph(nodes, &plan.edges)),
        fields: plan.fields.clone(),
        provenance: Some(prov),
        edges: plan.edges.clone(),
        anchors: vec![(
            flow.entry.path.clone(),
            Some(flow.entry.line_start),
            Some(flow.entry.line_end),
        )],
        error,
    }];
    for (i, s) in drawn.iter().enumerate() {
        rows.push(ElementRow {
            id: element_id(&p.id, &s.node),
            perspective: p.id.clone(),
            node: s.node.clone(),
            parent: Some(id.clone()),
            depth: 2,
            order_no: u32::try_from(i).unwrap_or(u32::MAX),
            name: name_of(s),
            path: Some(s.path.clone()),
            kind: "symbol".to_owned(),
            members: Vec::new(),
            leaf: true,
            origin: MapOrigin::Structure,
            facts_hash: String::new(),
            diagram: None,
            fields: MapFields::default(),
            provenance: None,
            edges: Vec::new(),
            anchors: vec![(s.path.clone(), Some(s.line_start), Some(s.line_end))],
            error: None,
        });
    }
    rows
}

// --- stage C validation ---------------------------------------------------------

struct DescribeItem {
    id: String,
    kind: String,
    name: String,
    path: Option<String>,
    members: Vec<String>,
    anchors: Vec<String>,
    /// The element's facts hash at describe time.
    hash: String,
    triggers: Vec<String>,
    children: Vec<(Child, Vec<String>)>,
    edges: Vec<MapEdge>,
}

fn validate_describe(
    r: &DescribeReply,
    items: &[DescribeItem],
) -> Result<Vec<Described>, Vec<Located>> {
    let mut errors = Vec::new();
    let mut seen = BTreeSet::new();
    for (i, d) in r.elements.iter().enumerate() {
        let path = format!("/elements/{i}");
        let Some(item) = items.iter().find(|it| it.id == d.id) else {
            errors.push(Located::new(
                format!("{path}/id"),
                format!("{:?} is not in this batch", d.id),
                "one of the listed element ids",
            ));
            continue;
        };
        if !seen.insert(d.id.as_str()) {
            errors.push(Located::new(
                format!("{path}/id"),
                format!("{:?} twice", d.id),
                "each element once",
            ));
        }
        if let Some(l) =
            check_description(&format!("{path}/fields/description"), &d.fields.description)
        {
            errors.push(l);
        }
        for (handle, label) in &d.edge_labels {
            let k = handle
                .strip_prefix('e')
                .and_then(|n| n.parse::<usize>().ok())
                .and_then(|n| n.checked_sub(1));
            if k.is_none_or(|k| k >= item.edges.len()) {
                errors.push(Located::new(
                    format!("{path}/edge_labels/{handle}"),
                    format!("{handle:?} is not an edge handle of {}", d.id),
                    format!("one of e1…e{}", item.edges.len()),
                ));
            }
            if let Ok(Some(l)) = check_edge_label(&format!("{path}/edge_labels/{handle}"), label) {
                errors.push(l);
            }
        }
    }
    for item in items {
        if !seen.contains(item.id.as_str()) {
            errors.push(Located::new(
                "/elements",
                format!("{} is missing", item.id),
                "one entry per listed element",
            ));
        }
    }
    if errors.is_empty() {
        Ok(r.elements.clone())
    } else {
        Err(errors)
    }
}

// --- prompts (§6.3; pure functions of the catalogue and the facts) ----------------------

/// The catalogue row's question (§7).
pub fn question_of(kind: &str) -> &'static str {
    match kind {
        "overall-architecture" => "what exists and how the pieces relate",
        "request-lifecycle" => "how a request enters and is handled end to end",
        "data-flow" => "where data comes from, how it transforms, where it lands",
        "dependency-map" => "what depends on what, what is shared",
        "external-integrations" => "what the system connects to and why",
        "state-transitions" => "how state changes and what triggers it",
        "route-page-map" => "page structure and navigation",
        "command-surface" => "command hierarchy and dispatch",
        "extension-points" => "extension architecture and registry",
        "pipeline" => "stage topology",
        "orchestration" => "publisher / subscriber / broker topology",
        "storage" => "storage topology",
        _ => "",
    }
}

/// Title case of a kind (`overall-architecture` → `Overall architecture`).
pub fn title_of(kind: &str) -> String {
    let mut s = kind.replace('-', " ");
    if let Some(c) = s.get_mut(0..1) {
        c.make_ascii_uppercase();
    }
    s
}

/// The system prompt of every generator call (§6.3).
pub fn system_prompt() -> String {
    let mut s = String::from(
        "You name and explain the structure of a codebase that an index has already measured. \
         You never invent a node, a path or an edge: use only the handles the facts give you \
         (units n1…, edges e1…, symbols s1…) and the group ids g1… you define.\n\
         Rules:\n\
         - Every edge is labelled with WHY it exists, in at most four words. Never put a number or a \
           count word (deps, imports, refs, calls) in a label: the engine renders counts from the evidence.\n\
         - Node labels are built by the engine from the `name` you give (2–4 words, Title case) and the \
           paths it knows; in Mermaid write bare handles: `g1 -->|\"sends user input\"| g2`.\n\
         - At most 12 nodes per diagram (hard cap 20): merge or leave units to the bundle instead.\n\
         - Diagrams are `flowchart LR` over handles only; `-->` for an edge the facts back, `-.->` for one they do not. \
           No classDef, style, click, or node labels; subgraphs are not needed.\n\
         - An edge the facts do not back (no manifest dep, import or ref between the two sides) needs a `reason` \
           entry in `edges`; it is drawn dashed. Cite the `evidence` handles you relied on when you can.\n\
         - Do not force a perspective the code does not evidence; a perspective is a question — choose it only \
           when the facts answer it, and say why.\n\
         - Prose: `description` is 2–5 plain sentences saying what the thing is and which paths it covers; \
           `concern` only when a listed trigger trips; `context` / `constraint` only when the facts show why.\n\
         - Output exactly one JSON object matching the stage's schema — no Markdown fence, no commentary.\n\
         Catalogue (kind — the question it answers):\n",
    );
    for kind in PERSPECTIVES {
        s.push_str(&format!("- {kind} — {}\n", question_of(kind)));
    }
    s
}

fn detection_json(d: &Detection) -> serde_json::Value {
    json!({ "kind": d.kind, "eligible": d.eligible, "hits": d.hits, "samples": d.samples, "confidence": d.confidence })
}

fn units_json(facts: &Facts, h: &Handles) -> Vec<serde_json::Value> {
    h.units
        .iter()
        .filter_map(|(handle, node)| facts.top_unit(node).map(|u| (handle, u)))
        .map(|(handle, u)| {
            let m = facts.measures(u);
            let indeg = facts.edges.iter().filter(|e| e.to == u.node).count();
            let outdeg = facts.edges.iter().filter(|e| e.from == u.node).count();
            json!({
                "handle": handle, "path": u.path, "kind": u.kind, "member": u.member,
                "files": m.files, "lines": m.lines, "symbols": m.symbols, "tests": m.tests,
                "share": (m.share * 1000.0).round() / 1000.0,
                "in": indeg, "out": outdeg, "entry": m.entry, "recent": m.recent,
            })
        })
        .collect()
}

fn edges_json(facts: &Facts, h: &Handles) -> Vec<serde_json::Value> {
    h.edges
        .iter()
        .map(|(handle, i)| {
            let e = &facts.edges[*i];
            json!({
                "handle": handle,
                "from": h.unit_handle(&e.from).unwrap_or("?"),
                "to": h.unit_handle(&e.to).unwrap_or("?"),
                "evidence": e.label,
            })
        })
        .collect()
}

/// Stage A's user message.
fn select_message(facts: &Facts) -> String {
    let h = Handles::new(&facts.top, &facts.edges);
    let mut hubs: Vec<(&str, usize)> = h
        .units
        .iter()
        .map(|(_, node)| {
            (
                node.as_str(),
                facts.edges.iter().filter(|e| e.to == *node).count(),
            )
        })
        .collect();
    hubs.sort_by_key(|h| std::cmp::Reverse(h.1));
    let out = json!({
        "stage": "select",
        "task": "Choose the perspectives this codebase evidences. `overall-architecture` is always included. Pick any number of the eligible kinds; decline eligible ones you would be forcing.",
        "schema": { "perspectives": [{ "kind": "<catalogue kind>", "title": "<display title>", "why": "<one sentence: what in the facts answers this question>" }] },
        "profile": facts.profile,
        "units": units_json(facts, &h),
        "edges_summary": { "kept": facts.edges.len(), "folded_minor": facts.folded },
        "hubs": hubs.iter().take(8).map(|(n, d)| json!({ "unit": h.unit_handle(n).unwrap_or("?"), "in_degree": d })).collect::<Vec<_>>(),
        "perspectives": facts.detections.iter().map(detection_json).collect::<Vec<_>>(),
        "imported": facts.imported.iter().map(|(t, d)| json!({ "title": t, "description": d })).collect::<Vec<_>>(),
    });
    serde_json::to_string_pretty(&out).unwrap_or_default()
}

/// A candidate tiering by in/out-degree (§5.7): a hint, not a partition.
fn tiers_json(facts: &Facts, h: &Handles) -> serde_json::Value {
    let (mut base, mut assemblers, mut rest) = (Vec::new(), Vec::new(), Vec::new());
    for (handle, node) in &h.units {
        let indeg = facts.edges.iter().filter(|e| e.to == *node).count();
        let outdeg = facts.edges.iter().filter(|e| e.from == *node).count();
        if indeg >= 15 {
            base.push(handle.as_str());
        } else if outdeg >= 25 {
            assemblers.push(handle.as_str());
        } else {
            rest.push(handle.as_str());
        }
    }
    json!({ "base_by_in_degree": base, "assemblers_by_out_degree": assemblers, "rest": rest })
}

fn flows_json(flows: &[Flow], named: bool) -> Vec<serde_json::Value> {
    flows
        .iter()
        .map(|f| {
            let handles: Vec<String> = (1..=f.symbols.len()).map(|n| format!("s{n}")).collect();
            let handle_of = |node: &str| {
                f.symbols
                    .iter()
                    .position(|s| s.node == node)
                    .map(|i| handles[i].clone())
                    .unwrap_or_default()
            };
            json!({
                "entry": { "path": f.entry.path, "symbol": f.entry.name, "line": f.entry.line_start },
                "symbols": f.symbols.iter().enumerate().map(|(i, s)| {
                    let mut v = json!({ "path": s.path, "name": s.name, "line": s.line_start, "signature": s.signature });
                    if named { v["handle"] = json!(handles[i]); }
                    v
                }).collect::<Vec<_>>(),
                "calls": f.calls.iter().map(|c| json!({ "from": handle_of(&c.from), "to": handle_of(&c.to), "site": c.site })).collect::<Vec<_>>(),
            })
        })
        .collect()
}

/// Stage B's user message.
fn top_message(
    facts: &Facts,
    p: &Planned,
    h: &Handles,
    detection: Option<&Detection>,
    flows: &[Flow],
) -> String {
    let mut out = json!({
        "stage": "top",
        "perspective": { "kind": p.kind, "title": p.title, "question": question_of(&p.kind), "why": p.why },
        "task": format!("Partition the units into at most {DIAGRAM_NODES} named groups that answer the question, draw the groups as a flowchart with labelled edges, and describe the whole. Units you leave out of every group are shown as one counted bundle; say in the description what the bundle holds."),
        "schema": {
            "fields": { "description": "<2–5 sentences>", "context": "<optional>", "constraint": "<optional>", "concern": "<optional, only on a trigger>" },
            "nodes": [{ "id": "g1", "name": "<2–4 words>", "members": ["n3", "n7"] }],
            "mermaid": "flowchart LR\n  g1 -->|\"why\"| g2\n  g2 -.->|\"why\"| g3",
            "edges": [{ "from": "g1", "to": "g2", "evidence": ["e4", "e9"] }, { "from": "g2", "to": "g3", "reason": "<why you believe it despite no evidence>" }]
        },
        "profile": facts.profile,
        "units": units_json(facts, h),
        "edges": edges_json(facts, h),
        "minor_edges_folded": facts.folded,
        "tiers": tiers_json(facts, h),
    });
    if let Some(d) = detection {
        out["detector"] = detection_json(d);
    }
    if !flows.is_empty() {
        out["flows"] = json!(flows_json(flows, false));
    }
    serde_json::to_string_pretty(&out).unwrap_or_default()
}

/// Stage D's user message.
fn flow_message(
    facts: &Facts,
    p: &Planned,
    flow: &Flow,
    handles: &[String],
    detection: Option<&Detection>,
) -> String {
    let one = flows_json(std::slice::from_ref(flow), true);
    let mut out = json!({
        "stage": "flow",
        "perspective": { "kind": p.kind, "title": p.title, "question": question_of(&p.kind) },
        "task": format!("Name this flow and its steps, draw the symbols that matter (≤ {DIAGRAM_NODES}) as a flowchart in call order with edges labelled by what each call does, and describe the flow. Return paths or event hops the chain does not contain are allowed as `-.->` edges with a reason."),
        "schema": {
            "title": "<2–6 words>",
            "fields": { "description": "<2–5 sentences>", "context": "<optional>", "concern": "<optional>" },
            "steps": [{ "id": "s1", "name": "<short step name>" }],
            "mermaid": "flowchart LR\n  s1 -->|\"parses args\"| s2",
            "edges": [{ "from": "s2", "to": "s1", "reason": "returns the reply" }]
        },
        "profile": facts.profile,
        "flow": one.into_iter().next().unwrap_or_default(),
        "handles": handles,
    });
    if let Some(d) = detection {
        out["detector"] = detection_json(d);
    }
    serde_json::to_string_pretty(&out).unwrap_or_default()
}

/// Stage C's user message.
fn describe_message(facts: &Facts, items: &[DescribeItem]) -> String {
    let out = json!({
        "stage": "describe",
        "task": "For every listed element: give it a name (2–4 words; keep the current one when it is already a good name), write its prose, and label each of its edges with why it exists. Edge handles are per element.",
        "schema": { "elements": [{ "id": "<element id>", "name": "<2–4 words>", "fields": { "description": "<2–5 sentences>", "concern": "<optional, only on a trigger>", "context": "<optional>", "constraint": "<optional>" }, "edge_labels": { "e1": "<why, ≤ 4 words>" } }] },
        "profile": facts.profile,
        "elements": items.iter().map(|it| json!({
            "id": it.id,
            "kind": it.kind,
            "current_name": it.name,
            "path": it.path,
            "members": it.members,
            "paths": it.anchors,
            "children": it.children.iter().map(|(c, top)| {
                let m = facts.measures(c);
                json!({
                    "node": c.node, "name": c.name, "path": c.path, "kind": c.kind, "files": m.files, "lines": m.lines,
                    "top_symbols": top,
                })
            }).collect::<Vec<_>>(),
            "edges": it.edges.iter().enumerate().map(|(i, e)| json!({
                "handle": format!("e{}", i + 1), "from": e.from, "to": e.to, "evidence": e.label,
            })).collect::<Vec<_>>(),
            "concern_triggers": it.triggers,
        })).collect::<Vec<_>>(),
    });
    serde_json::to_string_pretty(&out).unwrap_or_default()
}

#[cfg(test)]
#[path = "generator_tests.rs"]
mod tests;
