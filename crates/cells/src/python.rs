//! Owner: E2 — python kernels (design 171 §5.8): the resolved interpreter running
//! `<data dir>/cells/shim.py` on pipes in its own process group, NDJSON frames.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};

use crate::error::{CellsError, Result};
use crate::kernel::{CellsPaths, ExecuteRequest, Kernel, KernelCtx, KernelEvent, KernelSink};
use crate::types::{CellOutput, ExitStatus, KernelPhase, KernelSpec, RunId, RunMode};

/// The shim, embedded and written to `<data dir>/cells/shim.py` at start (binary-owned).
pub const SHIM: &str = include_str!("../shim.py");

/// How long the shim may take to say `status ready`.
const READY_TIMEOUT: Duration = Duration::from_secs(20);
/// Early stderr kept for the "python did not start" reason.
const STDERR_KEEP: usize = 4096;

/// E2's python keys of the `cells:` section (flattened into [`crate::CellsConfig`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PythonConfig {}

/// Write the embedded shim to `paths.shim()` when it differs (binary-owned; a user edit there
/// is overwritten).
pub fn write_shim(paths: &CellsPaths) -> std::io::Result<()> {
    static WRITE: Mutex<()> = Mutex::new(());
    let _g = WRITE.lock();
    let path = paths.shim();
    if std::fs::read_to_string(&path).is_ok_and(|s| s == SHIM) {
        return Ok(());
    }
    std::fs::create_dir_all(&paths.root)?;
    let tmp = path.with_extension("py.tmp");
    std::fs::write(&tmp, SHIM)?;
    std::fs::rename(&tmp, &path)
}

/// Start a python kernel for `spec` (`KernelSpec::Python`).
pub async fn start(spec: &KernelSpec, ctx: KernelCtx) -> Result<Box<dyn Kernel>, CellsError> {
    let KernelSpec::Python { path } = spec else {
        return Err(CellsError::Failed(
            "python::start needs a python spec".into(),
        ));
    };
    write_shim(&ctx.paths)?;
    let mut cmd = Command::new(path);
    cmd.arg("-u")
        .arg(ctx.paths.shim())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    if !ctx.login_env.is_empty() {
        cmd.env_clear().envs(ctx.login_env.iter());
    }
    cmd.env("MPLBACKEND", "Agg")
        .env("PYTHONIOENCODING", "utf-8");
    if let Some(home) = ctx
        .login_env
        .get("HOME")
        .filter(|h| std::path::Path::new(h).is_dir())
    {
        cmd.current_dir(home);
    }
    platform::process::own_session(&mut cmd);
    platform::process::hide_console(cmd.as_std_mut());
    let mut child = cmd
        .spawn()
        .map_err(|e| CellsError::Failed(format!("{}: {e}", path.display())))?;
    let tree = child
        .id()
        .and_then(|pid| platform::process::ProcessTree::adopt(pid).ok());
    let stdin = child.stdin.take().expect("piped stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
    let stderr = Arc::new(Mutex::new(String::new()));
    if let Some(mut err) = child.stderr.take() {
        let keep = stderr.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            while let Ok(n) = err.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                let text = String::from_utf8_lossy(&buf[..n]);
                tracing::debug!(target: "cells::python", "{text}");
                let mut k = keep.lock();
                if k.len() < STDERR_KEEP {
                    k.push_str(&text);
                }
            }
        });
    }

    let shared = Arc::new(Shared {
        phase: Mutex::new(KernelPhase::Starting),
        current: Mutex::new(None),
        cwd: Mutex::new(None),
    });
    let ready = tokio::time::timeout(READY_TIMEOUT, wait_ready(&mut stdout, &shared)).await;
    if !matches!(ready, Ok(true)) {
        child.start_kill().ok();
        let _ = child.wait().await;
        let err = stderr.lock().trim().to_string();
        return Err(CellsError::Failed(format!(
            "python did not start ({}){}",
            path.display(),
            if err.is_empty() {
                String::new()
            } else {
                format!(": {err}")
            }
        )));
    }
    *shared.phase.lock() = KernelPhase::Ready;
    let reader = shared.clone();
    tokio::spawn(async move { read_frames(stdout, child, reader).await });
    Ok(Box::new(PythonKernel {
        spec: spec.clone(),
        stdin: tokio::sync::Mutex::new(stdin),
        tree,
        shared,
    }))
}

struct Shared {
    phase: Mutex<KernelPhase>,
    /// The run in flight and its sink.
    current: Mutex<Option<(RunId, KernelSink)>>,
    /// The shim's cwd after the last run.
    cwd: Mutex<Option<String>>,
}

struct PythonKernel {
    spec: KernelSpec,
    stdin: tokio::sync::Mutex<ChildStdin>,
    tree: Option<platform::process::ProcessTree>,
    shared: Arc<Shared>,
}

/// Read until `status ready` (true) or EOF (false).
async fn wait_ready(stdout: &mut BufReader<ChildStdout>, shared: &Shared) -> bool {
    let mut line = String::new();
    loop {
        line.clear();
        match stdout.read_line(&mut line).await {
            Ok(0) | Err(_) => return false,
            Ok(_) => {}
        }
        let Ok(f) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if f["type"] == "status" && f["state"] == "ready" {
            *shared.cwd.lock() = f["cwd"].as_str().map(str::to_owned);
            return true;
        }
    }
}

/// The frame pump: shim frames → the current run's sink; EOF → the kernel died.
async fn read_frames(
    mut stdout: BufReader<ChildStdout>,
    mut child: tokio::process::Child,
    shared: Arc<Shared>,
) {
    let mut line = String::new();
    loop {
        line.clear();
        match stdout.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Ok(f) = serde_json::from_str::<Value>(&line) else {
            tracing::debug!(target: "cells::python", "not a frame: {line:?}");
            continue;
        };
        let Some(sink) = sink_for(&shared, &f) else {
            continue;
        };
        if let Some(ev) = event(&f) {
            let done = matches!(ev, KernelEvent::Finished { .. });
            if done {
                *shared.cwd.lock() = f["cwd"].as_str().map(str::to_owned);
                *shared.current.lock() = None;
                set_phase(&shared, KernelPhase::Ready);
            }
            let _ = sink.send(ev).await;
        }
    }
    let status = child.wait().await.ok();
    if *shared.phase.lock() != KernelPhase::Closed {
        *shared.phase.lock() = KernelPhase::Dead;
    }
    let run = shared.current.lock().take();
    if let Some((_, sink)) = run {
        let _ = sink
            .send(KernelEvent::Finished {
                exit: status.map(exit_of),
                log_to: None,
                kernel_died: true,
            })
            .await;
    }
}

fn set_phase(shared: &Shared, phase: KernelPhase) {
    let mut p = shared.phase.lock();
    if !matches!(*p, KernelPhase::Closed | KernelPhase::Dead) {
        *p = phase;
    }
}

/// The sink of the run a frame belongs to (its `id`, or the current run when it has none).
fn sink_for(shared: &Shared, f: &Value) -> Option<KernelSink> {
    let cur = shared.current.lock();
    let (run, sink) = cur.as_ref()?;
    match f["id"].as_str() {
        Some(id) if id != run.as_str() => None,
        _ => Some(sink.clone()),
    }
}

/// One shim frame as a [`KernelEvent`] (§5.8 frames 2–6 and 9).
fn event(f: &Value) -> Option<KernelEvent> {
    let s = |k: &str| f[k].as_str().unwrap_or_default().to_owned();
    let bundle = |v: &Value| match v {
        Value::Object(m) => m.clone().into_iter().collect(),
        _ => Default::default(),
    };
    Some(match f["type"].as_str()? {
        "stream" => KernelEvent::Output(CellOutput::Stream {
            name: s("name"),
            text: s("text"),
        }),
        "display" => KernelEvent::Output(CellOutput::DisplayData {
            data: bundle(&f["data"]),
            metadata: f.get("metadata").cloned().unwrap_or_else(|| json!({})),
        }),
        "result" => KernelEvent::Output(CellOutput::ExecuteResult {
            data: bundle(&f["data"]),
            metadata: json!({}),
            count: f["count"].as_u64().unwrap_or(0) as u32,
        }),
        "error" => KernelEvent::Output(CellOutput::Error {
            ename: s("ename"),
            evalue: s("evalue"),
            traceback: f["traceback"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|l| l.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
        }),
        "input_request" => KernelEvent::InputRequest {
            prompt: s("prompt"),
            password: f["password"].as_bool().unwrap_or(false),
        },
        "status" => match f["state"].as_str()? {
            "busy" => KernelEvent::Started { log_from: None },
            "idle" => KernelEvent::Finished {
                exit: Some(ExitStatus::Code(if f["ok"].as_bool() == Some(true) {
                    0
                } else {
                    1
                })),
                log_to: None,
                kernel_died: false,
            },
            _ => return None,
        },
        _ => return None,
    })
}

fn exit_of(status: std::process::ExitStatus) -> ExitStatus {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return ExitStatus::Signal(signal_name(sig));
        }
    }
    ExitStatus::Code(status.code().unwrap_or(-1))
}

#[cfg(unix)]
fn signal_name(sig: i32) -> String {
    match sig {
        2 => "SIGINT".into(),
        9 => "SIGKILL".into(),
        15 => "SIGTERM".into(),
        n => format!("SIG{n}"),
    }
}

impl PythonKernel {
    async fn write(&self, frame: Value) -> Result<()> {
        let mut line = frame.to_string();
        line.push('\n');
        let mut stdin = self.stdin.lock().await;
        let res = async {
            stdin.write_all(line.as_bytes()).await?;
            stdin.flush().await
        }
        .await;
        res.map_err(|e| {
            set_phase(&self.shared, KernelPhase::Dead);
            CellsError::Failed(format!("python kernel exited: {e}"))
        })
    }
}

#[async_trait::async_trait]
impl Kernel for PythonKernel {
    fn spec(&self) -> &KernelSpec {
        &self.spec
    }

    fn phase(&self) -> KernelPhase {
        *self.shared.phase.lock()
    }

    async fn execute(&self, req: ExecuteRequest, sink: KernelSink) -> Result<()> {
        {
            let mut phase = self.shared.phase.lock();
            if *phase != KernelPhase::Ready {
                return Err(CellsError::Failed(
                    format!("python kernel is {:?}", *phase).to_lowercase(),
                ));
            }
            *phase = KernelPhase::Busy;
            *self.shared.current.lock() = Some((req.run.clone(), sink));
        }
        let mode = match req.mode {
            RunMode::Cell => "cell",
            RunMode::Line { .. } => "line",
            RunMode::Selection { .. } => "selection",
        };
        let mut frame =
            json!({"type": "execute", "id": req.run.as_str(), "code": req.code, "mode": mode});
        if let Some(cwd) = &req.cwd {
            frame["cwd"] = json!(cwd);
        }
        if !req.env.is_empty() {
            frame["env"] = Value::Object(
                req.env
                    .into_iter()
                    .map(|(k, v)| (k, Value::String(v)))
                    .collect(),
            );
        }
        self.write(frame).await
    }

    async fn input(&self, run: &RunId, bytes: Vec<u8>) -> Result<()> {
        if self.shared.current.lock().as_ref().map(|(r, _)| r) != Some(run) {
            return Err(CellsError::NotFound(run.0.clone()));
        }
        let text = String::from_utf8_lossy(&bytes);
        let value = text.strip_suffix('\n').unwrap_or(&text);
        let value = value.strip_suffix('\r').unwrap_or(value);
        self.write(json!({"type": "input_reply", "id": run.as_str(), "value": value}))
            .await
    }

    async fn interrupt(&self) -> Result<()> {
        if cfg!(windows) {
            return self.write(json!({"type": "interrupt"})).await;
        }
        match &self.tree {
            Some(t) => t.interrupt().map_err(CellsError::from),
            None => self.write(json!({"type": "interrupt"})).await,
        }
    }

    async fn kill(&self) -> Result<()> {
        if let Some(t) = &self.tree {
            t.kill();
        }
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        *self.shared.phase.lock() = KernelPhase::Closed;
        if let Some(t) = &self.tree {
            t.kill();
        }
        Ok(())
    }

    fn live_cwd(&self) -> Option<String> {
        self.shared.cwd.lock().clone()
    }
}

impl Drop for PythonKernel {
    fn drop(&mut self) {
        if let Some(t) = &self.tree {
            t.kill();
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::types::{CellOutput, KernelId, Scope, SessionId};
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use tokio::sync::mpsc;

    fn python3() -> Option<PathBuf> {
        let p = std::env::split_paths(&std::env::var_os("PATH")?)
            .map(|d| d.join("python3"))
            .find(|p| p.is_file());
        if p.is_none() {
            eprintln!("python3 not on PATH; skipping");
        }
        p
    }

    async fn kernel(dir: &std::path::Path) -> Option<Box<dyn Kernel>> {
        let path = python3()?;
        let spec = KernelSpec::Python { path };
        let ctx = KernelCtx {
            id: KernelId {
                scope: Scope::Session(SessionId(uuid::Uuid::nil())),
                spec: spec.clone(),
                instance: 0,
            },
            owner: SessionId(uuid::Uuid::nil()),
            login_env: Arc::new(BTreeMap::new()),
            paths: CellsPaths::new(dir),
            config: Arc::default(),
            jobs: None,
            root: dir.to_path_buf(),
        };
        Some(start(&spec, ctx).await.expect("python kernel starts"))
    }

    fn req(run: &str, code: &str, mode: RunMode) -> ExecuteRequest {
        ExecuteRequest {
            run: RunId(run.into()),
            code: code.into(),
            mode,
            cwd: None,
            env: Vec::new(),
            timeout: None,
        }
    }

    /// Run to `Finished`; every event in order.
    async fn run(k: &dyn Kernel, r: ExecuteRequest) -> Vec<KernelEvent> {
        let (tx, mut rx) = mpsc::channel(64);
        k.execute(r, tx).await.unwrap();
        collect(&mut rx).await
    }

    async fn collect(rx: &mut mpsc::Receiver<KernelEvent>) -> Vec<KernelEvent> {
        let mut out = Vec::new();
        while let Some(ev) = tokio::time::timeout(Duration::from_secs(20), rx.recv())
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

    fn stdout(evs: &[KernelEvent]) -> String {
        evs.iter()
            .filter_map(|e| match e {
                KernelEvent::Output(CellOutput::Stream { name, text }) if name == "stdout" => {
                    Some(text.as_str())
                }
                _ => None,
            })
            .collect()
    }

    fn finished_ok(evs: &[KernelEvent]) -> bool {
        matches!(
            evs.last(),
            Some(KernelEvent::Finished {
                exit: Some(ExitStatus::Code(0)),
                kernel_died: false,
                ..
            })
        )
    }

    #[tokio::test]
    async fn streams_in_order_with_subprocess_output_and_status_last() {
        let t = tempfile::tempdir().unwrap();
        let Some(k) = kernel(t.path()).await else {
            return;
        };
        assert_eq!(k.phase(), KernelPhase::Ready);
        let code = "import os, sys\nprint('a')\nos.system('echo c')\nsys.stdout.write('d' * 100000 + '\\n')\nprint('e')";
        let evs = run(&*k, req("r_1", code, RunMode::Cell)).await;
        assert_eq!(evs[0], KernelEvent::Started { log_from: None });
        assert_eq!(stdout(&evs), format!("a\nc\n{}\ne\n", "d".repeat(100_000)));
        assert!(finished_ok(&evs), "{:?}", evs.last());
        assert_eq!(k.phase(), KernelPhase::Ready);
        // Names persist between runs.
        let evs = run(
            &*k,
            req("r_2", "print(sys.argv is not None)", RunMode::Cell),
        )
        .await;
        assert_eq!(stdout(&evs), "True\n");
        k.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn last_expression_rule_and_mime_bundle() {
        let t = tempfile::tempdir().unwrap();
        let Some(k) = kernel(t.path()).await else {
            return;
        };
        let code = "class T:\n    def _repr_html_(self):\n        return '<b>x</b>'\n    def __repr__(self):\n        return 'T()'\ndisplay(T())\nT()";
        let evs = run(&*k, req("r_1", code, RunMode::Cell)).await;
        let outs: Vec<_> = evs
            .iter()
            .filter_map(|e| match e {
                KernelEvent::Output(o) => Some(o.clone()),
                _ => None,
            })
            .collect();
        let bundle: BTreeMap<String, Value> = BTreeMap::from([
            ("text/html".into(), json!("<b>x</b>")),
            ("text/plain".into(), json!("T()")),
        ]);
        assert_eq!(
            outs,
            [
                CellOutput::DisplayData {
                    data: bundle.clone(),
                    metadata: json!({})
                },
                CellOutput::ExecuteResult {
                    data: bundle,
                    metadata: json!({}),
                    count: 1
                },
            ]
        );
        // 171F4 T2-6: a line of a bare expression echoes like a REPL; a statement line, or an
        // assignment last, displays nothing; selection does.
        let evs = run(&*k, req("r_2", "1 + 2", RunMode::Line { line: 1 })).await;
        assert!(evs.iter().any(|e| matches!(e,
            KernelEvent::Output(CellOutput::ExecuteResult { data, .. }) if data["text/plain"] == json!("3"))), "{evs:?}");
        let evs = run(&*k, req("r_2b", "x = 1", RunMode::Line { line: 1 })).await;
        assert!(
            !evs.iter().any(|e| matches!(e, KernelEvent::Output(_))),
            "{evs:?}"
        );
        let evs = run(&*k, req("r_3", "x = 3", RunMode::Cell)).await;
        assert!(
            !evs.iter().any(|e| matches!(e, KernelEvent::Output(_))),
            "{evs:?}"
        );
        let evs = run(
            &*k,
            req("r_4", "x * 2", RunMode::Selection { from: 0, to: 5 }),
        )
        .await;
        assert!(evs.iter().any(|e| matches!(e,
            KernelEvent::Output(CellOutput::ExecuteResult { data, .. }) if data["text/plain"] == json!("6"))));
        k.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn exception_traceback_is_the_users_frames_only() {
        let t = tempfile::tempdir().unwrap();
        let Some(k) = kernel(t.path()).await else {
            return;
        };
        let evs = run(
            &*k,
            req(
                "r_1",
                "def f():\n    return 1 / 0\nprint('before')\nf()",
                RunMode::Cell,
            ),
        )
        .await;
        assert_eq!(stdout(&evs), "before\n");
        let err = evs.iter().find_map(|e| match e {
            KernelEvent::Output(CellOutput::Error {
                ename, traceback, ..
            }) => Some((ename.clone(), traceback.join(""))),
            _ => None,
        });
        let (ename, tb) = err.expect("an error output");
        assert_eq!(ename, "ZeroDivisionError");
        assert!(tb.contains("return 1 / 0"), "{tb}");
        assert!(!tb.contains("shim.py"), "{tb}");
        assert!(matches!(
            evs.last(),
            Some(KernelEvent::Finished {
                exit: Some(ExitStatus::Code(1)),
                ..
            })
        ));
        // The kernel survives.
        assert!(finished_ok(
            &run(&*k, req("r_2", "pass", RunMode::Cell)).await
        ));
        k.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn input_round_trip_and_password_flag() {
        let t = tempfile::tempdir().unwrap();
        let Some(k) = kernel(t.path()).await else {
            return;
        };
        let (tx, mut rx) = mpsc::channel(64);
        let code =
            "import getpass\nx = input('name? ')\np = getpass.getpass()\nprint('hi', x, len(p))";
        k.execute(req("r_1", code, RunMode::Cell), tx)
            .await
            .unwrap();
        let mut asked = Vec::new();
        let evs = loop {
            let ev = tokio::time::timeout(Duration::from_secs(10), rx.recv())
                .await
                .unwrap()
                .unwrap();
            if let KernelEvent::InputRequest { prompt, password } = &ev {
                asked.push((prompt.clone(), *password));
                let answer: &[u8] = if *password { b"s3cret\n" } else { b"sky\n" };
                k.input(&RunId("r_1".into()), answer.to_vec())
                    .await
                    .unwrap();
            }
            if matches!(ev, KernelEvent::Finished { .. }) {
                break vec![ev];
            }
            if let KernelEvent::Output(CellOutput::Stream { text, .. }) = &ev {
                assert_eq!(text, "hi sky 6\n");
            }
        };
        assert_eq!(
            asked,
            [
                ("name? ".to_string(), false),
                ("Password: ".to_string(), true)
            ]
        );
        assert!(finished_ok(&evs));
        assert!(
            k.input(&RunId("r_other".into()), b"x".to_vec())
                .await
                .is_err()
        );
        k.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn interrupt_stops_a_sleep_within_a_second_and_kill_marks_dead() {
        let t = tempfile::tempdir().unwrap();
        let Some(k) = kernel(t.path()).await else {
            return;
        };
        let (tx, mut rx) = mpsc::channel(64);
        k.execute(req("r_1", "import time\ntime.sleep(30)", RunMode::Cell), tx)
            .await
            .unwrap();
        assert_eq!(
            rx.recv().await,
            Some(KernelEvent::Started { log_from: None })
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
        let at = std::time::Instant::now();
        k.interrupt().await.unwrap();
        let evs = collect(&mut rx).await;
        assert!(at.elapsed() < Duration::from_secs(1), "{:?}", at.elapsed());
        assert!(evs.iter().any(|e| matches!(e,
            KernelEvent::Output(CellOutput::Error { ename, .. }) if ename == "KeyboardInterrupt")));
        assert_eq!(k.phase(), KernelPhase::Ready);

        let (tx, mut rx) = mpsc::channel(64);
        k.execute(req("r_2", "import time\ntime.sleep(30)", RunMode::Cell), tx)
            .await
            .unwrap();
        assert_eq!(
            rx.recv().await,
            Some(KernelEvent::Started { log_from: None })
        );
        k.kill().await.unwrap();
        let evs = collect(&mut rx).await;
        assert!(
            matches!(
                evs.last(),
                Some(KernelEvent::Finished {
                    kernel_died: true,
                    ..
                })
            ),
            "{evs:?}"
        );
        assert_eq!(k.phase(), KernelPhase::Dead);
        assert!(
            k.execute(req("r_3", "pass", RunMode::Cell), mpsc::channel(1).0)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn cwd_and_env_prologue_apply_and_persist() {
        let t = tempfile::tempdir().unwrap();
        let Some(k) = kernel(t.path()).await else {
            return;
        };
        let dir = t.path().canonicalize().unwrap().join("work");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let mut r = req(
            "r_1",
            "import os\nos.chdir('sub')\nprint(os.environ['P'])",
            RunMode::Cell,
        );
        r.cwd = Some(dir.clone());
        r.env = vec![("P".into(), "1".into())];
        let evs = run(&*k, r).await;
        assert_eq!(stdout(&evs), "1\n");
        assert_eq!(k.live_cwd(), Some(dir.join("sub").display().to_string()));
        // No prologue: the `chdir` of the previous run holds.
        let evs = run(
            &*k,
            req("r_2", "print(os.getcwd())", RunMode::Line { line: 1 }),
        )
        .await;
        assert_eq!(stdout(&evs), format!("{}\n", dir.join("sub").display()));
        k.shutdown().await.unwrap();
    }
}
