//! Client of the documents helper (design 59 §3.2): the TypeScript in `packages/documents` (BlockNote +
//! yjs + linkedom), run in-process by [`js_runtime::JsThread`] serving [`js_runtime::DOCUMENTS`] — one `documents-js` thread owning a
//! deno_core runtime, started on the first request (docs/design/documents-inproc-as-built-P2.md). The wire is
//! unchanged: one JSON request line per call, binary fields base64, `{"ev":"update"}` lines pushed before
//! the reply of the `import`/`edit` that caused them; those reach the actor as [`HelperEvent`]s, in request
//! order, before the request's [`Pending`] resolves.
//!
//! A request past [`DEADLINE`], a runtime that reaches its heap limit, a panic on the runtime thread, an
//! event-loop error or a request that ends without its reply fails that request and every one queued
//! behind it ([`Error::Timeout`] / [`Error::Crashed`]); so does a reply whose `id` is not the pending
//! request's (the runtime is given up through [`DocumentsThread::restart`]). The runtime is rebuilt and the
//! actor learns of it through [`HelperEvent::Restarted`], sent by the one task that also delivers every
//! request's update events, right after the failed request's answer: every update of an earlier request
//! reaches the actor before it reopens its mirrors. [`helper_status`] reports the runtime for Settings.
//!
//! Not closed: a request enqueued after the failure but before the actor has handled `Restarted` runs on
//! the fresh runtime before the reopen and fails with "document X is not open" ([`Error::Protocol`]).
//! Requests enqueued before the failure fail with it instead (js-runtime epochs, `AfterFatal::FailQueued`).

use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use base64::Engine as _;
use js_runtime::{DOCUMENTS, JsThread as DocumentsThread, Ticket};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::meta::DocId;

/// Per request: from enqueueing the line to its reply (the runtime's watchdog deadline; [`DOCUMENTS`]' limits).
pub const DEADLINE: Duration = DOCUMENTS.limits.deadline;
/// Largest request line; longer ones never reach the runtime.
pub const MAX_LINE: usize = DOCUMENTS.limits.max_line;

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

// The doc comments of `HelperStatus`/`HelperProcess` are part of the OpenRPC wire snapshot
// (crates/theuth/tests/fixtures/wire/openrpc-v1.json): kept verbatim from the child-process era until
// the wire is revised. In-process meaning: `path` = `"in-process"`, `found` = always true
// (compiled in), `Starting` = no request served yet, `Crashed` = last fatal runtime error until the
// rebuilt runtime serves, `Missing` = never produced.
/// Whether the helper `t` would spawn is there and whether it runs (surfaced by `documents_status`,
/// shown in Settings; a missing or crashed helper disables notes, knowledge editing and meeting notes).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct HelperStatus {
    #[allow(rustdoc::broken_intra_doc_links)] // the text is in the wire snapshot; `helper_path` is gone
    /// Where [`helper_path`] points.
    pub path: String,
    /// The file exists there.
    pub found: bool,
    /// The protocol version this `t` wants (`theuth-documents-<version>`).
    pub version: String,
    /// The child process right now.
    pub process: HelperProcess,
}

/// The helper process as the worker last saw it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum HelperProcess {
    /// Not spawned yet (documents off, or the first spawn is in flight).
    Starting,
    /// Serving.
    Running,
    /// Exited, failed its handshake or was killed; `stderr` is what it said (or the error), respawn pending.
    Crashed {
        /// Its stderr, or the spawn error.
        stderr: String,
    },
    /// No executable at `path`.
    Missing,
}

// ponytail: process-global — one documents helper per harness; tests building several just overwrite it.
static PROCESS: std::sync::Mutex<HelperProcess> = std::sync::Mutex::new(HelperProcess::Starting);

fn set_process(p: HelperProcess) {
    *PROCESS.lock().unwrap_or_else(|e| e.into_inner()) = p;
}

/// The current [`HelperStatus`]. `found` is always true: the helper is compiled into `t`.
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

impl From<js_runtime::Error> for Error {
    fn from(e: js_runtime::Error) -> Self {
        match e {
            js_runtime::Error::Timeout => Self::Timeout,
            js_runtime::Error::OutOfMemory => Self::Crashed(
                "documents runtime reached its heap limit; it was terminated and is rebuilt".into(),
            ),
            js_runtime::Error::Crashed(m) => Self::Crashed(format!("documents runtime failed: {m}")),
            js_runtime::Error::TooLarge { len, max } => {
                Self::Protocol(format!("request is {len} bytes; the helper accepts {max}"))
            }
            js_runtime::Error::Js(m) => Self::Protocol(m),
            js_runtime::Error::Stopped => Self::Crashed("helper worker stopped".into()),
        }
    }
}

type Answer = oneshot::Sender<Result<Value, Error>>;

/// Where request lines go: the in-process runtime, or (tests) a canned responder.
#[derive(Clone)]
enum Backend {
    Js(DocumentsThread),
    Fake(Respond),
}

/// A canned responder: request line → output lines (events first, reply last).
type Respond = Arc<dyn Fn(&str) -> Vec<String> + Send + Sync>;

/// Handle to the helper; cheap to clone. The runtime starts on the first request.
#[derive(Clone)]
pub struct Helper {
    backend: Backend,
    next: Arc<AtomicU64>,
    /// Tickets in enqueue order, with their request ids → the forwarder, which emits each one's events then
    /// answers it.
    forward: mpsc::UnboundedSender<(Ticket, Answer, u64)>,
    /// Held across "enqueue on the runtime" + "enqueue on the forwarder": one order for both.
    order: Arc<Mutex<()>>,
}

impl Helper {
    /// `events` receives pushed updates and restarts; `shutdown` stops the runtime thread.
    pub fn new(events: mpsc::UnboundedSender<HelperEvent>, shutdown: CancellationToken) -> Self {
        let docs = DocumentsThread::new(&DOCUMENTS, DOCUMENTS.limits, shutdown);
        Self::with_backend(Backend::Js(docs), events)
    }

    /// A helper whose replies come from `respond` (request line → output lines, events first): for
    /// tests that need a fixed outline or transaction instead of real conversions.
    #[doc(hidden)]
    pub fn fake(
        events: mpsc::UnboundedSender<HelperEvent>,
        respond: impl Fn(&str) -> Vec<String> + Send + Sync + 'static,
    ) -> Self {
        Self::with_backend(Backend::Fake(Arc::new(respond)), events)
    }

    fn with_backend(backend: Backend, events: mpsc::UnboundedSender<HelperEvent>) -> Self {
        let (forward, rx) = mpsc::unbounded_channel();
        let docs = match &backend {
            Backend::Js(docs) => Some(docs.clone()),
            Backend::Fake(_) => None,
        };
        tokio::spawn(forwarder(rx, events, docs));
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
            Backend::Fake(respond) => Ticket::ready(fake_reply(respond(&line))),
        };
        // Forwarder gone = every handle dropped; the receiver then sees a closed channel.
        let _ = self.forward.send((ticket, reply, id));
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
/// `events` before its reply resolves, and events never interleave across requests. The only writer of
/// [`HelperEvent::Restarted`]: sent right after answering the request whose failure gave up the runtime
/// (`Settled::rebuilt`), or whose reply named another request (then this task gives the runtime up), so it
/// follows every update of the requests before it and is sent once per lost runtime.
async fn forwarder(
    mut rx: mpsc::UnboundedReceiver<(Ticket, Answer, u64)>,
    events: mpsc::UnboundedSender<HelperEvent>,
    docs: Option<DocumentsThread>,
) {
    // Tickets below this epoch belong to a runtime given up here; their output is not trusted.
    let mut floor = 0;
    while let Some((ticket, answer, id)) = rx.recv().await {
        let epoch = ticket.epoch();
        let settled = ticket.settled().await;
        let mut restarted = settled.rebuilt;
        let result = match settled.result {
            Ok(_) if epoch < floor => Err(Error::Crashed(
                "the documents runtime was given up before this request was answered".into(),
            )),
            Ok(reply) => match parse_reply(&reply.reply, id) {
                Ok(result) => {
                    set_process(HelperProcess::Running);
                    for line in &reply.events {
                        handle_event(line, &events);
                    }
                    result
                }
                Err(other) => {
                    // Positional pipeline out of step: this runtime's output can no longer be matched to
                    // requests. Drop it (and the events that came with the reply) and rebuild.
                    let e = format!(
                        "documents runtime answered request {id} with the reply to request {other}; it is rebuilt"
                    );
                    tracing::error!("{e}");
                    if let Some(docs) = &docs {
                        floor = docs.restart();
                        restarted = true;
                    }
                    set_process(HelperProcess::Crashed { stderr: e.clone() });
                    Err(Error::Crashed(e))
                }
            },
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
        if restarted {
            let _ = events.send(HelperEvent::Restarted);
        }
    }
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

/// The reply line to request `id` → its `result`, or the helper's `error` as [`Error::Protocol`]. The outer
/// `Err` carries the id the reply names when it is not `id` (an unreadable line has none and is a
/// protocol error).
fn parse_reply(line: &str, id: u64) -> Result<Result<Value, Error>, Value> {
    let v: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return Ok(Err(Error::Protocol(format!("unreadable reply: {e}")))),
    };
    if v.get("id").and_then(Value::as_u64) != Some(id) {
        return Err(v.get("id").cloned().unwrap_or(Value::Null));
    }
    Ok(if v.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    } else {
        Err(Error::Protocol(
            v.get("error")
                .and_then(Value::as_str)
                .unwrap_or("error without message")
                .to_owned(),
        ))
    })
}

fn fake_reply(mut lines: Vec<String>) -> Result<js_runtime::Reply, js_runtime::Error> {
    let reply = lines
        .pop()
        .ok_or_else(|| js_runtime::Error::Js("the fake produced no output".into()))?;
    Ok(js_runtime::Reply {
        events: lines,
        reply,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Y.encodeStateAsUpdate(new Y.Doc())`.
    const EMPTY: [u8; 2] = [0, 0];

    fn helper() -> (
        Helper,
        mpsc::UnboundedReceiver<HelperEvent>,
        CancellationToken,
    ) {
        let (tx, rx) = mpsc::unbounded_channel();
        let token = CancellationToken::new();
        (Helper::new(tx, token.clone()), rx, token)
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
        let mut js = js_runtime::JsRuntime::new(&DOCUMENTS, DOCUMENTS.limits).unwrap();
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

    /// A runtime of our own with a short deadline, behind the same forwarder as `new`.
    fn short() -> (
        Helper,
        DocumentsThread,
        mpsc::UnboundedReceiver<HelperEvent>,
    ) {
        let (tx, ev) = mpsc::unbounded_channel();
        let docs = DocumentsThread::new(
            &DOCUMENTS,
            js_runtime::Limits {
                deadline: Duration::from_millis(1500),
                first_deadline: Duration::from_millis(1500),
                ..DOCUMENTS.limits
            },
            CancellationToken::new(),
        );
        let h = Helper::with_backend(Backend::Js(docs.clone()), tx);
        (h, docs, ev)
    }

    /// Route a test-hook ticket (`eval_raw`, `panic_on_thread`) through the forwarder like a request.
    fn through(h: &Helper, ticket: Ticket) -> Pending {
        let (reply, rx) = oneshot::channel();
        let id = h.next.fetch_add(1, Ordering::Relaxed);
        let _ = h.forward.send((ticket, reply, id));
        Pending { id, rx }
    }

    async fn next_event(ev: &mut mpsc::UnboundedReceiver<HelperEvent>) -> HelperEvent {
        tokio::time::timeout(Duration::from_secs(60), ev.recv())
            .await
            .expect("an event")
            .expect("events open")
    }

    /// Nothing more arrives (in particular no second `Restarted`).
    async fn quiet(ev: &mut mpsc::UnboundedReceiver<HelperEvent>) {
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(ev.try_recv().is_err());
    }

    #[tokio::test]
    async fn a_timeout_fails_the_request_then_restarts_and_documents_reopen() {
        let (h, docs, mut ev) = short();
        let doc = DocId::new();
        h.open(&doc, &EMPTY, "document-store").await.unwrap();
        h.import(&doc, "before", ImportMode::Replace, false)
            .import_result()
            .await
            .unwrap();
        while ev.try_recv().is_ok() {}
        let hang = through(&h, docs.eval_raw("for (;;) {}".into()));
        let queued = h.request("markdown", &doc, json!({}));
        assert_eq!(hang.await, Err(Error::Timeout));
        assert_eq!(queued.await, Err(Error::Timeout));
        assert!(matches!(next_event(&mut ev).await, HelperEvent::Restarted));
        quiet(&mut ev).await;
        // The mirror died with the runtime; reopened from state, it is readable again.
        assert!(
            matches!(h.markdown(&doc).await, Err(Error::Protocol(m)) if m.contains("not open"))
        );
        h.open(&doc, &EMPTY, "document-store").await.unwrap();
        h.import(&doc, "after", ImportMode::Replace, false)
            .import_result()
            .await
            .unwrap();
        assert_eq!(h.markdown(&doc).await.unwrap(), "after\n");
    }

    #[tokio::test]
    async fn a_panic_on_the_runtime_thread_is_a_crash_then_a_restart() {
        let (h, docs, mut ev) = short();
        let doc = DocId::new();
        h.open(&doc, &EMPTY, "document-store").await.unwrap();
        let panicked = through(&h, docs.panic_on_thread()).await;
        assert!(matches!(panicked, Err(Error::Crashed(_))), "{panicked:?}");
        assert!(matches!(next_event(&mut ev).await, HelperEvent::Restarted));
        h.open(&doc, &EMPTY, "document-store").await.unwrap();
        assert!(h.markdown(&doc).await.is_ok());
    }

    /// M2: one writer, in ticket order: the updates of request A, then B's fatal failure, then exactly one
    /// `Restarted` (C, queued behind B, fails with it but reports no restart of its own).
    #[tokio::test]
    async fn restarted_follows_the_updates_of_earlier_requests() {
        let (h, docs, mut ev) = short();
        let doc = DocId::new();
        h.open(&doc, &EMPTY, "document-store").await.unwrap();
        let a = h.import(&doc, "# A\n\npara", ImportMode::Replace, false);
        let a_id = a.id();
        let b = through(&h, docs.eval_raw("for (;;) {}".into()));
        let c = h.request("markdown", &doc, json!({}));
        let (a, b, c) = tokio::join!(a.import_result(), b, c);
        assert_eq!(a.unwrap().changed, 2);
        assert_eq!(b, Err(Error::Timeout));
        assert_eq!(c, Err(Error::Timeout));
        let mut updates = 0;
        while let HelperEvent::Update { req, .. } = next_event(&mut ev).await {
            assert_eq!(req, a_id);
            updates += 1;
        }
        // The loop ended on the only other event: `Restarted`, after every update of A.
        assert!(updates > 0, "A's updates arrive, and before Restarted");
        quiet(&mut ev).await;
    }

    /// M1: a reply naming another request is never taken as this one's answer: the request fails as
    /// `Crashed`, the runtime is given up (requests queued behind fail too), `Restarted` follows once, and
    /// the next request is answered by a fresh runtime with its own reply.
    #[tokio::test]
    async fn a_reply_for_another_request_is_fatal_and_rebuilds() {
        let (h, docs, mut ev) = short();
        let doc = DocId::new();
        h.open(&doc, &EMPTY, "document-store").await.unwrap();
        assert_eq!(docs.generation(), 1);
        docs.eval_raw(
            r#"const orig = globalThis.theuthDocuments.call;
               globalThis.theuthDocuments.call = async () => {
                 globalThis.theuthDocuments.call = orig;
                 return JSON.stringify({ id: 999999, ok: true, result: { markdown: "stolen" } });
               };"#
            .into(),
        )
        .await
        .unwrap();
        let stolen = h.request("markdown", &doc, json!({}));
        let queued = h.request("outline", &doc, json!({}));
        match stolen.await {
            Err(Error::Crashed(m)) => assert!(m.contains("999999"), "{m}"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(queued.await, Err(Error::Crashed(_))));
        assert!(matches!(next_event(&mut ev).await, HelperEvent::Restarted));
        quiet(&mut ev).await;
        h.open(&doc, &EMPTY, "document-store").await.unwrap();
        h.import(&doc, "mine", ImportMode::Replace, false)
            .import_result()
            .await
            .unwrap();
        assert_eq!(h.markdown(&doc).await.unwrap(), "mine\n");
        assert_eq!(docs.generation(), 2);
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
