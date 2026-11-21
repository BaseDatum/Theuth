//! In-process JS bundles for the app plugin, run in a bare deno_core `JsRuntime` — the same V8 the `theuth`
//! binary already links through obscura-js. Two bundles, each in its own isolate on its own lazily started
//! thread with its own [`Limits`]: [`APPS`], the OpenUI Lang check/merge validator (`packages/apps/src/inproc.ts`,
//! `@openuidev/lang-core` + the generated library schema) that used to be the `theuth-app-parser` Bun binary,
//! and [`MERMAID`], the parse-only mermaid check (`packages/apps/src/inproc-mermaid.ts`, mermaid 12 with
//! DOMPurify aliased to an identity shim). As built: docs/design/apps-inproc-as-built-P1.md (phase 1 and 2).
//!
//! The runtime is generic over a [`Bundle`] (script + entry global + thread name). The code is a copy of
//! `crates/document-js` (feat/documents-inproc 0ca80bbf3) with that parameter and [`Limits::first_deadline`]
//! added; once both branches are merged the two crates collapse into one (see the as-built note).
//!
//! Wire: the bundle's own protocol, unchanged. One request line in; the bundle's output lines for it out
//! (for [`APPS`] and [`MERMAID`]: exactly one `protocol.ts` response line). The entry's `call` may answer
//! synchronously ([`APPS`]) or with a promise ([`MERMAID`]): the runtime drives the event loop until the reply.
//!
//! - [`BundleJs`] owns one runtime (`!Send`); [`BundleJs::call`] serves one request.
//! - [`BundleThread`] is the `Send + Clone` handle: one lazily started OS thread owns the runtime, requests
//!   run to completion in enqueue order, and a failure that leaves the runtime unusable (deadline, heap
//!   limit, panic, an event-loop error or a request that ended without its own reply) fails the request and
//!   every one queued behind it, drops the runtime and builds a fresh one, bumping
//!   [`BundleThread::generation`]. A dead thread is respawned by the next request.
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

#[cfg(test)]
mod tests;

use std::time::Duration;

pub use runtime::{BundleJs, init_platform};
pub use thread::{BundleThread, Settled, Ticket};

/// One classic-script bundle and how to reach it.
#[derive(Debug)]
pub struct Bundle {
    /// Script name in stack traces, e.g. `<theuth:apps>`.
    pub name: &'static str,
    /// The script: an IIFE that installs [`Self::global`].
    pub source: &'static str,
    /// The global the script installs: `{ call(line) -> string | Promise<string>, version }` where `version`
    /// is a string or a function returning one.
    pub global: &'static str,
    /// Name of the OS thread that owns this bundle's runtime.
    pub thread: &'static str,
}

/// The OpenUI check/merge validator: `just apps-bundle` rebuilds `js/theuth-apps-inproc.js` from
/// `packages/apps` (`scripts/build-inproc.ts`) and records its inputs' hash in `js/BUNDLE.sha256`;
/// `just apps-bundle-check` fails when they drift.
pub static APPS: Bundle = Bundle {
    name: "<theuth:apps>",
    source: include_str!("../js/theuth-apps-inproc.js"),
    global: "theuthApps",
    thread: "app-js",
};

/// The parse-only mermaid check: `just apps-bundle` rebuilds `js/theuth-mermaid-inproc.js` from `packages/apps`
/// (`scripts/build-inproc-mermaid.ts`, render-only libraries stubbed) and records its inputs' hash in
/// `js/BUNDLE.sha256`. `call` answers with a promise.
pub static MERMAID: Bundle = Bundle {
    name: "<theuth:mermaid>",
    source: include_str!("../js/theuth-mermaid-inproc.js"),
    global: "theuthMermaid",
    thread: "mermaid-js",
};

/// Default [`Limits::deadline`].
pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(30);
/// Floor of the deadline for building a runtime (polyfill + bundle load): construction is not a request, and
/// a debug build under load may take seconds to parse a bundle.
pub const LOAD_DEADLINE: Duration = Duration::from_secs(30);
/// Default [`Limits::heap_limit`].
pub const DEFAULT_HEAP_LIMIT: usize = 512 * 1024 * 1024;
/// Default [`Limits::max_line`].
pub const DEFAULT_MAX_LINE: usize = 16 * 1024 * 1024;

/// Per-runtime guards.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// Lines the request pushed before its reply, in order (document-js: `{"ev":"update"}`; [`APPS`]: none).
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
