//! In-process Monty Python worker pool for iOS (and native testing via `--features inproc-monty`).
//!
//! iOS forbids spawning child subprocesses, so instead of `monty-pool`'s
//! out-of-process worker pool we drive [`monty_proto::worker::Worker`] directly
//! on dedicated OS threads (up to `max_workers`, each with a 6 MiB stack so
//! recursive Python/Monty frames never blow iOS's 512 KiB default pthread stack).
//!
//! Safety invariants:
//! - Never calls `monty_alloc::set_hard_limit` (process-global allocator trap).
//! - Caps every checkout to iOS-safe cooperative [`ResourceLimits`]:
//!   `max_memory <= 96 MiB`, `max_feed_duration <= 30 s`,
//!   `max_turn_duration <= 30 s`, `max_recursion_depth <= 256`.
//! - Wraps every [`Worker::handle`] call in [`std::panic::catch_unwind`] so a
//!   sandbox bug turns into [`PoolError::Crashed`] instead of aborting the app.

use std::{
    borrow::Cow,
    future::{Future, ready},
    panic::AssertUnwindSafe,
    path::Path,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc as std_mpsc,
    },
    time::Duration,
};

use monty_fs::{Mount, MountCallOutcome, MountMode, MountRoot, MountTable, OverlayState};
use monty_proto::{
    FrameError, PROTOCOL_VERSION, ext_result_to_proto, future_results_to_proto,
    named_values_to_proto, os_call_from_proto, pb,
    worker::{EventSink, HandleOutcome, Worker},
};
use monty_types::{
    AssertMessageAnnotations, CallArgs, ExcType, ExtFunctionResult, MONTY_VERSION, ModuleFile,
    MontyException, MontyObject, MontyUuid, NameLookupResult, NamedValues, OsFunctionCall,
    OsPolicy, PrintStream, ResourceLimits, SleepMode, SourceRange, TypeCheckingConfig,
};
use parking_lot::Mutex;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc as tokio_mpsc};

/// Stack size for each in-process Monty worker OS thread (6 MiB).
///
/// iOS defaults new pthreads to 512 KiB, which is too small for Monty's
/// type-checker (`ty`) and nested Python evaluation frames.
const WORKER_STACK_SIZE: usize = 6 * 1024 * 1024;

/// Cooperative heap cap for in-process checkouts (96 MiB).
pub const INPROC_MAX_MEMORY: usize = 96 * 1024 * 1024;

/// Cooperative single-feed wall/execution duration cap (30 s).
pub const INPROC_MAX_FEED_DURATION: Duration = Duration::from_secs(30);

/// Cooperative single-turn wall/execution duration cap (30 s).
pub const INPROC_MAX_TURN_DURATION: Duration = Duration::from_secs(30);

/// Cooperative Python recursion depth cap (256 frames).
pub const INPROC_MAX_RECURSION_DEPTH: usize = 256;

/// Errors returned by [`InProcPool`] and [`InProcCheckout`].
#[derive(Debug)]
pub enum PoolError {
    /// Python runtime exception raised inside the sandbox (session survives).
    Runtime(MontyException),
    /// Static type-checking failed before running the cell (session survives).
    Typing(String),
    /// Protocol violation or invalid host call sequence.
    Protocol(Cow<'static, str>),
    /// The worker thread panicked, exited unexpectedly, or emitted `FatalError`.
    Crashed(String),
    /// The turn exceeded its request timeout.
    Timeout(Duration),
    /// The checkout was already finished or its worker was discarded.
    Finished,
    /// The pool has been closed.
    Closed,
    /// Spawning a worker OS thread failed.
    Spawn(String),
}

impl std::fmt::Display for PoolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Runtime(exc) => write!(f, "{exc}"),
            Self::Typing(diag) => write!(f, "type check failed: {diag}"),
            Self::Protocol(msg) => write!(f, "protocol error: {msg}"),
            Self::Crashed(msg) => write!(f, "worker crashed: {msg}"),
            Self::Timeout(d) => write!(f, "worker turn timed out after {d:?}"),
            Self::Finished => f.write_str("checkout finished"),
            Self::Closed => f.write_str("worker pool closed"),
            Self::Spawn(msg) => write!(f, "failed to spawn worker thread: {msg}"),
        }
    }
}

impl std::error::Error for PoolError {}

impl From<MontyException> for PoolError {
    fn from(exc: MontyException) -> Self {
        Self::Runtime(exc)
    }
}

/// Configuration for one in-process REPL checkout (mirrors `monty_pool::ReplConfig`).
#[derive(Debug, Clone)]
pub struct ReplConfig {
    /// Script name used in tracebacks and type-check diagnostics.
    pub script_name: String,
    /// Sandbox resource limits (clamped to iOS-safe ceilings on checkout).
    pub limits: Option<ResourceLimits>,
    /// Type-check every fed snippet before executing it.
    pub type_check: bool,
    /// Stub declarations made available to type checking.
    pub type_check_stubs: Option<String>,
    /// How the worker renders typing diagnostics.
    pub type_check_config: TypeCheckingConfig,
    /// Introspected `assert` message truncation configuration.
    pub assert_message_annotations: AssertMessageAnnotations,
    /// Print buffering interval (`None` = worker default).
    pub print_flush_interval: Option<Duration>,
    /// Session clock, random state, and sleep policy.
    pub os_policy: OsPolicy,
}

impl Default for ReplConfig {
    fn default() -> Self {
        Self {
            script_name: "main.py".to_owned(),
            limits: None,
            type_check: false,
            type_check_stubs: None,
            type_check_config: TypeCheckingConfig::default(),
            assert_message_annotations: AssertMessageAnnotations::default(),
            print_flush_interval: None,
            os_policy: OsPolicy::default(),
        }
    }
}

/// Access mode for a [`MountSpec`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountSpecMode {
    /// Reads succeed; writes raise `PermissionError`.
    ReadOnly,
    /// Reads and writes pass through to the host directory.
    ReadWrite,
    /// Reads fall through to host; writes stay in an ephemeral in-memory overlay.
    Overlay,
}

/// Host directory mounted into a sandbox feed's virtual filesystem.
#[derive(Debug, Clone)]
pub struct MountSpec {
    root: MountRoot,
    /// Access mode.
    pub mode: MountSpecMode,
    /// Optional cap on total bytes written through this mount.
    pub write_bytes_limit: Option<u64>,
    /// Aggregate memory budget for overlay data and transient results.
    pub memory_usage_limit: u64,
}

impl MountSpec {
    /// Opens `host_path` and creates a mount at `virtual_path`.
    pub fn new(
        virtual_path: &str,
        host_path: impl AsRef<Path>,
        mode: MountSpecMode,
    ) -> Result<Self, PoolError> {
        let root = MountRoot::open(virtual_path, host_path)
            .map_err(|err| PoolError::Runtime(err.into_exception()))?;
        Ok(Self::from_root(root, mode))
    }

    /// Creates a mount from an already-opened [`MountRoot`].
    #[must_use]
    pub fn from_root(root: MountRoot, mode: MountSpecMode) -> Self {
        Self {
            root,
            mode,
            write_bytes_limit: None,
            memory_usage_limit: monty_fs::DEFAULT_MEMORY_USAGE_LIMIT,
        }
    }

    /// Returns the normalized virtual path this mount answers on.
    #[must_use]
    pub fn virtual_path(&self) -> &str {
        self.root.virtual_path()
    }

    /// Returns the host directory path (diagnostics only).
    #[must_use]
    pub fn host_path(&self) -> &Path {
        self.root.host_path()
    }
}

/// Turn-ending events emitted by a feed or resume (mirrors `monty_pool::TurnEvent`).
#[derive(Debug)]
pub enum TurnEvent {
    /// The sandbox called an external host function.
    FunctionCall {
        /// Function name.
        function_name: String,
        /// Positional and keyword arguments.
        args: CallArgs,
        /// Worker-assigned call ID.
        call_id: u32,
        /// Optional host-backed receiver object ID.
        object_id: Option<MontyUuid>,
        /// Whether an eager settled future may be passed to `resume_futures`.
        allow_eager_await: bool,
        /// Source position of the call.
        position: SourceRange,
    },
    /// The sandbox issued an OS / filesystem / sleep call.
    OsCall {
        /// Normalized OS function name (e.g. `"os.getenv"`).
        function_name: String,
        /// Call arguments.
        args: CallArgs,
        /// Worker-assigned call ID.
        call_id: u32,
        /// Whether an eager settled future is accepted.
        allow_eager_await: bool,
        /// When `Some`, the host should sleep this duration and return `None`.
        system_sleep: Option<Duration>,
        /// Source position of the call.
        position: SourceRange,
    },
    /// The sandbox looked up an undefined global or lazy attribute.
    NameLookup {
        /// Looked-up identifier.
        name: String,
        /// Optional host-backed object ID for attribute lookups.
        object_id: Option<MontyUuid>,
        /// Source position.
        position: SourceRange,
    },
    /// All sandbox tasks are suspended waiting on external futures.
    ResolveFutures {
        /// Pending future call IDs to settle via [`InProcCheckout::resume_futures`].
        pending_call_ids: Vec<u32>,
        /// Source position.
        position: SourceRange,
    },
    /// The fed snippet completed with a value.
    Complete(MontyObject),
}

/// Host answer to a suspended [`TurnEvent::FunctionCall`] or [`TurnEvent::OsCall`].
#[derive(Debug)]
pub enum ResumeValue {
    /// Return a value to the sandbox.
    Return(MontyObject),
    /// Raise an exception in the sandbox.
    Error(MontyException),
    /// Return a pending future to be settled later by `resume_futures`.
    Future,
    /// Report the function/module as not found.
    NotFound,
    /// Let the sandbox apply its default handling for an OS call.
    NotHandled,
    /// Provide a module source file for an import OS call.
    ModuleFile(ModuleFile),
}

/// Boxed future returned by an [`OnPrint`] callback.
pub type PrintFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// Async print callback invoked for each streamed stdout/stderr chunk.
pub type OnPrint<'a> = &'a mut (dyn FnMut(PrintStream, &str) -> PrintFuture + Send);

/// Adapts a synchronous print sink to the [`OnPrint`] callback shape.
pub fn on_print_sync<F>(mut sink: F) -> impl FnMut(PrintStream, &str) -> PrintFuture + Send
where
    F: FnMut(PrintStream, &str) + Send,
{
    move |stream, text| {
        sink(stream, text);
        Box::pin(ready(()))
    }
}

/// In-memory [`EventSink`] forwarding cloned worker events to the async checkout.
struct ChannelSink {
    tx: tokio_mpsc::UnboundedSender<pb::WorkerEvent>,
}

impl EventSink for ChannelSink {
    fn send(&mut self, event: &pb::WorkerEvent) -> Result<(), FrameError> {
        self.tx.send(event.clone()).map_err(|_| {
            FrameError::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "inproc checkout receiver dropped",
            ))
        })
    }
}

/// Live handle to one dedicated OS worker thread.
struct WorkerHandle {
    id: u64,
    req_tx: std_mpsc::Sender<pb::ClientRequest>,
    ev_rx: tokio_mpsc::UnboundedReceiver<pb::WorkerEvent>,
}

impl WorkerHandle {
    fn spawn(id: u64) -> Result<Self, PoolError> {
        let (req_tx, req_rx) = std_mpsc::channel::<pb::ClientRequest>();
        let (ev_tx, ev_rx) = tokio_mpsc::unbounded_channel::<pb::WorkerEvent>();
        std::thread::Builder::new()
            .name("monty-inproc".to_owned())
            .stack_size(WORKER_STACK_SIZE)
            .spawn(move || run_worker_thread(req_rx, ev_tx))
            .map_err(|e| PoolError::Spawn(e.to_string()))?;
        Ok(Self { id, req_tx, ev_rx })
    }
}

/// Dedicated OS thread body driving one [`Worker`] instance.
fn run_worker_thread(
    req_rx: std_mpsc::Receiver<pb::ClientRequest>,
    ev_tx: tokio_mpsc::UnboundedSender<pb::WorkerEvent>,
) {
    let mut worker = Worker::default();
    let mut sink = ChannelSink { tx: ev_tx.clone() };
    while let Ok(req) = req_rx.recv() {
        let step = std::panic::catch_unwind(AssertUnwindSafe(|| worker.handle(req, &mut sink)));
        match step {
            Ok(Ok(HandleOutcome::Continue)) => {}
            Ok(Ok(HandleOutcome::Shutdown | HandleOutcome::Fatal)) | Ok(Err(_)) => break,
            Err(payload) => {
                let msg = panic_message(payload.as_ref());
                let _ = ev_tx.send(pb::WorkerEvent {
                    kind: Some(pb::worker_event::Kind::FatalError(pb::FatalError {
                        message: format!("monty worker panicked: {msg}"),
                    })),
                    ..Default::default()
                });
                break;
            }
        }
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_owned()
    }
}

struct PoolInner {
    semaphore: Arc<Semaphore>,
    idle: Mutex<Vec<WorkerHandle>>,
    next_id: AtomicU64,
    closed: AtomicBool,
    request_timeout: Duration,
}

impl PoolInner {
    fn release_worker(&self, worker: WorkerHandle) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        self.idle.lock().push(worker);
    }
}

/// Bounded pool of in-process Monty Python worker OS threads.
pub struct InProcPool {
    inner: Arc<PoolInner>,
}

impl InProcPool {
    /// Create a pool allowing at most `max_workers` concurrent checkouts.
    #[must_use]
    pub fn new(max_workers: usize) -> Self {
        let cap = max_workers.max(1);
        Self {
            inner: Arc::new(PoolInner {
                semaphore: Arc::new(Semaphore::new(cap)),
                idle: Mutex::new(Vec::with_capacity(cap)),
                next_id: AtomicU64::new(1),
                closed: AtomicBool::new(false),
                request_timeout: crate::repl::REQUEST_TIMEOUT,
            }),
        }
    }

    /// Check out a worker configured with `cfg`, waiting if the pool is full.
    pub async fn checkout(&self, cfg: &ReplConfig) -> Result<InProcCheckout, PoolError> {
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(PoolError::Closed);
        }
        let permit = Arc::clone(&self.inner.semaphore)
            .acquire_owned()
            .await
            .map_err(|_| PoolError::Closed)?;
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(PoolError::Closed);
        }
        let worker = {
            let mut idle = self.inner.idle.lock();
            idle.pop()
        };
        let worker = match worker {
            Some(w) => w,
            None => {
                let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
                WorkerHandle::spawn(id)?
            }
        };
        let effective_limits = clamp_limits(cfg.limits.as_ref());
        let configure = build_configure(cfg, &effective_limits);
        Ok(InProcCheckout {
            pool: Arc::clone(&self.inner),
            _permit: permit,
            worker: Some(worker),
            pending_configure: Some(configure),
            configured: false,
            restored_script_name: None,
            os_policy: cfg.os_policy.clone(),
            max_total_sleep: effective_limits.max_total_sleep,
            sleep_used: Duration::ZERO,
            pending: None,
            feed_mounts: None,
            cwd_set: false,
            turn_in_flight: false,
        })
    }

    /// Close the pool and drop all idle worker threads.
    pub async fn close(&self) {
        self.inner.closed.store(true, Ordering::Release);
        self.inner.semaphore.close();
        let drained: Vec<WorkerHandle> = std::mem::take(&mut *self.inner.idle.lock());
        drop(drained);
    }
}

/// Which kind of suspension is currently awaiting a host answer.
enum Pending {
    Call {
        call_id: u32,
        function_name: String,
        os_call: Option<Box<OsFunctionCall>>,
        allow_eager_await: bool,
    },
    NameLookup,
    Futures,
}

/// Internal outcome of one protocol request/reply exchange.
enum ControlEvent {
    Ok,
    Turn(TurnEvent),
    Dump(Vec<u8>),
}

/// One in-process worker thread dedicated to one REPL session.
pub struct InProcCheckout {
    pool: Arc<PoolInner>,
    _permit: OwnedSemaphorePermit,
    worker: Option<WorkerHandle>,
    pending_configure: Option<pb::Configure>,
    configured: bool,
    restored_script_name: Option<String>,
    os_policy: OsPolicy,
    max_total_sleep: Option<Duration>,
    sleep_used: Duration,
    pending: Option<Pending>,
    feed_mounts: Option<MountTable>,
    cwd_set: bool,
    turn_in_flight: bool,
}

impl InProcCheckout {
    /// Stable worker ID within the pool, or `None` once finished or discarded.
    #[must_use]
    pub fn worker_id(&self) -> Option<u64> {
        self.worker.as_ref().map(|w| w.id)
    }

    /// Feed Python code into the session and run until completion or suspension.
    pub async fn feed(
        &mut self,
        code: impl Into<String>,
        inputs: impl Into<NamedValues>,
        mounts: Vec<MountSpec>,
        skip_type_check: bool,
        on_print: OnPrint<'_>,
    ) -> Result<TurnEvent, PoolError> {
        self.ensure_ready()?;
        if self.pending.is_some() {
            return Err(PoolError::Protocol(
                "feed called while a suspension is awaiting an answer".into(),
            ));
        }
        let cwd = if self.cwd_set {
            String::new()
        } else {
            mounts
                .first()
                .map_or_else(|| "/".to_owned(), |m| m.virtual_path().to_owned())
        };
        self.feed_mounts = build_feed_mounts(mounts)?;
        self.ensure_configured().await?;
        let (inputs, values) = named_values_to_proto(inputs.into());
        let req = request(pb::client_request::Kind::Feed(pb::Feed {
            code: code.into(),
            inputs,
            values: Some(values),
            skip_type_check,
            cwd,
            async_functions: Vec::new().into(),
            stream_async_calls: false,
        }));
        let ev = self.expect_turn(req, on_print).await?;
        self.cwd_set = true;
        Ok(ev)
    }

    /// Restore a previously dumped session snapshot into a fresh checkout.
    pub async fn restore(
        &mut self,
        state: Vec<u8>,
        mounts: Vec<MountSpec>,
        on_print: OnPrint<'_>,
    ) -> Result<(Option<TurnEvent>, Option<String>), PoolError> {
        self.ensure_ready()?;
        self.restored_script_name = None;
        self.feed_mounts = build_feed_mounts(mounts)?;
        self.ensure_configured().await?;
        let req = request(pb::client_request::Kind::Load(pb::Load {
            state: state.into(),
        }));
        match self.request_turn(req, on_print).await? {
            ControlEvent::Ok => {
                self.pending = None;
                self.feed_mounts = None;
                Ok((None, self.restored_script_name.take()))
            }
            ControlEvent::Turn(ev) => Ok((Some(ev), self.restored_script_name.take())),
            ControlEvent::Dump(_) => {
                Err(self.protocol_violation("unexpected DumpResult reply to Load"))
            }
        }
    }

    /// Answer a suspended [`TurnEvent::FunctionCall`] or [`TurnEvent::OsCall`].
    pub async fn resume(
        &mut self,
        value: ResumeValue,
        on_print: OnPrint<'_>,
    ) -> Result<TurnEvent, PoolError> {
        self.ensure_ready()?;
        let Some(Pending::Call {
            call_id,
            function_name,
            os_call,
            ..
        }) = &self.pending
        else {
            return Err(PoolError::Protocol("no suspended call to resume".into()));
        };
        let (call_id, function_name, is_os_call) = (*call_id, function_name.clone(), os_call.is_some());
        if matches!(value, ResumeValue::NotHandled) && !is_os_call {
            return Err(PoolError::Protocol(
                "NotHandled is only valid answering an OS call".into(),
            ));
        }
        let (result, values) = match value {
            ResumeValue::Return(v) => ext_result_to_proto(ExtFunctionResult::Return(v)),
            ResumeValue::Error(exc) => ext_result_to_proto(ExtFunctionResult::Error(exc)),
            ResumeValue::Future => ext_result_to_proto(ExtFunctionResult::Future(call_id)),
            ResumeValue::NotFound => ext_result_to_proto(ExtFunctionResult::NotFound(function_name)),
            ResumeValue::ModuleFile(file) => {
                ext_result_to_proto(ExtFunctionResult::ModuleFile(file))
            }
            ResumeValue::NotHandled => (
                pb::ExtFunctionResult {
                    kind: Some(pb::ext_function_result::Kind::NotHandled(pb::Unit {})),
                },
                None,
            ),
        };
        let req = request(pb::client_request::Kind::ResumeCall(pb::ResumeCall {
            call_id,
            result: Some(result),
            values,
        }));
        self.expect_turn(req, on_print).await
    }

    /// Answer a suspended [`TurnEvent::NameLookup`].
    pub async fn resume_name_lookup(
        &mut self,
        result: impl Into<NameLookupResult>,
        on_print: OnPrint<'_>,
    ) -> Result<TurnEvent, PoolError> {
        self.ensure_ready()?;
        if !matches!(self.pending, Some(Pending::NameLookup)) {
            return Err(PoolError::Protocol(
                "no suspended name lookup to resume".into(),
            ));
        }
        let req = request(pb::client_request::Kind::ResumeNameLookup(
            result.into().into(),
        ));
        self.expect_turn(req, on_print).await
    }

    /// Settle pending external futures announced by [`TurnEvent::ResolveFutures`].
    pub async fn resume_futures(
        &mut self,
        results: Vec<(u32, ResumeValue)>,
        on_print: OnPrint<'_>,
    ) -> Result<TurnEvent, PoolError> {
        self.ensure_ready()?;
        match &self.pending {
            Some(Pending::Call {
                call_id,
                allow_eager_await: true,
                ..
            }) => {
                if results.len() != 1 || results[0].0 != *call_id {
                    return Err(PoolError::Protocol(
                        "eager result must match the suspended call id".into(),
                    ));
                }
            }
            Some(Pending::Futures) => {}
            _ => {
                return Err(PoolError::Protocol(
                    "no suspended futures to resume".into(),
                ));
            }
        }
        let converted = results
            .into_iter()
            .map(|(call_id, value)| {
                let ext = match value {
                    ResumeValue::Return(v) => ExtFunctionResult::Return(v),
                    ResumeValue::Error(exc) => ExtFunctionResult::Error(exc),
                    ResumeValue::Future
                    | ResumeValue::NotFound
                    | ResumeValue::NotHandled
                    | ResumeValue::ModuleFile(_) => {
                        return Err(PoolError::Protocol(
                            format!("future {call_id} must resolve to Return or Error").into(),
                        ));
                    }
                };
                Ok((call_id, ext))
            })
            .collect::<Result<Vec<_>, PoolError>>()?;
        let req = request(pb::client_request::Kind::ResumeFutures(
            future_results_to_proto(converted),
        ));
        self.expect_turn(req, on_print).await
    }

    /// Answer a pending [`TurnEvent::OsCall`] from this feed's mounts if covered.
    pub async fn resume_from_mounts(
        &mut self,
        on_print: OnPrint<'_>,
    ) -> Result<Option<TurnEvent>, PoolError> {
        self.ensure_ready()?;
        let Some(Pending::Call {
            os_call: os_slot @ Some(_),
            ..
        }) = &mut self.pending
        else {
            return Err(PoolError::Protocol(
                "resume_from_mounts is only valid answering an OS call".into(),
            ));
        };
        let call = os_slot.take().expect("matched Some above");
        let outcome = match self.feed_mounts.take() {
            Some(mut mounts) => {
                self.turn_in_flight = true;
                let joined = tokio::task::spawn_blocking(move || {
                    let outcome = mounts.handle_os_call(*call);
                    (mounts, outcome)
                })
                .await;
                let (mounts, outcome) = match joined {
                    Ok(pair) => pair,
                    Err(err) => {
                        return Err(self.protocol_violation(format!(
                            "blocking mount task failed: {err}"
                        )));
                    }
                };
                self.feed_mounts = Some(mounts);
                self.turn_in_flight = false;
                outcome
            }
            None => MountCallOutcome::NotHandled(*call),
        };
        match outcome {
            MountCallOutcome::Handled(result) => {
                let value = match result {
                    Ok(obj) => ResumeValue::Return(obj),
                    Err(err) => ResumeValue::Error(err.into_exception()),
                };
                self.resume(value, on_print).await.map(Some)
            }
            MountCallOutcome::NotHandled(call) => {
                if let Some(Pending::Call { os_call, .. }) = &mut self.pending {
                    *os_call = Some(Box::new(call));
                }
                Ok(None)
            }
        }
    }

    /// Serialize the current session state into an opaque snapshot.
    pub async fn dump(&mut self) -> Result<Vec<u8>, PoolError> {
        self.ensure_ready()?;
        self.ensure_configured().await?;
        let req = request(pb::client_request::Kind::Dump(pb::Dump {}));
        let mut quiet = on_print_sync(|_, _| {});
        match self.request_turn(req, &mut quiet).await? {
            ControlEvent::Dump(bytes) => Ok(bytes),
            ControlEvent::Ok | ControlEvent::Turn(_) => {
                Err(self.protocol_violation("unexpected reply to Dump"))
            }
        }
    }

    /// Reset the worker and return it to the pool for reuse.
    pub async fn finish(mut self) -> Result<(), PoolError> {
        if self.worker.is_none() {
            return Ok(());
        }
        if self.turn_in_flight {
            self.discard_worker();
            return Err(PoolError::Protocol(
                "previous turn was cancelled mid-flight; worker discarded".into(),
            ));
        }
        if !self.configured {
            if let Some(worker) = self.worker.take() {
                self.pool.release_worker(worker);
            }
            return Ok(());
        }
        let req = request(pb::client_request::Kind::Reset(pb::Reset {}));
        let mut quiet = on_print_sync(|_, _| {});
        match self.request_turn(req, &mut quiet).await? {
            ControlEvent::Ok => {
                if let Some(worker) = self.worker.take() {
                    self.pool.release_worker(worker);
                }
                Ok(())
            }
            ControlEvent::Turn(_) | ControlEvent::Dump(_) => {
                Err(self.protocol_violation("unexpected reply to Reset"))
            }
        }
    }

    fn ensure_ready(&mut self) -> Result<(), PoolError> {
        if self.worker.is_none() {
            Err(PoolError::Finished)
        } else if self.turn_in_flight {
            self.discard_worker();
            Err(PoolError::Protocol(
                "a previous turn was cancelled mid-flight; worker discarded".into(),
            ))
        } else {
            Ok(())
        }
    }

    async fn ensure_configured(&mut self) -> Result<(), PoolError> {
        if let Some(cfg) = self.pending_configure.take() {
            let req = request(pb::client_request::Kind::Configure(cfg));
            let mut quiet = on_print_sync(|_, _| {});
            match self.request_turn(req, &mut quiet).await? {
                ControlEvent::Ok => {
                    self.configured = true;
                }
                ControlEvent::Turn(_) | ControlEvent::Dump(_) => {
                    return Err(self.protocol_violation("unexpected reply to Configure"));
                }
            }
        }
        Ok(())
    }

    async fn expect_turn(
        &mut self,
        req: pb::ClientRequest,
        on_print: OnPrint<'_>,
    ) -> Result<TurnEvent, PoolError> {
        match self.request_turn(req, on_print).await? {
            ControlEvent::Turn(ev) => Ok(ev),
            ControlEvent::Ok | ControlEvent::Dump(_) => {
                Err(self.protocol_violation("unexpected control reply during turn"))
            }
        }
    }

    async fn request_turn(
        &mut self,
        req: pb::ClientRequest,
        on_print: OnPrint<'_>,
    ) -> Result<ControlEvent, PoolError> {
        let is_dump = matches!(req.kind, Some(pb::client_request::Kind::Dump(_)));
        let timeout = self.pool.request_timeout;
        {
            let worker = self.worker.as_ref().ok_or(PoolError::Finished)?;
            if worker.req_tx.send(req).is_err() {
                self.discard_worker();
                return Err(PoolError::Crashed("worker thread exited".into()));
            }
        }
        self.turn_in_flight = true;
        let result = tokio::time::timeout(timeout, self.recv_turn_events(is_dump, on_print)).await;
        self.turn_in_flight = false;
        match result {
            Ok(inner) => inner,
            Err(_) => {
                self.discard_worker();
                Err(PoolError::Timeout(timeout))
            }
        }
    }

    async fn recv_turn_events(
        &mut self,
        is_dump: bool,
        on_print: OnPrint<'_>,
    ) -> Result<ControlEvent, PoolError> {
        loop {
            let event = {
                let worker = self.worker.as_mut().ok_or(PoolError::Finished)?;
                worker.ev_rx.recv().await
            };
            let Some(event) = event else {
                self.discard_worker();
                return Err(PoolError::Crashed(
                    "worker channel closed without a turn-ending event".into(),
                ));
            };
            if let Some(name) = event.restored_script_name {
                self.restored_script_name = Some(name);
            }
            match event.kind {
                Some(pb::worker_event::Kind::Ping(_)) => {}
                Some(pb::worker_event::Kind::Print(print)) => {
                    for segment in &print.segments {
                        let stream = match segment.stream() {
                            pb::PrintStream::Stderr => PrintStream::Stderr,
                            pb::PrintStream::Stdout | pb::PrintStream::Unspecified => {
                                PrintStream::Stdout
                            }
                        };
                        on_print(stream, &segment.text).await;
                    }
                }
                Some(pb::worker_event::Kind::FunctionCall(mut call)) => {
                    let position = call.position.take().map_or_else(SourceRange::unknown, Into::into);
                    let function_name = call.function_name.clone();
                    let call_id = call.call_id;
                    let object_id = call.object_id;
                    let allow_eager_await = call.allow_eager_await;
                    let args = call.into_call_args().map_err(|err| {
                        self.protocol_violation(format!("invalid FunctionCall args: {err}"))
                    })?;
                    self.pending = Some(Pending::Call {
                        call_id,
                        function_name: function_name.clone(),
                        os_call: None,
                        allow_eager_await,
                    });
                    return Ok(ControlEvent::Turn(TurnEvent::FunctionCall {
                        function_name,
                        args,
                        call_id,
                        object_id,
                        allow_eager_await,
                        position,
                    }));
                }
                Some(pb::worker_event::Kind::OsCall(mut call)) => {
                    let position = call.position.take().map_or_else(SourceRange::unknown, Into::into);
                    let mut allow_eager_await = call.allow_eager_await;
                    let (call_id, function_call) = os_call_from_proto(call).map_err(|err| {
                        self.protocol_violation(format!("invalid OsCall payload: {err}"))
                    })?;
                    let function_name = function_call.name().to_owned();
                    allow_eager_await =
                        allow_eager_await && OsFunctionCall::accepts_future(function_call.name());
                    let system_sleep = match function_call {
                        OsFunctionCall::SystemSleep(delay)
                        | OsFunctionCall::AsyncSystemSleep(delay) => {
                            let capped = cap_sleep(delay, &self.os_policy);
                            let total = self.sleep_used.saturating_add(capped);
                            if let Some(limit) = self.max_total_sleep
                                && total > limit
                            {
                                self.pending = None;
                                self.feed_mounts = None;
                                return Err(PoolError::Runtime(MontyException::new(
                                    ExcType::TimeoutError,
                                    Some(format!("sleep limit exceeded: {total:?} > {limit:?}")),
                                )));
                            }
                            self.sleep_used = total;
                            Some(capped)
                        }
                        _ => None,
                    };
                    let args = function_call.clone().to_args();
                    self.pending = Some(Pending::Call {
                        call_id,
                        function_name: function_name.clone(),
                        os_call: Some(Box::new(function_call)),
                        allow_eager_await,
                    });
                    return Ok(ControlEvent::Turn(TurnEvent::OsCall {
                        function_name,
                        args,
                        call_id,
                        allow_eager_await,
                        system_sleep,
                        position,
                    }));
                }
                Some(pb::worker_event::Kind::NameLookup(lookup)) => {
                    let position = lookup.position.map_or_else(SourceRange::unknown, Into::into);
                    let object_id = match lookup.object_id {
                        None => None,
                        Some(uuid) => match MontyUuid::try_from_slice(&uuid.data) {
                            Some(u) => Some(u),
                            None => {
                                return Err(self.protocol_violation(
                                    "NameLookup.object_id is not a 16-byte uuid",
                                ));
                            }
                        },
                    };
                    self.pending = Some(Pending::NameLookup);
                    return Ok(ControlEvent::Turn(TurnEvent::NameLookup {
                        name: lookup.name,
                        object_id,
                        position,
                    }));
                }
                Some(pb::worker_event::Kind::ResolveFutures(futures)) => {
                    let position = futures.position.map_or_else(SourceRange::unknown, Into::into);
                    let pending_call_ids = futures.pending_call_ids.into_inner();
                    self.pending = Some(Pending::Futures);
                    return Ok(ControlEvent::Turn(TurnEvent::ResolveFutures {
                        pending_call_ids,
                        position,
                    }));
                }
                Some(pb::worker_event::Kind::Complete(complete)) => {
                    self.pending = None;
                    self.feed_mounts = None;
                    let value = MontyObject::try_from(complete).map_err(|err| {
                        self.protocol_violation(format!("invalid Complete payload: {err}"))
                    })?;
                    return Ok(ControlEvent::Turn(TurnEvent::Complete(value)));
                }
                Some(pb::worker_event::Kind::Error(error)) => {
                    if event.feed_suspended != Some(true) && !is_dump {
                        self.pending = None;
                        self.feed_mounts = None;
                    }
                    let Some(exception) = error.exception else {
                        return Err(self.protocol_violation("error event with no exception"));
                    };
                    let exc = MontyException::try_from(exception).map_err(|err| {
                        self.protocol_violation(format!("invalid exception payload: {err}"))
                    })?;
                    return Err(PoolError::Runtime(exc));
                }
                Some(pb::worker_event::Kind::TypingError(typing)) => {
                    self.pending = None;
                    self.feed_mounts = None;
                    return Err(PoolError::Typing(typing.diagnostics));
                }
                Some(pb::worker_event::Kind::Ok(_)) => return Ok(ControlEvent::Ok),
                Some(pb::worker_event::Kind::DumpResult(dump)) => {
                    return Ok(ControlEvent::Dump(dump.state.into_inner()));
                }
                Some(pb::worker_event::Kind::FatalError(fatal)) => {
                    self.discard_worker();
                    return Err(PoolError::Crashed(fatal.message));
                }
                Some(pb::worker_event::Kind::SessionRefused(refused)) => {
                    self.discard_worker();
                    return Err(PoolError::Protocol(
                        format!("session refused: {}", refused.message).into(),
                    ));
                }
                Some(pb::worker_event::Kind::StreamedCalls(_))
                | Some(pb::worker_event::Kind::Shutdown(_))
                | None => {
                    return Err(self.protocol_violation("unexpected event from in-process worker"));
                }
            }
        }
    }

    fn discard_worker(&mut self) {
        self.worker = None;
        self.pending = None;
        self.feed_mounts = None;
    }

    fn protocol_violation(&mut self, msg: impl Into<Cow<'static, str>>) -> PoolError {
        self.discard_worker();
        PoolError::Protocol(msg.into())
    }
}

fn request(kind: pb::client_request::Kind) -> pb::ClientRequest {
    pb::ClientRequest {
        kind: Some(kind),
        trace_parent: None,
    }
}

/// Apply iOS-safe cooperative resource ceilings without loosening tighter caller limits.
fn clamp_limits(requested: Option<&ResourceLimits>) -> ResourceLimits {
    let mut limits = requested.cloned().unwrap_or_default();
    limits.max_memory = Some(
        limits
            .max_memory
            .map_or(INPROC_MAX_MEMORY, |m| m.min(INPROC_MAX_MEMORY)),
    );
    limits.max_feed_duration = Some(
        limits
            .max_feed_duration
            .map_or(INPROC_MAX_FEED_DURATION, |d| d.min(INPROC_MAX_FEED_DURATION)),
    );
    limits.max_turn_duration = Some(
        limits
            .max_turn_duration
            .map_or(INPROC_MAX_TURN_DURATION, |d| d.min(INPROC_MAX_TURN_DURATION)),
    );
    limits.max_recursion_depth = if limits.max_recursion_depth == 0 {
        INPROC_MAX_RECURSION_DEPTH
    } else {
        limits.max_recursion_depth.min(INPROC_MAX_RECURSION_DEPTH)
    };
    limits
}

fn build_configure(repl: &ReplConfig, limits: &ResourceLimits) -> pb::Configure {
    pb::Configure {
        script_name: repl.script_name.clone(),
        execution_clock: Some(pb::ExecutionClock::Wall as i32),
        limits: Some(limits.into()),
        type_check: repl.type_check,
        type_check_stubs: repl.type_check_stubs.clone(),
        type_check_format: pb::TypeCheckFormat::from(repl.type_check_config.format).into(),
        type_check_color: repl.type_check_config.color,
        assert_message_annotations: Some(repl.assert_message_annotations.max_bytes()),
        protocol_version: PROTOCOL_VERSION,
        monty_version: MONTY_VERSION.to_owned(),
        print_flush_interval_ms: repl.print_flush_interval.map(flush_interval_ms),
        os_policy: Some((&repl.os_policy).into()),
        persistence: pb::Persistence::Unspecified.into(),
        profile: None,
        accepts_streamed_calls: false,
        first: None,
    }
}

fn flush_interval_ms(d: Duration) -> u32 {
    if d.is_zero() {
        0
    } else {
        u32::try_from(d.as_millis().max(1)).unwrap_or(u32::MAX)
    }
}

fn cap_sleep(delay: Duration, policy: &OsPolicy) -> Duration {
    match policy.sleep {
        SleepMode::Zero => Duration::ZERO,
        SleepMode::System(max) => delay.min(max),
        _ => delay,
    }
}

fn build_feed_mounts(mounts: Vec<MountSpec>) -> Result<Option<MountTable>, PoolError> {
    if mounts.is_empty() {
        return Ok(None);
    }
    let mut table = MountTable::new();
    for mount in mounts {
        let mode = match mount.mode {
            MountSpecMode::ReadOnly => MountMode::ReadOnly,
            MountSpecMode::ReadWrite => MountMode::ReadWrite,
            MountSpecMode::Overlay => MountMode::OverlayMemory(OverlayState::new()),
        };
        let entry = Mount::with_root(mount.root, mode, mount.write_bytes_limit)
            .with_memory_usage_limit(mount.memory_usage_limit);
        table
            .push_mount(entry)
            .map_err(|err| PoolError::Runtime(err.into_exception()))?;
    }
    Ok(Some(table))
}
