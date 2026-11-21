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
compiled stub is the conservative choice; it is a few hundred KB (release build size not measured).

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

`packages/apps`: `src/mermaid-check.ts` once its logic lives in the mermaid bundle entry (or keep it as that
entry's import), `jsdom` dependency if the mermaid bundle does not use it, `test/jsdom.d.ts`;
`crates/app/src/helper.rs`: `MermaidBackend::FailOpen` (or keep as an off switch); `inproc.ts`'s
`check_mermaid` refusal stays valid (the OpenUI bundle never serves it). With mermaid in-process and the stub
dropped (see above), nothing of the Bun helper remains.

## Verification

Results are in the final report of this work (commands, `test result:` lines). Not verified here: any
packaging/release/install/Docker run (not run by rule); `package-windows.ps1` (read only, not executed);
`crates/update` tests (changed; see the report for whether they ran); the site docs build.
