# Apps parser in-process — phase 1 as built (OpenUI check/merge)

2026-11-21, branch `feat/apps-inproc` (cut from main ccd3789ca). Design context: 46 §2.1 (the helper this
replaces; dated note added there), 00 §2 (dated note). Sibling: the documents helper went in-process the same
way on `feat/documents-inproc`; this branch copies its runtime instead of depending on it (see
"Duplication with document-js").

Phase 1 delivers OpenUI `check` / `merge_check` in-process. Mermaid (`check_mermaid`) is NOT in-process yet:
it fails open behind one seam (below) until the mermaid phase.

## What changed

- No `theuth-app-parser` process and no Bun binary at runtime. `packages/apps/src/inproc.ts` (same
  `protocol.ts` wire, same `check.ts`) is bundled into one runtime-neutral classic script, committed as
  `crates/app-js/js/theuth-apps-inproc.js`, embedded with `include_str!`, and run in a bare deno_core 0.412
  `JsRuntime` (the V8 `t` already links through obscura-js) on a dedicated, lazily started OS thread `app-js`.
- `crates/app/src/helper.rs` rewritten onto `app-js`; process spawn, `--version` probe, kill-and-replace,
  `helper_path()`, `THEUTH_APP_PARSER` and the shell-script fake helpers are gone.
- `t --app-parser-hash` and `app::HASH` unchanged. Release payloads keep a `theuth-app-parser-<hash>` entry,
  now a tiny cargo-built stub (below); the updater treats the entry as optional from now on.

## Public API

`crates/app-js` (new, lib):

```rust
pub struct Bundle { pub name: &'static str, pub source: &'static str, pub global: &'static str, pub thread: &'static str }
pub static APPS: Bundle;                       // theuth-apps-inproc.js, global "theuthApps", thread "app-js"
pub struct Limits { pub deadline: Duration, pub heap_limit: usize, pub max_line: usize }   // Default: 30 s, 512 MiB, 16 MiB
pub const DEFAULT_DEADLINE: Duration;          // 30 s
pub const LOAD_DEADLINE: Duration;             // 30 s floor for building a runtime (polyfill + bundle load)
pub const DEFAULT_HEAP_LIMIT: usize; pub const DEFAULT_MAX_LINE: usize;
pub struct Reply { pub events: Vec<String>, pub reply: String }
pub enum Error { TooLarge { len, max }, Timeout, OutOfMemory, Crashed(String), Js(String), Stopped }  // is_fatal()
pub fn init_platform();                         // JsRuntime::init_platform(None); idempotent, safe in async context

pub struct BundleJs;                            // one runtime, !Send
impl BundleJs {
    pub fn new(bundle: &'static Bundle, limits: Limits) -> Result<Self, Error>;
    pub fn call(&mut self, line: &str) -> Result<Vec<String>, Error>;
    pub fn version(&mut self) -> Result<String, Error>;
    pub fn bundle(&self) -> &'static Bundle; pub fn limits(&self) -> &Limits; pub fn failure(&self) -> Option<&Error>;
    #[doc(hidden)] pub fn eval_raw(&mut self, script: &str) -> Result<(), Error>;
}
#[derive(Clone)] pub struct BundleThread;       // Send + Sync handle; thread starts on first request
impl BundleThread {
    pub fn new(bundle: &'static Bundle, limits: Limits, shutdown: CancellationToken) -> Self;
    pub fn enqueue(&self, line: String) -> Ticket; pub async fn request(&self, line: String) -> Result<Reply, Error>;
    pub fn generation(&self) -> u64; pub fn epoch(&self) -> u64; pub fn restart(&self) -> u64;
    pub fn is_running(&self) -> bool; pub fn limits(&self) -> &Limits;
    #[doc(hidden)] pub fn eval_raw(&self, script: String) -> Ticket;
    #[doc(hidden)] pub fn panic_on_thread(&self) -> Ticket;
    #[doc(hidden)] pub fn liveness(&self) -> impl Fn() -> bool + Send + Sync + 'static;
}
pub struct Ticket;   // Future<Output = Result<Reply, Error>>; epoch(); settled() -> Settled { result, rebuilt }
```

`crates/app/src/helper.rs` (surface kept for tool.rs, fence.rs, lib.rs, actor.rs, meta.rs):

```rust
pub const PENDING: usize = 8; pub const DEADLINE: Duration = 2 s; pub const MERMAID_FIRST_DEADLINE: Duration = 15 s;
pub const MAX_LINE: usize = 4 MiB;
pub struct ToolSnapshot { name, schema, risk, identity }        // unchanged
pub struct StatementInfo { id, kind }; pub struct CheckResult { … } // unchanged
pub enum Error { Busy, Timeout, Crashed(String), Protocol(String) } // Missing(PathBuf) and Hash{..} removed
pub enum MermaidBackend { FailOpen }; pub const MERMAID: MermaidBackend = FailOpen;
impl Helper {
    pub fn new(shutdown: CancellationToken) -> Self;               // was new(path, hash, shutdown)
    pub async fn check(&self, source: &str, tools: &[ToolSnapshot]) -> Result<CheckResult, Error>;
    pub async fn merge_check(&self, patch: &str, base: &str, tools: &[ToolSnapshot]) -> Result<CheckResult, Error>;
    pub async fn check_mermaid(&self, source: &str) -> Result<Vec<tools::FenceError>, Error>;
    #[doc(hidden)] pub fn canned(respond: Responder) -> Self;
}
#[doc(hidden)] pub type Responder = Arc<dyn Fn(&str) -> String + Send + Sync>;
#[doc(hidden)] pub fn install_canned(respond: Responder) -> bool;  // AppPlugin::start uses it (crates/app/tests/publish.rs)
```

Semantics kept: admission = 1 being served + `PENDING` queued (a semaphore of 9 permits held until the reply;
more → `Busy`); `DEADLINE` 2 s per request, now enforced by the runtime's watchdog (`terminate_execution`), the
isolate is rebuilt and the next request runs fresh; `MAX_LINE` 4 MiB checked before the runtime (and again in
JS by `inproc.ts`). New: the reply's `id` must be the request's; otherwise `Crashed` and `BundleThread::restart`
(the runtime that produced it is given up). Isolate heap cap 256 MiB (`HEAP_LIMIT`). Building the isolate and
loading the bundle (first request, and after a rebuild) is not counted against the 2 s.

Removed from `app`: `helper_path()`, `THEUTH_APP_PARSER`, the `platform` dependency (only the child spawn used
it), `tool.rs`'s "parser helper missing" message, the `default_helper_path_carries_exe_suffix` test.

`t` (`crates/theuth/src/main.rs`): `app_js::init_platform()` right after `browser::warmup` (synchronously in
`run`, before `Runtime::new` starts any plugin), so V8 is initialised on the main thread also when the browser
is disabled. It is `JsRuntime::init_platform(None)` (process-wide `Once`): after both branches merge, the
documents call and this one are both harmless no-ops after the first.

## Files

- `packages/apps/src/inproc.ts` (new): `globalThis.theuthApps = { call(line) -> string, version() -> string }`,
  `handle`, `MAX_LINE_BYTES`. Request parsing copied from the deleted `helper.ts`; `check_mermaid` answers
  `{id, error: "check_mermaid is not served by the in-process OpenUI validator"}`.
- `packages/apps/src/check.ts`: `SELECT_AT` (positional index of `onSelect` per component) is read from the
  generated schema (`$defs[name].properties` order = positional order) instead of `DIAGRAMS` — the import of
  `diagrams.ts` (and with it zod) is gone. Same table: verified equal for all 16 entries (Graph 5,
  StateMachine 5, Entities 5, Swimlane 6, GraphDelta 5, Waterfall 4, Funnel 5, Gantt 4, Quadrant 8, Treemap 3,
  Sankey 5, Venn 4, Loop 4, Layers 4, Journey 6, Fishbone 4) by a throwaway bun script before the change.
- `packages/apps/scripts/build-inproc.ts` (new): `Bun.build` target browser, IIFE, minified, `NODE_ENV`
  production. Guards: no `react`, `react-dom`, `@openuidev/react-ui|react-lang|react-headless`, `zustand`,
  `jsdom`, `mermaid`, `zod` package; from `src/` only `check.ts`/`inproc.ts`/`protocol.ts`; no `"node:…"`/
  `"bun:…"` string and no `require(` (one allowed exception, see globals); the library hash must be embedded.
- `packages/apps/scripts/record-inproc-golden.ts` (new) → `crates/app-js/tests/golden.json`.
- `packages/apps/test/inproc-cases.ts`, `test/inproc.test.ts` (new); deleted `src/helper.ts`,
  `scripts/build-helper.ts`, `test/helper.test.ts`. `package.json`: `build:helper` → `build:inproc`,
  `record:golden`.
- `crates/app-js/` (new): `src/{lib,runtime,thread,watchdog,tests}.rs`, `js/polyfill.js`,
  `js/theuth-apps-inproc.js`, `js/BUNDLE.sha256`, `tests/golden.rs`, `tests/golden.json`.
- `crates/app-parser-stub/` (new): the payload stub.
- `justfile`: `apps-bundle`, `apps-bundle-check`, `_apps-bundle-hash` (additive); `check` depends on
  `apps-bundle-check`; `helper` builds the stub.
- `crates/update/src/lib.rs`: app-parser entry optional.
- Packaging: `scripts/app-parser-env.sh`, `package-macos.sh`, `package-linux.sh`, `package-windows.ps1`,
  `release-s3.sh` (gate 2), `deploy/update/Dockerfile`.

## Bundle

`crates/app-js/js/theuth-apps-inproc.js`: **130,843 bytes** (minified IIFE). Contents (from the unminified
probe's module banners): `@openuidev/lang-core` (with the in-tree patch applied by `bun install`),
`ci-info` (lang-core's telemetry, behind a lazy `require_ci_info()` that only the telemetry path calls),
`generated/library.json`, `src/check.ts`, `src/inproc.ts` (`protocol.ts` is type-only). No zod, no
diagrams.ts. `BUNDLE.sha256`: `inputs` = hash over `packages/apps/{src/**,generated/*,patches/*}`,
`package.json`, `bun.lock`, `tsconfig.json`, `scripts/build-inproc.ts`; `bundle` = the committed file.
A cargo build needs no bun.

## Globals and polyfill

What the bundle needs beyond ECMAScript, measured by `test/inproc.test.ts` (bun `node:vm` context; the test
asserts the context has no `TextEncoder`/`TextDecoder`/`structuredClone`/`fetch`/`process`/`URL`/`crypto`/`atob`):

- **`TextEncoder`** — the only one. With nothing but `TextEncoder` injected (no polyfill at all) all 36 cases
  match direct `call()`; without it the UTF-8 caps throw.
- `console` — present in bun's vm contexts; deno_core's bare runtime has its own; the polyfill installs a
  stderr one only if absent.
- lang-core's opt-in runtime telemetry (`OPENUI_RUNTIME_TELEMETRY_ENABLED`) needs `process`, `fetch`,
  `crypto.randomUUID`, `AbortSignal.timeout`: none of `process`/`fetch`/`AbortSignal` exists in the isolate, so
  it returns before doing anything (asserted: no `fetch` call over all cases). It contains the string
  `getBuiltinModule?.("node:child_process")` (a lookup on a `process` object, not an import) — the only
  `node:` string the build guard allows.

`crates/app-js/js/polyfill.js` (copied from document-js, op renamed `op_appjs_random_fill`): `crypto`
(`getRandomValues`, `randomUUID`), `atob`/`btoa`, `setTimeout`/`setInterval`/`clearTimeout`/`clearInterval`
(deno_core timers), plus new `TextEncoder`, `TextDecoder` (UTF-8, non-fatal), `structuredClone` (plain data,
cycles), `console` (if absent). Everything beyond `TextEncoder` is for later bundles (mermaid) and parity with
document-js.

## Mermaid seam

*Superseded by Phase 2 (end of this note): the seam is gone, `check_mermaid` runs on its own isolate.*

`crates/app/src/helper.rs`:

```rust
pub enum MermaidBackend { FailOpen }
pub const MERMAID: MermaidBackend = MermaidBackend::FailOpen;
pub async fn check_mermaid(&self, _source: &str) -> Result<Vec<tools::FenceError>, Error> {
    match MERMAID { MermaidBackend::FailOpen => Ok(Vec::new()) }
}
```

Phase 1 returns "no errors", exactly what `fence.rs` (`Mermaid` checker) produced before on any helper failure
(fail-open). To plug the mermaid worker in: (1) add a bundle `pub static MERMAID: app_js::Bundle` in `app-js`
(its own script, global, thread name e.g. `mermaid-js`); (2) add a variant `MermaidBackend::Js`, a second
`app_js::BundleThread` field on `Helper` created in `Helper::new` with its own `Limits` (deadline
`MERMAID_FIRST_DEADLINE` for the first request — or raise `LOAD_DEADLINE`-style construction time, since
bundle load is already outside the request deadline — then `DEADLINE`; a larger heap); (3) in the new arm send
`protocol.ts` `MermaidRequest` `{id, op:"check_mermaid", source}` and map `MermaidResponse`
(`{ok:true}` → `[]`; `{ok:false, errors:[{line, message}]}` → `FenceError { path: "line:N" or "", message }`),
with the same id matching and admission as `send`; (4) set `MERMAID` to the new variant. Nothing outside
`helper.rs`/`app-js` changes; `fence.rs` stays as is. `src/mermaid-check.ts`, the `mermaid` and `jsdom`
dependencies are untouched in this phase.

## Updater stub

What installed (old) updaters do with the entry (`crates/update/src/lib.rs` `unpack`/`install`/`probe` before
this branch): require exactly one regular file named `theuth-app-parser-<64 lowercase hex>`; write it beside
`t` as `<name>.new.<pid>-<nonce>`; `chmod 755` (`platform::files::set_executable`); run `t.new
--app-parser-hash` (the NEW `t`, never the helper) and require its output to equal the hash in the entry's
*name*; rename into place unless a file of that name already exists (then the existing one is kept). They never
execute the helper, never check its content or signature (the tarball's signature is verified as a whole).
The release gates (`release-s3.sh` 7/8, `app-parser-env.sh` `check_helper`, Dockerfile, package-windows.ps1)
run `<entry> --version` (must contain the hash) and `printf '' | <entry>` (must exit 0); gate 2 requires a
Developer ID signature on macOS; the Homebrew formula asserts `--version` names the hash.

Design: `crates/app-parser-stub`, a zero-dependency Rust binary `theuth-app-parser-stub`, copied into payloads
as `theuth-app-parser-<hash>[.exe]`. `--version`/`-V` prints `theuth-app-parser <hash>` (compiled in from
`packages/apps/generated/library-hash.txt`, the same file `app::HASH` embeds); with no arguments it answers each
stdin line with `{"id":null,"error":"theuth-app-parser is a compatibility stub: …"}` and exits 0 at EOF; other
arguments exit 2. Built by cargo in every packaging path (no Bun): `app-parser-env.sh` `build_helper`
(`cargo build --release -p app-parser-stub`, `HELPER_SRC=target/release/theuth-app-parser-stub`),
`deploy/update/Dockerfile` (with `t`), `package-windows.ps1`. macOS signs it like any nested Mach-O with
hardened runtime and no entitlements (it needs no JIT); gate 2 checks the JIT entitlement on the documents
helper only. `entitlements-app-parser.plist` stays: the documents helper on main still uses it.

Why a Mach-O/PE stub, not a shell script: a script in `Contents/MacOS` of a signed, notarized app is signed
through extended attributes that tar/brew do not reliably carry, and Windows payloads need a `.exe`. A
compiled stub is the conservative choice; it is 543,624 bytes as a debug build (release size not measured).

Who could run the stub: only an older `t` whose library hash equals this release's and whose own helper file
is missing — the updater keeps an existing helper of the same name, so an old `t` updated in place keeps its
real Bun helper. Such a `t` gets an immediate protocol error per request (apps unavailable), never a hang.

New updater (this branch): the entry is optional — a payload without it installs without the probe; with it,
behaviour is unchanged. **To drop the stub in a later release**: wait until every supported install runs an
updater that contains this change (i.e. the oldest updater that may apply the release is at least this
branch's release), then remove `app-parser-stub` from packaging (`app-parser-env.sh` `HELPER*`/`build_helper`/
`check_helper`, Dockerfile, package-macos/linux/windows, package-update-macos.sh's `[[ -x … $HELPER ]]`,
stage-linux.sh, setup-ubuntu-build.sh, release-s3.sh gates 2/7/8 expected listings, the tap formula's
`bin.install Dir["theuth-app-parser-*"]` and its test, install-macos.sh's loop), the crate, and — once no
supported updater probes it — `t --app-parser-hash` can stay (cheap) or go.

## Containment tests (crates/app-js/src/tests.rs)

Each guard is followed by a normal check + merge_check round trip on the same handle:
JS exception → `js_exception_is_an_error_reply_and_the_runtime_survives`; deadline →
`deadline_terminates_a_runaway_script_and_rebuilds` (raw spin, queued job fails with it, and a spinning entry
point); heap limit → `heap_limit_terminates_and_rebuilds`; panic in an op → `panic_in_an_op_is_contained`;
panic on the thread → `panic_on_the_thread_fails_pending_jobs_and_a_fresh_runtime_serves`; oversized line →
`oversized_line_is_rejected_before_js`; plus `thread_starts_lazily`, `dropping_the_last_handle_ends_the_thread`,
`cancelling_the_token_ends_the_thread`, `a_dead_runtime_thread_is_respawned`,
`an_event_loop_error_mid_request_is_fatal_and_the_next_request_gets_its_own_reply`,
`a_request_that_ends_without_its_reply_is_fatal`, `restart_fails_the_queued_requests_and_the_next_one_runs_fresh`,
`two_handles_run_two_isolates`, `init_platform_is_safe_inside_an_async_context`. Short test deadline 1.5 s;
bundle load runs under `LOAD_DEADLINE` (30 s) so slow debug builds do not trip it. Golden replay:
`tests/golden.rs` (36 cases, byte-for-byte). `crates/app/src/helper.rs` tests run the real bundle (check,
merge, oversize, 2 s deadline + recovery, foreign reply id + rebuild, Busy at PENDING+1, shutdown, mermaid
fail-open) and one canned responder (error lines).

## Parity

- `bun test` in packages/apps: before 226 pass / 0 fail (24 files); after 229 pass / 0 fail (24 files:
  helper.test.ts −, inproc.test.ts +). check.test.ts 24 tests and bounds.test.ts 20 tests (the brief said
  28/22; these are the counts bun reports on this checkout) pass unchanged.
- Bundle in a bare `node:vm` context with only the polyfill: 36 cases (23 `check` cases and 5 `merge_check`
  cases ported from check.test.ts, the fixtures.json program and both of its edit patches, 6 protocol-error
  lines) equal direct `call()` byte for byte; the same transcript, recorded from the TypeScript, is replayed in
  deno_core by `crates/app-js/tests/golden.rs`.

## Deviations

- `helper::Error::Missing` and `Error::Hash` removed (cannot happen in-process); `Helper::new` lost its `path`
  and `hash` arguments. Callers updated (lib.rs, tool.rs). The plugin uses `Helper::for_plugin`, which honours
  a test-installed canned responder (`install_canned`, `#[doc(hidden)]`) — the replacement for the
  `THEUTH_APP_PARSER` fake in `crates/app/tests/publish.rs`.
- `app_js::Error` messages say "in-process JS runtime …"; `helper::Error::Crashed` wraps them.
- The pump's error line for a throwing entry point is `{id, ok:false, error}` with `id` string or number (the
  documents pump only kept numeric ids).
- `BundleJs::new` loads under `max(deadline, LOAD_DEADLINE)`; document-js uses `max(deadline, DEFAULT_DEADLINE)`
  (both 30 s; separate constant here because the app deadline is 2 s).
- The `invalid JSON: …` protocol error now carries V8's `JSON.parse` message instead of JavaScriptCore's
  (Bun); every other response is byte-identical (golden replay, 36 cases). The Rust side never sends invalid
  JSON, so this text is unobservable in practice.
- Wire/RPC types and `crates/theuth/tests/fixtures/wire/openrpc-v1.json` untouched. Vestigial wire fields:
  none on the RPC surface; in `protocol.ts` the `MermaidRequest`/`MermaidResponse` types remain for the
  mermaid phase (unused by Rust in phase 1).

## Duplication with document-js and how to unify

`crates/app-js/src/{runtime,thread,watchdog}.rs` and `js/polyfill.js` are copies of
`crates/document-js` at feat/documents-inproc **0ca80bbf3** (the tip when this work started, including the
`init_platform(None)` fix, reply/request matching, fatal request-without-reply, epochs/restart and respawn of a
dead thread), with mechanical renames (`DocumentsJs`→`BundleJs`, `DocumentsThread`→`BundleThread`,
`op_documents_*`→`op_appjs_*`) and one parameter added: `&'static Bundle` (script, global, thread name), threaded
through `BundleJs::new`, `BundleThread::new`, `build`, `serve`. File and function structure is otherwise the
same. After both branches merge: keep `app-js`'s generic form as one crate (rename to e.g. `js-runtime`), make
document-js's bundle a `Bundle { global: "theuthDocuments", thread: "documents-js", … }`, delete the
duplicated runtime/thread/watchdog files, keep one polyfill (this one is a superset; op name unified), and keep
one `init_platform` call in `main.rs` (both are the same idempotent `JsRuntime::init_platform(None)`).
One V8: `deno_core = "0.412"` exactly as obscura-js resolves it (see verification for `cargo tree -i v8`).

## Deletion list for when mermaid is in-process

*Phase 2 status at the end of this note.*

`packages/apps`: `src/mermaid-check.ts` once its logic lives in the mermaid bundle entry (or keep it as that
entry's import), `jsdom` dependency if the mermaid bundle does not use it, `test/jsdom.d.ts`;
`crates/app/src/helper.rs`: `MermaidBackend::FailOpen` (or keep as an off switch); `inproc.ts`'s
`check_mermaid` refusal stays valid (the OpenUI bundle never serves it). With mermaid in-process and the stub
dropped (see above), nothing of the Bun helper remains.

## Verification

All with `CARGO_TARGET_DIR=<worktree>/target CARGO_BUILD_JOBS=4 --offline`, 2026-11-21:

- `bun test` (packages/apps): before 226 pass / 0 fail, after 229 pass / 0 fail, 24 files each.
- `just apps-bundle-check`: ok. `just --list`: ok. `bash -n` on app-parser-env.sh, package-macos.sh,
  package-linux.sh, package-update-macos.sh, stage-linux.sh, setup-ubuntu-build.sh, release-s3.sh,
  install-macos.sh: ok. `tsc --noEmit` (packages/apps): exit 0.
- `cargo test -p app-js`: unit `18 passed; 0 failed`, golden `1 passed; 0 failed`.
- `cargo test -p app -p update`: app unit `49 passed`, `tests/publish.rs` `1 passed`, update unit `5 passed`,
  0 failed.
- `cargo tree -i v8 -p app-js` and `-p theuth`: one `v8 v150.4.0` via `deno_v8 0.4.0` / `deno_core 0.412.0`,
  shared with `obscura-js` in `theuth`.
- `cargo check -p theuth`: `Finished`, exit 0 (no warnings from changed crates).
- `cargo clippy -p app -p app-js -p app-parser-stub -p update --all-targets --no-deps`: clean, exit 0. Without
  `--no-deps` clippy stops in unrelated workspace dependencies (`voice`: `chunks_exact_to_as_chunks`; with that
  allowed, `talk`: `single_match` in `crates/talk/src/rpc_hub.rs:50`), both pre-existing on main.
- `cargo build -p app-parser-stub` (debug, 543,624 bytes): `--version` → `theuth-app-parser <hash>` exit 0;
  empty stdin → exit 0; one request line → the error line, exit 0; `--bogus` → exit 2.

Not verified: any packaging/release/install/Docker run (not run by rule); `package-windows.ps1` (read only,
not executed; no PowerShell parse check); the stub inside a signed/notarized app bundle; the site docs build;
`t` end to end with a real model (the app tool path is covered by the crate tests above).

Build note: the seeded `target/` carried CMake caches from another checkout; `opusic-sys` and `btls-sys`
needed `cargo clean -p` (crate-scoped) before `cargo test -p app -p update` built.

# Phase 2: mermaid (2026-11-21)

The fail-open seam is replaced by the real thing: `check_mermaid` runs the parse-only mermaid bundle in a
second isolate. Based on the sibling spike `spike/apps-mermaid-inproc` @ 24f8fbc2c (its doc is now
`docs/design/apps-mermaid-inproc-spike.md`); its TS side was brought over, its Rust crate was not (`app-js`
serves both bundles).

## API

- `app_js::MERMAID: Bundle` — `js/theuth-mermaid-inproc.js`, global `theuthMermaid` (`call(line) → Promise<string>`,
  `version = "theuth-mermaid 1"`), thread `mermaid-js`. The pump already drove the event loop until the reply
  op fired (`Promise.resolve(entry.call(line))`), so the async entry needed no runtime change; the sync APPS
  entry behaves as before.
- `app_js::Limits` gains `first_deadline`: the first request on each runtime runs under
  `max(first_deadline, deadline)`, later ones under `deadline` (bundle load stays under `LOAD_DEADLINE`, 30 s).
  APPS sets it equal to `DEADLINE` (2 s), so its behaviour is unchanged.
- `app::helper`: `MermaidBackend` and `MERMAID` (the seam) are deleted. New consts `MERMAID_DEADLINE = 5 s`,
  `MERMAID_FIRST_DEADLINE = 10 s` (redefined: was 15 s for the jsdom child); `HEAP_LIMIT = 256 MB` and
  `MAX_LINE = 4 MiB` for both isolates. `Helper::check_mermaid(&self, source) -> Result<Vec<FenceError>, Error>`
  unchanged: `{ok:true}` → `[]`, `{ok:false, errors}` → `FenceError { path: "line:N" | "", message }`.
  `Helper::canned` answers both ops with the same responder. `fence.rs` is unchanged and still fails open on
  any `Err` (agent `check_fences`).

## Queue decision

**Separate lanes.** Each isolate has its own admission semaphore of `PENDING + 1` (one served + 8 queued);
`check`/`merge_check` and `check_mermaid` never share slots, and they run on different threads, so neither
queued mermaid work nor a 5 s mermaid parse can delay or `Busy` a `check`
(test `a_busy_mermaid_isolate_never_delays_check`). Cost: up to 18 in-flight parser requests in total instead
of 9; both bounds are per isolate, which is what protects each deadline.

## Bundle, globals

`crates/app-js/js/theuth-mermaid-inproc.js`: **2,860,387 bytes** (minified IIFE, render-only libraries elkjs,
cytoscape(+fcose, cose-bilkent), katex, roughjs, @upsetjs/venn.js stubbed; dompurify → identity shim), byte-
identical to the spike's. `BUNDLE.sha256` now has four lines: `inputs`/`bundle` (OpenUI, unchanged scheme) and
`mermaid-inputs` (packages/apps `src/**`, `patches/*`, `package.json`, `bun.lock`, `tsconfig.json`,
`scripts/build-inproc-mermaid.ts`) / `mermaid-bundle`. `just apps-bundle` builds both; `apps-bundle-check`
checks both. Polyfill additions: `URL` (constructor throws, `canParse` → false) and `structuredClone` via
deno_core's `core.structuredClone` (V8 serializer, isolate realm) with the JS fallback kept for the bun vm
tests. Mermaid's needs: `TextEncoder`/`TextDecoder`, `structuredClone`, `URL`; timers referenced, not called.

## packages/apps changes

- New: `src/inproc-mermaid.ts`, `src/mermaid-core.ts` (error formatting shared with the reference path),
  `src/inproc/{dompurify-shim,stub}.ts`, `scripts/build-inproc-mermaid.ts`, `test-inproc/mermaid-corpus/`
  (`valid.ts`, `corpus.ts`, `reference.ts`, `golden.json`), `test/inproc-mermaid.test.ts`.
- `src/mermaid-check.ts` (the jsdom path) moved to `test-inproc/mermaid-corpus/jsdom-check.ts`: it is now only the
  reference `reference.ts` records `golden.json` from (`warmMermaid` deleted). The spike's `parity-vm.ts` is
  replaced by `test/inproc-mermaid.test.ts` (same comparison, inside `bun test`).
- `jsdom` moved to devDependencies (still used by `test/dom.ts`, the reference recorder, and — through this
  package's node_modules — the desktop UI's tests); `bun.lock`'s workspace block edited to match (a plain
  `bun install` also re-resolved iconv-lite 0.7.3 → 0.6.3, so that resolution was discarded;
  `bun install --frozen-lockfile` accepts the edited lock). `mermaid` stays a dependency. Both bundles
  rebuilt byte-identical after the change.

## Tests

- `crates/app-js/tests/mermaid_golden.rs`: all 148 corpus sources through `BundleThread(&MERMAID)` against the
  jsdom-recorded `golden.json`: 147 exact, `edge/timeline-typeerror` matched on `ok` + `line` (V8's TypeError
  text differs), one runtime for the whole corpus.
- `crates/app-js/src/tests.rs`: `mermaid_bundle_answers_asynchronously_on_its_own_lazy_thread`;
  `a_request_sent_after_a_fatal_reply_runs_fresh_and_the_ones_queued_before_fail` — the spike's ordering fix.
  This copy already had the guarantee: the thread moves the epoch on *before* sending the failing job's reply,
  so jobs queued before it fail without running (with the same error) and a job enqueued after the reply is
  seen runs on the fresh runtime; the test pins it.
- `crates/app/src/helper.rs`: `check_mermaid_runs_on_its_own_lazily_started_isolate` (no mermaid isolate
  until the first fence; generation 1 on both), `a_busy_mermaid_isolate_never_delays_check` (mermaid thread
  blocked 3 s with a full lane → next mermaid request `Busy`, five `check`s answered < 1.5 s),
  `mermaid_failures_are_contained_on_their_own_isolate` (runaway → Timeout at 5 s; heap bomb → OutOfMemory at
  256 MB; hostile diagrams from the spike: `%%{init}%%` nested 200k, YAML front matter nested 100k,
  `flowchart LR A((((…1M`, sequence 100k messages, pie + 1M `"`; each ends as an answer, `Timeout`, or the
  heap ceiling — pie + 1M `"` reached 256 MB under the 5 s deadline here (the spike's 2 s deadline stopped it
  first) — and each is followed by a successful parse AND a successful `check`; the OpenUI
  isolate stays generation 1).
- `crates/app/src/fence.rs`: `mermaid_fences_are_checked_by_the_in_process_bundle` (the `Mermaid` fence checker
  over a real `Core`).
- `packages/apps/test/inproc-mermaid.test.ts`: the committed bundle + polyfill in a bare `node:vm` context,
  147/148 exact against `golden.json`, the same engine-wording case matched on ok + line.

## Deviations (phase 2)

- Bundle load is under `LOAD_DEADLINE` (30 s), not the 10 s first-call deadline the spike used for "runtime
  build + first request": construction is outside every request deadline in `app-js`. The first request
  itself gets 10 s.
- `helper::Error::Timeout`'s text still says `DEADLINE` (2 s) also for a mermaid timeout; fence checks fail
  open, so the text is not shown to the model.
- The spike's runtime differences (no epochs, two deadlines inside the runtime) were not taken: `app-js` keeps
  document-js's epochs and gets `first_deadline` instead.

## Deletion list after phase 2

Done: the seam, `mermaid-check.ts` from `src/`, `jsdom` from runtime dependencies. Left: `test/jsdom.d.ts` and
the jsdom devDependency (tests); the `theuth-app-parser-<hash>` payload stub (see "Updater stub");
`protocol.ts`'s `MermaidRequest`/`MermaidResponse` are now used by the mermaid bundle, not vestigial.

## Verification (phase 2)

Same environment as phase 1, 2026-11-21:

- `bun test` (packages/apps): 231 pass / 0 fail, 25 files (229 before + 2 in `test/inproc-mermaid.test.ts`).
  `tsc --noEmit`: exit 0. `just apps-bundle-check`: ok (both bundles). Both bundles rebuild byte-identical.
- `cargo test -p app-js`: unit `20 passed; 0 failed`, `tests/golden.rs` `1 passed`, `tests/mermaid_golden.rs`
  `1 passed` (148 cases, 147 exact + 1 engine wording, 1.08 s).
- `cargo test -p app -p update`: app unit `52 passed; 0 failed`, `tests/publish.rs` `1 passed`, update `5 passed`.
  (A first run failed `mermaid_failures_are_contained_on_their_own_isolate` because pie + 1M quotes ended at
  the heap ceiling, not in a Timeout; the test now accepts that contained outcome.)
- `cargo check -p theuth`: Finished, exit 0.
- `cargo clippy -p app -p app-js -p app-parser-stub -p update --all-targets --no-deps`: clean.
- `cargo tree -i v8 -p app-js`: one `v8 v150.4.0` via `deno_core 0.412.0`.

Not verified (phase 2): RSS of `t` with and without a mermaid fence (the spike measured ~60 MB after load,
~110 MB after every diagram type, in its own harness); a real model reply with mermaid fences through
`agent::loop_::check_fences`; release-build timings; packaging (not run by rule).

# Review fixes (2026-11-21)

Commit `cacd530b7` unless noted. Results of the verification commands are at the end of this section.

- **M1 — library hash vs embedded bundle: fixed.** Verified against the code first: before this, only
  `cargo test -p app` (`check_and_merge_check_against_the_real_validator` compares `library_hash`) and
  `just check` (bundle input hash) caught a mismatch. Now (a) app test
  `helper::tests::the_bundle_embeds_this_binarys_library_hash` asserts the bundle's `version()` is
  `theuth-app-parser <app::HASH>`; (b) `scripts/apps-bundle-hash.sh` (bash + shasum, no bun) is the one
  implementation behind `just apps-bundle-check`, `app-parser-env.sh` `check_apps_bundle` (first step of
  `build_helper`, so every packaging path that builds the stub checks the bundles; it also greps the bundle for
  the hash) and `release-s3.sh` gate 0; `package-windows.ps1` only does the plain-text check (bundle file
  contains `library-hash.txt`'s hash) with a marked TODO for porting the full input hash; (c) `crates/app/build.rs`
  fails `cargo build` of anything depending on `app` (so `t`) when the committed OpenUI bundle does not contain
  the hash from `library-hash.txt`. Not covered: the Dockerfile path builds the stub without `build_helper`; the
  build.rs check still applies there (it builds `t`). Unverified: `shasum` availability on the Ubuntu build box
  (perl's `shasum`; `setup-ubuntu-build.sh` calls `build_helper`).
- **M2 — first isolate off the main thread: fixed, segfault not reproduced.** `app_js::init_platform()` now
  initialises the platform and builds + drops one bare `JsRuntime` on the calling thread, once (`Once`). Inside
  an async context (where `t` calls it) the isolate is built under the current tokio handle, as
  `browser::warmup` does with obscura's runtime; outside one, a throwaway current-thread runtime is entered and
  disposed with `shutdown_background()` (no runtime is dropped in an async context). A process-wide mutex
  (`runtime.rs` `isolate_lock`) is held only around `JsRuntime::new` in `BundleJs::new` and `init_platform`;
  obscura's own isolate-creation lock is private to obscura-js and cannot be shared, so the two are independent
  and the ordering comes from `init_platform` running on the main thread before any plugin starts. Tests:
  `init_platform_builds_an_isolate_inside_a_multi_thread_async_context`,
  `..._inside_a_current_thread_async_context`, `..._on_a_plain_thread`. Reproduction: not attempted with a real
  `t` (it needs a full `cargo build -p theuth` binary and a run with the browser disabled, which I did not do);
  note that the app-js test binaries build their first isolate off the main thread in every run without a
  crash, so the segfault is not reproduced in that setting — the `t` scenario is unverified either way.
- **m1 — one runaway fails everything queued: fixed.** `thread.rs` no longer fails older-epoch jobs: after a
  fatal failure only the executing job fails; queued jobs run on the rebuilt runtime (both bundles are stateless).
  Epochs still guarantee that no job runs on, and no reply comes from, a given-up runtime (`restart` still
  discards the running job's output). Tests: `one_runaway_does_not_fail_the_checks_queued_behind_it`
  (1 runaway + 5 queued checks → Timeout, then 5 OK), and the updated
  `deadline_terminates_a_runaway_script_and_rebuilds`, `panic_on_the_thread_fails_that_job_and_a_fresh_runtime_serves_the_queue`,
  `restart_fails_the_running_request_and_the_queued_ones_run_fresh`,
  `after_a_fatal_failure_no_job_runs_on_the_abandoned_runtime`.
- **m2 — stubbed render-only library turns a valid diagram invalid: fixed.** `src/inproc/stub.ts` throws
  `RenderOnlyUnavailable` carrying the `RENDER_ONLY_UNAVAILABLE` symbol (`Symbol.for`); `mermaid-core.ts`
  `answer` rethrows it, so `call` rejects, the pump answers `{id, ok:false, error}`, `helper.rs` maps that to
  `Error::Protocol` and `fence.rs` fails open. Test: `test/inproc-mermaid.test.ts` "render-only stub" (a fake
  parser calling into the stub → `answer` rejects; `use` stays a no-op; a real parse error is still a verdict).
  Mermaid bundle rebuilt: 2,860,655 bytes; corpus still 147 exact + 1 engine wording in the bun vm run and in
  `crates/app-js/tests/mermaid_golden.rs` (see results).
- **m3/m4 — notes, not implemented.** (i) The stub and the old Bun helper share the file name
  `theuth-app-parser-<hash>` while the library hash is unchanged: an updater keeps an existing file of that
  name, so a later *old-style* (Bun-helper) payload at a higher version would leave the stub in place and that
  `t` would get protocol errors from it. Any re-cut of an old-style release must bump the library hash. (ii)
  Existing installs keep their ~71 MB Bun helper beside `t` forever: the updater never sweeps old helpers.
  Follow-up (not done here): a sweep from the newest binary (the `t --gc-helpers` idea in `crates/update`).
- **m5 — over-broad bundle inputs: fixed.** `scripts/build-inproc.ts` / `build-inproc-mermaid.ts` write
  `<bundle>.inputs` (package-local files from the unminified build's module banners, via
  `scripts/bundle-inputs.ts`), committed in `crates/app-js/js/`. The hashes cover exactly those files plus
  `package.json`, `bun.lock`, `tsconfig.json`, `patches/*`, the build scripts and (OpenUI only) `generated/*`.
  OpenUI: `generated/library.json`, `src/check.ts`, `src/inproc.ts`; mermaid: `src/inproc-mermaid.ts`,
  `src/inproc/dompurify-shim.ts`, `src/inproc/stub.ts`, `src/mermaid-core.ts`. Type-only imports
  (`protocol.ts`) emit no code and are not inputs. `apps-bundle-check` still needs only shasum.
- **m6 — SELECT_AT parity: fixed.** `test/select-at.test.ts` imports `SELECT_AT` (now exported from
  `check.ts`) and `DIAGRAMS` and asserts equality (16 entries).
- **m7 — Timeout text: fixed.** `helper::Error::Timeout(Duration)` carries the isolate's warm deadline (2 s
  OpenUI, 5 s mermaid); test `timeout_text_names_the_isolates_own_deadline`. The first request of a mermaid
  runtime runs under 10 s but would still be reported with 5 s (the warm deadline).
- **m8 — HELPER_SRC: fixed.** `"${CARGO_TARGET_DIR:-target}/release/theuth-app-parser-stub"`.
- **m9 — stale text: fixed** in `app-parser-env.sh`, `package-windows.ps1` (stale message, Build-Helper
  comment, THEUTH_SKIP_BUN_INSTALL), `entitlements-app-parser.plist` (now documents the documents helper; the
  stub is signed without it), `install-macos.sh`, `package-linux.sh` (ldd comment, THEUTH_SKIP_BUN_INSTALL),
  `package-macos.sh` (THEUTH_SKIP_BUN_INSTALL), `packages/apps/src/index.ts`, `docs/plugins.md`,
  `docs/update-setup.md` step 2, site `development.mdx` (packages table, `just helper`/`apps-bundle`/`install`),
  `installation.mdx` (§ "Two things", helper versioning), `cli.mdx` (`--app-parser-hash`); `46-apps.md` got a
  dated marker before the pre-in-process "As built" paragraph.

## Review fixes: verification

- `bun test` (packages/apps): 233 pass / 0 fail, 26 files (+ `select-at.test.ts`, + the stub test).
  `tsc --noEmit`: exit 0. `just apps-bundle-check`: ok; `check_apps_bundle` from `app-parser-env.sh`: ok.
  `bash -n` on app-parser-env.sh, release-s3.sh, install-macos.sh, package-linux.sh, package-macos.sh,
  apps-bundle-hash.sh: ok; `just --list`: ok. package-windows.ps1: read only, unverified.
- `cargo test -p app-js`: unit `23 passed; 0 failed`, golden `1 passed`, mermaid_golden `1 passed` (corpus
  147 exact + 1 engine wording with the rebuilt mermaid bundle). The first run after the m1 change failed 5
  tests: a re-run queued job was still judged by its *enqueue* epoch and had its output replaced by the
  offender's error; fixed in `95f7518a8` (judge by the epoch the job started in), and the `restart` test now
  restarts while the busy job is provably running.
- `cargo test -p app -p update`: app unit `54 passed; 0 failed` (incl. `the_bundle_embeds_this_binarys_library_hash`,
  `timeout_text_names_the_isolates_own_deadline`; `build.rs` ran), publish `1 passed`, update `5 passed`.
- `cargo check -p theuth` and `cargo clippy …`: see the final report (lock contention at the time of writing).
