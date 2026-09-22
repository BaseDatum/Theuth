//! Client of the `theuth-documents` helper (design 59 §3.2; the pattern is
//! 46 §2.1's `app::helper`): one long-lived child, newline-delimited JSON
//! over stdio, binary fields base64. Unlike the app parser, requests are
//! pipelined (each carries an id) and the child pushes `{"ev":"update"}`
//! lines on its own after an import; those reach the actor as
//! [`HelperEvent`]s. A crash, a malformed line or a request past
//! [`DEADLINE`] kills the child; the next request respawns it after a
//! backoff and the actor learns of it through [`HelperEvent::Restarted`]
//! (its mirrors are gone and must be reopened). The executable's
//! `--version` must print our [`protocol_version`] before it serves anything.

use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::meta::DocId;

/// Per request: from writing the line to reading its response.
pub const DEADLINE: Duration = Duration::from_secs(30);
/// Largest line either way (the helper enforces the same on requests).
pub const MAX_LINE: usize = 16 * 1024 * 1024;
const BACKOFF_MIN: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

const PROTOCOL_TS: &str = include_str!("../../../packages/documents/src/protocol.ts");

/// `PROTOCOL_VERSION` from `packages/documents/src/protocol.ts`: one definition, read at compile time.
pub fn protocol_version() -> &'static str {
    static V: LazyLock<&'static str> = LazyLock::new(|| {
        let tail = PROTOCOL_TS
            .split_once("PROTOCOL_VERSION = \"")
            .map(|(_, t)| t)
            .expect("protocol.ts declares PROTOCOL_VERSION");
        tail.split_once('"').map(|(v, _)| v).expect("closing quote")
    });
    &V
}

/// Where the helper executable lives: `THEUTH_DOCUMENTS_HELPER` (a file path),
/// else `theuth-documents-<version>` beside the running binary; debug builds
/// fall back to `packages/documents/dist/theuth-documents` in the source tree
/// so `cargo run` and `just desktop` find the one `just documents-helper` built.
pub fn helper_path() -> PathBuf {
    if let Some(p) = std::env::var_os("THEUTH_DOCUMENTS_HELPER").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    let beside = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_default()
        .join(format!("theuth-documents-{}", protocol_version()));
    #[cfg(debug_assertions)]
    if !beside.is_file() {
        let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../packages/documents/dist/theuth-documents");
        if dev.is_file() {
            return dev;
        }
    }
    beside
}

/// Whether the helper `t` would spawn is there (surfaced by `documents_status`, shown in Settings;
/// a missing helper disables notes, knowledge editing and meeting notes).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct HelperStatus {
    /// Where [`helper_path`] points.
    pub path: String,
    /// The file exists there.
    pub found: bool,
    /// The protocol version this `t` wants (`theuth-documents-<version>`).
    pub version: String,
}

/// The current [`HelperStatus`].
pub fn helper_status() -> HelperStatus {
    let path = helper_path();
    HelperStatus {
        found: path.is_file(),
        path: path.display().to_string(),
        version: protocol_version().to_owned(),
    }
}

/// `import` mode (§4.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportMode {
    /// Block-diff: unchanged blocks keep ids and formatting.
    Diff,
    /// Drop every body block.
    Replace,
    /// Add after the existing body.
    Append,
}

/// `import` result.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ImportResult {
    /// Body blocks replaced or added.
    pub changed: u32,
    /// Body blocks after the import.
    pub total: u32,
    /// Ids the transaction wrote.
    #[serde(default)]
    pub touched: Vec<String>,
}

/// One §5.2 block edit, as `packages/documents/src/protocol.ts` `Edit` spells it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Edit {
    /// Parse `markdown` (several top-level blocks allowed) and place the blocks; exactly one of after/before/at.
    Insert {
        /// Markdown to parse with the document schema.
        markdown: String,
        /// Sibling to insert after.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<String>,
        /// Sibling to insert before.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<String>,
        /// `start` or `end` of the top level.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<String>,
    },
    /// Remove `blocks`, put the parsed blocks where the first one was.
    Replace {
        /// Ids to remove.
        blocks: Vec<String>,
        /// Their replacement.
        markdown: String,
    },
    /// Remove `blocks`.
    Delete {
        /// Ids to remove.
        blocks: Vec<String>,
    },
    /// Move `blocks` (ids kept) after/before a block or to `start`/`end`; exactly one of the three (§7.4).
    Move {
        /// Ids to move, in any order (document order is kept).
        blocks: Vec<String>,
        /// Sibling to land after.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<String>,
        /// Sibling to land before.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<String>,
        /// `start` or `end` of the top level.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<String>,
    },
    /// The inverse the helper computed for an earlier edit (§5.5); opaque here, passed back verbatim.
    Restore {
        /// Ids the edit inserted.
        remove: Vec<String>,
        /// Exact pre-edit block JSON.
        blocks: Vec<Value>,
        /// Where they go back.
        anchor: Value,
    },
}

/// `edit` result.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct EditResult {
    /// Ids the edit inserted.
    pub inserted: Vec<String>,
    /// Ids the edit removed.
    pub removed: Vec<String>,
    /// Ids the transaction wrote (= inserted; the reveal keys on it).
    #[serde(default)]
    pub touched: Vec<String>,
    /// Restores that put the exact prior blocks back, newest first.
    #[serde(default)]
    pub undo: Vec<Edit>,
}

/// `outline` result: the §5.1 text and the same as a tree.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct OutlineResult {
    /// One line per block.
    pub outline: String,
    /// `[{id, type, text, children}]`.
    #[serde(default)]
    pub blocks: Vec<Value>,
}

/// Why the helper refused an `edit` (`protocol.ts` `EditError`, decoded from the error string).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum EditError {
    /// No block has this id; `nearest` are the ids around the best match, in outline order.
    UnknownBlock {
        /// The id given.
        id: String,
        /// Candidates.
        nearest: Vec<String>,
    },
    /// The frontmatter block is edited through `knowledge_put`, never here.
    FrontmatterImmutable {
        /// Its id.
        id: String,
    },
    /// Malformed edit.
    BadEdit {
        /// Why.
        message: String,
    },
    /// Anything else (helper crash, timeout, unreadable reply).
    #[serde(skip)]
    Other(String),
}

impl EditError {
    /// Decode a helper error; non-JSON text is [`EditError::Other`].
    pub fn from_helper(e: &Error) -> Self {
        match e {
            Error::Protocol(s) => serde_json::from_str(s).unwrap_or_else(|_| Self::Other(s.clone())),
            other => Self::Other(other.to_string()),
        }
    }
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownBlock { id, nearest } => {
                write!(f, "no block `{id}` in this document")?;
                if !nearest.is_empty() {
                    write!(f, "; nearest ids in outline order: {}", nearest.join(", "))?;
                }
                write!(f, ". Read the outline again and quote an id from it.")
            }
            Self::FrontmatterImmutable { .. } => write!(
                f,
                "the frontmatter block cannot be replaced or deleted; edit metadata with knowledge_put"
            ),
            Self::BadEdit { message } => write!(f, "bad edit: {message}"),
            Self::Other(s) => f.write_str(s),
        }
    }
}

/// A line the helper pushed on its own, or the lifecycle notice.
#[derive(Clone, Debug)]
pub enum HelperEvent {
    /// One transaction the helper made (import now, edit in step 2), pushed
    /// before the reply of the request that caused it.
    Update {
        /// Document.
        doc: DocId,
        /// v1 update bytes.
        update: Vec<u8>,
        /// Block ids written.
        touched: Vec<String>,
        /// The request (see [`Pending::id`]) this transaction belongs to.
        req: u64,
        /// For an import: body blocks changed of `total` (the §4.6 `import-file` trigger).
        ratio: Option<(u32, u32)>,
    },
    /// A fresh child is serving: every mirror it held is gone.
    Restarted,
}

/// Why a request produced no result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// No executable at the expected path.
    Missing(PathBuf),
    /// The executable speaks another protocol version.
    Version {
        /// Ours.
        expected: String,
        /// Its `--version`.
        found: String,
    },
    /// [`DEADLINE`] passed; the child was killed.
    Timeout,
    /// The child exited or its pipes broke.
    Crashed(String),
    /// The helper answered `ok: false`, or the line was unreadable.
    Protocol(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(p) => write!(f, "documents helper not found at {}", p.display()),
            Self::Version { expected, found } => write!(
                f,
                "documents helper speaks protocol {found}, this build needs {expected}"
            ),
            Self::Timeout => write!(f, "documents helper did not answer within {DEADLINE:?}"),
            Self::Crashed(e) => write!(f, "documents helper failed: {e}"),
            Self::Protocol(e) => write!(f, "documents helper: {e}"),
        }
    }
}

impl std::error::Error for Error {}

/// Requests written and not yet answered: id → (sent at, reply).
type PendingMap = HashMap<u64, (Instant, oneshot::Sender<Result<Value, Error>>)>;

struct Job {
    id: u64,
    line: Vec<u8>,
    reply: oneshot::Sender<Result<Value, Error>>,
}

/// Handle to the helper; cheap to clone. The child starts on the first request.
#[derive(Clone)]
pub struct Helper {
    tx: mpsc::UnboundedSender<Job>,
    next: std::sync::Arc<AtomicU64>,
}

impl Helper {
    /// `path` is the executable; `events` receives pushed lines and restarts; `shutdown` kills the child.
    pub fn new(
        path: PathBuf,
        events: mpsc::UnboundedSender<HelperEvent>,
        shutdown: CancellationToken,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(worker(path, rx, events, shutdown));
        Self {
            tx,
            next: std::sync::Arc::new(AtomicU64::new(1)),
        }
    }

    /// Enqueue one request synchronously (ordering is the caller's enqueue order); await the reply.
    pub fn request(&self, op: &str, doc: &DocId, fields: Value) -> Pending {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let mut body = json!({ "id": id, "op": op, "doc": doc.as_str() });
        if let (Some(b), Some(f)) = (body.as_object_mut(), fields.as_object()) {
            for (k, v) in f {
                b.insert(k.clone(), v.clone());
            }
        }
        let mut line = serde_json::to_vec(&body).unwrap_or_default();
        line.push(b'\n');
        let (reply, rx) = oneshot::channel();
        let pending = Pending { id, rx };
        if line.len() > MAX_LINE {
            let _ = reply.send(Err(Error::Protocol(format!(
                "request is {} bytes; the helper accepts {MAX_LINE}",
                line.len()
            ))));
        } else if self.tx.send(Job { id, line, reply }).is_err() {
            // Worker gone (shutdown): the receiver sees a closed channel.
        }
        pending
    }

    /// Load a mirror from its full state. Enqueued synchronously so a following `update` cannot overtake it.
    pub fn open(&self, doc: &DocId, state: &[u8], fragment: &str) -> Pending {
        self.request(
            "open",
            doc,
            json!({ "state": b64(state), "fragment": fragment }),
        )
    }

    /// Forward one accepted update.
    pub fn update(&self, doc: &DocId, update: &[u8]) -> Pending {
        self.request("update", doc, json!({ "update": b64(update) }))
    }

    /// Drop a mirror.
    pub fn close(&self, doc: &DocId) -> Pending {
        self.request("close", doc, json!({}))
    }

    /// Lossy markdown of the whole document (frontmatter block → YAML header).
    pub async fn markdown(&self, doc: &DocId) -> Result<String, Error> {
        self.markdown_of(doc, None, false, &Default::default()).await
    }

    /// Lossy markdown of `blocks` (any depth) or the whole document; `annotate` prefixes each block with
    /// `<!-- id -->`, or `<!-- id by <label> -->` for ids in `authors` (§5.1, §11).
    pub async fn markdown_of(
        &self,
        doc: &DocId,
        blocks: Option<&[String]>,
        annotate: bool,
        authors: &std::collections::HashMap<String, String>,
    ) -> Result<String, Error> {
        let mut fields = json!({ "annotate": annotate });
        if let Some(b) = blocks {
            fields["blocks"] = json!(b);
        }
        if !authors.is_empty() {
            fields["authors"] = json!(authors);
        }
        let v = self.request("markdown", doc, fields).await?;
        v.get("markdown")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| Error::Protocol("markdown result lacks `markdown`".into()))
    }

    /// Import markdown; the transaction comes back as [`HelperEvent::Update`]
    /// (stamped with this request's [`Pending::id`]) before the reply, which
    /// [`Pending::import_result`] decodes.
    pub fn import(
        &self,
        doc: &DocId,
        markdown: &str,
        mode: ImportMode,
        frontmatter: bool,
    ) -> Pending {
        self.request(
            "import",
            doc,
            json!({ "markdown": markdown, "mode": mode, "frontmatter": frontmatter }),
        )
    }

    /// §5.2 edits in one transaction; like `import`, the update precedes the reply ([`Pending::edit_result`]).
    pub fn edit(&self, doc: &DocId, edits: &[Edit]) -> Pending {
        self.request("edit", doc, json!({ "edits": edits }))
    }

    /// §5.1 outline: text plus tree.
    pub async fn outline(&self, doc: &DocId, depth: Option<u32>) -> Result<OutlineResult, Error> {
        let v = self
            .request("outline", doc, json!({ "depth": depth }))
            .await?;
        serde_json::from_value(v).map_err(|e| Error::Protocol(format!("outline result: {e}")))
    }
}

/// A reply not yet read. Await it, or drop it to fire and forget.
pub struct Pending {
    id: u64,
    rx: oneshot::Receiver<Result<Value, Error>>,
}

impl Pending {
    /// The request id, as [`HelperEvent::Update::req`] will carry it.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Decode an `import` reply.
    pub async fn import_result(self) -> Result<ImportResult, Error> {
        let v = self.await?;
        serde_json::from_value(v).map_err(|e| Error::Protocol(format!("import result: {e}")))
    }

    /// Decode an `edit` reply.
    pub async fn edit_result(self) -> Result<EditResult, Error> {
        let v = self.await?;
        serde_json::from_value(v).map_err(|e| Error::Protocol(format!("edit result: {e}")))
    }

    /// Log a failure instead of returning it (open/update/close from the mailbox).
    pub fn log(self, what: &'static str) {
        tokio::spawn(async move {
            if let Err(e) = self.await {
                tracing::warn!(error = %e, "documents helper {what} failed");
            }
        });
    }
}

impl Future for Pending {
    type Output = Result<Value, Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.rx).poll(cx).map(|r| match r {
            Ok(r) => r,
            Err(_) => Err(Error::Crashed("helper worker stopped".into())),
        })
    }
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

struct Proc {
    child: Child,
    stdin: ChildStdin,
    /// Lines from the reader task; `None` = EOF or a read error.
    lines: mpsc::UnboundedReceiver<Option<Vec<u8>>>,
}

impl Proc {
    async fn kill(mut self) {
        let _ = self.child.kill().await;
    }
}

/// Owns the child: writes requests, matches replies to pending ids, forwards events.
async fn worker(
    path: PathBuf,
    mut rx: mpsc::UnboundedReceiver<Job>,
    events: mpsc::UnboundedSender<HelperEvent>,
    shutdown: CancellationToken,
) {
    let mut proc: Option<Proc> = None;
    let mut pending: PendingMap = HashMap::new();
    let mut backoff = BACKOFF_MIN;
    let mut next_spawn = Instant::now();
    let mut generation: u64 = 0;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        // A child that died: fail what it owed, tell the actor its mirrors are gone.
        let dead = proc.as_ref().is_some_and(|p| p.lines.is_closed());
        if dead {
            if let Some(p) = proc.take() {
                p.kill().await;
            }
            fail_all(&mut pending, Error::Crashed("exited".into()));
            let _ = events.send(HelperEvent::Restarted);
        }
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = tick.tick() => {
                if let Some(p) = proc.as_ref()
                    && pending.values().any(|(at, _)| at.elapsed() > DEADLINE)
                {
                    tracing::warn!("documents helper killed after {DEADLINE:?}");
                    let _ = p;
                    if let Some(p) = proc.take() { p.kill().await; }
                    fail_all(&mut pending, Error::Timeout);
                    let _ = events.send(HelperEvent::Restarted);
                }
            }
            line = async {
                match proc.as_mut() {
                    Some(p) => p.lines.recv().await.flatten(),
                    None => std::future::pending().await,
                }
            } => {
                match line {
                    Some(line) => handle_line(&line, &mut pending, &events),
                    None => {
                        // Reader ended: loop head reaps and reports.
                        if let Some(p) = proc.as_mut() { p.lines.close(); }
                    }
                }
            }
            job = rx.recv() => {
                let Some(job) = job else { break };
                if proc.is_none() {
                    let wait = next_spawn.saturating_duration_since(Instant::now());
                    if !wait.is_zero() {
                        tokio::time::sleep(wait).await;
                    }
                    match tokio::time::timeout(DEADLINE, spawn(&path)).await {
                        Ok(Ok(p)) => {
                            proc = Some(p);
                            backoff = BACKOFF_MIN;
                            generation += 1;
                            if generation > 1 {
                                let _ = events.send(HelperEvent::Restarted);
                            }
                        }
                        Ok(Err(e)) => {
                            next_spawn = Instant::now() + backoff;
                            backoff = (backoff * 2).min(BACKOFF_MAX);
                            let _ = job.reply.send(Err(e));
                            continue;
                        }
                        Err(_) => {
                            next_spawn = Instant::now() + backoff;
                            backoff = (backoff * 2).min(BACKOFF_MAX);
                            let _ = job.reply.send(Err(Error::Timeout));
                            continue;
                        }
                    }
                }
                let p = proc.as_mut().expect("spawned above");
                if let Err(e) = p.stdin.write_all(&job.line).await {
                    let _ = job.reply.send(Err(Error::Crashed(format!("write: {e}"))));
                    if let Some(p) = proc.take() { p.kill().await; }
                    fail_all(&mut pending, Error::Crashed("write failed".into()));
                    let _ = events.send(HelperEvent::Restarted);
                    continue;
                }
                pending.insert(job.id, (Instant::now(), job.reply));
            }
        }
    }
    if let Some(p) = proc {
        p.kill().await;
    }
}

fn fail_all(pending: &mut PendingMap, e: Error) {
    for (_, (_, tx)) in pending.drain() {
        let _ = tx.send(Err(e.clone()));
    }
}

fn handle_line(line: &[u8], pending: &mut PendingMap, events: &mpsc::UnboundedSender<HelperEvent>) {
    let v: Value = match serde_json::from_slice(line) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "documents helper: unreadable line dropped");
            return;
        }
    };
    if v.get("ev").and_then(Value::as_str) == Some("update") {
        let doc = v.get("doc").and_then(Value::as_str).and_then(DocId::parse);
        let update = v
            .get("update")
            .and_then(Value::as_str)
            .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok());
        let touched = v
            .get("touched")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let req = v.get("req").and_then(Value::as_u64).unwrap_or_default();
        let ratio = match (
            v.get("changed").and_then(Value::as_u64),
            v.get("total").and_then(Value::as_u64),
        ) {
            (Some(c), Some(t)) => Some((c as u32, t as u32)),
            _ => None,
        };
        match (doc, update) {
            (Some(doc), Some(update)) => {
                let _ = events.send(HelperEvent::Update {
                    doc,
                    update,
                    touched,
                    req,
                    ratio,
                });
            }
            _ => tracing::warn!("documents helper: malformed update event dropped"),
        }
        return;
    }
    let Some(id) = v.get("id").and_then(Value::as_u64) else {
        tracing::warn!("documents helper: line without id dropped: {v}");
        return;
    };
    let Some((_, tx)) = pending.remove(&id) else {
        // id 0 = the helper could not parse a line at all; nothing waits on it.
        if id != 0 {
            tracing::warn!(id, "documents helper: reply for unknown request");
        }
        return;
    };
    let r = if v.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    } else {
        Err(Error::Protocol(
            v.get("error")
                .and_then(Value::as_str)
                .unwrap_or("error without message")
                .to_owned(),
        ))
    };
    let _ = tx.send(r);
}

/// `--version` must print our protocol version; then start the line server.
async fn spawn(path: &std::path::Path) -> Result<Proc, Error> {
    if !path.is_file() {
        return Err(Error::Missing(path.to_path_buf()));
    }
    let out = Command::new(path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| Error::Crashed(format!("{}: {e}", path.display())))?;
    let found = String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .last()
        .unwrap_or_default()
        .to_owned();
    if !out.status.success() || found != protocol_version() {
        return Err(Error::Version {
            expected: protocol_version().to_owned(),
            found: if found.is_empty() {
                format!("exit {}", out.status)
            } else {
                found
            },
        });
    }
    let mut child = Command::new(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
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
    let (tx, lines) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut stdout = BufReader::new(stdout);
        loop {
            let mut buf = Vec::new();
            let n = (&mut stdout)
                .take(MAX_LINE as u64 + 1)
                .read_until(b'\n', &mut buf)
                .await;
            match n {
                Ok(0) | Err(_) => break,
                Ok(_) if buf.len() > MAX_LINE => {
                    tracing::warn!("documents helper: line above {MAX_LINE} bytes; child replaced");
                    break;
                }
                Ok(_) => {
                    if tx.send(Some(buf)).is_err() {
                        break;
                    }
                }
            }
        }
        let _ = tx.send(None);
    });
    Ok(Proc {
        child,
        stdin,
        lines,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `sh` helper: `--version` prints our version; then `body` runs with the request stream on stdin.
    fn script(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(
            &p,
            format!(
                "#!/bin/sh\nif [ \"$1\" = --version ]; then echo \"theuth-documents {}\"; exit 0; fi\n{body}\n",
                protocol_version()
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

    /// Echo every request as `{"id":<id>,"ok":true,"result":{"markdown":"x"}}`, preceded by one update event.
    const ECHO: &str = r#"while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  printf '{"ev":"update","doc":"aaaaaaaaaaaaaaaaaaaaaaaaaa","update":"AQID","touched":["b1"],"req":%s,"changed":1,"total":2}\n' "$id"
  printf '{"id":%s,"ok":true,"result":{"markdown":"x","changed":1,"total":2}}\n' "$id"
done"#;

    fn helper(path: PathBuf) -> (Helper, mpsc::UnboundedReceiver<HelperEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Helper::new(path, tx, CancellationToken::new()), rx)
    }

    #[test]
    fn protocol_version_is_read_from_protocol_ts() {
        let v = protocol_version();
        assert!(
            !v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.'),
            "{v}"
        );
    }

    #[tokio::test]
    async fn requests_pipeline_and_events_arrive_before_the_reply() {
        let dir = tempfile::tempdir().unwrap();
        let (h, mut ev) = helper(script(dir.path(), "ok.sh", ECHO));
        let doc = DocId::parse("aaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        let a = h.markdown(&doc);
        let b = h.import(&doc, "md", ImportMode::Diff, true);
        let import_id = b.id();
        let (a, b) = tokio::join!(a, b.import_result());
        assert_eq!(a.unwrap(), "x");
        let r = b.unwrap();
        assert_eq!((r.changed, r.total), (1, 2));
        // The echo emits one event per request; the import's names the import request.
        let mut seen = Vec::new();
        for _ in 0..2 {
            match ev.recv().await.unwrap() {
                HelperEvent::Update {
                    update,
                    touched,
                    req,
                    ratio,
                    ..
                } => {
                    assert_eq!(update, vec![1, 2, 3]);
                    assert_eq!(touched, vec!["b1"]);
                    assert_eq!(ratio, Some((1, 2)));
                    seen.push(req);
                }
                other => panic!("{other:?}"),
            }
        }
        assert!(seen.contains(&import_id), "{seen:?} lacks {import_id}");
    }

    #[tokio::test]
    async fn wrong_version_and_missing_binary() {
        let dir = tempfile::tempdir().unwrap();
        let other = dir.path().join("other.sh");
        std::fs::write(&other, "#!/bin/sh\necho 'theuth-documents 0.0'\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let (h, _ev) = helper(other);
        let doc = DocId::new();
        assert_eq!(
            h.markdown(&doc).await.unwrap_err(),
            Error::Version {
                expected: protocol_version().into(),
                found: "0.0".into()
            }
        );
        let (h, _ev) = helper(dir.path().join("nope"));
        assert!(matches!(
            h.markdown(&doc).await.unwrap_err(),
            Error::Missing(_)
        ));
    }

    #[tokio::test]
    async fn error_replies_and_a_crash_fail_the_request_then_restart() {
        let dir = tempfile::tempdir().unwrap();
        let (h, mut ev) = helper(script(
            dir.path(),
            "flaky.sh",
            r#"IFS= read -r line
id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
printf '{"id":%s,"ok":false,"error":"nope"}\n' "$id"
IFS= read -r line
exit 3"#,
        ));
        let doc = DocId::new();
        assert_eq!(
            h.markdown(&doc).await.unwrap_err(),
            Error::Protocol("nope".into())
        );
        assert!(matches!(
            h.markdown(&doc).await.unwrap_err(),
            Error::Crashed(_)
        ));
        // The child is respawned for the next request (after the backoff) and the actor is told.
        assert!(matches!(ev.recv().await.unwrap(), HelperEvent::Restarted));
        assert_eq!(
            h.markdown(&doc).await.unwrap_err(),
            Error::Protocol("nope".into())
        );
    }

    #[tokio::test]
    async fn shutdown_kills_the_child() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let token = CancellationToken::new();
        let h = Helper::new(script(dir.path(), "ok.sh", ECHO), tx, token.clone());
        let doc = DocId::new();
        assert!(h.markdown(&doc).await.is_ok());
        token.cancel();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(matches!(
            h.markdown(&doc).await.unwrap_err(),
            Error::Crashed(_)
        ));
    }
}
