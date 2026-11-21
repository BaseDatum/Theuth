//! The headless parser (46 §2.1), in-process: two bundles in two deno_core
//! isolates, each on its own lazily started thread ([`app_js::BundleThread`]):
//! the OpenUI check/merge validator (`packages/apps/src/inproc.ts`,
//! [`app_js::APPS`]) and the parse-only mermaid check
//! (`packages/apps/src/inproc-mermaid.ts`, [`app_js::MERMAID`]); no
//! executable, no child process (apps-inproc-as-built-P1.md). This side owns
//! admission — each isolate has its own lane of one request being served plus
//! [`PENDING`] queued, more → [`Error::Busy`], so queued mermaid work never
//! takes a `check` slot — and the wire (`protocol.ts`, unchanged); the runtime
//! owns the deadlines (a watchdog terminates the script), the heap ceiling
//! and panic containment, and rebuilds itself after any of them: the request
//! fails, the next one runs on a fresh isolate. A reply whose id is not the
//! request's is treated the same way. Shutdown stops both threads.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::{AppBinding, AppDiagnostic};

/// Requests queued behind the one being served (46 §7 "parser admission").
pub const PENDING: usize = 8;
/// Per OpenUI request: how long the validator may run (event loop included)
/// before it is terminated and rebuilt. Building the isolate (bundle load,
/// after start or a rebuild) is not counted (`app_js::LOAD_DEADLINE`).
pub const DEADLINE: Duration = Duration::from_secs(2);
/// Per warm `check_mermaid`: V8 runs mermaid's super-linear class-diagram
/// parser ~1.6× slower than the old helper's JSC, so a 32 KB fence needs ~3 s
/// (docs/design/apps-mermaid-inproc-spike.md §4). Own thread: it never delays
/// `check`.
pub const MERMAID_DEADLINE: Duration = Duration::from_secs(5);
/// The first `check_mermaid` on each mermaid runtime (its first diagram
/// chunk evaluates; measured ≤ 35 ms, the rest is headroom for a loaded box).
pub const MERMAID_FIRST_DEADLINE: Duration = Duration::from_secs(10);
/// Largest request line accepted; the bundles refuse longer lines too
/// (`inproc.ts` `MAX_LINE_BYTES`).
pub const MAX_LINE: usize = 4 * 1024 * 1024;
/// V8 heap ceiling of each isolate (OpenUI and mermaid).
const HEAP_LIMIT: usize = 256 * 1024 * 1024;

/// One visible tool as the validator binds it (`protocol.ts` `ToolSnapshot`).
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
    /// The validator's library hash.
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
    /// [`PENDING`] requests already queued.
    Busy,
    /// [`DEADLINE`] passed; the validator was terminated and is rebuilt.
    Timeout,
    /// The validator's runtime failed (heap limit, panic, a reply for
    /// another request, stopped); it is rebuilt for the next request.
    Crashed(String),
    /// The validator answered with an error line or an unparseable response,
    /// or the request could not be sent.
    Protocol(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => write!(
                f,
                "app parser busy: {PENDING} validations pending; try again"
            ),
            Self::Timeout => write!(f, "app parser did not answer within {DEADLINE:?}"),
            Self::Crashed(e) => write!(f, "app parser failed: {e}"),
            Self::Protocol(e) => write!(f, "app parser protocol error: {e}"),
        }
    }
}

impl std::error::Error for Error {}

/// `check_mermaid` request line (`protocol.ts` `MermaidRequest`).
#[derive(Serialize)]
struct MermaidRequest<'a> {
    id: &'a str,
    op: &'static str,
    source: &'a str,
}

/// `check_mermaid` response body (`protocol.ts` `MermaidResponse`).
#[derive(Deserialize)]
struct MermaidResponse {
    ok: bool,
    #[serde(default)]
    errors: Vec<MermaidError>,
}

#[derive(Deserialize)]
struct MermaidError {
    line: Option<u32>,
    message: String,
}

/// A canned responder for other crates' tests: request line → response line.
#[doc(hidden)]
pub type Responder = Arc<dyn Fn(&str) -> String + Send + Sync>;

enum Backend {
    Js(app_js::BundleThread),
    Canned(Responder),
}

/// One isolate (or canned responder) and its admission: the request being
/// served plus [`PENDING`] queued.
struct Lane {
    backend: Backend,
    admission: Arc<Semaphore>,
}

impl Lane {
    fn new(backend: Backend) -> Self {
        Self {
            backend,
            admission: Arc::new(Semaphore::new(PENDING + 1)),
        }
    }
}

/// Runtime input (a test's closure), so `OnceLock`, not `LazyLock`.
static CANNED: std::sync::OnceLock<Responder> = std::sync::OnceLock::new();

/// Make every later [`crate::AppPlugin`] start use `respond` instead of the
/// validator (process-global; for integration tests of the plugin whose
/// verdict must be scripted). Returns false when one is already installed.
#[doc(hidden)]
pub fn install_canned(respond: Responder) -> bool {
    CANNED.set(respond).is_ok()
}

/// Handle to the parsers. Cheap to share behind an `Arc`; each thread and
/// isolate starts on its first request (no mermaid fence → no mermaid isolate).
pub struct Helper {
    openui: Lane,
    mermaid: Lane,
    next: std::sync::atomic::AtomicU64,
}

impl Helper {
    /// The in-process parsers; `shutdown` stops their threads.
    pub fn new(shutdown: CancellationToken) -> Self {
        let openui = app_js::Limits {
            deadline: DEADLINE,
            first_deadline: DEADLINE,
            heap_limit: HEAP_LIMIT,
            max_line: MAX_LINE,
        };
        let mermaid = app_js::Limits {
            deadline: MERMAID_DEADLINE,
            first_deadline: MERMAID_FIRST_DEADLINE,
            heap_limit: HEAP_LIMIT,
            max_line: MAX_LINE,
        };
        Self::with(
            Backend::Js(app_js::BundleThread::new(
                &app_js::APPS,
                openui,
                shutdown.clone(),
            )),
            Backend::Js(app_js::BundleThread::new(
                &app_js::MERMAID,
                mermaid,
                shutdown,
            )),
        )
    }

    /// A helper answering every request line (both ops) with `respond`'s
    /// line (tests of crates that need a parser verdict without the real one).
    #[doc(hidden)]
    pub fn canned(respond: Responder) -> Self {
        Self::with(Backend::Canned(respond.clone()), Backend::Canned(respond))
    }

    /// What [`crate::AppPlugin`] starts: the in-process parsers, or the
    /// responder a test installed with [`install_canned`].
    pub(crate) fn for_plugin(shutdown: CancellationToken) -> Self {
        match CANNED.get() {
            Some(respond) => Self::canned(respond.clone()),
            None => Self::new(shutdown),
        }
    }

    fn with(openui: Backend, mermaid: Backend) -> Self {
        Self {
            openui: Lane::new(openui),
            mermaid: Lane::new(mermaid),
            next: std::sync::atomic::AtomicU64::new(1),
        }
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

    /// Syntax-check one mermaid diagram (30 §5) on the mermaid isolate: `path`
    /// is `line:N` when the parser located the error, else `""`.
    pub async fn check_mermaid(&self, source: &str) -> Result<Vec<tools::FenceError>, Error> {
        let id = self.next_id();
        let line = serde_json::to_string(&MermaidRequest {
            id: &id,
            op: "check_mermaid",
            source,
        })
        .map_err(|e| Error::Protocol(format!("serialize request: {e}")))?;
        let v = send(&self.mermaid, line, &id).await?;
        let r: MermaidResponse = serde_json::from_value(v)
            .map_err(|e| Error::Protocol(format!("response body: {e}")))?;
        Ok(if r.ok {
            Vec::new()
        } else {
            r.errors
                .into_iter()
                .map(|e| tools::FenceError {
                    path: e.line.map(|l| format!("line:{l}")).unwrap_or_default(),
                    message: e.message,
                })
                .collect()
        })
    }

    fn next_id(&self) -> String {
        self.next
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .to_string()
    }

    async fn request(
        &self,
        op: &'static str,
        source: &str,
        base: Option<&str>,
        tools: &[ToolSnapshot],
    ) -> Result<CheckResult, Error> {
        let id = self.next_id();
        let line = serde_json::to_string(&Request {
            id: id.clone(),
            op,
            source,
            base,
            tools,
        })
        .map_err(|e| Error::Protocol(format!("serialize request: {e}")))?;
        let v = send(&self.openui, line, &id).await?;
        serde_json::from_value(v).map_err(|e| Error::Protocol(format!("response body: {e}")))
    }
}

/// Admit on `lane`, run, match the reply to `id`.
async fn send(lane: &Lane, line: String, id: &str) -> Result<serde_json::Value, Error> {
    if line.len() > MAX_LINE {
        return Err(Error::Protocol(format!(
            "request is {} bytes; the helper accepts {MAX_LINE}",
            line.len()
        )));
    }
    let _permit = lane
        .admission
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::Busy)?;
    let reply = match &lane.backend {
        Backend::Canned(respond) => respond(&line),
        Backend::Js(js) => match js.enqueue(line).await {
            Ok(r) => r.reply,
            Err(app_js::Error::Timeout) => return Err(Error::Timeout),
            Err(app_js::Error::TooLarge { len, max }) => {
                return Err(Error::Protocol(format!(
                    "request is {len} bytes; the helper accepts {max}"
                )));
            }
            Err(e) => return Err(Error::Crashed(e.to_string())),
        },
    };
    let v: serde_json::Value =
        serde_json::from_str(&reply).map_err(|e| Error::Protocol(format!("response: {e}")))?;
    if v.get("id").and_then(|i| i.as_str()) != Some(id) {
        // Output that answers another request cannot be trusted, nor can the
        // runtime that produced it: give it up; the next request runs fresh.
        if let Backend::Js(js) = &lane.backend {
            js.restart();
        }
        return Err(Error::Crashed(format!(
            "response id {} for request {id}; the parser runtime is rebuilt",
            v.get("id").unwrap_or(&serde_json::Value::Null)
        )));
    }
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Err(Error::Protocol(e.to_owned()));
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn helper() -> Helper {
        Helper::new(CancellationToken::new())
    }

    fn sync_tool() -> ToolSnapshot {
        ToolSnapshot {
            name: "argo_sync".into(),
            schema: serde_json::json!({ "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] }),
            risk: tools::Risk::Mutating,
            identity: "mcp:argo/argo_sync#1".into(),
        }
    }

    const SOURCE: &str = "root = Stack([b])\nsync = Mutation(\"argo_sync\", {name: \"web\"})\nb = Button(\"Sync\", Action([@Run(sync)]))";

    #[tokio::test]
    async fn check_and_merge_check_against_the_real_validator() {
        let h = helper();
        let r = h.check(SOURCE, &[sync_tool()]).await.unwrap();
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.library_hash, crate::hash());
        assert_eq!(r.bindings.len(), 1);
        assert_eq!(r.bindings[0].statement_id, "sync");
        assert_eq!(r.bindings[0].identity, "mcp:argo/argo_sync#1");
        assert_eq!(r.statements.len(), 3);
        // Without the tool: a located diagnostic, still a result.
        let r = h.check(SOURCE, &[]).await.unwrap();
        assert_eq!(r.diagnostics.len(), 1);
        assert_eq!(r.diagnostics[0].at, "sync");
        // A patch onto the base; the merged program is what is validated.
        let r = h
            .merge_check(
                "b = Button(\"Go\", Action([@Run(sync)]))",
                SOURCE,
                &[sync_tool()],
            )
            .await
            .unwrap();
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert!(r.source.contains("Button(\"Go\""), "{}", r.source);
        // Many requests, in order, one runtime.
        for _ in 0..5 {
            assert!(
                h.check("root = Stack([])", &[])
                    .await
                    .unwrap()
                    .diagnostics
                    .is_empty()
            );
        }
    }

    #[tokio::test]
    async fn oversized_requests_are_refused_before_the_runtime() {
        let h = helper();
        let big = "x".repeat(MAX_LINE);
        assert!(
            matches!(h.check(&big, &[]).await, Err(Error::Protocol(m)) if m.contains("the helper accepts"))
        );
        assert!(h.check("root = Stack([])", &[]).await.is_ok());
    }

    #[tokio::test]
    async fn a_hang_is_terminated_at_the_deadline_and_the_next_request_is_served() {
        let h = helper();
        assert!(h.check("root = Stack([])", &[]).await.is_ok());
        let Backend::Js(js) = &h.openui.backend else {
            unreachable!()
        };
        js.eval_raw("globalThis.theuthApps.call = () => { for (;;) {} };".into())
            .await
            .unwrap();
        let t = std::time::Instant::now();
        assert_eq!(
            h.check("root = Stack([])", &[]).await.unwrap_err(),
            Error::Timeout
        );
        assert!(
            t.elapsed() >= DEADLINE && t.elapsed() < DEADLINE * 5,
            "{:?}",
            t.elapsed()
        );
        assert!(
            h.check("root = Stack([])", &[]).await.is_ok(),
            "fresh runtime serves"
        );
    }

    #[tokio::test]
    async fn a_reply_for_another_request_fails_it_and_the_runtime_is_rebuilt() {
        let h = helper();
        assert!(h.check("root = Stack([])", &[]).await.is_ok());
        let Backend::Js(js) = &h.openui.backend else {
            unreachable!()
        };
        js.eval_raw(
            "const orig = globalThis.theuthApps.call;
             globalThis.theuthApps.call = (line) => { globalThis.theuthApps.call = orig; return orig(line).replace(/^\\{\"id\":\"[^\"]*\"/, '{\"id\":\"999\"'); };"
                .into(),
        )
        .await
        .unwrap();
        let generation = js.generation();
        assert!(
            matches!(h.check("root = Stack([])", &[]).await, Err(Error::Crashed(m)) if m.contains("999"))
        );
        assert!(h.check("root = Stack([])", &[]).await.is_ok());
        assert_eq!(js.generation(), generation + 1);
    }

    #[tokio::test]
    async fn error_lines_are_protocol_errors() {
        let h = Helper::canned(Arc::new(|line: &str| {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            serde_json::json!({ "id": v["id"], "error": "bad" }).to_string()
        }));
        assert_eq!(
            h.check("x", &[]).await.unwrap_err(),
            Error::Protocol("bad".into())
        );
        let h = Helper::canned(Arc::new(|_: &str| "not json".to_owned()));
        assert!(matches!(h.check("x", &[]).await, Err(Error::Protocol(_))));
    }

    #[tokio::test]
    async fn more_than_pending_queued_requests_are_busy_not_queued() {
        let h = Arc::new(helper());
        assert!(h.check("root = Stack([])", &[]).await.is_ok());
        let Backend::Js(js) = &h.openui.backend else {
            unreachable!()
        };
        // Occupy the runtime thread (no admission permit), then admit PENDING + 1.
        let busy =
            js.eval_raw("{ const t0 = Date.now(); while (Date.now() - t0 < 1000) {} }".into());
        let waiting: Vec<_> = (0..=PENDING)
            .map(|_| {
                let h = h.clone();
                tokio::spawn(async move { h.check("root = Stack([])", &[]).await })
            })
            .collect();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while h.openui.admission.available_permits() > 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "admission never filled"
            );
            tokio::task::yield_now().await;
        }
        assert_eq!(
            h.check("root = Stack([])", &[]).await.unwrap_err(),
            Error::Busy
        );
        busy.await.unwrap();
        for w in waiting {
            assert!(w.await.unwrap().is_ok(), "admitted requests are served");
        }
        assert!(h.check("root = Stack([])", &[]).await.is_ok());
    }

    #[tokio::test]
    async fn shutdown_stops_serving() {
        let token = CancellationToken::new();
        let h = Helper::new(token.clone());
        assert!(h.check("root = Stack([])", &[]).await.is_ok());
        token.cancel();
        let Backend::Js(js) = &h.openui.backend else {
            unreachable!()
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while js.is_running() {
            assert!(std::time::Instant::now() < deadline, "thread never stopped");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(matches!(h.check("x", &[]).await, Err(Error::Crashed(_))));
    }

    fn js(lane: &Lane) -> &app_js::BundleThread {
        match &lane.backend {
            Backend::Js(js) => js,
            Backend::Canned(_) => unreachable!("real helper"),
        }
    }

    const FLOW: &str = "flowchart TD\n  A --> B";
    const FLOW_BAD: &str = "flowchart TD\n  A --> B\n}}} ]]] ((( @@@ !!! <<<";

    /// A valid diagram passes, an invalid one is located, and `check` still serves.
    async fn both_serve(h: &Helper, what: &str) {
        assert_eq!(
            h.check_mermaid(FLOW).await,
            Ok(vec![]),
            "mermaid after {what}"
        );
        assert!(
            h.check("root = Stack([])", &[]).await.is_ok(),
            "check after {what}"
        );
    }

    #[tokio::test]
    async fn check_mermaid_runs_on_its_own_lazily_started_isolate() {
        let h = helper();
        assert!(h.check("root = Stack([])", &[]).await.is_ok());
        assert!(js(&h.openui).is_running());
        assert!(
            !js(&h.mermaid).is_running(),
            "no mermaid fence, no mermaid isolate"
        );
        assert_eq!(h.check_mermaid(FLOW).await, Ok(vec![]));
        assert!(js(&h.mermaid).is_running());
        let errs = h.check_mermaid(FLOW_BAD).await.unwrap();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].path, "line:3");
        assert!(errs[0].message.contains("line 3"), "{}", errs[0].message);
        assert_eq!(js(&h.openui).generation(), 1);
        assert_eq!(js(&h.mermaid).generation(), 1);
    }

    #[tokio::test]
    async fn a_busy_mermaid_isolate_never_delays_check() {
        let h = Arc::new(helper());
        assert!(h.check("root = Stack([])", &[]).await.is_ok());
        assert_eq!(h.check_mermaid(FLOW).await, Ok(vec![]));
        // Block the mermaid thread for 3 s (under its 5 s deadline) and fill its lane.
        let blocked = js(&h.mermaid)
            .eval_raw("{ const t0 = Date.now(); while (Date.now() - t0 < 3000) {} }".into());
        let queued: Vec<_> = (0..=PENDING)
            .map(|_| {
                let h = h.clone();
                tokio::spawn(async move { h.check_mermaid(FLOW).await })
            })
            .collect();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while h.mermaid.admission.available_permits() > 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "mermaid lane never filled"
            );
            tokio::task::yield_now().await;
        }
        assert_eq!(h.check_mermaid(FLOW).await, Err(Error::Busy));
        // Meanwhile `check` has its own lane and isolate: answered at once, several times.
        let started = std::time::Instant::now();
        for _ in 0..5 {
            assert!(h.check("root = Stack([])", &[]).await.is_ok());
        }
        assert!(
            started.elapsed() < Duration::from_millis(1500),
            "check waited on mermaid: {:?}",
            started.elapsed()
        );
        blocked.await.unwrap();
        for q in queued {
            assert_eq!(q.await.unwrap(), Ok(vec![]));
        }
    }

    /// Containment on the mermaid isolate; after each, a parse AND a `check`
    /// on the other isolate succeed, and the OpenUI isolate was never rebuilt.
    #[tokio::test]
    async fn mermaid_failures_are_contained_on_their_own_isolate() {
        let h = helper();
        both_serve(&h, "start").await;
        let m = js(&h.mermaid);
        // Runaway script: terminated at the 5 s mermaid deadline.
        assert_eq!(
            m.eval_raw("for (;;) {}".into()).await,
            Err(app_js::Error::Timeout)
        );
        both_serve(&h, "runaway").await;
        // Heap growth: stopped at the 256 MB ceiling.
        assert_eq!(
            m.eval_raw("const keep = []; for (;;) keep.push(new Array(100000).fill(0.5));".into())
                .await,
            Err(app_js::Error::OutOfMemory)
        );
        both_serve(&h, "heap").await;
        // Hostile diagrams from the spike: each ends as an answer or a contained error.
        let nest =
            |open: &str, close: &str, n: usize| format!("{}{}", open.repeat(n), close.repeat(n));
        let mut sequence = String::from("sequenceDiagram\n");
        for i in 0..100_000 {
            sequence.push_str(&format!("  A{} ->> B: m\n", i % 50));
        }
        let hostile = [
            (
                "init-json-nested-200k",
                format!(
                    "%%{{init: {}}}%%\nflowchart LR\n A-->B",
                    nest("[", "]", 200_000)
                ),
            ),
            (
                "frontmatter-yaml-nested-100k",
                format!(
                    "---\nconfig: {}\n---\nflowchart LR\n A-->B",
                    nest("[", "]", 100_000)
                ),
            ),
            (
                "flowchart-parens-1M",
                format!("flowchart LR\n A{}", "(".repeat(1_000_000)),
            ),
            ("sequence-100k-messages", sequence),
            ("pie-1M-quotes", format!("pie\n{}", "\"".repeat(1_000_000))),
        ];
        for (name, source) in hostile {
            let out = h.check_mermaid(&source).await;
            assert!(
                matches!(&out, Ok(_) | Err(Error::Timeout)),
                "{name}: {out:?}"
            );
            both_serve(&h, name).await;
        }
        assert_eq!(
            js(&h.openui).generation(),
            1,
            "the OpenUI isolate never noticed"
        );
        assert!(js(&h.mermaid).generation() >= 3);
    }
}
