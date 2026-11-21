//! Client of the documents helper (design 59 §3.2): the TypeScript in `packages/documents` (BlockNote +
//! yjs + linkedom), run in-process by [`document_js::DocumentsThread`] — one `documents-js` thread owning a
//! deno_core runtime, started on the first request (docs/design/documents-inproc-as-built-P2.md). The wire is
//! unchanged: one JSON request line per call, binary fields base64, `{"ev":"update"}` lines pushed before
//! the reply of the `import`/`edit` that caused them; those reach the actor as [`HelperEvent`]s, in request
//! order, before the request's [`Pending`] resolves.
//!
//! A request past [`DEADLINE`], a runtime that reaches its heap limit or a panic on the runtime thread fails
//! that request and every one queued behind it ([`Error::Timeout`] / [`Error::Crashed`]); the runtime is
//! rebuilt at once and the actor learns of it through [`HelperEvent::Restarted`] (its mirrors are gone and
//! must be reopened). [`helper_status`] reports the runtime for Settings.

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use base64::Engine as _;
use document_js::{DocumentsThread, Limits, Ticket};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

use crate::meta::DocId;

/// Per request: from enqueueing the line to its reply (the runtime's watchdog deadline).
pub const DEADLINE: Duration = Duration::from_secs(30);
/// Largest request line; longer ones never reach the runtime.
pub const MAX_LINE: usize = 16 * 1024 * 1024;

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

/// Initialise the V8 platform on the calling thread (idempotent: deno_core's `init_v8` sits behind a
/// process-wide `Once`). `t` calls it on the main thread right after `browser::warmup`, so the documents
/// thread never initialises V8 itself, browser enabled or not.
pub fn init_js_platform() {
    document_js::init_platform();
}

/// Where the stdio helper executable lived: `THEUTH_DOCUMENTS_HELPER` (a file path),
/// else `theuth-documents-<version>` beside the running binary; debug builds
/// fall back to `packages/documents/dist/theuth-documents` in the source tree.
/// No longer spawned (the helper runs in-process); kept until the packaging is removed.
pub fn helper_path() -> PathBuf {
    if let Some(p) = std::env::var_os("THEUTH_DOCUMENTS_HELPER").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    let beside = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_default()
        .join(format!(
            "theuth-documents-{}{}",
            protocol_version(),
            std::env::consts::EXE_SUFFIX
        ));
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

/// Whether the helper is there and whether it runs (surfaced by `documents_status`, shown in Settings;
/// a missing or crashed helper disables notes, knowledge editing and meeting notes).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct HelperStatus {
    /// Where the helper lives (`in-process` since it runs inside `t`).
    pub path: String,
    /// The helper is available (always, since it is compiled in).
    pub found: bool,
    /// The protocol version this `t` speaks (`theuth-documents-<version>`).
    pub version: String,
    /// The runtime right now.
    pub process: HelperProcess,
}

/// The helper runtime as the client last saw it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum HelperProcess {
    /// Has not served a request yet.
    Starting,
    /// Serving.
    Running,
    /// Failed fatally (deadline, heap limit, panic); `stderr` is the error; a fresh runtime is being built.
    Crashed {
        /// The fatal error.
        stderr: String,
    },
    /// Never produced since the helper runs in-process (kept for the wire shape).
    Missing,
}

// ponytail: process-global — one documents helper per harness; tests building several just overwrite it.
static PROCESS: std::sync::Mutex<HelperProcess> = std::sync::Mutex::new(HelperProcess::Starting);

fn set_process(p: HelperProcess) {
    *PROCESS.lock().unwrap_or_else(|e| e.into_inner()) = p;
}

/// The current [`HelperStatus`].
pub fn helper_status() -> HelperStatus {
    HelperStatus {
        path: "in-process".to_owned(),
        found: true,
        version: protocol_version().to_owned(),
        process: PROCESS.lock().unwrap_or_else(|e| e.into_inner()).clone(),
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
            Error::Protocol(s) => {
                serde_json::from_str(s).unwrap_or_else(|_| Self::Other(s.clone()))
            }
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
    /// A fresh runtime is serving: every mirror the previous one held is gone.
    Restarted,
}

/// Why a request produced no result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// [`DEADLINE`] passed; the runtime was terminated and is rebuilt.
    Timeout,
    /// The runtime failed fatally (heap limit, panic) or is gone; it is rebuilt.
    Crashed(String),
    /// The helper answered `ok: false`, the line was unreadable, or the request was too large.
    Protocol(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => write!(f, "documents helper did not answer within {DEADLINE:?}"),
            Self::Crashed(e) => write!(f, "documents helper failed: {e}"),
            Self::Protocol(e) => write!(f, "documents helper: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<document_js::Error> for Error {
    fn from(e: document_js::Error) -> Self {
        match e {
            document_js::Error::Timeout => Self::Timeout,
            document_js::Error::OutOfMemory | document_js::Error::Crashed(_) => {
                Self::Crashed(e.to_string())
            }
            document_js::Error::TooLarge { len, max } => Self::Protocol(format!(
                "request is {len} bytes; the helper accepts {max}"
            )),
            document_js::Error::Js(m) => Self::Protocol(m),
            document_js::Error::Stopped => Self::Crashed("helper worker stopped".into()),
        }
    }
}

type Answer = oneshot::Sender<Result<Value, Error>>;

/// Where request lines go: the in-process runtime, or (unit tests) a canned responder.
#[derive(Clone)]
enum Backend {
    Js(DocumentsThread),
    #[cfg(test)]
    Fake(Arc<dyn Fn(&str) -> Vec<String> + Send + Sync>),
}

/// Handle to the helper; cheap to clone. The runtime starts on the first request.
#[derive(Clone)]
pub struct Helper {
    backend: Backend,
    next: Arc<AtomicU64>,
    /// Tickets in enqueue order → the forwarder, which emits each one's events then answers it.
    forward: mpsc::UnboundedSender<(Ticket, Answer)>,
    /// Held across "enqueue on the runtime" + "enqueue on the forwarder": one order for both.
    order: Arc<Mutex<()>>,
}

impl Helper {
    /// `events` receives pushed updates and restarts; `shutdown` stops the runtime thread. `path` is the
    /// former executable path and is ignored (removed with [`helper_path`] in phase 3).
    pub fn new(
        path: PathBuf,
        events: mpsc::UnboundedSender<HelperEvent>,
        shutdown: CancellationToken,
    ) -> Self {
        let _ = path;
        let docs = DocumentsThread::new(
            Limits {
                deadline: DEADLINE,
                max_line: MAX_LINE,
                ..Limits::default()
            },
            shutdown.clone(),
        );
        tokio::spawn(restarts(docs.subscribe(), events.clone(), shutdown));
        Self::with_backend(Backend::Js(docs), events)
    }

    /// A helper whose replies come from `respond` (request line → output lines, events first).
    #[cfg(test)]
    pub(crate) fn fake(
        events: mpsc::UnboundedSender<HelperEvent>,
        respond: impl Fn(&str) -> Vec<String> + Send + Sync + 'static,
    ) -> Self {
        Self::with_backend(Backend::Fake(Arc::new(respond)), events)
    }

    fn with_backend(backend: Backend, events: mpsc::UnboundedSender<HelperEvent>) -> Self {
        let (forward, rx) = mpsc::unbounded_channel();
        tokio::spawn(forwarder(rx, events));
        Self {
            backend,
            next: Arc::new(AtomicU64::new(1)),
            forward,
            order: Arc::new(Mutex::new(())),
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
        let line = serde_json::to_string(&body).unwrap_or_default();
        let (reply, rx) = oneshot::channel();
        let _order = self.order.lock().unwrap_or_else(|e| e.into_inner());
        let ticket = match &self.backend {
            Backend::Js(docs) => docs.enqueue(line),
            #[cfg(test)]
            Backend::Fake(respond) => Ticket::ready(fake_reply(respond(&line))),
        };
        // Forwarder gone = every handle dropped; the receiver then sees a closed channel.
        let _ = self.forward.send((ticket, reply));
        Pending { id, rx }
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
        self.markdown_of(doc, None, false, &Default::default())
            .await
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

/// Settle tickets in enqueue order (= the runtime's completion order): each request's update events reach
/// `events` before its reply resolves, and events never interleave across requests.
async fn forwarder(
    mut rx: mpsc::UnboundedReceiver<(Ticket, Answer)>,
    events: mpsc::UnboundedSender<HelperEvent>,
) {
    while let Some((ticket, answer)) = rx.recv().await {
        let result = match ticket.await {
            Ok(reply) => {
                set_process(HelperProcess::Running);
                for line in &reply.events {
                    handle_event(line, &events);
                }
                parse_reply(&reply.reply)
            }
            Err(e) => {
                if e.is_fatal() {
                    set_process(HelperProcess::Crashed {
                        stderr: e.to_string(),
                    });
                    tracing::warn!(error = %e, "documents runtime failed; it is rebuilt");
                }
                Err(Error::from(e))
            }
        };
        let _ = answer.send(result);
    }
}

/// Every rebuilt runtime (generation 2, 3, …) is a fresh helper: report it running and tell the actor.
async fn restarts(
    mut generations: watch::Receiver<u64>,
    events: mpsc::UnboundedSender<HelperEvent>,
    shutdown: CancellationToken,
) {
    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            changed = generations.changed() => {
                if changed.is_err() {
                    break;
                }
                if *generations.borrow_and_update() > 1 {
                    tracing::info!("documents runtime running again");
                    set_process(HelperProcess::Running);
                    let _ = events.send(HelperEvent::Restarted);
                }
            }
        }
    }
    set_process(HelperProcess::Starting);
}

/// One pushed `{"ev":"update"}` line → [`HelperEvent::Update`].
fn handle_event(line: &str, events: &mpsc::UnboundedSender<HelperEvent>) {
    let v: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "documents helper: unreadable line dropped");
            return;
        }
    };
    if v.get("ev").and_then(Value::as_str) != Some("update") {
        tracing::warn!("documents helper: unexpected event dropped: {v}");
        return;
    }
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
}

/// The reply line → its `result`, or the helper's `error` as [`Error::Protocol`].
fn parse_reply(line: &str) -> Result<Value, Error> {
    let v: Value = serde_json::from_str(line)
        .map_err(|e| Error::Protocol(format!("unreadable reply: {e}")))?;
    if v.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    } else {
        Err(Error::Protocol(
            v.get("error")
                .and_then(Value::as_str)
                .unwrap_or("error without message")
                .to_owned(),
        ))
    }
}

#[cfg(test)]
fn fake_reply(mut lines: Vec<String>) -> Result<document_js::Reply, document_js::Error> {
    let reply = lines
        .pop()
        .ok_or_else(|| document_js::Error::Js("the fake produced no output".into()))?;
    Ok(document_js::Reply {
        events: lines,
        reply,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Y.encodeStateAsUpdate(new Y.Doc())`.
    const EMPTY: [u8; 2] = [0, 0];

    fn helper() -> (Helper, mpsc::UnboundedReceiver<HelperEvent>, CancellationToken) {
        let (tx, rx) = mpsc::unbounded_channel();
        let token = CancellationToken::new();
        (
            Helper::new(PathBuf::new(), tx, token.clone()),
            rx,
            token,
        )
    }

    #[test]
    fn protocol_version_is_read_from_protocol_ts() {
        let v = protocol_version();
        assert!(
            !v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.'),
            "{v}"
        );
    }

    /// The bundle compiled into `t` speaks the protocol `protocol.ts` declares (the old `--version` check).
    #[test]
    fn the_embedded_bundle_speaks_our_protocol_version() {
        let mut js = document_js::DocumentsJs::new(Limits::default()).unwrap();
        assert_eq!(
            js.version().unwrap(),
            format!("theuth-documents {}", protocol_version())
        );
    }

    #[tokio::test]
    async fn requests_pipeline_and_events_arrive_before_the_reply() {
        let (h, mut ev, _t) = helper();
        let doc = DocId::new();
        h.open(&doc, &EMPTY, "document-store").await.unwrap();
        let a = h.import(&doc, "# One\n\npara", ImportMode::Replace, false);
        let import_id = a.id();
        let b = h.markdown(&doc);
        // Both are queued before either is awaited; the import's update is delivered before its reply.
        let (a, b) = tokio::join!(a.import_result(), b);
        let r = a.unwrap();
        assert_eq!((r.changed, r.total), (2, 2));
        assert_eq!(b.unwrap(), "# One\n\npara\n");
        match ev.try_recv().expect("update before the reply") {
            HelperEvent::Update {
                doc: d,
                update,
                touched,
                req,
                ratio,
            } => {
                assert_eq!(d, doc);
                assert!(!update.is_empty());
                assert_eq!(touched.len(), 2);
                assert_eq!(req, import_id);
                assert_eq!(ratio, Some((2, 2)));
            }
            other => panic!("{other:?}"),
        }
        let outline = h.outline(&doc, None).await.unwrap();
        assert_eq!(outline.blocks.len(), 2);
        let e = h
            .edit(
                &doc,
                &[Edit::Delete {
                    blocks: vec!["nope".into()],
                }],
            )
            .edit_result()
            .await
            .unwrap_err();
        assert!(
            matches!(EditError::from_helper(&e), EditError::UnknownBlock { .. }),
            "{e}"
        );
    }

    #[tokio::test]
    async fn a_timeout_fails_the_request_then_restarts_and_documents_reopen() {
        let (tx, mut ev) = mpsc::unbounded_channel();
        let token = CancellationToken::new();
        // A runtime of our own with a short deadline, behind the same forwarder/restart plumbing as `new`.
        let docs = DocumentsThread::new(
            Limits {
                deadline: Duration::from_millis(1500),
                ..Limits::default()
            },
            token.clone(),
        );
        tokio::spawn(restarts(docs.subscribe(), tx.clone(), token.clone()));
        let h = Helper::with_backend(Backend::Js(docs.clone()), tx);
        let doc = DocId::new();
        h.open(&doc, &EMPTY, "document-store").await.unwrap();
        h.import(&doc, "before", ImportMode::Replace, false)
            .import_result()
            .await
            .unwrap();
        while ev.try_recv().is_ok() {}
        let hang = docs.eval_raw("for (;;) {}".into());
        let queued = h.request("markdown", &doc, json!({}));
        assert_eq!(hang.await, Err(document_js::Error::Timeout));
        assert_eq!(queued.await, Err(Error::Timeout));
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(10), ev.recv())
                .await
                .unwrap(),
            Some(HelperEvent::Restarted)
        ));
        // The mirror died with the runtime; reopened from state, it is readable again.
        assert!(matches!(h.markdown(&doc).await, Err(Error::Protocol(m)) if m.contains("not open")));
        h.open(&doc, &EMPTY, "document-store").await.unwrap();
        h.import(&doc, "after", ImportMode::Replace, false)
            .import_result()
            .await
            .unwrap();
        assert_eq!(h.markdown(&doc).await.unwrap(), "after\n");
    }

    #[tokio::test]
    async fn a_panic_on_the_runtime_thread_is_a_crash_then_a_restart() {
        let (tx, mut ev) = mpsc::unbounded_channel();
        let token = CancellationToken::new();
        let docs = DocumentsThread::new(Limits::default(), token.clone());
        tokio::spawn(restarts(docs.subscribe(), tx.clone(), token.clone()));
        let h = Helper::with_backend(Backend::Js(docs.clone()), tx);
        let doc = DocId::new();
        h.open(&doc, &EMPTY, "document-store").await.unwrap();
        let panicked = docs.panic_on_thread().await;
        assert!(matches!(panicked, Err(document_js::Error::Crashed(_))));
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(10), ev.recv())
                .await
                .unwrap(),
            Some(HelperEvent::Restarted)
        ));
        h.open(&doc, &EMPTY, "document-store").await.unwrap();
        assert!(h.markdown(&doc).await.is_ok());
    }

    #[tokio::test]
    async fn an_oversized_request_is_refused_before_the_runtime() {
        let (h, _ev, _t) = helper();
        let doc = DocId::new();
        let big = "x".repeat(MAX_LINE);
        match h.import(&doc, &big, ImportMode::Replace, false).await {
            Err(Error::Protocol(m)) => assert!(m.contains("the helper accepts"), "{m}"),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn shutdown_stops_the_runtime() {
        let (h, _ev, token) = helper();
        let doc = DocId::new();
        h.open(&doc, &EMPTY, "document-store").await.unwrap();
        token.cancel();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(matches!(
            h.markdown(&doc).await.unwrap_err(),
            Error::Crashed(_)
        ));
    }
}
