//! The documents helper (`packages/documents`: BlockNote 0.54.2 + yjs + linkedom), bundled to one classic
//! script and run in-process in a bare deno_core `JsRuntime` — the same V8 the `theuth` binary already links
//! through obscura-js. Design: docs/design/documents-inproc-spike.md; as built: docs/design/documents-inproc-as-built-P1.md.
//!
//! Wire: `packages/documents/src/protocol.ts`, unchanged. One request line in; the lines the stdio helper
//! would have written for it out — any `{"ev":"update"}` lines first, the reply last.
//!
//! - [`DocumentsJs`] owns one runtime (`!Send`); [`DocumentsJs::call`] serves one request.
//! - [`DocumentsThread`] is the `Send + Clone` handle: one lazily started `documents-js` OS thread owns the
//!   runtime, requests run to completion in enqueue order, and a failure that leaves the runtime unusable
//!   (deadline, heap limit, panic, an event-loop error or a request that ended without its own reply) fails
//!   the request and every one queued behind it, drops the runtime and builds a fresh one, bumping
//!   [`DocumentsThread::generation`]. A reply is therefore only ever taken from the request it answers.
//!
//! Containment (a JS failure must not take down the process): a JS exception is an `ok:false` reply;
//! a request past [`Limits::deadline`] is terminated from the watchdog thread ([`Error::Timeout`]); the heap is
//! capped at [`Limits::heap_limit`] and a near-limit callback terminates instead of letting V8 abort
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

pub use runtime::{DocumentsJs, init_platform};
pub use thread::{DocumentsThread, Settled, Ticket};

/// The embedded bundle: `just documents-bundle` rebuilds it from `packages/documents` (`scripts/build-inproc.ts`)
/// and records its inputs' hash in `js/BUNDLE.sha256`; `just documents-bundle-check` fails when they drift.
pub const BUNDLE: &str = include_str!("../js/theuth-documents-inproc.js");

/// Default [`Limits::deadline`]: the stdio helper's `DEADLINE`.
pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(30);
/// Default [`Limits::heap_limit`].
pub const DEFAULT_HEAP_LIMIT: usize = 512 * 1024 * 1024;
/// Default [`Limits::max_line`]: the stdio helper's `MAX_LINE`.
pub const DEFAULT_MAX_LINE: usize = 16 * 1024 * 1024;

/// Per-runtime guards.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Longest one request (or raw script) may run, event loop included.
    pub deadline: Duration,
    /// V8 heap ceiling in bytes (`v8::CreateParams::heap_limits` max).
    pub heap_limit: usize,
    /// Largest request line in bytes; longer ones are rejected before they reach JS.
    pub max_line: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            deadline: DEFAULT_DEADLINE,
            heap_limit: DEFAULT_HEAP_LIMIT,
            max_line: DEFAULT_MAX_LINE,
        }
    }
}

/// The helper's output for one request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    /// `{"ev":"update",…}` lines the request's transactions pushed, in order, before the reply.
    pub events: Vec<String>,
    /// The `{"id":…,"ok":…}` reply line (an `ok:false` line for a JS exception).
    pub reply: String,
}

impl Reply {
    /// Split the helper's output lines: the last one is the reply, the ones before it are events.
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
    #[error("request is {len} bytes; the documents runtime accepts {max}")]
    TooLarge {
        /// Line length in bytes.
        len: usize,
        /// The configured cap.
        max: usize,
    },
    /// [`Limits::deadline`] passed; execution was terminated and the runtime is rebuilt.
    #[error(
        "documents runtime did not answer within its deadline; it was terminated and is rebuilt"
    )]
    Timeout,
    /// The V8 heap reached [`Limits::heap_limit`]; execution was terminated and the runtime is rebuilt.
    #[error("documents runtime reached its heap limit; it was terminated and is rebuilt")]
    OutOfMemory,
    /// A Rust panic on the runtime thread (in an op or the loop), an event-loop error, a request that ended
    /// without its reply, or the runtime could not be built.
    #[error("documents runtime failed: {0}")]
    Crashed(String),
    /// Script-level failure that leaves the runtime usable: an uncaught exception in a raw script (or the
    /// `version` script producing nothing).
    #[error("documents runtime: {0}")]
    Js(String),
    /// The runtime thread has shut down (last handle dropped or the shutdown token cancelled).
    #[error("documents runtime stopped")]
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
