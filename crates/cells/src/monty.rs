//! Owner: E2 — the Monty kernel (design 171 §5.9): a checkout from the cells-owned pool, no
//! tool table, `/cwd` mounted read-only, 60 s per run; plus the presentation-time sufficiency
//! check (§6.2).
//!
//! The pool is the agent's production shape (`agent::repl::open_pool` over this binary's
//! `t subprocess`), sized `cells.monty_workers`. A kernel keeps its checkout between runs (names
//! not starting with `_` persist); a Stop, a timeout or a crash drops the checkout, which kills
//! its worker (monty-pool's rule: a turn dropped mid-flight cannot be resumed), so the kernel is
//! `dead` and the next run starts fresh.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Weak};
use std::time::{Duration, Instant};

use monty_pool::{
    Checkout, MountSpec, MountSpecMode, Pool, PoolError, ReplConfig, ResumeValue, TurnEvent,
    on_print_sync,
};
use monty_types::{
    ExcType, MontyException, MontyObject, NameLookupResult, OsPolicy, PrintStream, ResourceLimits,
    SleepMode,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::error::{CellsError, Result};
use crate::kernel::{ExecuteRequest, Kernel, KernelCtx, KernelEvent, KernelSink};
use crate::types::{CellOutput, ExitStatus, KernelPhase, KernelSpec, RunId, RunMode};

/// §5.9: one run is one protocol turn, 60 s wall clock (`agent::repl::REQUEST_TIMEOUT`).
pub const RUN_LIMIT: Duration = agent::repl::REQUEST_TIMEOUT;
/// §5.9: the worker's heap limit (`agent::repl::MAX_MEMORY`, 512 MiB).
pub const MAX_MEMORY: usize = agent::repl::MAX_MEMORY;
/// What the strip says about a kernel evicted for a fifth one (§5.9).
pub const EVICTED: &str = "Monty kernel stopped to make room";

/// E2's Monty keys of the `cells:` section (flattened into [`crate::CellsConfig`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MontyConfig {}

/// The cells-owned pool, opened on first use; `None` when no worker can start.
static POOL: tokio::sync::OnceCell<Option<Arc<Pool>>> = tokio::sync::OnceCell::const_new();
/// Live Monty kernels, for the LRU eviction of §5.9.
static LIVE: LazyLock<Mutex<Vec<Weak<Inner>>>> = LazyLock::new(Default::default);
/// §6.2 results by code digest.
static SUFFICIENT: LazyLock<Mutex<HashMap<String, Result<(), String>>>> =
    LazyLock::new(Default::default);

/// The worker binary: this executable (`t subprocess`), as the agent's production pool.
#[cfg(not(test))]
fn worker() -> Option<PathBuf> {
    std::env::current_exe()
        .inspect_err(|e| tracing::warn!("cannot locate own executable ({e}); Monty cells disabled"))
        .ok()
}

/// Tests: `$THEUTH_MONTY_WORKER`, else `monty` on PATH (repl.rs's precedent; never an
/// installed `t`, which may speak an older protocol).
#[cfg(test)]
fn worker() -> Option<PathBuf> {
    std::env::var_os("THEUTH_MONTY_WORKER")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .or_else(|| {
            std::env::split_paths(&std::env::var_os("PATH")?)
                .map(|d| d.join("monty"))
                .find(|p| p.is_file())
        })
}

async fn pool(workers: u32) -> Option<Arc<Pool>> {
    POOL.get_or_init(|| async move {
        let exe = worker()?;
        agent::repl::open_pool(&exe, workers.max(1) as usize).await
    })
    .await
    .clone()
}

fn repl_config() -> ReplConfig {
    ReplConfig {
        script_name: "cell.py".into(),
        limits: Some(ResourceLimits {
            max_memory: Some(MAX_MEMORY),
            max_total_sleep: Some(RUN_LIMIT),
            ..ResourceLimits::default()
        }),
        type_check: true,
        type_check_stubs: Some("class PermissionError(Exception): ...\n".into()),
        os_policy: OsPolicy {
            sleep: SleepMode::System(RUN_LIMIT),
            ..OsPolicy::default()
        },
        ..ReplConfig::default()
    }
}

/// Start a Monty kernel (`KernelSpec::Monty`). The checkout is taken by the first run.
pub async fn start(spec: &KernelSpec, ctx: KernelCtx) -> Result<Box<dyn Kernel>, CellsError> {
    let workers = ctx.config.monty_workers.max(1);
    let pool = pool(workers)
        .await
        .ok_or_else(|| CellsError::NoInterpreter("Monty is not available on this engine".into()))?;
    make_room(workers as usize)?;
    let inner = Arc::new(Inner {
        pool,
        checkout: tokio::sync::Mutex::new(None),
        phase: Mutex::new(KernelPhase::Ready),
        cancel: Mutex::new(None),
        cwd: Mutex::new(None),
        last_used: Mutex::new(Instant::now()),
        count: Mutex::new(0),
        note: Mutex::new(None),
    });
    LIVE.lock().push(Arc::downgrade(&inner));
    Ok(Box::new(MontyKernel {
        spec: spec.clone(),
        inner,
    }))
}

/// §5.9: with `workers` kernels alive, close the least recently used idle one; none idle →
/// refused.
fn make_room(workers: usize) -> Result<()> {
    let mut live = LIVE.lock();
    live.retain(|w| {
        w.upgrade()
            .is_some_and(|k| !matches!(*k.phase.lock(), KernelPhase::Dead | KernelPhase::Closed))
    });
    if live.len() < workers {
        return Ok(());
    }
    let victim = live
        .iter()
        .filter_map(Weak::upgrade)
        .filter(|k| *k.phase.lock() == KernelPhase::Ready)
        .min_by_key(|k| *k.last_used.lock());
    let Some(v) = victim else {
        return Err(CellsError::KernelBusy(workers as u32));
    };
    *v.phase.lock() = KernelPhase::Closed;
    *v.note.lock() = Some(EVICTED.into());
    // An idle kernel's checkout is free: dropping it kills its worker.
    if let Ok(mut c) = v.checkout.try_lock() {
        c.take();
    }
    live.retain(|w| !std::ptr::eq(w.as_ptr(), Arc::as_ptr(&v)));
    tracing::info!("{EVICTED}");
    Ok(())
}

struct Inner {
    pool: Arc<Pool>,
    checkout: tokio::sync::Mutex<Option<Checkout>>,
    phase: Mutex<KernelPhase>,
    /// Cancels the run in flight (Stop / Kill / Close).
    cancel: Mutex<Option<CancellationToken>>,
    /// The host folder mounted at `/cwd` (kept for line runs without a prologue).
    cwd: Mutex<Option<PathBuf>>,
    last_used: Mutex<Instant>,
    count: Mutex<u32>,
    /// Why the kernel closed when it was not asked to (eviction).
    note: Mutex<Option<String>>,
}

struct MontyKernel {
    spec: KernelSpec,
    inner: Arc<Inner>,
}

/// How one run ended, before it becomes events.
enum Outcome {
    Value(MontyObject),
    /// An exception; `dead` = the worker is gone.
    Raised {
        exc: MontyException,
        dead: bool,
    },
    Typing(String),
    Crashed(String),
    Cancelled,
    Timeout,
}

#[async_trait::async_trait]
impl Kernel for MontyKernel {
    fn spec(&self) -> &KernelSpec {
        &self.spec
    }

    fn phase(&self) -> KernelPhase {
        *self.inner.phase.lock()
    }

    async fn execute(&self, req: ExecuteRequest, sink: KernelSink) -> Result<()> {
        let token = CancellationToken::new();
        {
            let mut phase = self.inner.phase.lock();
            if *phase != KernelPhase::Ready {
                let why = self.inner.note.lock().clone();
                return Err(CellsError::Failed(why.unwrap_or_else(|| {
                    format!("Monty kernel is {:?}", *phase).to_lowercase()
                })));
            }
            *phase = KernelPhase::Busy;
            *self.inner.cancel.lock() = Some(token.clone());
            *self.inner.last_used.lock() = Instant::now();
        }
        let inner = self.inner.clone();
        tokio::spawn(async move { run(inner, req, sink, token).await });
        Ok(())
    }

    async fn input(&self, run: &RunId, _bytes: Vec<u8>) -> Result<()> {
        Err(CellsError::NotFound(format!("{run}: Monty takes no input")))
    }

    async fn interrupt(&self) -> Result<()> {
        if let Some(t) = self.inner.cancel.lock().as_ref() {
            t.cancel();
        }
        Ok(())
    }

    async fn kill(&self) -> Result<()> {
        self.interrupt().await
    }

    async fn shutdown(&self) -> Result<()> {
        *self.inner.phase.lock() = KernelPhase::Closed;
        self.interrupt().await?;
        if let Ok(mut c) = self.inner.checkout.try_lock() {
            c.take();
        }
        Ok(())
    }

    fn live_cwd(&self) -> Option<String> {
        self.inner
            .cwd
            .lock()
            .as_ref()
            .map(|p| p.display().to_string())
    }
}

async fn run(inner: Arc<Inner>, req: ExecuteRequest, sink: KernelSink, token: CancellationToken) {
    let mut slot = inner.checkout.lock().await;
    if slot.is_none() {
        match inner.pool.checkout(&repl_config()).await {
            Ok(c) => *slot = Some(c),
            Err(e) => {
                let _ = sink.send(KernelEvent::Started { log_from: None }).await;
                let _ = sink
                    .send(error_output(
                        "RuntimeError",
                        &format!("Monty did not start: {e}"),
                    ))
                    .await;
                finish(&inner, &sink, 1, true).await;
                return;
            }
        }
    }
    let _ = sink.send(KernelEvent::Started { log_from: None }).await;
    if let Some(cwd) = &req.cwd {
        *inner.cwd.lock() = Some(cwd.clone());
    }
    let cwd = inner.cwd.lock().clone();
    let mounts = cwd
        .iter()
        .filter_map(|c| {
            MountSpec::new("/cwd", c, MountSpecMode::ReadOnly)
                .inspect_err(|e| tracing::warn!("Monty /cwd → {}: {e}", c.display()))
                .ok()
        })
        .collect::<Vec<_>>();
    let printed: Arc<Mutex<Vec<(&'static str, String)>>> = Arc::default();
    let sinkp = printed.clone();
    let mut on_print = on_print_sync(move |stream, text| {
        let name = match stream {
            PrintStream::Stderr => "stderr",
            _ => "stdout",
        };
        let mut p = sinkp.lock();
        match p.last_mut() {
            Some((n, t)) if *n == name => t.push_str(text),
            _ => p.push((name, text.to_owned())),
        }
    });
    let limit = req.timeout.map_or(RUN_LIMIT, |t| t.min(RUN_LIMIT));
    let outcome = {
        let checkout = slot.as_mut().expect("checkout taken above");
        let drive = drive(checkout, req.code.clone(), mounts, &mut on_print);
        tokio::select! {
            r = tokio::time::timeout(limit, drive) => match r {
                Ok(o) => o,
                Err(_) => Outcome::Timeout,
            },
            _ = token.cancelled() => Outcome::Cancelled,
        }
    };
    drop(on_print);
    // §5.9 "Monty shows output when the run ends".
    let printed = std::mem::take(&mut *printed.lock());
    for (name, text) in printed {
        let _ = sink
            .send(KernelEvent::Output(CellOutput::Stream {
                name: name.into(),
                text,
            }))
            .await;
    }
    let (exit, dead) = match outcome {
        Outcome::Value(v) => {
            if v.type_name() != "NoneType" && !matches!(req.mode, RunMode::Line { .. }) {
                let count = {
                    let mut c = inner.count.lock();
                    *c += 1;
                    *c
                };
                let data = [(
                    "text/plain".to_string(),
                    serde_json::Value::String(v.py_repr()),
                )]
                .into();
                let _ = sink
                    .send(KernelEvent::Output(CellOutput::ExecuteResult {
                        data,
                        metadata: serde_json::json!({}),
                        count,
                    }))
                    .await;
            }
            (0, false)
        }
        Outcome::Raised { exc, dead } => {
            let _ = sink
                .send(KernelEvent::Output(CellOutput::Error {
                    ename: exc.exc_type().to_string(),
                    evalue: exc.message().unwrap_or_default().to_owned(),
                    traceback: exc.to_string().lines().map(|l| format!("{l}\n")).collect(),
                }))
                .await;
            (1, dead)
        }
        Outcome::Typing(diag) => {
            let _ = sink
                .send(error_output(
                    "TypeError",
                    &format!("type check failed (nothing ran):\n{diag}"),
                ))
                .await;
            (1, false)
        }
        Outcome::Crashed(e) => {
            let _ = sink
                .send(error_output("RuntimeError", &format!("Monty crashed: {e}")))
                .await;
            (1, true)
        }
        Outcome::Cancelled => {
            let _ = sink
                .send(error_output(
                    "KeyboardInterrupt",
                    "stopped — the Monty kernel restarts on the next run",
                ))
                .await;
            (1, true)
        }
        Outcome::Timeout => {
            let msg = format!("Monty runs at most {} s", limit.as_secs());
            let _ = sink.send(error_output("TimeoutError", &msg)).await;
            (1, true)
        }
    };
    if dead {
        slot.take(); // drops the checkout: its worker is killed, never returned to the pool
    }
    drop(slot);
    finish(&inner, &sink, exit, dead).await;
}

async fn finish(inner: &Inner, sink: &KernelSink, exit: i32, dead: bool) {
    {
        let mut phase = inner.phase.lock();
        if *phase != KernelPhase::Closed {
            *phase = if dead {
                KernelPhase::Dead
            } else {
                KernelPhase::Ready
            };
        }
        *inner.cancel.lock() = None;
        *inner.last_used.lock() = Instant::now();
    }
    let _ = sink
        .send(KernelEvent::Finished {
            exit: Some(ExitStatus::Code(exit)),
            log_to: None,
            kernel_died: dead,
        })
        .await;
}

fn error_output(ename: &str, evalue: &str) -> KernelEvent {
    KernelEvent::Output(CellOutput::Error {
        ename: ename.into(),
        evalue: evalue.into(),
        traceback: vec![format!("{ename}: {evalue}\n")],
    })
}

/// One protocol turn with an EMPTY tool table: every host name is undefined, every function
/// call not found, OS calls only through the read-only `/cwd` mount.
async fn drive(
    checkout: &mut Checkout,
    code: String,
    mounts: Vec<MountSpec>,
    on_print: &mut (dyn FnMut(PrintStream, &str) -> monty_pool::PrintFuture + Send),
) -> Outcome {
    let mut ev = checkout
        .feed(code, Vec::new(), mounts, false, &mut *on_print)
        .await;
    loop {
        let event = match ev {
            Ok(e) => e,
            Err(PoolError::Runtime(exc)) => {
                let dead = exc.exc_type() == ExcType::MemoryError;
                return Outcome::Raised { exc, dead };
            }
            Err(PoolError::Typing(diag)) => return Outcome::Typing(diag),
            Err(e) => return Outcome::Crashed(e.to_string()),
        };
        ev = match event {
            TurnEvent::Complete(v) => return Outcome::Value(v),
            TurnEvent::NameLookup { .. } => {
                checkout
                    .resume_name_lookup(NameLookupResult::Undefined, &mut *on_print)
                    .await
            }
            TurnEvent::FunctionCall { .. } => {
                checkout.resume(ResumeValue::NotFound, &mut *on_print).await
            }
            TurnEvent::ResolveFutures {
                pending_call_ids, ..
            } => {
                let fail = pending_call_ids
                    .into_iter()
                    .map(|id| {
                        let e = MontyException::new(
                            ExcType::RuntimeError,
                            Some("no host functions in a cell".into()),
                        );
                        (id, ResumeValue::Error(e))
                    })
                    .collect();
                checkout.resume_futures(fail, &mut *on_print).await
            }
            TurnEvent::OsCall {
                system_sleep: Some(d),
                ..
            } => {
                tokio::time::sleep(d).await;
                checkout
                    .resume(ResumeValue::Return(MontyObject::none()), &mut *on_print)
                    .await
            }
            TurnEvent::OsCall { .. } => match checkout.resume_from_mounts(&mut *on_print).await {
                Ok(Some(e)) => Ok(e),
                Ok(None) => {
                    checkout
                        .resume(ResumeValue::NotHandled, &mut *on_print)
                        .await
                }
                Err(e) => Err(e),
            },
        };
    }
}

/// §6.2: `Ok(())` when Monty can run `code` (`surface::cell::monty_scan` + the pool's type
/// check), else the reason shown on the cell. Cached by digest. `None` = not checked (no Monty
/// worker on this engine): validation then skips `monty_insufficient`.
///
/// monty-pool has no check-only call, so the type check feeds the code as the body of a
/// never-called `async def` on a throwaway checkout: the checker sees every statement, nothing
/// runs.
pub async fn sufficient(code: &str) -> Option<Result<(), String>> {
    if let Err(why) = surface::cell::monty_scan(code) {
        return Some(Err(why));
    }
    let key = surface::cell::digest(code);
    if let Some(r) = SUFFICIENT.lock().get(&key) {
        return Some(r.clone());
    }
    let pool = pool(crate::CellsConfig::default().monty_workers).await?;
    // Every worker may be held by a kernel: a check never waits behind them.
    let cfg = repl_config();
    let checkout = tokio::time::timeout(Duration::from_secs(5), pool.checkout(&cfg));
    let mut checkout = checkout.await.ok()?.ok()?;
    let mut body = String::from("async def __theuth_cell_check():\n    pass\n");
    for line in code.lines() {
        body.push_str("    ");
        body.push_str(line);
        body.push('\n');
    }
    let mut on_print = on_print_sync(|_, _| {});
    let res = match checkout
        .feed(body, Vec::new(), Vec::new(), false, &mut on_print)
        .await
    {
        Ok(_) | Err(PoolError::Runtime(_)) => Ok(()),
        // The wrapper's two lines come first; names an earlier cell bound are not a "no".
        Err(PoolError::Typing(diag)) => match surface::cell::monty_reason(&diag, 2) {
            Some(why) => Err(why),
            None => Ok(()),
        },
        Err(e) => {
            tracing::debug!("Monty sufficiency check failed to run: {e}");
            return None;
        }
    };
    drop(on_print);
    let _ = checkout.finish().await;
    SUFFICIENT.lock().insert(key, res.clone());
    Some(res)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::CellsPaths;
    use crate::types::{KernelId, Scope, SessionId};
    use tokio::sync::mpsc;

    fn ctx(dir: &std::path::Path) -> KernelCtx {
        KernelCtx {
            id: KernelId {
                scope: Scope::Session(SessionId(uuid::Uuid::nil())),
                spec: KernelSpec::Monty,
            },
            owner: SessionId(uuid::Uuid::nil()),
            login_env: Arc::default(),
            paths: CellsPaths::new(dir),
            config: Arc::default(),
            jobs: None,
            root: dir.to_path_buf(),
        }
    }

    async fn kernel(dir: &std::path::Path) -> Option<Box<dyn Kernel>> {
        if pool(4).await.is_none() {
            eprintln!("no THEUTH_MONTY_WORKER and monty not on PATH; skipping");
            return None;
        }
        Some(start(&KernelSpec::Monty, ctx(dir)).await.unwrap())
    }

    async fn run(
        k: &dyn Kernel,
        id: &str,
        code: &str,
        mode: RunMode,
        cwd: Option<PathBuf>,
    ) -> Vec<KernelEvent> {
        let (tx, mut rx) = mpsc::channel(64);
        let req = ExecuteRequest {
            run: RunId(id.into()),
            code: code.into(),
            mode,
            cwd,
            env: Vec::new(),
            timeout: None,
        };
        k.execute(req, tx).await.unwrap();
        let mut out = Vec::new();
        while let Some(ev) = tokio::time::timeout(Duration::from_secs(90), rx.recv())
            .await
            .unwrap()
        {
            let done = matches!(ev, KernelEvent::Finished { .. });
            out.push(ev);
            if done {
                break;
            }
        }
        out
    }

    #[tokio::test]
    async fn static_scan_refuses_without_a_worker() {
        assert_eq!(
            sufficient("import pandas").await,
            Some(Err(
                "line 1: no module named pandas — Monty runs the standard library only; \
                 use a local python"
                    .into()
            ))
        );
        assert!(matches!(sufficient("open('x')").await, Some(Err(_))));
    }

    #[tokio::test]
    async fn names_persist_output_at_the_end_and_last_expression_is_plain_text() {
        let t = tempfile::tempdir().unwrap();
        let Some(k) = kernel(t.path()).await else {
            return;
        };
        let evs = run(
            &*k,
            "r_1",
            "x = 20\nprint('hi')\nx + 1",
            RunMode::Cell,
            None,
        )
        .await;
        assert_eq!(evs[0], KernelEvent::Started { log_from: None });
        assert!(evs.contains(&KernelEvent::Output(CellOutput::Stream {
            name: "stdout".into(),
            text: "hi\n".into()
        })));
        assert!(evs.iter().any(|e| matches!(e,
            KernelEvent::Output(CellOutput::ExecuteResult { data, .. }) if data["text/plain"] == "21")));
        assert!(matches!(
            evs.last(),
            Some(KernelEvent::Finished {
                exit: Some(ExitStatus::Code(0)),
                kernel_died: false,
                ..
            })
        ));
        let evs = run(&*k, "r_2", "x * 2", RunMode::Cell, None).await;
        assert!(evs.iter().any(|e| matches!(e,
            KernelEvent::Output(CellOutput::ExecuteResult { data, .. }) if data["text/plain"] == "40")));
        // An exception keeps the kernel; a host name is undefined (empty tool table).
        let evs = run(&*k, "r_3", "read_file('x')", RunMode::Cell, None).await;
        assert!(
            evs.iter()
                .any(|e| matches!(e, KernelEvent::Output(CellOutput::Error { .. }))),
            "{evs:?}"
        );
        assert_eq!(k.phase(), KernelPhase::Ready);
        k.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn cwd_is_mounted_read_only() {
        let t = tempfile::tempdir().unwrap();
        let Some(k) = kernel(t.path()).await else {
            return;
        };
        let dir = t.path().join("work");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("in.txt"), "data").unwrap();
        let code = "from pathlib import Path\nPath('/cwd/in.txt').read_text()";
        let evs = run(&*k, "r_1", code, RunMode::Cell, Some(dir.clone())).await;
        assert!(evs.iter().any(|e| matches!(e,
            KernelEvent::Output(CellOutput::ExecuteResult { data, .. }) if data["text/plain"] == "'data'")), "{evs:?}");
        let evs = run(
            &*k,
            "r_2",
            "Path('/cwd/out.txt').write_text('x')",
            RunMode::Cell,
            None,
        )
        .await;
        assert!(
            evs.iter()
                .any(|e| matches!(e, KernelEvent::Output(CellOutput::Error { .. }))),
            "{evs:?}"
        );
        assert!(!dir.join("out.txt").exists());
        k.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn stop_kills_the_checkout_and_the_kernel_is_dead() {
        let t = tempfile::tempdir().unwrap();
        let Some(k) = kernel(t.path()).await else {
            return;
        };
        let (tx, mut rx) = mpsc::channel(64);
        let req = ExecuteRequest {
            run: RunId("r_1".into()),
            code: "while True:\n    pass".into(),
            mode: RunMode::Cell,
            cwd: None,
            env: Vec::new(),
            timeout: None,
        };
        k.execute(req, tx).await.unwrap();
        assert_eq!(
            rx.recv().await,
            Some(KernelEvent::Started { log_from: None })
        );
        k.interrupt().await.unwrap();
        let mut last = None;
        while let Some(ev) = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
        {
            last = Some(ev);
            if matches!(last, Some(KernelEvent::Finished { .. })) {
                break;
            }
        }
        assert!(matches!(
            last,
            Some(KernelEvent::Finished {
                kernel_died: true,
                ..
            })
        ));
        assert_eq!(k.phase(), KernelPhase::Dead);
    }

    #[tokio::test]
    async fn type_check_decides_sufficiency() {
        if pool(4).await.is_none() {
            eprintln!("no Monty worker; skipping");
            return;
        }
        assert_eq!(sufficient("import math\nmath.sqrt(2)").await, Some(Ok(())));
        assert!(matches!(sufficient("x: int = 'a'").await, Some(Err(_))));
        // B10: the reason names Monty's error and the user's line.
        let rows = "rows: list[dict[str, str | float]] = [{\"usd\": 1.0, \"k\": \"a\"}]\n\
                    total = sum(r[\"usd\"] for r in rows)";
        let why = sufficient(rows).await.unwrap().unwrap_err();
        assert!(why.contains("No overload") && why.starts_with("line 2: "), "{why}");
        // B10b: a name an earlier cell bound in the kernel is not a refusal.
        assert_eq!(sufficient("print(rows_from_before)").await, Some(Ok(())));
    }
}
