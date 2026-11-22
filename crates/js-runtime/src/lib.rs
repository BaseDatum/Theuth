//! The in-process JS runtime: TypeScript helpers bundled to classic scripts and run in bare deno_core
//! `JsRuntime`s — the same V8 the `theuth` binary already links through obscura-js. One generic runtime,
//! three [`Bundle`]s, each in its own isolate on its own lazily started thread with its own [`Limits`]:
//!
//! | bundle | script | global | thread | deadline (first) | heap | line | reply | after a fatal failure |
//! |---|---|---|---|---|---|---|---|---|
//! | [`DOCUMENTS`] | `js/theuth-documents-inproc.js` (`packages/documents`, BlockNote + yjs + linkedom) | `theuthDocuments` | `documents-js` | 30 s (30 s) | 512 MiB | 16 MiB | promise, `ev:update` lines first | queued requests fail too |
//! | [`APPS`] | `js/theuth-apps-inproc.js` (`packages/apps/src/inproc.ts`, OpenUI check/merge) | `theuthApps` | `app-js` | 2 s (2 s) | 256 MiB | 4 MiB | sync | queued requests re-run |
//! | [`MERMAID`] | `js/theuth-mermaid-inproc.js` (`packages/apps/src/inproc-mermaid.ts`, parse only) | `theuthMermaid` | `mermaid-js` | 5 s (10 s) | 256 MiB | 4 MiB | promise | queued requests re-run |
//!
//! As built: docs/design/js-runtime-unified-as-built.md (history: documents-inproc-as-built-P1..P3.md,
//! apps-inproc-as-built-P1.md).
//!
//! Wire: each bundle's own protocol, unchanged. One request line in; the bundle's output lines for it out
//! (any event lines first — [`DOCUMENTS`]' `{"ev":"update"}` — the reply last; see [`Reply`]).
//!
//! - [`JsRuntime`] owns one isolate (`!Send`); [`JsRuntime::call`] serves one request.
//! - [`JsThread`] is the `Send + Clone` handle: one lazily started OS thread ([`Bundle::thread`]) owns the
//!   runtime, requests run to completion in enqueue order, and a failure that leaves the runtime unusable
//!   (deadline, heap limit, panic, an event-loop error or a request that ended without its own reply) fails
//!   that request, drops the runtime and builds a fresh one (bumping [`JsThread::generation`], from which
//!   `crates/document` emits `HelperEvent::Restarted`). What happens to the requests queued behind it is
//!   [`Bundle::after_fatal`]. A reply is only ever taken from the request it answers: the request slot and
//!   the reply slot are per request, and a runtime abandoned in an older epoch never answers again. A dead
//!   thread is respawned by the next request.
//! - [`init_platform`] initialises V8 and builds one throwaway isolate on the calling (main) thread under
//!   the process-wide isolate-creation lock; idempotent, so every caller may call it.
//!
//! Containment (a JS failure must not take down the process): a JS exception is an `{id, ok:false, error}`
//! reply; a request past [`Limits::deadline`] is terminated from the watchdog thread ([`Error::Timeout`]); the
//! heap is capped at [`Limits::heap_limit`] and a near-limit callback terminates instead of letting V8 abort
//! ([`Error::OutOfMemory`]); a Rust panic in one of our ops or on the thread is caught ([`Error::Crashed`]); a
//! line over [`Limits::max_line`] never reaches JS ([`Error::TooLarge`]).
//!
//! Not containable; each of these aborts the whole process:
//! - a V8 internal fatal error (`V8_Fatal`/`FatalProcessOutOfMemory` outside the heap-limit path, a failed
//!   V8 `CHECK`);
//! - a single allocation larger than the headroom the near-heap-limit callback grants (64 MB above the
//!   current limit): V8 aborts before the terminated script can unwind;
//! - memory outside the V8 heap, e.g. `ArrayBuffer` backing stores, which [`Limits::heap_limit`] does not
//!   count: exhausting it is a process allocation failure;
//! - a panic inside a deno_core builtin op (not ours, so not behind the panic guard): it unwinds into V8
//!   through an `extern "C"` callback.

mod runtime;
mod thread;
mod watchdog;

// The unit tests live under tests/unit/ (next to the golden integration tests) but compile into the library,
// for its test hooks (`op_jsrt_test_panic`, `JsThread::exit_thread`, `first_isolate`). One module per former
// crate, so equally named tests stay distinct: `tests_documents::…` (document-js), `tests_apps::…` (app-js).
#[cfg(test)]
#[path = "../tests/unit/apps.rs"]
mod tests_apps;
#[cfg(test)]
#[path = "../tests/unit/documents.rs"]
mod tests_documents;

use std::time::Duration;

pub use runtime::{JsRuntime, init_platform};
pub use thread::{JsThread, Settled, Ticket};

/// One classic-script bundle, how to reach it, and how its runtime is guarded.
#[derive(Debug)]
pub struct Bundle {
    /// Script name in stack traces, e.g. `<theuth:apps>`.
    pub name: &'static str,
    /// The script: an IIFE that installs [`Self::global`].
    pub source: &'static str,
    /// The global the script installs: `{ call(line) -> string | Promise<string>, version }` where `version`
    /// is a string or a function returning one.
    pub global: &'static str,
    /// Name of the OS thread that owns this bundle's runtime (also in panic messages).
    pub thread: &'static str,
    /// Load `js/polyfill-web.js` (TextEncoder/TextDecoder, structuredClone, URL, console) after the shared
    /// `js/polyfill.js`.
    pub web_polyfill: bool,
    /// What a fatal failure does to the requests queued behind the failed one.
    pub after_fatal: AfterFatal,
    /// The guards today's callers run it with (tests may pass others to [`JsThread::new`]).
    pub limits: Limits,
}

/// What a fatal failure ([`Error::is_fatal`]) or [`JsThread::restart`] does to requests already queued.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfterFatal {
    /// They fail with the same error: the runtime held per-session state (the documents' mirrors), so a
    /// request written against it must not run on a fresh one. The client reopens and resends.
    FailQueued,
    /// They run on the fresh runtime: the bundle is stateless (every request carries all it needs), so one
    /// runaway request must not fail other sessions' queued checks.
    RerunQueued,
}

/// The documents helper: `just documents-bundle` rebuilds `js/theuth-documents-inproc.js` from
/// `packages/documents` (`scripts/build-inproc.ts`) and records its inputs' hash in `js/documents.sha256`;
/// `just documents-bundle-check` fails when they drift.
pub const DOCUMENTS: Bundle = Bundle {
    name: "<theuth:documents>",
    source: include_str!("../js/theuth-documents-inproc.js"),
    global: "theuthDocuments",
    thread: "documents-js",
    web_polyfill: false,
    after_fatal: AfterFatal::FailQueued,
    limits: Limits {
        deadline: DEFAULT_DEADLINE,
        first_deadline: DEFAULT_DEADLINE,
        heap_limit: DEFAULT_HEAP_LIMIT,
        max_line: DEFAULT_MAX_LINE,
    },
};

/// The OpenUI check/merge validator: `just apps-bundle` rebuilds `js/theuth-apps-inproc.js` from
/// `packages/apps` (`scripts/build-inproc.ts`) and records its inputs' hash in `js/apps.sha256`;
/// `just apps-bundle-check` fails when they drift.
pub const APPS: Bundle = Bundle {
    name: "<theuth:apps>",
    source: include_str!("../js/theuth-apps-inproc.js"),
    global: "theuthApps",
    thread: "app-js",
    web_polyfill: true,
    after_fatal: AfterFatal::RerunQueued,
    limits: Limits {
        deadline: Duration::from_secs(2),
        first_deadline: Duration::from_secs(2),
        heap_limit: APPS_HEAP_LIMIT,
        max_line: APPS_MAX_LINE,
    },
};

/// The parse-only mermaid check: `just apps-bundle` rebuilds `js/theuth-mermaid-inproc.js` from
/// `packages/apps` (`scripts/build-inproc-mermaid.ts`, render-only libraries stubbed) and records its inputs'
/// hash in `js/apps.sha256`. Warm deadline 5 s: V8 runs mermaid's super-linear class-diagram parser ~1.6×
/// slower than JSC, so a 32 KB fence needs ~3 s (docs/design/apps-mermaid-inproc-spike.md §4); the first
/// request on each runtime evaluates its diagram type's chunk (measured ≤ 35 ms; 10 s is headroom).
pub const MERMAID: Bundle = Bundle {
    name: "<theuth:mermaid>",
    source: include_str!("../js/theuth-mermaid-inproc.js"),
    global: "theuthMermaid",
    thread: "mermaid-js",
    web_polyfill: true,
    after_fatal: AfterFatal::RerunQueued,
    limits: Limits {
        deadline: Duration::from_secs(5),
        first_deadline: Duration::from_secs(10),
        heap_limit: APPS_HEAP_LIMIT,
        max_line: APPS_MAX_LINE,
    },
};

/// V8 heap ceiling of each app isolate ([`APPS`], [`MERMAID`]).
const APPS_HEAP_LIMIT: usize = 256 * 1024 * 1024;
/// Largest app request line ([`APPS`], [`MERMAID`]); the bundles refuse longer lines too.
const APPS_MAX_LINE: usize = 4 * 1024 * 1024;

/// Default [`Limits::deadline`] (and [`DOCUMENTS`]': the former stdio helper's `DEADLINE`).
pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(30);
/// Floor of the deadline for building a runtime (polyfill + bundle load): construction is not a request, and
/// a debug build under load may take seconds to parse a bundle.
pub const LOAD_DEADLINE: Duration = Duration::from_secs(30);
/// Default [`Limits::heap_limit`].
pub const DEFAULT_HEAP_LIMIT: usize = 512 * 1024 * 1024;
/// Default [`Limits::max_line`] (and [`DOCUMENTS`]': the former stdio helper's `MAX_LINE`).
pub const DEFAULT_MAX_LINE: usize = 16 * 1024 * 1024;

/// Per-runtime guards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Longest one warm request (or raw script) may run, event loop included.
    pub deadline: Duration,
    /// Longest the first request of each runtime may run (it pays for lazily evaluated chunks, e.g. a mermaid
    /// diagram type's parser); the effective value is never below [`Self::deadline`].
    pub first_deadline: Duration,
    /// V8 heap ceiling in bytes (`v8::CreateParams::heap_limits` max).
    pub heap_limit: usize,
    /// Largest request line in bytes; longer ones are rejected before they reach JS.
    pub max_line: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            deadline: DEFAULT_DEADLINE,
            first_deadline: DEFAULT_DEADLINE,
            heap_limit: DEFAULT_HEAP_LIMIT,
            max_line: DEFAULT_MAX_LINE,
        }
    }
}

/// The bundle's output for one request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    /// Lines the request pushed before its reply, in order ([`DOCUMENTS`]: `{"ev":"update",…}`; the app
    /// bundles: none).
    pub events: Vec<String>,
    /// The reply line (an `{id, ok:false, error}` line for a JS exception).
    pub reply: String,
}

impl Reply {
    /// Split the output lines: the last one is the reply, the ones before it are events.
    fn from_lines(mut lines: Vec<String>) -> Result<Self, Error> {
        let reply = lines
            .pop()
            .ok_or_else(|| Error::Js("the request produced no output".into()))?;
        Ok(Self {
            events: lines,
            reply,
        })
    }
}

/// Why a request produced no reply.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The line exceeds [`Limits::max_line`]; it never reached JS.
    #[error("request is {len} bytes; the in-process JS runtime accepts {max}")]
    TooLarge {
        /// Line length in bytes.
        len: usize,
        /// The configured cap.
        max: usize,
    },
    /// [`Limits::deadline`] passed; execution was terminated and the runtime is rebuilt.
    #[error(
        "in-process JS runtime did not answer within its deadline; it was terminated and is rebuilt"
    )]
    Timeout,
    /// The V8 heap reached [`Limits::heap_limit`]; execution was terminated and the runtime is rebuilt.
    #[error("in-process JS runtime reached its heap limit; it was terminated and is rebuilt")]
    OutOfMemory,
    /// A Rust panic on the runtime thread (in an op or the loop), an event-loop error, a request that ended
    /// without its reply, or the runtime could not be built.
    #[error("in-process JS runtime failed: {0}")]
    Crashed(String),
    /// Script-level failure that leaves the runtime usable: an uncaught exception in a raw script (or the
    /// `version` script producing nothing).
    #[error("in-process JS runtime: {0}")]
    Js(String),
    /// The runtime thread has shut down (last handle dropped or the shutdown token cancelled).
    #[error("in-process JS runtime stopped")]
    Stopped,
}

impl Error {
    /// The runtime is unusable after this error and must be dropped and rebuilt.
    pub fn is_fatal(&self) -> bool {
        matches!(self, Self::Timeout | Self::OutOfMemory | Self::Crashed(_))
    }
}

/// The message of a caught panic payload.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&'static str>()
                .map(|s| (*s).to_owned())
        })
        .unwrap_or_else(|| "unknown panic".to_owned())
}
