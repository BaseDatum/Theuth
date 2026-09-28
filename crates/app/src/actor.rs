//! The [`Apps`] actor (46 §6.3–§6.4, §7): serves committed apps to clients
//! and runs their Query/Mutation calls through [`tools::AppInvoke`].
//!
//! The mailbox only does bookkeeping ([`sched::Scheduler`]) and cache hits.
//! Everything that awaits another actor or a file — loading a record,
//! claiming a press, the tool body, status lookups — runs in a spawned task
//! and comes back as a message. The client's reply handle is held by the
//! scheduler until the call finishes, is superseded, or its view closes.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use artifact::{
    ARTIFACT_ACTOR, Artifact, ArtifactActor, Claim, Invocation, InvocationStatus, Kind,
};
use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use kameo::reply::{DelegatedReply, ReplySender};
use link::RpcPeer;
use parking_lot::Mutex;
use runtime::Runtime;
use tools::{
    AppInvoke, AppInvoked, REGISTRY_ACTOR, ToolCall, ToolOutput, ToolRegistry, UserActivation,
};

use crate::helper::Helper;
use crate::meta::{self, AppMeta, MAX_ARGS, MAX_META, MAX_RESPONSE, MAX_SOURCE};
use crate::sched::{Admit, Bind, Run, Scheduler};
use crate::{
    AppBinding, AppCallChanged, AppDiagnostic, AppDoc, AppRecentCall, AppToolResult, CallArgs,
    CallStatus,
};

/// Parsed records kept in memory so a 5 s refresh does not reread two files.
const DOC_CACHE: usize = 64;

/// A committed app record read back from the store.
#[derive(Debug)]
pub struct Loaded {
    /// The record.
    pub artifact: Artifact,
    /// Canonical source.
    pub source: String,
    /// Its sidecar.
    pub meta: AppMeta,
}

/// State shared by the actor, its tasks and the `app` tool.
pub struct Core {
    /// Runtime (bus, data dir).
    pub rt: Runtime,
    /// The parser helper.
    pub helper: Arc<Helper>,
    docs: Mutex<HashMap<String, Arc<Loaded>>>,
    /// Bumped by [`Self::forget_session`]; a load that spans a bump re-verifies.
    epoch: AtomicU64,
}

impl Core {
    /// New, empty cache.
    pub fn new(rt: Runtime, helper: Arc<Helper>) -> Self {
        Self {
            rt,
            helper,
            docs: Mutex::new(HashMap::new()),
            epoch: AtomicU64::new(0),
        }
    }

    /// The artifact actor, or why not.
    pub fn artifacts(&self) -> Result<ActorRef<ArtifactActor>, String> {
        ActorRef::<ArtifactActor>::lookup(ARTIFACT_ACTOR)
            .ok()
            .flatten()
            .ok_or_else(|| "artifact store is not running".to_owned())
    }

    /// The tool registry, or why not.
    pub fn registry(&self) -> Result<ActorRef<ToolRegistry>, String> {
        ActorRef::<ToolRegistry>::lookup(REGISTRY_ACTOR)
            .ok()
            .flatten()
            .ok_or_else(|| "tool registry is not running".to_owned())
    }

    /// Remember a record just published or loaded.
    pub fn remember(&self, loaded: Arc<Loaded>) {
        let mut docs = self.docs.lock();
        if docs.len() >= DOC_CACHE && !docs.contains_key(&loaded.artifact.call_id) {
            // ponytail: evict an arbitrary entry; LRU when a harness really opens >64 apps at once.
            if let Some(k) = docs.keys().next().cloned() {
                docs.remove(&k);
            }
        }
        docs.insert(loaded.artifact.call_id.clone(), loaded);
    }

    fn cached(&self, record: &str) -> Option<Arc<Loaded>> {
        self.docs.lock().get(record).cloned()
    }

    /// Drop every cached record of `session` (deleted) and invalidate loads in flight.
    pub fn forget_session(&self, session: runtime::SessionId) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        self.docs
            .lock()
            .retain(|_, d| d.artifact.session != session);
    }

    /// The record `call_id` as a committed app: cached, or read from the
    /// store with bounded file reads. A session deleted while the read was
    /// in progress ([`Self::forget_session`] bumped the epoch) makes the
    /// row be looked up again before anything is cached, so a stale record
    /// can never be remembered.
    pub async fn load(&self, record: &str) -> Result<Arc<Loaded>, String> {
        if let Some(l) = self.cached(record) {
            return Ok(l);
        }
        let epoch = self.epoch.load(Ordering::Acquire);
        let artifacts = self.artifacts()?;
        let get = || {
            artifacts.ask(artifact::GetRecord {
                call_id: record.to_owned(),
            })
        };
        let artifact = get().await.map_err(|e| e.to_string())?;
        let loaded = Arc::new(read(artifact).await?);
        if self.epoch.load(Ordering::Acquire) != epoch {
            get()
                .await
                .map_err(|e| format!("{e} (deleted while loading)"))?;
        }
        self.remember(loaded.clone());
        Ok(loaded)
    }

    /// [`Self::load`] plus live binding diagnostics: a bound tool that is
    /// now missing, rebound or denied is visible before the first call.
    pub async fn doc(&self, record: &str) -> Result<AppDoc, String> {
        let l = self.load(record).await?;
        let mut diagnostics = l.meta.diagnostics.clone();
        if let Ok(reg) = self.registry() {
            let mut seen: HashMap<&str, Option<AppDiagnostic>> = HashMap::new();
            for b in &l.meta.bindings {
                let live = match seen.get(b.tool.as_str()) {
                    Some(d) => d.clone(),
                    None => {
                        let d = live_diagnostic(&reg, &l.artifact, b).await;
                        seen.insert(b.tool.as_str(), d.clone());
                        d
                    }
                };
                if let Some(mut d) = live {
                    d.at = b.statement_id.clone();
                    diagnostics.push(d);
                }
            }
        }
        // Pressed outcomes survive restarts and reconnects; a reopened view
        // shows them without replaying anything.
        let recent_calls = self
            .artifacts()?
            .ask(artifact::ListInvocations {
                record_call_id: record.to_owned(),
                limit: artifact::MAX_LIST_INVOCATIONS,
            })
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|inv| {
                let statement_id = inv.statement_id.clone();
                let tool = inv.tool.clone();
                let r = from_invocation(inv);
                AppRecentCall {
                    request_id: r.request_id,
                    statement_id,
                    tool,
                    status: r.status,
                    content: r.content,
                    structured: r.structured,
                    is_error: r.is_error,
                }
            })
            .collect();
        Ok(AppDoc {
            artifact: l.artifact.clone(),
            source: l.source.clone(),
            library_hash: l.meta.library_hash.clone(),
            bindings: l.meta.bindings.clone(),
            diagnostics,
            state: l.meta.state.clone(),
            recent_calls,
        })
    }
}

/// Read one record's source and sidecar under their caps.
async fn read(artifact: Artifact) -> Result<Loaded, String> {
    if artifact.kind != Kind::App {
        return Err(format!(
            "{} is a {} artifact, not an app",
            artifact.call_id,
            artifact.kind.as_str()
        ));
    }
    let source = meta::read_bounded(&artifact.path, MAX_SOURCE).await?;
    let source = String::from_utf8(source).map_err(|e| format!("app source: {e}"))?;
    let side = artifact
        .source
        .as_ref()
        .ok_or_else(|| format!("{}: app record has no metadata sidecar", artifact.call_id))?;
    let meta = meta::parse(&meta::read_bounded(side, MAX_META).await?)?;
    Ok(Loaded {
        artifact,
        source,
        meta,
    })
}

/// Whether `b`'s tool is currently what the app bound, as the record's session sees it.
async fn live_diagnostic(
    reg: &ActorRef<ToolRegistry>,
    artifact: &Artifact,
    b: &AppBinding,
) -> Option<AppDiagnostic> {
    let diag = |code: &str, message: String| {
        Some(AppDiagnostic {
            at: String::new(),
            message,
            code: code.to_owned(),
        })
    };
    match reg
        .ask(tools::InspectTool {
            session: artifact.session,
            name: b.tool.clone(),
            args: None,
        })
        .await
    {
        Err(_) => diag(
            "tool-unknown",
            format!(
                "{}: tool is not installed or not visible to this session",
                b.tool
            ),
        ),
        Ok(i) if i.identity != b.identity => diag(
            "tool-identity",
            format!(
                "{}: tool changed since the app was published; republish to rebind",
                b.tool
            ),
        ),
        Ok(i) if i.action == tools::PolicyAction::Deny => diag(
            "tool-denied",
            format!("{}: denied by the tool policy", b.tool),
        ),
        Ok(_) => None,
    }
}

type Reply = ReplySender<Result<AppToolResult, String>>;

/// `ask` → `Result<AppDoc, String>`: a committed app with live binding diagnostics.
#[derive(Clone, Debug)]
pub struct GetApp {
    /// Record key.
    pub record_call_id: String,
}

/// `ask` → `Result<AppDoc, String>`: the desktop publishes a reply fence's source as an
/// app artifact (30 §9, `publish_app`), validated and committed exactly like the `app`
/// tool with `edit: false` under a synthetic `ui-<uuid v7>` call id. No model turn, no
/// transcript entry; the artifact list and [`artifact::ArtifactShown`] see it as usual.
#[derive(Clone, Debug)]
pub struct PublishFromUi {
    /// Owning session.
    pub session: runtime::SessionId,
    /// App name (`argo-health` → `argo-health.openui`).
    pub name: String,
    /// Complete OpenUI source.
    pub source: String,
}

/// `ask` → `Result<AppToolResult, String>`: run one bound Query/Mutation
/// (46 §6.3). `Err` is protocol misuse (unbound statement, foreign instance,
/// bad shape); refusals and outcomes are statuses.
#[derive(Debug)]
pub struct CallTool {
    /// The authenticated connection.
    pub peer: RpcPeer,
    /// The call.
    pub args: CallArgs,
}

/// `ask` → `Result<AppToolResult, String>`: current status of a request:
/// `running` while in flight here, else the durable row (presses), else `unknown`.
#[derive(Debug)]
pub struct CallStatusOf {
    /// The authenticated connection.
    pub peer: RpcPeer,
    /// The request.
    pub request_id: String,
}

/// `ask` → `Result<(), String>`: a view closed; its reads stop, accepted
/// presses finish. Unknown instance is fine.
#[derive(Debug)]
pub struct CloseApp {
    /// The authenticated connection.
    pub peer: RpcPeer,
    /// The view.
    pub instance_id: String,
}

/// Internal: a record finished loading for a call.
struct Dispatch {
    peer: RpcPeer,
    args: CallArgs,
    doc: Result<Arc<Loaded>, String>,
    tx: Reply,
}

/// Internal: a call's task ended. `event` carries the outcome the tool
/// produced; the actor decides what to publish once it knows whether the
/// view still wanted it.
struct Finished {
    request_id: String,
    result: Result<AppToolResult, String>,
    event: AppCallChanged,
}

/// Internal: a connection is gone.
struct PeerClosed(u64);

/// The [`crate::APP_ACTOR`] actor.
#[derive(kameo::Actor)]
pub struct Apps {
    core: Arc<Core>,
    sched: Scheduler<Reply>,
    /// Records with bound views, pinned so a queued read never reloads.
    open: HashMap<String, Arc<Loaded>>,
    /// Connections with a close watcher task.
    watched: HashSet<u64>,
    /// Cold record loads in flight per `(peer, instance)`: bounded by
    /// [`MAX_LOADING`] in total, and a view closed while its first call is
    /// still loading must not come back to life when the load lands.
    loading: HashMap<(u64, String), Loading>,
    loading_total: usize,
}

#[derive(Default)]
struct Loading {
    count: usize,
    closed: bool,
}

/// Cold loads in flight before any admission applies.
const MAX_LOADING: usize = 64;
/// Longest client-chosen identifier (instance, request, statement, action).
const MAX_ID: usize = 256;

impl Apps {
    /// New actor state over `core`.
    pub fn new(core: Arc<Core>) -> Self {
        Self {
            core,
            sched: Scheduler::default(),
            open: HashMap::new(),
            watched: HashSet::new(),
            loading: HashMap::new(),
            loading_total: 0,
        }
    }

    fn cancel(handles: Vec<Reply>, why: &str) {
        for tx in handles {
            // The request id is unknown here; the client matches by its own handle.
            tx.send(Ok(AppToolResult::status("", CallStatus::Cancelled, why)));
        }
    }

    /// Validate, bind, admit, and dispatch (or answer) one call.
    fn dispatch(
        &mut self,
        me: &ActorRef<Apps>,
        peer: RpcPeer,
        args: CallArgs,
        doc: Arc<Loaded>,
        tx: Reply,
    ) {
        let id = args.request_id.clone();
        if let Err(e) = validate(&peer, &args) {
            tx.send(Err(e));
            return;
        }
        let Some(binding) = doc
            .meta
            .bindings
            .iter()
            .find(|b| b.statement_id == args.statement_id)
            .cloned()
        else {
            tx.send(Ok(AppToolResult::status(
                id,
                CallStatus::Unavailable,
                format!(
                    "{}: not a bound Query/Mutation of this app",
                    args.statement_id
                ),
            )));
            return;
        };
        if binding.kind == "mutation" && !args.trigger.is_press() {
            tx.send(Ok(AppToolResult::status(
                id,
                CallStatus::Unavailable,
                format!(
                    "{}: a Mutation runs only from a pressed action",
                    args.statement_id
                ),
            )));
            return;
        }
        let size = serde_json::to_vec(&args.args)
            .map(|v| v.len())
            .unwrap_or(usize::MAX);
        if size > MAX_ARGS {
            tx.send(Ok(AppToolResult::status(
                id,
                CallStatus::Failed,
                format!("arguments are {size} bytes; the limit is {MAX_ARGS}"),
            )));
            return;
        }
        let record = doc.artifact.call_id.clone();
        // A view may move to a new committed version of the same app only:
        // same session and name, checked here where both records are known.
        let previous = self
            .sched
            .record_of(&args.instance_id)
            .filter(|r| *r != record);
        if let Some(prev) = &previous {
            let same_lineage = self
                .open
                .get(prev)
                .cloned()
                .or_else(|| self.core.cached(prev))
                .is_some_and(|old| {
                    old.artifact.session == doc.artifact.session
                        && old.artifact.name == doc.artifact.name
                });
            if !same_lineage {
                tx.send(Err(format!(
                    "instance shows {prev}, another app; open {record} in a new instance"
                )));
                return;
            }
        }
        match self
            .sched
            .bind(&args.instance_id, peer.id(), &record, args.generation)
        {
            Bind::Refused(e) => {
                tx.send(Err(e));
                return;
            }
            Bind::Stale => {
                tx.send(Ok(AppToolResult::status(
                    id,
                    CallStatus::Cancelled,
                    "view is on a newer generation",
                )));
                return;
            }
            Bind::Ok { cancelled } => Self::cancel(cancelled, "superseded by a newer generation"),
        }
        if let Some(prev) = previous {
            self.unpin(&prev);
        }
        self.open
            .entry(record.clone())
            .or_insert_with(|| doc.clone());
        if self.watched.insert(peer.id()) {
            let me = me.clone();
            let closed = peer.closed();
            let pid = peer.id();
            tokio::spawn(async move {
                closed.await;
                let _ = me.tell(PeerClosed(pid)).await;
            });
        }
        let run = Run {
            record,
            instance: args.instance_id,
            generation: args.generation,
            statement_id: args.statement_id,
            request_id: args.request_id,
            args: args.args,
            trigger: args.trigger,
            action_id: args.action_id,
        };
        match self.sched.admit(run, tx) {
            Admit::Start(run) => self.launch(me, doc, binding, *run),
            Admit::Queued => {}
            Admit::Superseded(tx) => tx.send(Ok(AppToolResult::status(
                "",
                CallStatus::Cancelled,
                "superseded by a newer value of the same query",
            ))),
            Admit::Busy(tx) => tx.send(Ok(AppToolResult::status(
                id,
                CallStatus::Busy,
                "app call limit reached; try again",
            ))),
            Admit::Running(tx) => tx.send(Ok(AppToolResult::status(
                id,
                CallStatus::Running,
                "already running",
            ))),
            Admit::Conflict(tx) => tx.send(Ok(AppToolResult::status(
                id,
                CallStatus::Unavailable,
                "request id is in use with different input",
            ))),
        }
    }

    /// Run one admitted call off the mailbox.
    fn launch(&self, me: &ActorRef<Apps>, doc: Arc<Loaded>, binding: AppBinding, run: Run) {
        let core = self.core.clone();
        let me = me.clone();
        tokio::spawn(async move {
            let started = Instant::now();
            let result = execute(&core, &doc, &binding, &run).await;
            let status = result.as_ref().map_or(CallStatus::Failed, |r| r.status);
            let event = AppCallChanged {
                session: doc.artifact.session,
                record_call_id: run.record,
                request_id: run.request_id.clone(),
                statement_id: run.statement_id,
                tool: binding.tool,
                risk: binding.risk,
                trigger: run.trigger,
                status,
                ms: started.elapsed().as_millis() as u64,
            };
            let _ = me
                .tell(Finished {
                    request_id: run.request_id,
                    result,
                    event,
                })
                .await;
        });
    }

    /// Start whatever any record's queue allows now; each run dispatches
    /// under its own record's pinned doc.
    fn pump_all(&mut self, me: &ActorRef<Apps>) {
        let (started, cancelled) = self.sched.pump_all();
        Self::cancel(cancelled, "view closed");
        for run in started {
            let Some(doc) = self.open.get(&run.record).cloned() else {
                // The last view closed between queueing and now; detach handled it.
                if let Some(f) = self.sched.finish(&run.request_id) {
                    Self::cancel(f.reply.into_iter().collect(), "view closed");
                }
                continue;
            };
            let Some(binding) = doc
                .meta
                .bindings
                .iter()
                .find(|b| b.statement_id == run.statement_id)
                .cloned()
            else {
                continue;
            };
            self.launch(me, doc, binding, run);
        }
    }

    fn unpin(&mut self, record: &str) {
        if !self.sched.has_record(record) {
            self.open.remove(record);
        }
    }

    /// The rows of `session` are gone (deleted, or its private tree closed):
    /// nothing cached may keep running for them; its views are cancelled.
    fn forget(&mut self, session: runtime::SessionId, why: &str) {
        self.core.forget_session(session);
        let records: Vec<String> = self
            .open
            .iter()
            .filter(|(_, d)| d.artifact.session == session)
            .map(|(r, _)| r.clone())
            .collect();
        for r in records {
            Self::cancel(self.sched.close_record(&r), why);
            self.open.remove(&r);
        }
    }
}

/// Shape checks that are protocol misuse (`Err`), not statuses.
fn validate(peer: &RpcPeer, args: &CallArgs) -> Result<(), String> {
    if peer.is_closed() {
        return Err("connection closed".into());
    }
    if args.instance_id.is_empty() || args.request_id.is_empty() || args.statement_id.is_empty() {
        return Err("instance_id, request_id and statement_id are required".into());
    }
    if [&args.instance_id, &args.request_id, &args.statement_id]
        .into_iter()
        .chain(args.action_id.iter())
        .any(|s| s.len() > MAX_ID)
    {
        return Err(format!("identifiers are limited to {MAX_ID} bytes"));
    }
    let press = args.trigger.is_press();
    match &args.action_id {
        Some(a) if press && a.is_empty() => {
            return Err("a press needs a non-empty action_id".into());
        }
        None if press => return Err("a press needs an action_id".into()),
        Some(_) if !press => return Err("action_id is only valid with trigger \"press\"".into()),
        _ => {}
    }
    if !args.args.is_object() {
        return Err("args must be a JSON object".into());
    }
    Ok(())
}

/// Claim (press), invoke, bound, persist (press). `Err` = an actor is gone.
async fn execute(
    core: &Core,
    doc: &Loaded,
    binding: &AppBinding,
    run: &Run,
) -> Result<AppToolResult, String> {
    let session = doc.artifact.session;
    let id = run.request_id.clone();
    let press = run.trigger.is_press();
    let artifacts = core.artifacts()?;
    let registry = core.registry()?;
    if press {
        let claim = artifacts
            .ask(artifact::ClaimInvocation {
                request_id: id.clone(),
                session,
                record_call_id: run.record.clone(),
                statement_id: run.statement_id.clone(),
                action_id: run.action_id.clone(),
                tool: binding.tool.clone(),
                args: run.args.clone(),
            })
            .await
            .map_err(|e| e.to_string())?;
        match claim {
            Claim::New => {}
            Claim::Existing(inv) => return Ok(from_invocation(*inv)),
            Claim::Conflict => {
                return Ok(AppToolResult::status(
                    id,
                    CallStatus::Unavailable,
                    "request id was already used with different input",
                ));
            }
        }
    }
    let activation = run.action_id.as_ref().map(|action_id| UserActivation {
        record_call_id: run.record.clone(),
        action_id: action_id.clone(),
        request_id: id.clone(),
    });
    let invoked = match registry
        .ask(AppInvoke {
            call: ToolCall {
                id: id.clone(),
                name: binding.tool.clone(),
                args: run.args.clone(),
                session,
                quiet: false,
            },
            activation,
            expected_identity: binding.identity.clone(),
        })
        .await
    {
        Ok(i) => i,
        Err(e) => {
            // The registry never answered: whether the tool ran is unknowable
            // here. A claimed press is marked so instead of staying `running`.
            let why = format!("tool call not delivered: {e}");
            if press
                && let Err(m) = artifacts
                    .ask(artifact::MarkInvocationUnknown {
                        request_id: id.clone(),
                        reason: Some(why.clone()),
                    })
                    .await
            {
                tracing::warn!(request = %id, "app invocation not marked unknown: {m}");
            }
            return Ok(AppToolResult::status(
                id,
                CallStatus::Unknown,
                format!("{why}; check the service before repeating this action"),
            ));
        }
    };
    let mut result = match invoked {
        AppInvoked::Completed(mut out) => {
            tools::secrets::redact_output(&mut out);
            bounded(&id, out)
        }
        AppInvoked::NeedsApproval => AppToolResult::status(
            id.clone(),
            CallStatus::NeedsApproval,
            format!("{}: needs your activation to run", binding.tool),
        ),
        AppInvoked::Unavailable(why) => {
            AppToolResult::status(id.clone(), CallStatus::Unavailable, why)
        }
    };
    if press {
        // The whole envelope is stored, so a later status read decodes the
        // exact result and status instead of guessing from tool JSON keys.
        let stored = serde_json::to_value(&result).map_err(|e| e.to_string())?;
        if let Err(e) = artifacts
            .ask(artifact::CompleteInvocation {
                request_id: id,
                ok: !result.is_error,
                result: Some(stored),
            })
            .await
        {
            // The tool did run; this is the live outcome. Only its durable
            // record failed, so a status lookup will say running/unknown.
            tracing::warn!(request = %run.request_id, "app invocation not recorded: {e}");
            result.content.push_str(&format!(
                "\n(not recorded: {e}; status lookups for this request report running/unknown)"
            ));
            return Ok(capped(result));
        }
    }
    Ok(result)
}

/// `io::Write` that only counts.
struct Count(usize);

impl std::io::Write for Count {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 += buf.len();
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Serialized size of a result as the client would receive it.
fn serialized_len(r: &AppToolResult) -> usize {
    let mut c = Count(0);
    // `AppToolResult` always serializes; a failure would be a serde_json bug.
    let _ = serde_json::to_writer(&mut c, r);
    c.0
}

/// A tool output as a result, or a real error when its serialized envelope
/// exceeds [`MAX_RESPONSE`]. Never truncated JSON.
fn bounded(request_id: &str, out: ToolOutput) -> AppToolResult {
    let status = if out.is_error {
        CallStatus::Failed
    } else {
        CallStatus::Succeeded
    };
    capped(AppToolResult {
        request_id: request_id.to_owned(),
        content: out.content,
        structured: out.structured,
        is_error: out.is_error,
        status,
    })
}

/// `result` if its serialized envelope fits [`MAX_RESPONSE`], else a
/// `Failed` result saying so. Applied to the final envelope, after any note.
fn capped(result: AppToolResult) -> AppToolResult {
    let request_id = result.request_id.clone();
    let total = serialized_len(&result);
    if total > MAX_RESPONSE {
        return AppToolResult::status(
            request_id,
            CallStatus::Failed,
            format!(
                "tool response is {total} bytes serialized; apps forward at most {MAX_RESPONSE}"
            ),
        );
    }
    result
}

/// A durable row as a client result: the stored envelope decoded exactly;
/// the row's status when there is none (running, or unknown after a crash).
fn from_invocation(inv: Invocation) -> AppToolResult {
    let row_status = match inv.status {
        InvocationStatus::Running => CallStatus::Running,
        InvocationStatus::Succeeded => CallStatus::Succeeded,
        InvocationStatus::Failed => CallStatus::Failed,
        InvocationStatus::Unknown => CallStatus::Unknown,
    };
    let request_id = inv.request_id;
    match (inv.status, inv.result) {
        (InvocationStatus::Unknown, result) => {
            let why = result
                .as_ref()
                .and_then(|v| v.get("unknown"))
                .and_then(|r| r.as_str())
                .map_or_else(
                    || "the harness restarted while this action was running".to_owned(),
                    str::to_owned,
                );
            AppToolResult::status(
                request_id,
                CallStatus::Unknown,
                format!("outcome unknown: {why}; check the service before repeating this action"),
            )
        }
        (InvocationStatus::Running, _) => {
            AppToolResult::status(request_id, CallStatus::Running, "running")
        }
        (_, None) => {
            AppToolResult::status(request_id, row_status, "finished; no result was stored")
        }
        (_, Some(v))
            if v.as_object()
                .is_some_and(|o| o.len() == 1 && o.contains_key("omitted")) =>
        {
            let why = v["omitted"].as_str().unwrap_or("too large").to_owned();
            AppToolResult::status(
                request_id,
                row_status,
                format!(
                    "the action {} but its result was not stored ({why}); inspect the service for the outcome",
                    if row_status == CallStatus::Succeeded {
                        "succeeded"
                    } else {
                        "failed"
                    }
                ),
            )
        }
        (_, Some(v)) => match serde_json::from_value::<AppToolResult>(v) {
            Ok(mut stored) => {
                stored.request_id = request_id;
                stored
            }
            Err(e) => AppToolResult::status(
                request_id,
                row_status,
                format!("stored result is unreadable ({e}); the action {row_status:?} as recorded"),
            ),
        },
    }
}

impl Message<GetApp> for Apps {
    type Reply = DelegatedReply<Result<AppDoc, String>>;

    async fn handle(&mut self, m: GetApp, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let core = self.core.clone();
        tokio::spawn(async move {
            tx.send(core.doc(&m.record_call_id).await);
        });
        reply
    }
}

impl Message<PublishFromUi> for Apps {
    type Reply = DelegatedReply<Result<AppDoc, String>>;

    async fn handle(
        &mut self,
        m: PublishFromUi,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let core = self.core.clone();
        tokio::spawn(async move {
            tx.send(publish_from_ui(&core, m).await);
        });
        reply
    }
}

/// [`crate::tool::commit_source`] under a fresh `ui-` call id, then the committed doc.
pub(crate) async fn publish_from_ui(core: &Core, m: PublishFromUi) -> Result<AppDoc, String> {
    let args = crate::AppArgs {
        name: m.name,
        source: m.source,
        ..crate::AppArgs::default()
    };
    let call_id = format!("ui-{}", uuid::Uuid::now_v7());
    let (loaded, _) =
        crate::tool::commit_source(core, args, m.session, call_id, crate::tool::Origin::Ui).await?;
    core.doc(&loaded.artifact.call_id).await
}

impl Message<CallTool> for Apps {
    type Reply = DelegatedReply<Result<AppToolResult, String>>;

    async fn handle(&mut self, m: CallTool, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let me = ctx.actor_ref().clone();
        let record = m.args.record_call_id.clone();
        if let Some(doc) = self
            .open
            .get(&record)
            .cloned()
            .or_else(|| self.core.cached(&record))
        {
            self.dispatch(&me, m.peer, m.args, doc, tx);
            return reply;
        }
        // Cold load: off the mailbox, bounded, and remembered per view so a
        // close in the meantime is honoured when the load lands.
        if let Err(e) = validate(&m.peer, &m.args) {
            tx.send(Err(e));
            return reply;
        }
        if self.loading_total >= MAX_LOADING {
            tx.send(Ok(AppToolResult::status(
                m.args.request_id,
                CallStatus::Busy,
                "too many apps opening at once; try again",
            )));
            return reply;
        }
        self.loading_total += 1;
        self.loading
            .entry((m.peer.id(), m.args.instance_id.clone()))
            .or_default()
            .count += 1;
        let core = self.core.clone();
        tokio::spawn(async move {
            let doc = core.load(&record).await;
            let _ = me
                .tell(Dispatch {
                    peer: m.peer,
                    args: m.args,
                    doc,
                    tx,
                })
                .await;
        });
        reply
    }
}

impl Message<Dispatch> for Apps {
    type Reply = ();

    async fn handle(&mut self, m: Dispatch, ctx: &mut Context<Self, Self::Reply>) {
        let key = (m.peer.id(), m.args.instance_id.clone());
        let closed = match self.loading.get_mut(&key) {
            Some(l) => {
                l.count -= 1;
                let closed = l.closed;
                if l.count == 0 {
                    self.loading.remove(&key);
                }
                closed
            }
            None => false,
        };
        self.loading_total = self.loading_total.saturating_sub(1);
        if closed {
            m.tx.send(Ok(AppToolResult::status(
                m.args.request_id,
                CallStatus::Cancelled,
                "view closed",
            )));
            return;
        }
        match m.doc {
            Ok(doc) => {
                let me = ctx.actor_ref().clone();
                self.dispatch(&me, m.peer, m.args, doc, m.tx);
            }
            Err(e) => m.tx.send(Err(e)),
        }
    }
}

impl Message<Finished> for Apps {
    type Reply = ();

    async fn handle(&mut self, mut m: Finished, ctx: &mut Context<Self, Self::Reply>) {
        let mut f = self.sched.finish(&m.request_id);
        // A read detached by close or a newer generation has no handle left:
        // nobody sees its outcome, so the event says so too. The tool really
        // ran; only the view that asked is gone. Presses keep their handle
        // and publish the real outcome, which the durable row also holds.
        match f.as_mut().and_then(|f| f.reply.take()) {
            Some(tx) => tx.send(m.result),
            None if m.event.trigger.is_press() => {}
            None => m.event.status = CallStatus::Cancelled,
        }
        self.core.rt.publish(m.event).await;
        if f.is_none() {
            return;
        }
        let me = ctx.actor_ref().clone();
        // A freed slot may unblock another record's queue under the global cap.
        self.pump_all(&me);
    }
}

impl Message<PeerClosed> for Apps {
    type Reply = ();

    async fn handle(&mut self, PeerClosed(peer): PeerClosed, _: &mut Context<Self, Self::Reply>) {
        self.watched.remove(&peer);
        Self::cancel(self.sched.peer_closed(peer), "connection closed");
        let records: Vec<String> = self.open.keys().cloned().collect();
        for r in records {
            self.unpin(&r);
        }
    }
}

impl Message<agent::AgentEvent> for Apps {
    type Reply = ();

    async fn handle(&mut self, ev: agent::AgentEvent, _: &mut Context<Self, Self::Reply>) {
        if let agent::AgentEvent::SessionDeleted { session } = ev {
            self.forget(session, "session deleted");
        }
    }
}

impl Message<CallStatusOf> for Apps {
    type Reply = DelegatedReply<Result<AppToolResult, String>>;

    async fn handle(
        &mut self,
        m: CallStatusOf,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        if m.peer.is_closed() {
            tx.send(Err("connection closed".into()));
            return reply;
        }
        if self.sched.is_running(&m.request_id) {
            tx.send(Ok(AppToolResult::status(
                m.request_id,
                CallStatus::Running,
                "running",
            )));
            return reply;
        }
        let core = self.core.clone();
        tokio::spawn(async move {
            let r = async {
                let row = core
                    .artifacts()?
                    .ask(artifact::GetInvocation {
                        request_id: m.request_id.clone(),
                    })
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(match row {
                    Some(inv) => from_invocation(inv),
                    None => AppToolResult::status(
                        m.request_id,
                        CallStatus::Unknown,
                        "no such request: not running here and never recorded",
                    ),
                })
            }
            .await;
            tx.send(r);
        });
        reply
    }
}

impl Message<CloseApp> for Apps {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: CloseApp, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        // A first call still loading its record must not bind after this close.
        if let Some(l) = self.loading.get_mut(&(m.peer.id(), m.instance_id.clone())) {
            l.closed = true;
        }
        match self.sched.peer_of(&m.instance_id) {
            None => return Ok(()),
            Some(p) if p != m.peer.id() => {
                return Err("instance is bound to another connection".into());
            }
            Some(_) => {}
        }
        let record = self.sched.record_of(&m.instance_id);
        Self::cancel(self.sched.close(&m.instance_id), "view closed");
        if let Some(r) = record {
            self.unpin(&r);
        }
        Ok(())
    }
}

impl Message<tools::CloseTree> for Apps {
    type Reply = tools::CloseAck;

    /// A private tree closes (51 §4.4): every view on a record of a subtree
    /// session is cancelled and unpinned, and its cached records are dropped,
    /// before the artifact owner removes the rows the records point at.
    async fn handle(
        &mut self,
        m: tools::CloseTree,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        for &session in &m.subtree {
            self.forget(session, "session closed");
        }
        tools::CloseAck {
            owner: "app".into(),
            ok: true,
            detail: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(content: &str, structured: Option<serde_json::Value>, is_error: bool) -> ToolOutput {
        ToolOutput {
            content: content.into(),
            is_error,
            structured,
            files: Vec::new(),
        }
    }

    #[test]
    fn oversized_responses_become_a_real_error_not_truncated_json() {
        // Escaping counts: a content string of quotes serializes to twice its bytes.
        let quotes = "\"".repeat(MAX_RESPONSE / 2 + 8);
        let r = bounded("r", out(&quotes, None, false));
        assert_eq!(r.status, CallStatus::Failed);
        assert!(r.is_error && r.structured.is_none());
        assert!(r.content.contains("at most"));
        let ok = bounded("r", out("hi", Some(serde_json::json!({"a": 1})), false));
        assert_eq!(ok.status, CallStatus::Succeeded);
        assert_eq!(ok.structured, Some(serde_json::json!({"a": 1})));
        assert!(serialized_len(&ok) == serde_json::to_vec(&ok).unwrap().len());
        assert_eq!(
            bounded("r", out("boom", None, true)).status,
            CallStatus::Failed
        );
    }

    fn inv(status: InvocationStatus, result: Option<serde_json::Value>) -> Invocation {
        Invocation {
            request_id: "r".into(),
            session: runtime::SessionId::new(),
            record_call_id: "rec".into(),
            statement_id: "s".into(),
            action_id: Some("a".into()),
            tool: "t".into(),
            args_hash: "h".into(),
            status,
            result,
            started_at: 0,
            finished_at: None,
        }
    }

    #[test]
    fn durable_rows_decode_the_exact_envelope_and_handle_omitted_and_unknown() {
        // Whatever the tool returned, including keys named `error`/`content`/`status`,
        // comes back verbatim with its own status.
        let live = AppToolResult {
            request_id: "old".into(),
            content: "denied".into(),
            structured: Some(
                serde_json::json!({"error": "not an error", "content": 1, "status": "x"}),
            ),
            is_error: true,
            status: CallStatus::Unavailable,
        };
        let row = inv(
            InvocationStatus::Failed,
            Some(serde_json::to_value(&live).unwrap()),
        );
        let r = from_invocation(row);
        assert_eq!(r.status, CallStatus::Unavailable);
        assert_eq!(r.request_id, "r", "row key wins over the stored copy");
        assert_eq!(r.structured, live.structured);
        assert_eq!(r.content, "denied");

        let r = from_invocation(inv(
            InvocationStatus::Succeeded,
            Some(serde_json::json!({"omitted": "result of 70 KB exceeds 64 KB"})),
        ));
        assert_eq!(r.status, CallStatus::Succeeded);
        assert!(
            r.structured.is_none()
                && r.content.contains("succeeded")
                && r.content.contains("inspect the service")
        );
        assert!(!r.content.contains("re-run"));

        let r = from_invocation(inv(InvocationStatus::Unknown, None));
        assert_eq!(r.status, CallStatus::Unknown);
        assert!(r.is_error && r.content.contains("restarted"));
        assert_eq!(
            from_invocation(inv(InvocationStatus::Running, None)).status,
            CallStatus::Running
        );
        let r = from_invocation(inv(
            InvocationStatus::Succeeded,
            Some(serde_json::json!({"not": "an envelope"})),
        ));
        assert_eq!(r.status, CallStatus::Succeeded);
        assert!(r.content.contains("unreadable"));
    }

    fn args(trigger: crate::Trigger, action_id: Option<&str>) -> CallArgs {
        CallArgs {
            record_call_id: "rec".into(),
            instance_id: "i".into(),
            generation: 1,
            statement_id: "s".into(),
            request_id: "r".into(),
            args: serde_json::json!({}),
            trigger,
            action_id: action_id.map(str::to_owned),
        }
    }

    #[test]
    fn request_shape_is_protocol_misuse() {
        let peer = RpcPeer::internal();
        assert!(validate(&peer, &args(crate::Trigger::Press, Some("a"))).is_ok());
        assert!(validate(&peer, &args(crate::Trigger::Load, None)).is_ok());
        assert!(
            validate(&peer, &args(crate::Trigger::Press, None)).is_err(),
            "press needs action"
        );
        assert!(validate(&peer, &args(crate::Trigger::Press, Some(""))).is_err());
        assert!(
            validate(&peer, &args(crate::Trigger::Refresh, Some("a"))).is_err(),
            "no consent on automatic triggers"
        );
        let mut a = args(crate::Trigger::Load, None);
        a.args = serde_json::json!([1]);
        assert!(validate(&peer, &a).is_err());
        let mut a = args(crate::Trigger::Load, None);
        a.request_id.clear();
        assert!(validate(&peer, &a).is_err());
        let (closed, guard) = RpcPeer::open();
        drop(guard);
        assert!(validate(&closed, &args(crate::Trigger::Load, None)).is_err());
    }
}
