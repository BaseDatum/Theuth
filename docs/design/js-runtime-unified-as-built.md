# In-process JS runtime unified — as built

Branch `feat/js-runtime-unified` (from main ed0257692), 2026-11-22. `crates/document-js` (documents helper) and
`crates/app-js` (OpenUI check/merge + mermaid parse) were copies of one runtime; they are now one crate,
`crates/js-runtime` (package `js-runtime`), and both old crates are deleted. History:
[documents P1](documents-inproc-as-built-P1.md)–[P3](documents-inproc-as-built-P3.md),
[apps P1](apps-inproc-as-built-P1.md) ("Duplication with document-js and how to unify" is what this does).

## API (`js_runtime`)

```rust
pub struct Bundle {                  // one JS program and how it is guarded
    pub name: &'static str,          // script name in stack traces
    pub source: &'static str,        // include_str! of the committed bundle
    pub global: &'static str,        // `{ call(line) -> string | Promise<string>, version }`
    pub thread: &'static str,        // OS thread name (also in panic messages)
    pub web_polyfill: bool,          // load js/polyfill-web.js after js/polyfill.js
    pub after_fatal: AfterFatal,     // FailQueued | RerunQueued
    pub limits: Limits,              // today's limits; callers pass them to JsThread::new
}
pub enum AfterFatal { FailQueued, RerunQueued }
pub const DOCUMENTS: Bundle; pub const APPS: Bundle; pub const MERMAID: Bundle;
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits { pub deadline: Duration, pub first_deadline: Duration, pub heap_limit: usize, pub max_line: usize }
pub const DEFAULT_DEADLINE, LOAD_DEADLINE: Duration;   // 30 s, 30 s
pub const DEFAULT_HEAP_LIMIT, DEFAULT_MAX_LINE: usize; // 512 MiB, 16 MiB
pub struct Reply { pub events: Vec<String>, pub reply: String }
pub enum Error { TooLarge { len, max }, Timeout, OutOfMemory, Crashed(String), Js(String), Stopped } // + is_fatal()

pub struct JsRuntime;   // !Send: new(&'static Bundle, Limits), call(&str) -> Vec<String>, version(), bundle(),
                        // limits(), failure(), #[doc(hidden)] eval_raw
pub struct JsThread;    // Send + Clone: new(&'static Bundle, Limits, CancellationToken), enqueue(String) -> Ticket,
                        // request(String), generation(), epoch(), restart(), is_running(), bundle(), limits(),
                        // #[doc(hidden)] eval_raw / panic_on_thread / liveness
pub struct Ticket;      // Future<Output = Result<Reply, Error>>; epoch(), settled() -> Settled, #[doc(hidden)] ready()
pub struct Settled { pub result: Result<Reply, Error>, pub rebuilt: bool }
pub fn init_platform(); // V8 init + one throwaway isolate on the calling thread under the creation lock; Once
```

Bundles are `const` so consumers derive their constants from them (`document::helper::DEADLINE =
DOCUMENTS.limits.deadline`, `app::helper::{DEADLINE, MERMAID_DEADLINE, MERMAID_FIRST_DEADLINE, MAX_LINE}`);
`&DOCUMENTS` etc. are promoted to `'static`.

Runtime behaviour is app-js's (the generic copy), unchanged: one isolate and one lazily started thread per
`JsThread` (so per bundle in `t`); watchdog deadline (`terminate_execution` from one process-wide
`js-runtime-watchdog` thread); heap cap + near-heap-limit callback (64 MB headroom, then terminate);
panics caught in our ops (`guarded`) and on the thread (`catch_unwind` around build/run/drop); line cap before
JS (`enqueue` and `JsRuntime::serve`); per-request request/reply slots (no reply leaks to the next request);
epochs + generation; a request that ends without its reply or an event-loop error is fatal; a dead thread is
respawned by the next request; the first request of each runtime runs under `max(first_deadline, deadline)`;
loading runs under `max(deadline, LOAD_DEADLINE)`.

What differed between the two copies, and where it went:

| difference | now |
|---|---|
| bundle/global/thread parameter (app-js only) | `Bundle` for all three |
| `Limits::first_deadline` (app-js only) | in `Limits`; DOCUMENTS sets it equal to `deadline` (= no first-request extension, as before) |
| queued requests after a fatal failure: fail (document-js) vs re-run (app-js, review m1) | `Bundle::after_fatal`: DOCUMENTS `FailQueued` (its runtime held the document mirrors), APPS/MERMAID `RerunQueued`; only the executing request fails in the latter. Both paths are the old code, selected by one `if` in `thread::serve` |
| promise replies (MERMAID) | one pump for all: `Promise.resolve(entry.call(line))`, event loop driven until the reply. A sync/promise switch per bundle was not added: APPS' containment tests (event-loop error, request without reply) install promise-returning entries, which a "sync" pump would answer with `[object Promise]` |
| `init_platform`: document-js `JsRuntime::init_platform(None)` only; app-js also builds one bare isolate on the calling thread under a process-wide creation lock (M2) | app-js's, `Once`-guarded; `t` calls `js_runtime::init_platform()` once (`crates/theuth/src/main.rs`); `document::helper::init_js_platform` (only caller was `main.rs`) is deleted |
| ops `op_documents_*` / `op_appjs_*` | `op_jsrt_take_request`, `op_jsrt_reply`, `op_jsrt_random_fill` (+ `op_jsrt_test_panic` in unit tests) |
| polyfill: app-js's = document-js's + TextEncoder/TextDecoder, structuredClone, URL, console | shared base `js/polyfill.js` + supplement `js/polyfill-web.js` loaded for `web_polyfill` bundles (APPS, MERMAID). Not one superset file: the documents bundle `typeof`-probes `TextEncoder` (lib0 `typeof TextEncoder<"u"?new TextEncoder:null`, twice), so defining it would switch the documents' UTF-8 path from lib0's fallback to our encoder (lone surrogates: lib0's fallback throws, the polyfill encodes U+FFFD) — not inert |
| error wording "documents runtime …" vs "in-process JS runtime …" | `js_runtime::Error` says "in-process JS runtime …"; `crates/document`'s `From<js_runtime::Error>` maps `OutOfMemory`/`Crashed(m)` to the old "documents runtime reached its heap limit; …" / "documents runtime failed: {m}" so its messages keep their prefix |

## Bundles and limits

| bundle | file (crates/js-runtime/js/) | sha256 (unchanged) | global | thread | deadline | first deadline | heap | line | after fatal | polyfills |
|---|---|---|---|---|---|---|---|---|---|---|
| `DOCUMENTS` | theuth-documents-inproc.js (1,104,902 B) | a9ae5753…62bb9e | theuthDocuments | documents-js | 30 s | 30 s | 512 MiB | 16 MiB | FailQueued | base |
| `APPS` | theuth-apps-inproc.js (130,843 B) | 0c22be45…3242c | theuthApps | app-js | 2 s | 2 s | 256 MiB | 4 MiB | RerunQueued | base + web |
| `MERMAID` | theuth-mermaid-inproc.js (2,860,655 B) | 047139c8…adafcea | theuthMermaid | mermaid-js | 5 s | 10 s | 256 MiB | 4 MiB | RerunQueued | base + web |

Admission (one running + `PENDING` = 8 queued per isolate, `Error::Busy` beyond) stays in `crates/app/src/helper.rs`,
one lane per bundle; documents admission stays in `crates/document`.

Moved with `git mv`, byte-identical (`shasum -a 256` of all 12 moved data files, before vs after: equal):
`js/theuth-{documents,apps,mermaid}-inproc.js`, `js/theuth-{apps,mermaid}-inproc.inputs`,
`js/documents.sha256` (was document-js `js/BUNDLE.sha256`), `js/apps.sha256` (was app-js `js/BUNDLE.sha256`),
`js/polyfill.js` / `js/polyfill-web.js` (then edited as above), and the golden JSON files below.

## Build, scripts, packages

- `justfile`: `documents-bundle`, `documents-bundle-check`, `apps-bundle`, `apps-bundle-check` (and `check`,
  which depends on both checks) read/write `crates/js-runtime/js/{documents,apps}.sha256` and copy bundles
  and `.inputs` lists into `crates/js-runtime/js/`.
- `scripts/apps-bundle-hash.sh`, `scripts/app-parser-env.sh`, `scripts/release-s3.sh`,
  `scripts/package-{linux,macos}.sh` (comment), `scripts/package-windows.ps1`: new paths.
- `crates/app/build.rs`: the library-hash guard reads `crates/js-runtime/js/theuth-apps-inproc.js`.
- Transcript writers: `packages/documents/scripts/golden-inproc.ts` →
  `crates/js-runtime/tests/documents_golden.json`; `packages/apps/scripts/record-inproc-golden.ts` →
  `crates/js-runtime/tests/apps_golden.json`; `packages/apps/test-inproc/mermaid-corpus/reference.ts` →
  `crates/js-runtime/tests/mermaid_golden.json` (moved from `test-inproc/mermaid-corpus/golden.json`; it holds
  all 148 sources and their replies; the corpus generators `corpus.ts`/`valid.ts`/`jsdom-check.ts` stay in
  `packages/apps`, which owns their dependencies).
- Readers: `packages/apps/test/inproc.test.ts` and `inproc-mermaid.test.ts` load the bundles, `polyfill.js` +
  `polyfill-web.js` and the golden files from `crates/js-runtime`; their fake `Deno.core` provides
  `op_jsrt_random_fill`.
- Left on purpose: comments naming `crates/app-js` / `crates/document-js` in files that are bundle hash inputs
  (`packages/documents/{package.json,scripts/build-inproc.ts,src/inproc.ts}`,
  `packages/apps/{src/inproc.ts,src/mermaid-core.ts,scripts/build-inproc.ts}`): editing them changes the
  recorded `inputs` hashes, which would need a bundle rebuild (out of scope: bundles unchanged). Fix them with
  the next `just documents-bundle` / `just apps-bundle`. `crates/app-parser-stub` untouched.

## Tests moved

Before (main ed0257692): `crates/document-js/src/tests.rs` 19 unit tests + `tests/golden.rs` 1;
`crates/app-js/src/tests.rs` 23 unit tests + `tests/golden.rs` 1 + `tests/mermaid_golden.rs` 1 — 45.

After (`crates/js-runtime`): 45, same names (sorted name lists of each old file equal the new file's):

| old | new | tests |
|---|---|---|
| document-js `src/tests.rs` | `tests/unit/documents.rs`, module `tests_documents` | 19 |
| app-js `src/tests.rs` | `tests/unit/apps.rs`, module `tests_apps` | 23 |
| document-js `tests/golden.rs` + `golden.json` | `tests/documents_golden.rs` + `documents_golden.json` | 1 (28 cases) |
| app-js `tests/golden.rs` + `golden.json` | `tests/apps_golden.rs` + `apps_golden.json` | 1 (36 cases) |
| app-js `tests/mermaid_golden.rs` + `packages/apps/test-inproc/mermaid-corpus/golden.json` | `tests/mermaid_golden.rs` + `mermaid_golden.json` | 1 (148 sources) |

The unit tests live under `tests/unit/` but compile into the library (`#[cfg(test)] #[path] mod` in `lib.rs`)
because they need test-only hooks (`op_jsrt_test_panic`, `JsThread::exit_thread`, `runtime::first_isolate`);
`tests/unit/` has no `main.rs`, so cargo does not make it an integration target. Eleven names exist in both
files (e.g. `thread_starts_lazily`); the two modules keep both. Changes inside the tests: type/constructor
names, the test op name and its panic text (`panic inside a js-runtime op`), and the documents tests' short
limits now also set `first_deadline` to the short deadline (= the old semantics, which had none).

## Behaviour changes

None to the wire, to limits, or to which requests fail. Wording only:
- op panic message: `panic in a documents op: …` / `panic in an app-js op: …` → `panic in a <thread> op: …`
  (`documents-js`, `app-js`, `mermaid-js`); thread panic and spawn messages name the bundle's thread as before
  (`documents-js`/`app-js`; mermaid's said `app-js`, now `mermaid-js`);
- `the documents runtime was given up …` / `the JS runtime was given up …` → `the <thread> runtime was given up …`;
- the pump's fallback id for a *thrown/rejected* `call` on a line without a usable id: DOCUMENTS used `0` and
  only numeric ids, now `null` and numeric or string ids (app-js's). Unreachable from `crates/document`
  (it always sends numeric ids, and `inproc.ts` `call` catches its own errors);
- watchdog thread name `documents-js-watchdog` / `app-js-watchdog` → one `js-runtime-watchdog` (one per process
  before as well, per crate).

## Verification

All cargo runs crate-scoped, `--offline`, `CARGO_BUILD_JOBS=4`, one command per build-lock hold, 2026-11-22.

- `cargo test -p js-runtime` (10:36): unit `42 passed; 0 failed` (`tests_documents` 19 + `tests_apps` 23),
  `apps_golden` 1 passed, `documents_golden` 1 passed, `mermaid_golden` 1 passed (148 sources, 147 exact + 1
  engine-wording), doc-tests 0.
- `cargo test -p document` (10:41): lib 116, `actor` 12, `cite_check_links` 4, `index_search` 3, `ingest_audio`
  1, `ingest_job` 5, `insights` 2, `replicate` 3 — all passed, 0 failed.
- `cargo test -p app -p update` (10:45): app lib 54, `publish` 1, update lib 5 — all passed.
- `cargo clippy -p js-runtime -p document -p app -p update --all-targets --no-deps` (10:47): no warnings (only
  the pre-existing future-incompat note for `block v0.1.6`).
- `cargo check -p theuth` (10:52): finished, no warnings.
- `cargo build -p theuth --bin t` (10:55), then `target/debug/t --headless --data-dir <fresh empty dir>` with
  `THEUTH_RPC__LISTEN=127.0.0.1:<free port>` under `perl -e 'alarm 25; exec @ARGV'`: exit 142, RPC port
  listening at 18 s, no panic in stdout/stderr or the data-dir log; again with `THEUTH_BROWSER__ENABLED=false`
  (V8's first isolate then comes from `js_runtime::init_platform`): exit 142, listening, no panic.
- `cargo test -p theuth --bin t --quiet export_bindings` (10:58): 1 passed; worktree unchanged afterwards.
- `cargo test -p theuth --test wire_schema_snapshot` (10:59): 1 passed;
  `crates/theuth/tests/fixtures/wire/openrpc-v1.json` identical to ed0257692.
- `bun test` in `packages/documents`: 84 pass, 0 fail; in `packages/apps`: 233 pass, 0 fail (after
  `bun install --frozen-lockfile` in this fresh worktree).
- `just documents-bundle-check`, `just apps-bundle-check`: pass (exit 0) against the moved hash files.
- `bun run scripts/golden-inproc.ts` (packages/documents): `28 cases, 28 match …/crates/js-runtime/tests/documents_golden.json`;
  `bun run scripts/record-inproc-golden.ts` (packages/apps): rewrote `crates/js-runtime/tests/apps_golden.json`
  byte-identically (36 cases).
- `crates/app/build.rs` guard: compiled standalone with `rustc` against the real tree (exit 0) and against a
  tree whose `library-hash.txt` is all zeros next to the real bundle (panics "does not embed library hash",
  exit 101).
- `shasum -a 256` of the 12 moved data files before and after the move: equal; the three bundles' hashes equal
  the `bundle`/`mermaid-bundle` lines of the hash files.

## Not verified

- `bun run test-inproc/mermaid-corpus/reference.ts` (re-records the mermaid golden from the jsdom reference) was
  not run; its new output path was checked by reading it.
- `just documents-bundle` / `just apps-bundle` (rebuilds) not run: bundles are unchanged by design.
- `cargo build -p theuth` through `cargo` with a deliberately mismatched library hash (the guard was exercised
  standalone instead, see above).
- No packaging scripts were run (`scripts/package-*`, `release-s3.sh`); their path edits are by reading.
- Windows/Linux not built.
