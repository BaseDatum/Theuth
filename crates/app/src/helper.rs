//! The headless parser (46 §2.1), in-process: the OpenUI check/merge validator
//! (`packages/apps/src/inproc.ts`) runs in a deno_core isolate on its own
//! lazily started thread ([`app_js::BundleThread`] serving [`app_js::APPS`]);
//! no executable, no child process (apps-inproc-as-built-P1.md). This side
//! owns admission ([`PENDING`] queued requests behind the one being served,
//! more → [`Error::Busy`]) and the wire (`protocol.ts`, unchanged); the
//! runtime owns the [`DEADLINE`] (a watchdog terminates the script), the
//! heap ceiling and panic containment, and rebuilds itself after any of
//! them: the request fails, the next one runs on a fresh isolate. A reply
//! whose id is not the request's is treated the same way. Shutdown stops
//! the thread.
//!
//! `check_mermaid` is not served in-process yet: see [`MermaidBackend`].

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::{AppBinding, AppDiagnostic};

/// Requests queued behind the one being served (46 §7 "parser admission").
pub const PENDING: usize = 8;
/// Per request: how long the validator may run (event loop included) before
/// it is terminated and rebuilt. Building the isolate (bundle load, first
/// request after start or a rebuild) is not counted (`app_js::LOAD_DEADLINE`).
pub const DEADLINE: Duration = Duration::from_secs(2);
/// The first `check_mermaid` once a mermaid backend is plugged in (mermaid +
/// jsdom load lazily and take seconds under load, 30 §5); later ones get
/// [`DEADLINE`]. Unused while [`MERMAID`] is [`MermaidBackend::FailOpen`].
pub const MERMAID_FIRST_DEADLINE: Duration = Duration::from_secs(15);
/// Largest request line accepted; the validator refuses longer lines too
/// (`inproc.ts` `MAX_LINE_BYTES`).
pub const MAX_LINE: usize = 4 * 1024 * 1024;
/// V8 heap ceiling of the validator's isolate.
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

/// THE MERMAID SEAM (apps-inproc-as-built-P1.md § "Mermaid seam"): what
/// [`Helper::check_mermaid`] runs on. Phase 1 has no in-process mermaid
/// parser, so it answers "no errors" — exactly what `fence.rs` already does
/// when the checker fails (fail-open). The mermaid phase adds a variant
/// (e.g. `Js` over a second [`app_js::BundleThread`] with its own bundle,
/// limits and [`MERMAID_FIRST_DEADLINE`]), sets [`MERMAID`] to it and fills
/// in its arm in [`Helper::check_mermaid`]; nothing else changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MermaidBackend {
    /// No mermaid parser: every diagram passes.
    FailOpen,
}

/// The backend [`Helper::check_mermaid`] uses.
pub const MERMAID: MermaidBackend = MermaidBackend::FailOpen;

/// A canned responder for other crates' tests: request line → response line.
#[doc(hidden)]
pub type Responder = Arc<dyn Fn(&str) -> String + Send + Sync>;

enum Backend {
    Js(app_js::BundleThread),
    Canned(Responder),
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

/// Handle to the validator. Cheap to share behind an `Arc`; its thread and
/// isolate start on the first request.
pub struct Helper {
    backend: Backend,
    /// [`PENDING`] + the one being served.
    admission: Arc<Semaphore>,
    next: std::sync::atomic::AtomicU64,
}

impl Helper {
    /// The in-process validator; `shutdown` stops its thread.
    pub fn new(shutdown: CancellationToken) -> Self {
        let limits = app_js::Limits {
            deadline: DEADLINE,
            heap_limit: HEAP_LIMIT,
            max_line: MAX_LINE,
        };
        Self::with(Backend::Js(app_js::BundleThread::new(
            &app_js::APPS,
            limits,
            shutdown,
        )))
    }

    /// A helper answering every request line with `respond`'s line (tests of
    /// crates that need a parser verdict without the real validator).
    #[doc(hidden)]
    pub fn canned(respond: Responder) -> Self {
        Self::with(Backend::Canned(respond))
    }

    /// What [`crate::AppPlugin`] starts: the in-process validator, or the
    /// responder a test installed with [`install_canned`].
    pub(crate) fn for_plugin(shutdown: CancellationToken) -> Self {
        match CANNED.get() {
            Some(respond) => Self::canned(respond.clone()),
            None => Self::new(shutdown),
        }
    }

    fn with(backend: Backend) -> Self {
        Self {
            backend,
            admission: Arc::new(Semaphore::new(PENDING + 1)),
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

    /// Syntax-check one mermaid diagram (30 §5): `path` is `line:N` when the
    /// parser located the error, else `""`. Runs on [`MERMAID`].
    pub async fn check_mermaid(&self, _source: &str) -> Result<Vec<tools::FenceError>, Error> {
        match MERMAID {
            MermaidBackend::FailOpen => Ok(Vec::new()),
        }
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
        let line = serde_json::to_string(&Request {
            id: id.clone(),
            op,
            source,
            base,
            tools,
        })
        .map_err(|e| Error::Protocol(format!("serialize request: {e}")))?;
        let v = self.send(line, &id).await?;
        serde_json::from_value(v).map_err(|e| Error::Protocol(format!("response body: {e}")))
    }

    /// Admit, run, match the reply to `id`.
    async fn send(&self, line: String, id: &str) -> Result<serde_json::Value, Error> {
        if line.len() > MAX_LINE {
            return Err(Error::Protocol(format!(
                "request is {} bytes; the helper accepts {MAX_LINE}",
                line.len()
            )));
        }
        let _permit = self
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Busy)?;
        let reply = match &self.backend {
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
            if let Backend::Js(js) = &self.backend {
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
            .merge_check("b = Button(\"Go\", Action([@Run(sync)]))", SOURCE, &[sync_tool()])
            .await
            .unwrap();
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert!(r.source.contains("Button(\"Go\""), "{}", r.source);
        // Many requests, in order, one runtime.
        for _ in 0..5 {
            assert!(h.check("root = Stack([])", &[]).await.unwrap().diagnostics.is_empty());
        }
    }

    #[tokio::test]
    async fn oversized_requests_are_refused_before_the_runtime() {
        let h = helper();
        let big = "x".repeat(MAX_LINE);
        assert!(matches!(h.check(&big, &[]).await, Err(Error::Protocol(m)) if m.contains("the helper accepts")));
        assert!(h.check("root = Stack([])", &[]).await.is_ok());
    }

    #[tokio::test]
    async fn a_hang_is_terminated_at_the_deadline_and_the_next_request_is_served() {
        let h = helper();
        assert!(h.check("root = Stack([])", &[]).await.is_ok());
        let Backend::Js(js) = &h.backend else { unreachable!() };
        js.eval_raw("globalThis.theuthApps.call = () => { for (;;) {} };".into())
            .await
            .unwrap();
        let t = std::time::Instant::now();
        assert_eq!(h.check("root = Stack([])", &[]).await.unwrap_err(), Error::Timeout);
        assert!(t.elapsed() >= DEADLINE && t.elapsed() < DEADLINE * 5, "{:?}", t.elapsed());
        assert!(h.check("root = Stack([])", &[]).await.is_ok(), "fresh runtime serves");
    }

    #[tokio::test]
    async fn a_reply_for_another_request_fails_it_and_the_runtime_is_rebuilt() {
        let h = helper();
        assert!(h.check("root = Stack([])", &[]).await.is_ok());
        let Backend::Js(js) = &h.backend else { unreachable!() };
        js.eval_raw(
            "const orig = globalThis.theuthApps.call;
             globalThis.theuthApps.call = (line) => { globalThis.theuthApps.call = orig; return orig(line).replace(/^\\{\"id\":\"[^\"]*\"/, '{\"id\":\"999\"'); };"
                .into(),
        )
        .await
        .unwrap();
        let generation = js.generation();
        assert!(matches!(h.check("root = Stack([])", &[]).await, Err(Error::Crashed(m)) if m.contains("999")));
        assert!(h.check("root = Stack([])", &[]).await.is_ok());
        assert_eq!(js.generation(), generation + 1);
    }

    #[tokio::test]
    async fn error_lines_are_protocol_errors() {
        let h = Helper::canned(Arc::new(|line: &str| {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            serde_json::json!({ "id": v["id"], "error": "bad" }).to_string()
        }));
        assert_eq!(h.check("x", &[]).await.unwrap_err(), Error::Protocol("bad".into()));
        let h = Helper::canned(Arc::new(|_: &str| "not json".to_owned()));
        assert!(matches!(h.check("x", &[]).await, Err(Error::Protocol(_))));
    }

    #[tokio::test]
    async fn more_than_pending_queued_requests_are_busy_not_queued() {
        let h = Arc::new(helper());
        assert!(h.check("root = Stack([])", &[]).await.is_ok());
        let Backend::Js(js) = &h.backend else { unreachable!() };
        // Occupy the runtime thread (no admission permit), then admit PENDING + 1.
        let busy = js.eval_raw("{ const t0 = Date.now(); while (Date.now() - t0 < 1000) {} }".into());
        let waiting: Vec<_> = (0..=PENDING)
            .map(|_| {
                let h = h.clone();
                tokio::spawn(async move { h.check("root = Stack([])", &[]).await })
            })
            .collect();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while h.admission.available_permits() > 0 {
            assert!(std::time::Instant::now() < deadline, "admission never filled");
            tokio::task::yield_now().await;
        }
        assert_eq!(h.check("root = Stack([])", &[]).await.unwrap_err(), Error::Busy);
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
        let Backend::Js(js) = &h.backend else { unreachable!() };
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while js.is_running() {
            assert!(std::time::Instant::now() < deadline, "thread never stopped");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(matches!(h.check("x", &[]).await, Err(Error::Crashed(_))));
    }

    #[tokio::test]
    async fn check_mermaid_fails_open_in_phase_1() {
        assert_eq!(MERMAID, MermaidBackend::FailOpen);
        assert_eq!(helper().check_mermaid("graph TD\nA-->B[").await.unwrap(), vec![]);
    }
}
