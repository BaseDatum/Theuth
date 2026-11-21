//! [`DocumentsJs`]: one deno_core `JsRuntime` with the polyfill and the bundle loaded, and the guards that
//! keep a JS failure inside it (deadline watchdog, heap ceiling, panic-catching ops).

use std::borrow::Cow;
use std::future::poll_fn;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Poll;
use std::time::Instant;

use deno_core::{
    Extension, JsRuntime, ModuleCodeString, OpState, PollEventLoopOptions, RuntimeOptions, op2, v8,
};

use crate::{BUNDLE, Error, Limits, watchdog};

const POLYFILL: &str = include_str!("../js/polyfill.js");
/// Hands the pending request to the bundle and its output (or an `ok:false` line naming the request's id
/// when the bundle itself throws) back through `op_documents_reply`.
const PUMP: &str = r#"globalThis.__theuthPump = () => {
  const ops = Deno.core.ops;
  const line = ops.op_documents_take_request();
  const fail = (e) => {
    let id = 0;
    try {
      const r = JSON.parse(line);
      if (r && typeof r.id === "number") id = r.id;
    } catch {}
    ops.op_documents_reply(JSON.stringify({ id, ok: false, error: String((e && e.stack) || e) }));
  };
  try {
    globalThis.theuthDocuments.call(line).then((out) => ops.op_documents_reply(out), fail);
  } catch (e) {
    fail(e);
  }
};"#;
const VERSION: &str = "Deno.core.ops.op_documents_reply(String(globalThis.theuthDocuments.version))";
/// What the near-heap-limit callback grants so V8 can unwind the terminated script instead of aborting.
const HEAP_HEADROOM: usize = 64 * 1024 * 1024;

/// The request being served, its output, and a panic caught in an op — handed across the JS boundary.
#[derive(Default)]
struct Exchange {
    request: Option<String>,
    reply: Option<String>,
    crashed: Option<String>,
}

/// Run an op body; a panic must not unwind into V8 (the op callbacks are `extern "C"`, so it would abort the
/// process): record it, terminate the script, return a default.
fn guarded<R: Default>(state: &mut OpState, f: impl FnOnce(&mut OpState) -> R) -> R {
    match catch_unwind(AssertUnwindSafe(|| f(&mut *state))) {
        Ok(r) => r,
        Err(payload) => {
            let message = crate::panic_message(&*payload);
            state
                .borrow_mut::<Exchange>()
                .crashed
                .get_or_insert_with(|| format!("panic in a documents op: {message}"));
            state.borrow::<v8::IsolateHandle>().terminate_execution();
            R::default()
        }
    }
}

#[op2]
#[string]
fn op_documents_take_request(state: &mut OpState) -> String {
    guarded(state, |s| {
        s.borrow_mut::<Exchange>()
            .request
            .take()
            .unwrap_or_default()
    })
}

#[op2(fast)]
fn op_documents_reply(state: &mut OpState, #[string] out: String) {
    guarded(state, |s| s.borrow_mut::<Exchange>().reply = Some(out));
}

#[op2(fast)]
fn op_documents_random_fill(state: &mut OpState, #[buffer] buf: &mut [u8]) {
    guarded(state, |_| getrandom::getrandom(buf).expect("OS randomness"));
}

/// Test hook for the panic guard: registered only in this crate's unit tests.
#[cfg(test)]
#[op2(fast)]
fn op_documents_test_panic(state: &mut OpState) {
    guarded(state, |_| panic!("test: panic inside a documents op"));
}

fn extension() -> Extension {
    #[allow(unused_mut)]
    let mut ops = vec![
        op_documents_take_request(),
        op_documents_reply(),
        op_documents_random_fill(),
    ];
    #[cfg(test)]
    ops.push(op_documents_test_panic());
    Extension {
        name: "theuth_documents",
        ops: Cow::Owned(ops),
        ..Default::default()
    }
}

/// One in-process documents helper: a `JsRuntime` with the bundle loaded. `!Send`, and it drives its own
/// current-thread tokio runtime, so it must not be called from inside an async context: use
/// [`crate::DocumentsThread`] there.
///
/// After [`Error::Timeout`], [`Error::OutOfMemory`] or [`Error::Crashed`] the runtime refuses further work
/// ([`Self::failure`]); drop it and build a new one.
pub struct DocumentsJs {
    // Field order is drop order: the isolate goes before the tokio runtime it is bound to.
    js: JsRuntime,
    tokio: tokio::runtime::Runtime,
    isolate: v8::IsolateHandle,
    heap_tripped: Arc<AtomicBool>,
    limits: Limits,
    failure: Option<Error>,
}

impl DocumentsJs {
    /// Build the isolate (heap-capped), load the polyfill and the bundle under the same guards as a request.
    ///
    /// V8's platform is initialised by deno_core behind a process-wide `Once` on the first `JsRuntime::new`:
    /// in `t` that already happened on the main thread (`browser::warmup`); standalone it happens here.
    pub fn new(limits: Limits) -> Result<Self, Error> {
        let tokio = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| Error::Crashed(format!("tokio runtime: {e}")))?;
        // deno_core binds the isolate to the tokio handle current at `JsRuntime::new`.
        let mut js = {
            let _guard = tokio.enter();
            JsRuntime::new(RuntimeOptions {
                extensions: vec![extension()],
                create_params: Some(v8::CreateParams::default().heap_limits(0, limits.heap_limit)),
                ..Default::default()
            })
        };
        let isolate = js.v8_isolate().thread_safe_handle();
        let heap_tripped = Arc::new(AtomicBool::new(false));
        {
            let tripped = heap_tripped.clone();
            let isolate = isolate.clone();
            // Grant headroom (V8 would otherwise abort the process) and stop the script; the runtime is
            // then unusable and gets rebuilt, so the callback never needs re-arming.
            js.add_near_heap_limit_callback(move |current, _initial| {
                tripped.store(true, Ordering::SeqCst);
                isolate.terminate_execution();
                current.saturating_add(HEAP_HEADROOM)
            });
        }
        {
            let state = js.op_state();
            let mut state = state.borrow_mut();
            state.put(Exchange::default());
            state.put(isolate.clone());
        }
        let mut this = Self {
            js,
            tokio,
            isolate,
            heap_tripped,
            limits,
            failure: None,
        };
        this.run("<theuth:polyfill>", ModuleCodeString::from_static(POLYFILL), false)?;
        this.run("<theuth:documents>", ModuleCodeString::from_static(BUNDLE), false)?;
        this.run("<theuth:pump>", ModuleCodeString::from_static(PUMP), false)?;
        Ok(this)
    }

    /// The guards this runtime enforces.
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// The fatal error that stopped this runtime, if any: it must be dropped and rebuilt.
    pub fn failure(&self) -> Option<&Error> {
        self.failure.as_ref()
    }

    /// One request line → the helper's output lines for it: `ev:update` lines first, the reply last.
    /// A JS exception is an `ok:false` reply line, not an `Err`.
    pub fn call(&mut self, line: &str) -> Result<Vec<String>, Error> {
        self.serve(line.to_owned())
    }

    /// [`Self::call`] taking the line by value (no copy).
    pub(crate) fn serve(&mut self, line: String) -> Result<Vec<String>, Error> {
        if line.len() > self.limits.max_line {
            return Err(Error::TooLarge {
                len: line.len(),
                max: self.limits.max_line,
            });
        }
        self.exchange(|x| x.request = Some(line));
        let out = self
            .run("<theuth:call>", ModuleCodeString::from_static("globalThis.__theuthPump()"), true)?
            .ok_or_else(|| Error::Js("the request settled without a reply".into()))?;
        Ok(out.split('\n').map(str::to_owned).collect())
    }

    /// The bundle's `theuth-documents <protocol>` string (`protocol.ts` `PROTOCOL_VERSION`).
    pub fn version(&mut self) -> Result<String, Error> {
        self.run("<theuth:version>", ModuleCodeString::from_static(VERSION), true)?
            .ok_or_else(|| Error::Js("no version".into()))
    }

    /// Run a raw classic script (and the event loop until idle) under the same guards as a request. For the
    /// containment tests: `for(;;){}`, a heap that grows forever, a panicking op.
    #[doc(hidden)]
    pub fn eval_raw(&mut self, script: &str) -> Result<(), Error> {
        self.run("<theuth:raw>", script.to_owned().into(), false).map(|_| ())
    }

    fn exchange<R>(&mut self, f: impl FnOnce(&mut Exchange) -> R) -> R {
        f(self.js.op_state().borrow_mut().borrow_mut::<Exchange>())
    }

    /// Execute `source`, then drive the event loop until a reply arrives (`until_reply`) or it is idle, all
    /// within [`Limits::deadline`]; classify a failure (panic > heap > deadline > script error).
    fn run(
        &mut self,
        name: &'static str,
        source: ModuleCodeString,
        until_reply: bool,
    ) -> Result<Option<String>, Error> {
        if let Some(e) = &self.failure {
            return Err(Error::Crashed(format!("runtime unusable after an earlier failure: {e}")));
        }
        let started = Instant::now();
        let armed = watchdog::arm(self.isolate.clone(), self.limits.deadline);
        let result = self.drive(name, source, until_reply, started);
        let fired = watchdog::disarm(armed);
        let failure = if let Some(m) = self.exchange(|x| x.crashed.take()) {
            Some(Error::Crashed(m))
        } else if self.heap_tripped.load(Ordering::SeqCst) {
            Some(Error::OutOfMemory)
        } else if fired || matches!(result, Err(Error::Timeout)) {
            Some(Error::Timeout)
        } else {
            None
        };
        if let Some(e) = failure {
            self.failure = Some(e.clone());
            return Err(e);
        }
        result
    }

    fn drive(
        &mut self,
        name: &'static str,
        source: ModuleCodeString,
        until_reply: bool,
        started: Instant,
    ) -> Result<Option<String>, Error> {
        self.exchange(|x| x.reply = None);
        self.js
            .execute_script(name, source)
            .map_err(|e| Error::Js(e.to_string()))?;
        let state = self.js.op_state();
        let take_reply = || state.borrow_mut().borrow_mut::<Exchange>().reply.take();
        let remaining = self.limits.deadline.saturating_sub(started.elapsed());
        let js = &mut self.js;
        self.tokio.block_on(async {
            let driven = poll_fn(|cx| {
                if until_reply && let Some(reply) = take_reply() {
                    return Poll::Ready(Ok(Some(reply)));
                }
                match js.poll_event_loop(cx, PollEventLoopOptions::default()) {
                    Poll::Ready(Ok(())) => Poll::Ready(Ok(if until_reply { take_reply() } else { None })),
                    Poll::Ready(Err(e)) => Poll::Ready(Err(Error::Js(e.to_string()))),
                    Poll::Pending => Poll::Pending,
                }
            });
            tokio::time::timeout(remaining, driven)
                .await
                .unwrap_or(Err(Error::Timeout))
        })
    }
}
