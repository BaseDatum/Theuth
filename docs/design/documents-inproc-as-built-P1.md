# Documents helper in-process — phase 1 as built

Branch `feat/documents-inproc` (from main 012e280eb). Phase 1 of 3: a production crate `crates/document-js` that runs the documents helper's TypeScript (`packages/documents`) in-process in deno_core, with crash containment. **Not wired into `crates/document` yet** (phase 2); the sidecar binary and its packaging stay (phase 3). Background and measurements: [documents-inproc-spike.md](documents-inproc-spike.md).

## What exists

### Files

| path | what |
|---|---|
| `crates/document-js/Cargo.toml` | lib crate; `deno_core = "0.412"` (as obscura-js), `getrandom` 0.2, `thiserror`, `tokio`, `tokio-util`, `tracing`; workspace lints |
| `crates/document-js/js/theuth-documents-inproc.js` | the committed bundle (1,104,902 bytes), `include_str!`'d — a cargo build needs no bun |
| `crates/document-js/js/BUNDLE.sha256` | `inputs <sha256>` over `packages/documents/src/**`, `package.json`, `bun.lock`, `scripts/build-inproc.ts`; `bundle <sha256>` of the committed bundle |
| `crates/document-js/js/polyfill.js` | `crypto.getRandomValues`/`randomUUID`, `atob`/`btoa`, timers (spike's, ops renamed `op_documents_*`) |
| `crates/document-js/src/lib.rs` | `Limits`, `Reply`, `Error`, `BUNDLE`, defaults |
| `crates/document-js/src/runtime.rs` | `DocumentsJs`, the ops and their panic guard, the heap guard, the pump |
| `crates/document-js/src/watchdog.rs` | one process-wide `documents-js-watchdog` thread (obscura `cdp_watchdog.rs` pattern) |
| `crates/document-js/src/thread.rs` | `DocumentsThread`, `Ticket`, the `documents-js` thread loop |
| `crates/document-js/src/tests.rs` | unit tests (round trip, ordering, lifecycle, the five guards) |
| `crates/document-js/tests/golden.rs`, `tests/golden.json` | 28 golden transcripts recorded from the shipped jsdom helper |
| `packages/documents/src/inproc.ts`, `src/inproc/{editor,dom,dom-happy}.ts`, `scripts/build-inproc.ts`, `test-inproc/*` | spike TS side (cherry-picked from 03bcacecb, unchanged) |
| `packages/documents/scripts/parity-inproc.ts` | parity driver (spike), corpus moved out to `scripts/parity-corpus.ts` |
| `packages/documents/scripts/golden-inproc.ts` | writes `crates/document-js/tests/golden.json` from a stdio peer (default `bun src/helper.ts`, i.e. jsdom + server-util) |
| `justfile` | new recipes `documents-bundle`, `documents-bundle-check`, private `_documents-bundle-hash`; existing recipes untouched |
| `Cargo.toml` | new `[workspace.dependencies]` entry `document-js` (for phase 2) |

### Public API (`document_js`)

```rust
pub const BUNDLE: &str;                       // include_str!("../js/theuth-documents-inproc.js")
pub const DEFAULT_DEADLINE: Duration;         // 30 s  (helper.rs DEADLINE)
pub const DEFAULT_HEAP_LIMIT: usize;          // 512 MiB
pub const DEFAULT_MAX_LINE: usize;            // 16 MiB (helper.rs MAX_LINE)

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Limits { pub deadline: Duration, pub heap_limit: usize, pub max_line: usize }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply { pub events: Vec<String>, pub reply: String }   // ev:update lines, then the reply line

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error { TooLarge { len: usize, max: usize }, Timeout, OutOfMemory, Crashed(String), Js(String), Stopped }
impl Error { pub fn is_fatal(&self) -> bool }            // Timeout | OutOfMemory | Crashed

pub struct DocumentsJs;                                   // !Send; owns JsRuntime + a current-thread tokio runtime
impl DocumentsJs {
    pub fn new(limits: Limits) -> Result<Self, Error>;
    pub fn call(&mut self, line: &str) -> Result<Vec<String>, Error>;   // events…, reply
    pub fn version(&mut self) -> Result<String, Error>;                 // "theuth-documents <PROTOCOL_VERSION>"
    pub fn limits(&self) -> &Limits;
    pub fn failure(&self) -> Option<&Error>;              // Some after a fatal error: drop and rebuild
    #[doc(hidden)] pub fn eval_raw(&mut self, script: &str) -> Result<(), Error>;
}

#[derive(Clone)] pub struct DocumentsThread;              // Send + Sync
impl DocumentsThread {
    pub fn new(limits: Limits, shutdown: CancellationToken) -> Self;     // lazy: no thread yet
    pub fn enqueue(&self, line: String) -> Ticket;        // enqueued synchronously (ordering = call order)
    pub async fn request(&self, line: String) -> Result<Reply, Error>;  // enqueue + await
    pub fn generation(&self) -> u64;                      // 0 never started, 1 first runtime, +1 per rebuild
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<u64>;
    pub fn is_running(&self) -> bool;
    pub fn limits(&self) -> &Limits;
    #[doc(hidden)] pub fn eval_raw(&self, script: String) -> Ticket;
    #[doc(hidden)] pub fn panic_on_thread(&self) -> Ticket;
    #[doc(hidden)] pub fn liveness(&self) -> Arc<AtomicBool>;
}
#[must_use] pub struct Ticket;                            // impl Future<Output = Result<Reply, Error>>
```

### How a request runs

`DocumentsThread::enqueue` checks `max_line`, then under one mutex starts the thread if needed and sends `Job { work, reply: oneshot }` on an unbounded mpsc — one total order across every clone. The `documents-js` thread (16 MiB stack) waits on `select!{ shutdown.cancelled(), rx.recv() }` in its own current-thread tokio runtime, builds a `DocumentsJs` if it has none (generation +1), runs the job under `catch_unwind`, answers the oneshot. `DocumentsJs::call` puts the line into `OpState`, executes `__theuthPump()` (calls `theuthDocuments.call(line)`, hands the joined output to `op_documents_reply`; a throw/rejection becomes `{"id":<request id>,"ok":false,"error":…}`) and polls the event loop until the reply arrives (not until idle, so a stray refed timer cannot stall a request), all inside the deadline. Output is split on `\n` (JSON.stringify never emits a raw newline): last line = reply, the rest = events.

V8 platform: `JsRuntime::new` calls deno_core's `init_v8`, which is behind a process-wide `Once` (`deno_core-0.412.0/runtime/setup.rs:224-243`). In `t`, `browser::warmup` → obscura-js has already run it on the main thread, so the documents thread only creates an isolate; standalone (tests) the first `DocumentsJs::new` initialises it. Both use the default platform and non-snapshotting mode, so the mode assertion in `init_v8` holds. No second platform is created.

## The guards

| | how | test | result |
|---|---|---|---|
| a. JS exception | The bundle catches per op (`ok:false`); the pump catches a throw/rejection of the entry point itself and answers `ok:false` with the request id; a raw script's uncaught exception is `Error::Js` (not fatal, same generation). | `tests::js_exception_is_an_error_reply_and_the_runtime_survives` | ok (run 2, 18:03) |
| b. Deadline | Every `run` arms the shared watchdog (`watchdog::arm(IsolateHandle, deadline)`); on expiry the watchdog thread calls `terminate_execution()`. The event-loop wait is additionally wrapped in `tokio::time::timeout(remaining)` (a request waiting on a timer runs no JS to terminate). Either → `Error::Timeout`; the runtime marks itself failed; the thread fails every queued job with the same error (as `helper.rs` `fail_all` does for a killed child), drops the runtime, builds a new one (generation +1). | `tests::deadline_terminates_a_runaway_script_and_rebuilds` (1.5 s deadline; `for(;;){}` + a queued request; then a never-resolving timer promise) | ok (run 2) |
| c. Heap | `RuntimeOptions::create_params = v8::CreateParams::default().heap_limits(0, heap_limit)`; `add_near_heap_limit_callback` sets a flag, calls `terminate_execution()` and returns `current + 64 MiB` so V8 can unwind instead of aborting (obscura `install_heap_limit_guard`). → `Error::OutOfMemory`, rebuild. The callback is never re-armed: the isolate is discarded. | `tests::heap_limit_terminates_and_rebuilds` (128 MiB cap, array grown forever) | ok (run 2) |
| d. Panic | Two places. (1) Ops: deno_ops generates `extern "C"` callbacks and neither deno_core nor deno_ops catches unwinds (no `catch_unwind` in either crate), so a panic in an op would abort the process; every op body runs in `guarded()` (`catch_unwind`, record the message, `terminate_execution()`) → `Error::Crashed`. (2) The thread: every job and every build/drop of a runtime runs under `catch_unwind`; a panic → `Error::Crashed`, queued jobs fail with it, fresh runtime. | `tests::panic_in_an_op_is_contained` (test-only `op_documents_test_panic`, registered only under `cfg(test)`), `tests::panic_on_the_thread_fails_pending_jobs_and_a_fresh_runtime_serves` | both ok (run 2) |
| e. Input cap | `DocumentsThread::enqueue` rejects `line.len() > max_line` with `Error::TooLarge` before the job is queued (the thread is not even started); `DocumentsJs::call` checks the same. | `tests::oversized_line_is_rejected_before_js` | ok (run 2) |

Each failure test then runs an `open → import → markdown → close` round trip on the same handle; the test process continuing is the proof the process survived. A V8 internal fatal error (e.g. a fatal OOM the callback cannot avert, a CHECK failure) still aborts the process — accepted, not attempted.

**Limit of (d), found by the first test run:** a panic inside one of deno_core's *builtin* ops cannot be caught by this crate (their `extern "C"` callbacks are deno_core's, not ours) and aborts the process ("panic in a function that cannot unwind"). The first run hit exactly that: a script arming `setTimeout` ran `execute_script` outside the tokio context, deno_core's timer op (`ops_builtin_v8.rs:132` → `reactor_tokio.rs:26`, "there is no reactor running") panicked and the test binary aborted. Fix: every script and every event-loop poll now runs inside the runtime's own `block_on`, so the builtin ops always find their reactor. Other builtin-op panics remain uncontainable, like V8 fatal errors.

Panic profile: the workspace `Cargo.toml` sets no `panic = "abort"` in any `[profile.*]` (`dev`, `dev-full`, `release` overrides only set `debug`/`codegen-units`), and `.cargo/config.toml` sets only an `[env]` var and aarch64-linux rustflags/linker (no `-C panic=abort`). So (d) works in every shipped profile.

Other tests: `documents_js_serves_requests_directly`, `round_trip_open_import_markdown_outline_edit_close`, `update_events_come_before_the_reply`, `concurrent_requests_from_several_handles_run_in_enqueue_order` (4 tasks × 5 appends, markdown order = enqueue order), `thread_starts_lazily`, `dropping_the_last_handle_ends_the_thread`, `cancelling_the_token_ends_the_thread`, and `tests/golden.rs::golden_transcripts_match_the_jsdom_helper` (28 cases: the 15 `test:` and 13 `syn:` parity inputs; script open → import replace → markdown → outline → annotated markdown → edit insert/replace/move → markdown → import diff → markdown → close; every output line canonicalised: `update` bytes dropped, keys sorted, UUIDs → `ID<n>`).

## Deviations from the brief

- `Error::Timeout`/`OutOfMemory` are unit variants (like `helper.rs`'s `Timeout`); `Error::Js` and `Error::Stopped` added (non-fatal script error; thread gone).
- A failure fails not only the running job but every job queued behind it (same error) — the in-process analogue of `helper.rs` failing all pending requests of a dead child. A job enqueued after the drain runs on the fresh runtime.
- Rebuild is eager (right after the failure), so `generation` bumps before the next request arrives and phase 2 can reopen mirrors at once; a failed rebuild is retried on the next request.
- `DocumentsJs` owns its own current-thread tokio runtime (the spike's shape) so it is usable standalone; the thread has a second, tiny current-thread runtime only for waiting on jobs/cancellation. `DocumentsJs` must not be called from inside an async context (it `block_on`s).
- `.gitattributes`: the repo has none, so the bundle is not marked `linguist-generated` (adding the file was out of scope).
- Golden expectations come from `bun src/helper.ts` (the shipped jsdom helper run from source), not from the compiled 69 MB binary (not built in this worktree); same code.
- Obscura serialises isolate creation behind private locks (`ISOLATE_CREATE_LOCK`); this crate does not share them. Concurrent `JsRuntime::new` on the documents thread and an obscura thread is plain V8 multi-isolate use; not stress-tested.

## Verification (2026-11-21, dev profile, shared machine, under `/tmp/theuth-build.lock`)

- `CARGO_TARGET_DIR=<worktree>/target CARGO_BUILD_JOBS=4 cargo test --offline -p document-js` — run 1 (17:55): aborted (the timer-op panic above; plus two wrong test expectations: outline heading text is `[h1] Heading`, and a `DocumentsJs` called inside `#[tokio::test]` `block_on`-panics). Run 2 (18:03) after the fixes: `test result: ok. 13 passed; 0 failed` (unit, 3.27 s), `test result: ok. 1 passed; 0 failed` (golden, 28 cases), doc-tests `ok. 0 passed`.
- `cargo clippy --offline -p document-js --all-targets` (18:04, workspace `clippy::all = deny`): no warnings. Then `rustfmt --edition 2024` on the crate's files only (formatting only, not rebuilt).
- `cargo tree --offline -i v8`: one `v8 v150.4.0` ← `deno_v8 v0.4.0` ← `deno_core v0.412.0` ← {`document-js`, `obscura-js`}.
- `bun test` in packages/documents: 83 pass, 1 fail (`markdown round trip per block type > code: link`, the known stale test).
- The committed bundle and `BUNDLE.sha256` were written by `just documents-bundle` itself; `just documents-bundle-check` passes, and fails after appending a line to `src/yaml.ts` (reverted). Bit-for-bit reproducibility of the bun build across two runs was not checked.

## Not built in phase 1

- No change to `crates/document`, `crates/theuth`, packaging, `helper.ts`, `build-helper.ts`, jsdom/server-util deps.
- No startup snapshot, no `TextEncoder`/`TextDecoder` polyfill (spike §3: optional).
- No CI wiring of `just documents-bundle-check` or of the parity script.
- Heap recovery without rebuild (obscura's `recover_heap_limit`) — deliberately not done: a terminated-by-OOM isolate is discarded.

## Phase 2: putting `DocumentsThread` behind `Helper`

Keep `Helper`'s public surface (`new` minus `path`, `request`, `open/update/close/markdown/markdown_of/import/edit/outline`, `Pending`, `HelperEvent`, result types, `Error` minus `Missing`/`Version`).

1. `Helper { docs: DocumentsThread, next: Arc<AtomicU64>, events: mpsc::UnboundedSender<HelperEvent> }`. `Helper::new(events, shutdown)` builds `DocumentsThread::new(Limits::default(), shutdown.clone())` (defaults equal `DEADLINE`/`MAX_LINE`) and spawns one tokio task watching `docs.subscribe()`: on every change to a value > 1 send `HelperEvent::Restarted`; end on `shutdown`.
2. `Helper::request` builds the same JSON body (without the trailing `\n`), calls `docs.enqueue(line)` **synchronously** (this keeps `open` ahead of `update`), and returns `Pending { id, rx }` where `rx` is fed by a spawned task: `let r = ticket.await;` → for each `r.events` line run the existing `handle_line` event branch (→ `HelperEvent::Update`) **before** resolving the reply; parse `r.reply` with the existing reply branch (`ok:false` → `Error::Protocol`). Or make `Pending` hold the `Ticket` and do the parsing in `poll` — but events must be sent before the caller sees the reply, which the spawned task guarantees.
3. Map errors: `document_js::Error::Timeout → Error::Timeout`, `OutOfMemory`/`Crashed(m)` → `Error::Crashed(..)`, `TooLarge` → `Error::Protocol("request is N bytes; …")` (today's message), `Js(m)` → `Error::Protocol(m)`, `Stopped` → `Error::Crashed("helper worker stopped")`.
4. Delete from `helper.rs`: `Proc`, `spawn`, `worker`, reader task, `PendingMap`, backoff, `helper_path`, the `--version` handshake (compare `DocumentsJs::version()`/`PROTOCOL_TS` in a test instead), `set_process`/`HelperStatus` (or report `generation`/`is_running`). `HelperEvent::Restarted` semantics unchanged: the hub reopens mirrors.
5. Callers of `Helper::new(path, …)`: `hub.rs` tests, `crates/agent/src/rpc_documents.rs`, `crates/artifact/tests/{document,read}.rs` — drop the `path` argument. `crates/document` then depends on `document-js` (links V8 into its tests; the spike §6 alternative is a backend trait with a fake).
6. `t` must keep calling `browser::warmup` before the first documents request (already true); nothing else to initialise.
