//! The headless parser helper (46 §2.1): one lazy, long-lived
//! `theuth-app-parser` child per harness, JSON lines over stdio, served strictly
//! in order. This side owns admission ([`PENDING`] queued requests, more →
//! [`Error::Busy`]), the [`DEADLINE`] covering write and response read, and
//! the kill/replace policy: a timeout, a crash, an oversized or malformed
//! response line kills the child and fails that one request; the next
//! request starts a fresh child. The executable's `--version` hash must
//! equal ours before it serves anything, whatever its file name says.
//! Shutdown kills the child; `kill_on_drop` covers everything else.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::{AppBinding, AppDiagnostic};

/// Requests queued behind the one being served (46 §7 "parser admission").
pub const PENDING: usize = 8;
/// Per request: from writing the line to having read the whole response.
pub const DEADLINE: Duration = Duration::from_secs(2);
/// Largest response line accepted; the helper refuses request lines above
/// the same size, so a longer response is a broken helper.
pub const MAX_LINE: usize = 4 * 1024 * 1024;

/// One visible tool as the helper binds it (`protocol.ts` `ToolSnapshot`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolSnapshot {
    /// Registry name.
    pub name: String,
    /// Input JSON Schema.
    pub schema: serde_json::Value,
    /// [`tools::Risk`] spelled snake_case.
    pub risk: tools::Risk,
    /// [`tools::ToolInspection::identity`].
    pub identity: String,
}

/// `check` / `merge_check` request line (`protocol.ts` `ParserRequest`).
#[derive(Serialize)]
struct Request<'a> {
    id: String,
    op: &'static str,
    source: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    base: Option<&'a str>,
    tools: &'a [ToolSnapshot],
}

/// One statement of a checked program.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatementInfo {
    /// Identifier.
    pub id: String,
    /// `value`, `state`, `query` or `mutation`.
    pub kind: String,
}

/// What a check produced (`protocol.ts` `CheckResult`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CheckResult {
    /// Canonical validated source (merge output for `merge_check`).
    pub source: String,
    /// The helper's library hash.
    pub library_hash: String,
    /// Every statement.
    #[serde(default)]
    pub statements: Vec<StatementInfo>,
    /// Query/Mutation → tool bindings.
    #[serde(default)]
    pub bindings: Vec<AppBinding>,
    /// Empty means publishable.
    #[serde(default)]
    pub diagnostics: Vec<AppDiagnostic>,
    /// Declared `$state` defaults.
    #[serde(default)]
    pub state: serde_json::Value,
    /// Unreachable value statements (informational).
    #[serde(default)]
    pub orphaned: Vec<String>,
}

/// Why a request did not produce a [`CheckResult`]. Every variant is an
/// "app unavailable / try again" the model or user may read; none commits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// No executable at the expected path.
    Missing(PathBuf),
    /// The executable is another library version.
    Hash {
        /// Ours.
        expected: String,
        /// The executable's `--version`.
        found: String,
    },
    /// [`PENDING`] requests already queued.
    Busy,
    /// [`DEADLINE`] passed; the child was killed.
    Timeout,
    /// The child exited or its pipes broke; killed and reaped.
    Crashed(String),
    /// The helper answered with an error line or an unparseable response.
    Protocol(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(p) => write!(f, "app parser helper not found at {}", p.display()),
            Self::Hash { expected, found } => write!(
                f,
                "app parser helper is library {found}, this build needs {expected}"
            ),
            Self::Busy => write!(
                f,
                "app parser busy: {PENDING} validations pending; try again"
            ),
            Self::Timeout => write!(f, "app parser did not answer within {DEADLINE:?}"),
            Self::Crashed(e) => write!(f, "app parser helper failed: {e}"),
            Self::Protocol(e) => write!(f, "app parser protocol error: {e}"),
        }
    }
}

impl std::error::Error for Error {}

struct Job {
    line: Vec<u8>,
    id: String,
    reply: oneshot::Sender<Result<CheckResult, Error>>,
}

/// Handle to the helper. Cheap to share behind an `Arc`; the worker task
/// and child process start on the first request.
pub struct Helper {
    path: PathBuf,
    tx: mpsc::Sender<Job>,
    next: std::sync::atomic::AtomicU64,
}

impl Helper {
    /// `path` is the executable; `hash` the library hash its `--version`
    /// must print; `shutdown` kills the child.
    pub fn new(path: PathBuf, hash: &'static str, shutdown: CancellationToken) -> Self {
        let (tx, rx) = mpsc::channel(PENDING);
        tokio::spawn(worker(path.clone(), hash, rx, shutdown));
        Self {
            path,
            tx,
            next: std::sync::atomic::AtomicU64::new(1),
        }
    }

    /// The executable path this helper runs.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Validate complete `source`.
    pub async fn check(&self, source: &str, tools: &[ToolSnapshot]) -> Result<CheckResult, Error> {
        self.request("check", source, None, tools).await
    }

    /// Merge `patch` into `base` with upstream semantics and validate the result.
    pub async fn merge_check(
        &self,
        patch: &str,
        base: &str,
        tools: &[ToolSnapshot],
    ) -> Result<CheckResult, Error> {
        self.request("merge_check", patch, Some(base), tools).await
    }

    async fn request(
        &self,
        op: &'static str,
        source: &str,
        base: Option<&str>,
        tools: &[ToolSnapshot],
    ) -> Result<CheckResult, Error> {
        let id = self
            .next
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .to_string();
        let mut line = serde_json::to_vec(&Request {
            id: id.clone(),
            op,
            source,
            base,
            tools,
        })
        .map_err(|e| Error::Protocol(format!("serialize request: {e}")))?;
        if line.len() > MAX_LINE {
            return Err(Error::Protocol(format!(
                "request is {} bytes; the helper accepts {MAX_LINE}",
                line.len()
            )));
        }
        line.push(b'\n');
        let (reply, rx) = oneshot::channel();
        self.tx
            .try_send(Job { line, id, reply })
            .map_err(|e| match e {
                mpsc::error::TrySendError::Full(_) => Error::Busy,
                mpsc::error::TrySendError::Closed(_) => {
                    Error::Crashed("helper worker stopped (shutdown)".into())
                }
            })?;
        rx.await
            .unwrap_or_else(|_| Err(Error::Crashed("helper worker stopped".into())))
    }
}

struct Proc {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Proc {
    async fn kill(mut self) {
        // `kill` also reaps; `kill_on_drop` is the net under a cancelled await.
        let _ = self.child.kill().await;
    }
}

/// Serves jobs in order on one child at a time.
async fn worker(
    path: PathBuf,
    hash: &'static str,
    mut rx: mpsc::Receiver<Job>,
    shutdown: CancellationToken,
) {
    let mut proc: Option<Proc> = None;
    loop {
        let job = tokio::select! {
            _ = shutdown.cancelled() => break,
            j = rx.recv() => match j { Some(j) => j, None => break },
        };
        let mut p = match proc.take() {
            Some(p) => p,
            None => match tokio::time::timeout(DEADLINE, spawn(&path, hash)).await {
                Ok(Ok(p)) => p,
                Ok(Err(e)) => {
                    let _ = job.reply.send(Err(e));
                    continue;
                }
                Err(_) => {
                    let _ = job.reply.send(Err(Error::Timeout));
                    continue;
                }
            },
        };
        let served = tokio::select! {
            _ = shutdown.cancelled() => None,
            r = tokio::time::timeout(DEADLINE, serve(&mut p, &job)) => Some(r),
        };
        let Some(served) = served else {
            p.kill().await;
            let _ = job.reply.send(Err(Error::Crashed("shutdown".into())));
            break;
        };
        match served {
            Ok(Ok(result)) => {
                let _ = job.reply.send(Ok(result));
                proc = Some(p);
            }
            Ok(Err(e)) => {
                tracing::warn!("app parser helper replaced: {e}");
                p.kill().await;
                let _ = job.reply.send(Err(e));
            }
            Err(_) => {
                tracing::warn!("app parser helper killed after {DEADLINE:?}");
                p.kill().await;
                let _ = job.reply.send(Err(Error::Timeout));
            }
        }
    }
    if let Some(p) = proc {
        p.kill().await;
    }
}

fn command(path: &std::path::Path) -> Command {
    let mut cmd = Command::new(path);
    cmd.env("OPENUI_TELEMETRY_DISABLED", "1")
        .env("DO_NOT_TRACK", "1")
        .kill_on_drop(true);
    platform::process::hide_console(cmd.as_std_mut());
    cmd
}

/// `--version` must print our hash; then start the line server.
async fn spawn(path: &std::path::Path, hash: &str) -> Result<Proc, Error> {
    if !path.is_file() {
        return Err(Error::Missing(path.to_path_buf()));
    }
    let out = command(path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .map_err(|e| Error::Crashed(format!("{}: {e}", path.display())))?;
    let found = String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .last()
        .unwrap_or_default()
        .to_owned();
    if !out.status.success() || found != hash {
        return Err(Error::Hash {
            expected: hash.to_owned(),
            found: if found.is_empty() {
                format!("exit {}", out.status)
            } else {
                found
            },
        });
    }
    let mut child = command(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| Error::Crashed(format!("{}: {e}", path.display())))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| Error::Crashed("no stdin".into()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::Crashed("no stdout".into()))?;
    Ok(Proc {
        child,
        stdin,
        stdout: BufReader::new(stdout),
    })
}

/// Write one request line, read one bounded response line, check its id.
async fn serve(p: &mut Proc, job: &Job) -> Result<CheckResult, Error> {
    p.stdin
        .write_all(&job.line)
        .await
        .map_err(|e| Error::Crashed(format!("write: {e}")))?;
    p.stdin
        .flush()
        .await
        .map_err(|e| Error::Crashed(format!("flush: {e}")))?;
    let mut buf = Vec::new();
    let n = (&mut p.stdout)
        .take(MAX_LINE as u64 + 1)
        .read_until(b'\n', &mut buf)
        .await
        .map_err(|e| Error::Crashed(format!("read: {e}")))?;
    if n == 0 {
        return Err(Error::Crashed("exited without answering".into()));
    }
    if buf.len() > MAX_LINE {
        return Err(Error::Protocol(format!(
            "response line above {MAX_LINE} bytes"
        )));
    }
    let v: serde_json::Value =
        serde_json::from_slice(&buf).map_err(|e| Error::Protocol(format!("response: {e}")))?;
    if v.get("id").and_then(|i| i.as_str()) != Some(job.id.as_str()) {
        return Err(Error::Protocol(format!(
            "response id {} for request {}",
            v.get("id").unwrap_or(&serde_json::Value::Null),
            job.id
        )));
    }
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Err(Error::Protocol(e.to_owned()));
    }
    serde_json::from_value(v).map_err(|e| Error::Protocol(format!("response body: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "abc123";

    /// A `sh` helper: `--version` prints `HASH`; then `body` runs with the
    /// request stream on stdin. `$COUNT` is a file `body` may append to.
    fn script(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(
            &p,
            format!(
                "#!/bin/sh\nif [ \"$1\" = --version ]; then echo \"theuth-app-parser {HASH}\"; exit 0; fi\n{body}\n"
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p
    }

    /// Answers every line with an empty valid `CheckResult` carrying its id.
    const ECHO: &str = r#"while IFS= read -r line; do
id=$(printf '%s' "$line" | sed -E 's/^\{"id":"([^"]*)".*/\1/')
printf '{"id":"%s","source":"root = Stack([])","library_hash":"abc123","statements":[{"id":"root","kind":"value"}],"bindings":[],"diagnostics":[],"state":{},"orphaned":[]}\n' "$id"
done"#;

    fn helper(path: PathBuf) -> Helper {
        Helper::new(path, HASH, CancellationToken::new())
    }

    #[tokio::test]
    async fn serves_in_order_on_one_child_and_matches_ids() {
        let dir = tempfile::tempdir().unwrap();
        let h = helper(script(dir.path(), "ok.sh", ECHO));
        for _ in 0..3 {
            let r = h.check("root = Stack([])", &[]).await.unwrap();
            assert_eq!(r.statements[0].id, "root");
            assert_eq!(r.library_hash, HASH);
        }
    }

    #[tokio::test]
    async fn wrong_hash_or_missing_binary_never_serves() {
        let dir = tempfile::tempdir().unwrap();
        // `script` always prints HASH for `--version`; this one lies about its library.
        let other = script(dir.path(), "other.sh", ECHO);
        std::fs::write(&other, "#!/bin/sh\necho 'theuth-app-parser deadbeef'\n").unwrap();
        let h = helper(other);
        assert_eq!(
            h.check("x", &[]).await.unwrap_err(),
            Error::Hash {
                expected: HASH.into(),
                found: "deadbeef".into()
            }
        );
        let missing = dir.path().join("nope");
        let h = helper(missing.clone());
        assert_eq!(
            h.check("x", &[]).await.unwrap_err(),
            Error::Missing(missing)
        );
    }

    #[tokio::test]
    async fn crash_fails_that_request_and_the_next_one_respawns() {
        let dir = tempfile::tempdir().unwrap();
        let count = dir.path().join("count");
        let body = format!("echo x >> {}\nIFS= read -r line\nexit 3", count.display());
        let h = helper(script(dir.path(), "crash.sh", &body));
        assert!(matches!(h.check("x", &[]).await, Err(Error::Crashed(_))));
        assert!(matches!(h.check("x", &[]).await, Err(Error::Crashed(_))));
        assert_eq!(std::fs::read_to_string(&count).unwrap().lines().count(), 2);
    }

    #[tokio::test]
    async fn hang_is_killed_at_the_deadline_and_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let count = dir.path().join("count");
        let body = format!(
            "echo x >> {}\nif [ ! -f {} ]; then touch {}; IFS= read -r line; exec sleep 30; fi\n{ECHO}",
            count.display(),
            dir.path().join("hung").display(),
            dir.path().join("hung").display(),
        );
        let h = helper(script(dir.path(), "hang.sh", &body));
        let t = std::time::Instant::now();
        assert_eq!(h.check("x", &[]).await.unwrap_err(), Error::Timeout);
        assert!(t.elapsed() >= DEADLINE && t.elapsed() < DEADLINE * 3);
        // Second child is the echo server.
        assert!(h.check("x", &[]).await.is_ok());
        assert_eq!(std::fs::read_to_string(&count).unwrap().lines().count(), 2);
    }

    #[tokio::test]
    async fn more_than_pending_queued_requests_are_busy_not_queued() {
        let dir = tempfile::tempdir().unwrap();
        // The child marks that it has taken a request, then hangs on it.
        let taken = dir.path().join("taken");
        let body = format!(
            "IFS= read -r line; touch {}; exec sleep 30",
            taken.display()
        );
        let h = std::sync::Arc::new(helper(script(dir.path(), "slow.sh", &body)));
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        // One request occupies the worker: wait until the child has read it.
        let mut waiting = vec![tokio::spawn({
            let h = h.clone();
            async move { h.check("x", &[]).await }
        })];
        while !taken.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "worker never took the first request"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // Then exactly PENDING more fill the queue; wait until it is full.
        for _ in 0..PENDING {
            let h = h.clone();
            waiting.push(tokio::spawn(async move { h.check("x", &[]).await }));
        }
        while h.tx.capacity() > 0 {
            assert!(std::time::Instant::now() < deadline, "queue never filled");
            tokio::task::yield_now().await;
        }
        assert_eq!(h.check("x", &[]).await.unwrap_err(), Error::Busy);
        for w in waiting {
            w.abort();
        }
    }

    #[tokio::test]
    async fn malformed_and_mismatched_responses_are_protocol_errors() {
        let dir = tempfile::tempdir().unwrap();
        let body = r#"IFS= read -r line; echo '{"id":"999","error":"bad"}'
IFS= read -r line; echo 'not json'"#;
        let h = helper(script(dir.path(), "bad.sh", body));
        assert!(matches!(h.check("x", &[]).await, Err(Error::Protocol(_))));
        // The child was replaced; a fresh one answers the id mismatch again.
        assert!(matches!(h.check("x", &[]).await, Err(Error::Protocol(_))));
    }

    #[tokio::test]
    async fn shutdown_stops_serving() {
        let dir = tempfile::tempdir().unwrap();
        let token = CancellationToken::new();
        let h = Helper::new(script(dir.path(), "ok.sh", ECHO), HASH, token.clone());
        assert!(h.check("x", &[]).await.is_ok());
        token.cancel();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(matches!(h.check("x", &[]).await, Err(Error::Crashed(_))));
    }
}
