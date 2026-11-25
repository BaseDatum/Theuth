//! Owner: B1 — boot (orphan reap, scan, start), per-plugin supervisors (one task per running
//! process: writer, reader, stderr), and [`Conn`], the host's end of one plugin's pipe.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use kameo::actor::ActorRef;
use serde_json::{Value, json};
use tokio::io::AsyncBufReadExt as _;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::actor::{Boot, Incoming, PluginsActor, ProcessEnded, ProcessUp, ShutdownAll};
use crate::config::PluginsConfig;
use crate::error::PluginsError;
use crate::logs::PluginLog;
use crate::manifest::Manifest;
use crate::proc::{self, ExitKind, Outbox, ReadItem, StrayGuard};

/// Lines of stderr kept for `PluginDetail.stderr_tail`.
pub(crate) const STDERR_TAIL: usize = 40;

/// Where everything lives (design §4.1, §11).
#[derive(Clone, Debug)]
pub(crate) struct Paths {
    /// `<data_dir>/<cfg.dir>`.
    pub plugins: PathBuf,
    /// `<data_dir>/plugin-data`.
    pub data: PathBuf,
    /// `<data_dir>/logs/plugins`.
    pub logs: PathBuf,
    /// `<data_dir>/mcp` (gate lines under `plugin.<name>`).
    pub mcp: PathBuf,
}

impl Paths {
    /// Paths under `data_dir`.
    pub fn new(data_dir: &std::path::Path, cfg: &PluginsConfig) -> Self {
        Self {
            plugins: data_dir.join(&cfg.dir),
            data: data_dir.join("plugin-data"),
            logs: data_dir.join("logs").join("plugins"),
            mcp: data_dir.join("mcp"),
        }
    }
    /// `.pgids` of running process groups.
    pub fn pgids(&self) -> PathBuf {
        self.plugins.join(".pgids")
    }
    /// `grants.yaml`.
    pub fn grants(&self) -> PathBuf {
        self.plugins.join(crate::grants::FILE)
    }
}

/// A JSON-RPC error a handler answers the plugin with (design §6.5).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PipeError {
    /// JSON-RPC code.
    pub code: i64,
    /// Message.
    pub message: String,
    /// `data`, when the code defines one.
    pub data: Option<Value>,
}

impl PipeError {
    /// `-32601`.
    pub fn not_found(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("method not found: {method}"),
            data: None,
        }
    }
    /// `-32602` with `data.path`.
    pub fn params(path: &str, message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
            data: Some(json!({ "path": path })),
        }
    }
    /// `-32003` with `data: {<kind>: <what>}`.
    pub fn not_granted(kind: &str, what: &str) -> Self {
        let mut data = serde_json::Map::new();
        data.insert(kind.to_owned(), Value::String(what.to_owned()));
        Self {
            code: -32003,
            message: format!("not granted: {kind} {what}"),
            data: Some(Value::Object(data)),
        }
    }
    /// `-32005` with `data.retry_after_ms`.
    pub fn rate_limited(retry_after_ms: u64) -> Self {
        Self {
            code: -32005,
            message: format!("rate limited; retry in {retry_after_ms} ms"),
            data: Some(json!({ "retry_after_ms": retry_after_ms })),
        }
    }
}

type Pending = HashMap<String, oneshot::Sender<Result<Value, PluginsError>>>;

/// The host's end of one running plugin: the outbox (two lanes) and the requests the host is
/// waiting on. Shared by the actor, the router (as a target), plugin tools and the supervisor.
pub(crate) struct Conn {
    /// Plugin name.
    pub name: String,
    /// Host → plugin lanes.
    pub outbox: Outbox,
    pending: Mutex<Pending>,
    next: AtomicU64,
    timeout: Duration,
    /// Cancelled when the host asked the process to stop (disable, restart, shutdown).
    pub stop: CancellationToken,
    /// Cancelled when the process is gone.
    pub gone: CancellationToken,
}

impl Conn {
    /// A connection whose requests time out after `timeout`.
    pub fn new(name: &str, outbox: Outbox, timeout: Duration) -> Arc<Self> {
        Arc::new(Self {
            name: name.to_owned(),
            outbox,
            pending: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            timeout,
            stop: CancellationToken::new(),
            gone: CancellationToken::new(),
        })
    }

    /// Send a request on the control lane and wait for its answer (`request_timeout_ms`), or
    /// until `cancel` fires (then `notifications/cancelled` follows).
    pub async fn request_with(
        &self,
        method: &str,
        params: Value,
        cancel: Option<CancellationToken>,
    ) -> Result<Value, PluginsError> {
        if self.gone.is_cancelled() || self.stop.is_cancelled() {
            return Err(PluginsError::NotRunning {
                name: self.name.clone(),
            });
        }
        let prefix = method.chars().next().unwrap_or('r');
        let id = format!("{prefix}-{}", self.next.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = oneshot::channel();
        if let Ok(mut p) = self.pending.lock() {
            p.insert(id.clone(), tx);
        }
        self.outbox
            .control(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        let cancelled = async {
            match &cancel {
                Some(c) => c.cancelled().await,
                None => std::future::pending().await,
            }
        };
        let out = tokio::select! {
            r = rx => r.unwrap_or_else(|_| Err(PluginsError::Exited { name: self.name.clone() })),
            _ = tokio::time::sleep(self.timeout) => {
                self.cancel(&id, "timeout");
                Err(PluginsError::Timeout { name: self.name.clone(), method: method.to_owned() })
            }
            _ = cancelled => {
                self.cancel(&id, "cancelled");
                Err(PluginsError::Io { message: format!("{method} cancelled") })
            }
        };
        out
    }

    /// [`Conn::request_with`] without a cancel token.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, PluginsError> {
        self.request_with(method, params, None).await
    }

    fn cancel(&self, id: &str, reason: &str) {
        if let Ok(mut p) = self.pending.lock() {
            p.remove(id);
        }
        self.notify(
            "notifications/cancelled",
            json!({ "requestId": id, "reason": reason }),
        );
    }

    /// A notification on the control lane.
    pub fn notify(&self, method: &str, params: Value) {
        self.outbox
            .control(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    /// Answer one of the plugin's requests.
    pub fn reply(&self, id: Value, result: Result<Value, PipeError>) {
        let msg = match result {
            Ok(v) => json!({ "jsonrpc": "2.0", "id": id, "result": v }),
            Err(e) => {
                let mut err = json!({ "code": e.code, "message": e.message });
                if let Some(d) = e.data {
                    err["data"] = d;
                }
                json!({ "jsonrpc": "2.0", "id": id, "error": err })
            }
        };
        self.outbox.control(msg);
    }

    /// Route a response to the request waiting on it. False when `msg` is not a response.
    pub fn resolve(&self, msg: &Value) -> bool {
        if msg.get("method").is_some() {
            return false;
        }
        let Some(id) = msg.get("id") else {
            return false;
        };
        let key = match id {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let tx = self.pending.lock().ok().and_then(|mut p| p.remove(&key));
        let Some(tx) = tx else { return true }; // late or unknown answer: discarded
        let out = match (msg.get("result"), msg.get("error")) {
            (_, Some(e)) => Err(PluginsError::Plugin {
                code: e.get("code").and_then(Value::as_i64).unwrap_or(-32000),
                message: e
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("plugin error")
                    .to_owned(),
            }),
            (Some(r), None) => Ok(r.clone()),
            (None, None) => Ok(Value::Null),
        };
        let _ = tx.send(out);
        true
    }

    /// The process is gone: every pending request answers `Exited` (design §5 "Crash").
    pub fn fail_all(&self) {
        self.gone.cancel();
        let drained: Vec<_> = self
            .pending
            .lock()
            .map(|mut p| p.drain().collect())
            .unwrap_or_default();
        for (_, tx) in drained {
            let _ = tx.send(Err(PluginsError::Exited {
                name: self.name.clone(),
            }));
        }
    }
}

/// Everything a supervisor needs to run one process generation.
pub(crate) struct Launch {
    pub name: String,
    pub epoch: u64,
    pub dir: PathBuf,
    pub manifest: Manifest,
    pub data_dir: PathBuf,
    pub base_env: Arc<BTreeMap<String, String>>,
    pub hello: Value,
    pub conn: Arc<Conn>,
    pub log: Arc<PluginLog>,
    pub stderr_tail: Arc<Mutex<VecDeque<String>>>,
    pub cfg: PluginsConfig,
    pub pgids: PathBuf,
    pub actor: ActorRef<PluginsActor>,
}

/// The environment of one plugin (design §5 "Spawn"): login env, `THEUTH_*`, then the expanded
/// manifest `env`. The RPC token never appears in it.
async fn plugin_env(l: &Launch) -> Result<BTreeMap<String, String>, String> {
    let mut env = (*l.base_env).clone();
    env.insert("THEUTH_PLUGIN".into(), l.name.clone());
    env.insert(
        "THEUTH_PLUGIN_DATA".into(),
        l.data_dir.display().to_string(),
    );
    env.insert("THEUTH_PROTOCOL".into(), crate::PROTOCOL.to_string());
    env.insert("THEUTH_PLUGIN_DIR".into(), l.dir.display().to_string());
    env.insert("PYTHONUNBUFFERED".into(), "1".into());
    let expanded = ::tools::mcp::expand_map(&l.manifest.env)
        .await
        .map_err(|e| format!("env: {e}"))?;
    env.extend(expanded);
    Ok(env)
}

/// The argv with `run[0]` made absolute when it names a file in the plugin's cwd.
fn argv(l: &Launch, cwd: &std::path::Path) -> Vec<String> {
    let mut argv = l.manifest.run.clone();
    if let Some(first) = argv.first_mut() {
        let p = std::path::Path::new(first.as_str());
        if p.is_relative() && first.contains('/') {
            let abs = cwd.join(p);
            if abs.exists() {
                *first = abs.display().to_string();
            }
        }
    }
    argv
}

/// Run one process generation to its end, then tell the actor how it ended.
pub(crate) async fn supervise(l: Launch) {
    let ended = |kind: ExitKind, error: Option<String>, ran: Duration| ProcessEnded {
        name: l.name.clone(),
        epoch: l.epoch,
        kind,
        error,
        ran_for: ran,
    };
    let cwd = l.dir.join(&l.manifest.cwd);
    let env = match plugin_env(&l).await {
        Ok(e) => e,
        Err(e) => {
            l.log.line("host", &e);
            l.conn.fail_all();
            let _ = l
                .actor
                .tell(ended(ExitKind::Failed, Some(e), Duration::ZERO))
                .await;
            return;
        }
    };
    let _ = std::fs::create_dir_all(&l.data_dir);
    let spec = proc::SpawnSpec {
        argv: argv(&l, &cwd),
        cwd,
        env,
    };
    let spawned = match proc::spawn(&spec) {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("spawn {}: {e}", spec.argv.join(" "));
            l.log.line("host", &msg);
            l.conn.fail_all();
            let _ = l
                .actor
                .tell(ended(ExitKind::Failed, Some(msg), Duration::ZERO))
                .await;
            return;
        }
    };
    let started = Instant::now();
    let proc::Spawned {
        mut child,
        tree,
        pgid,
        stdin,
        stdout,
        stderr,
    } = spawned;
    let pgids = proc::Pgids::new(l.pgids.clone());
    if let Some(g) = pgid {
        pgids.add(g, &spec.argv);
    }
    l.log
        .line("host", &format!("started: {}", spec.argv.join(" ")));
    l.conn
        .outbox
        .control(json!({ "jsonrpc": "2.0", "method": "hello", "params": l.hello }));
    let _ = l
        .actor
        .tell(ProcessUp {
            name: l.name.clone(),
            epoch: l.epoch,
        })
        .await;

    let writer = tokio::spawn(proc::write_loop(l.conn.outbox.clone(), stdin));
    let err_log = l.log.clone();
    let tail = l.stderr_tail.clone();
    let err_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let red = ::tools::secrets::redact(&line).into_owned();
            err_log.line("stderr", &red);
            if let Ok(mut t) = tail.lock() {
                t.push_back(red);
                while t.len() > STDERR_TAIL {
                    t.pop_front();
                }
            }
        }
    });

    let mut reader = proc::LineReader::new(stdout, l.cfg.max_line_bytes);
    let mut stray = StrayGuard::new(started);
    let mut flood = false;
    loop {
        let item = tokio::select! {
            r = reader.next() => r,
            _ = l.conn.stop.cancelled() => break,
        };
        match item {
            Ok(Some(ReadItem::Line(bytes))) => match proc::classify(&bytes) {
                proc::Incoming::Msg(msg) => {
                    if l.conn.resolve(&msg) {
                        continue;
                    }
                    if l.actor
                        .tell(Incoming {
                            name: l.name.clone(),
                            epoch: l.epoch,
                            msg,
                        })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                proc::Incoming::Stray(line) => {
                    l.log.line("stdout", &::tools::secrets::redact(&line));
                    if stray.note(Instant::now()) {
                        l.log.line(
                            "host",
                            "more than 100 stray stdout lines in 10 s: treated as a crash",
                        );
                        flood = true;
                        break;
                    }
                }
            },
            Ok(Some(ReadItem::TooLong { bytes, id })) => {
                l.log.line(
                    "host",
                    &format!("line over max_line_bytes ({bytes} bytes) discarded"),
                );
                if let Some(id) = id {
                    l.conn
                        .reply(id, Err(PipeError::params("", "line over max_line_bytes")));
                }
            }
            Ok(None) | Err(_) => break,
        }
    }

    // Stopping or EOF: give the process its grace, then kill the tree.
    let grace = Duration::from_millis(l.cfg.shutdown_grace_ms);
    let status = if flood {
        if let Some(t) = &tree {
            t.kill();
        }
        child.wait().await.ok()
    } else {
        match tokio::time::timeout(grace, child.wait()).await {
            Ok(s) => s.ok(),
            Err(_) => {
                if let Some(t) = &tree {
                    proc::stop_tree(t, Duration::from_millis(200)).await;
                }
                let _ = child.kill().await;
                child.wait().await.ok()
            }
        }
    };
    if let Some(t) = &tree
        && t.alive()
    {
        t.kill();
    }
    l.conn.outbox.close();
    l.conn.fail_all();
    writer.abort();
    let _ = tokio::time::timeout(Duration::from_millis(500), err_task).await;
    if let Some(g) = pgid {
        pgids.remove(g);
    }
    let ran = started.elapsed();
    let (kind, error) = match (flood, status) {
        (true, _) => (ExitKind::Failed, Some("stray-output flood".to_owned())),
        (false, Some(s)) if s.success() => (ExitKind::Clean, None),
        (false, Some(s)) => {
            let last = l.stderr_tail.lock().ok().and_then(|t| t.back().cloned());
            let why = match s.code() {
                Some(c) => format!("exit {c}"),
                None => "killed by a signal".to_owned(),
            };
            (
                ExitKind::Failed,
                Some(last.map_or(why.clone(), |line| format!("{why}: {line}"))),
            )
        }
        (false, None) => (ExitKind::Failed, Some("lost the process".to_owned())),
    };
    l.log.line(
        "host",
        &format!("ended: {}", error.as_deref().unwrap_or("exit 0")),
    );
    let _ = l.actor.tell(ended(kind, error, ran)).await;
}

/// The login environment every plugin starts from: this process's env without `THEUTH_*`
/// keys, with the login shell's `PATH` (as `tools.login_path` does for shells).
pub(crate) async fn base_env() -> BTreeMap<String, String> {
    let mut env: BTreeMap<String, String> = std::env::vars()
        .filter(|(k, _)| !k.starts_with("THEUTH_"))
        .collect();
    if let Some(shell) = std::env::var_os("SHELL")
        && let Ok(path) = platform::env::login_path(&shell, Duration::from_secs(5)).await
    {
        env.insert("PATH".into(), path);
    }
    env
}

/// Background start: reap orphans from `.pgids`, scan `plugins/`, validate, start what the
/// authenticated grants cover, file cards for the rest, register the attention Source and the
/// tool registry entries, start the watcher. Never panics; logs and returns on shutdown.
pub(crate) async fn boot(rt: runtime::Runtime, actor: ActorRef<PluginsActor>, cfg: PluginsConfig) {
    let paths = Paths::new(&rt.data_dir, &cfg);
    for d in [&paths.plugins, &paths.data, &paths.logs] {
        if let Err(e) = std::fs::create_dir_all(d) {
            tracing::warn!(error = %e, dir = %d.display(), "plugins: cannot create folder");
        }
    }
    proc::reap_orphans(&paths.pgids(), Duration::from_millis(cfg.shutdown_grace_ms)).await;
    crate::logs::sweep(&paths.logs, cfg.log_days);
    let key = match secrets::handle(&rt).await {
        Some(v) => match v.plugins_grants_key().await {
            Ok(k) => Some(k),
            Err(e) => {
                tracing::warn!(error = %e, "plugins: no grants key; nothing can be approved");
                None
            }
        },
        None => {
            tracing::warn!("plugins: no vault; nothing can be approved");
            None
        }
    };
    let env = Arc::new(base_env().await);
    let reply = match actor.ask(Boot { key, base_env: env }).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "plugins: boot failed");
            return;
        }
    };
    tokio::spawn(crate::attention::register(rt.clone(), reply.inbox));
    tokio::spawn(crate::route::feed_loop(
        rt.clone(),
        reply.targets,
        cfg.clone(),
    ));
    if cfg.watch {
        tokio::spawn(crate::watch::run(
            rt.clone(),
            paths.plugins.clone(),
            actor.clone(),
        ));
    }
    rt.shutdown.cancelled().await;
    let grace = Duration::from_millis(cfg.shutdown_grace_ms + 500);
    let _ = tokio::time::timeout(grace, actor.ask(ShutdownAll)).await;
    // Every tree was asked to stop and killed after its grace (design §5 "Engine shutdown").
    proc::Pgids::new(paths.pgids()).clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn request_resolves_and_times_out() {
        let outbox = Outbox::new(16);
        let conn = Conn::new("t", outbox, Duration::from_millis(200));
        let c2 = conn.clone();
        let task = tokio::spawn(async move { c2.request("action", json!({})).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(conn.resolve(&json!({"jsonrpc": "2.0", "id": "a-1", "result": {"ok": true}})));
        assert_eq!(task.await.unwrap().unwrap(), json!({"ok": true}));
        let err = conn.request("query", json!({})).await.unwrap_err();
        assert!(matches!(err, PluginsError::Timeout { .. }), "{err:?}");
        let c3 = conn.clone();
        let task = tokio::spawn(async move { c3.request("render", json!({})).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        conn.fail_all();
        assert!(matches!(
            task.await.unwrap().unwrap_err(),
            PluginsError::Exited { .. }
        ));
    }

    #[test]
    fn plugin_errors_pass_through() {
        let conn = Conn::new("t", Outbox::new(4), Duration::from_secs(1));
        let (tx, mut rx) = oneshot::channel();
        conn.pending.lock().unwrap().insert("7".into(), tx);
        assert!(conn.resolve(
            &json!({"jsonrpc": "2.0", "id": 7, "error": {"code": -32601, "message": "nope"}})
        ));
        assert_eq!(
            rx.try_recv().unwrap(),
            Err(PluginsError::Plugin {
                code: -32601,
                message: "nope".into()
            })
        );
        assert!(!conn.resolve(&json!({"jsonrpc": "2.0", "method": "emit", "params": {}})));
    }
}
