# Documents helper in-process — phase 2 as built

Branch `feat/documents-inproc`. Phase 2 of 3: `crates/document`'s `Helper` runs the documents helper in-process through `document_js::DocumentsThread` (phase 1: [documents-inproc-as-built-P1.md](documents-inproc-as-built-P1.md)). The engine no longer spawns `theuth-documents`; every document operation goes through the in-process runtime. Callers are unchanged. Packaging, `helper_path`, `THEUTH_DOCUMENTS_HELPER` and the justfile recipes stay for phase 3.

## What changed in `crates/document/src/helper.rs`

1,089 lines before (main 012e280eb) → 865 after (`git diff --stat`: +322 −541), tests included.

Deleted: `Proc` (child, stdin, reader channel, stderr ring, `kill`, `reap`), `spawn` (the `--version` probe, piping, reader task, stderr task), `worker` (backoff respawn loop, deadline tick, `PendingMap`), `head`, `BACKOFF_MIN/MAX`, `STDERR_KEEP`, `STDERR_IN_ERROR`, `Job`, `Error::Missing`, `Error::Version`, and the shell-script fake-helper tests (`script`, `ECHO`, `wrong_version_and_missing_binary`, `a_handshake_failure_carries_the_stderr`, `error_replies_and_a_crash_fail_the_request_then_restart`, `shutdown_kills_the_child`, `default_helper_path_carries_exe_suffix`).

Kept unchanged: `DEADLINE`, `MAX_LINE`, `protocol_version`, `helper_path` (phase 3), `HelperStatus`/`HelperProcess` (shape *and* doc comments, which are part of the OpenRPC snapshot), `ImportMode`, `ImportResult`, `Edit`, `EditResult`, `OutlineResult`, `EditError`, `HelperEvent`, `Pending` (+ `id`, `import_result`, `edit_result`, `log`, `Future`), all `Helper` methods and their signatures, including `Helper::new(path, events, shutdown)`, where `path` is now ignored.

New:

- `Helper { backend: Backend, next, forward: mpsc::UnboundedSender<(Ticket, Answer)>, order: Arc<Mutex<()>> }`. `Backend::Js(DocumentsThread)` in production; `Backend::Fake(Arc<dyn Fn(&str) -> Vec<String>>)` under `cfg(test)` only (`Helper::fake`), replacing the relay tests' canned `sh` helper.
- `Helper::new` builds `DocumentsThread::new(Limits { deadline: DEADLINE, max_line: MAX_LINE, ..default }, shutdown)` (heap limit: document-js default 512 MiB) and spawns two tasks: the **forwarder** and **`restarts`**.
- `request` builds the same JSON body. Under `order` it calls `docs.enqueue(line)` (synchronous, so `open` cannot be overtaken by `update`) and hands the `Ticket` to the forwarder; it returns `Pending { id, rx }` as before.
- **Forwarder**: awaits tickets one by one in enqueue order, which is also the runtime's completion order, so no latency is added. For each one it sends every `Reply.events` line as `HelperEvent::Update` (through `handle_event`, the old `handle_line` event branch), then resolves the reply (`parse_reply`, the old reply branch). Events therefore reach the actor before the reply resolves and never interleave across requests, as with the old single reader. A per-request `tokio::spawn` would not guarantee that.
- **`restarts`**: watches `DocumentsThread::subscribe()`. Generation > 1 → `HelperEvent::Restarted` (the hub reopens mirrors, unchanged) and status `Running`. On shutdown, status → `Starting` (as the old worker did).
- `init_js_platform()` (→ `document_js::init_platform()`), called by `t` (see V8 init order below).

## Error mapping (`impl From<document_js::Error> for helper::Error`)

| document-js | helper::Error | message |
|---|---|---|
| `Timeout` | `Timeout` | `documents helper did not answer within 30s` |
| `OutOfMemory` | `Crashed(…)` | `documents helper failed: documents runtime reached its heap limit; it was terminated and is rebuilt` |
| `Crashed(m)` | `Crashed(…)` | `documents helper failed: documents runtime failed: <panic message>` |
| `TooLarge { len, max }` | `Protocol(…)` | `documents helper: request is <len> bytes; the helper accepts <max>` (today's text) |
| `Js(m)` | `Protocol(m)` | the script error |
| `Stopped` | `Crashed("helper worker stopped")` | same as today's dead-worker text |
| `ok:false` reply | `Protocol(error)` | unchanged; `EditError::from_helper` still decodes it |

As with a dead child, a fatal failure (Timeout/OutOfMemory/Crashed) fails that request and every request queued behind it with the same error; document-js then rebuilds the runtime and the generation bump emits `Restarted`.

## Status mapping (`helper_status()`, wire unchanged)

| state | when |
|---|---|
| `starting` | before the first successful reply (and after shutdown) |
| `running` | set by every successful reply, and by a rebuild (`restarts`) |
| `crashed { stderr: <fatal error text> }` | set by the forwarder on a fatal error; lasts until the rebuilt runtime is reported |
| `missing` | never produced |

`found` is always `true` (the helper is compiled in). The Settings UI (`App.tsx`) would otherwise show "Helper missing". `path` is still `helper_path()`, `version` is `protocol_version()`. The doc comments of both types are kept verbatim because `crates/theuth/tests/fixtures/wire/openrpc-v1.json` embeds them; a source comment above the types states their in-process meaning.

## V8 init order

`crates/theuth/src/main.rs` calls `document::helper::init_js_platform()` right after `browser::warmup(&browser_cfg)` and before `Runtime::new`, i.e. before any plugin starts. `browser::warmup` returns early when the browser is disabled and then never touches V8, so without this call the documents thread would have initialised V8 itself. `document_js::init_platform` builds and drops one bare `JsRuntime` inside a throwaway current-thread tokio runtime, behind a `std::sync::Once`; deno_core's own `init_v8` is behind a process-wide `Once`. When warmup already ran (browser enabled), this costs one isolate creation (~5 ms per the spike) and initialises nothing twice.

Independently, the documents thread is lazy: its isolate is created on the first `Helper::request`, which needs the document actor, which a plugin starts after `Runtime::new`.

Verified by reading `main.rs` (warmup at 1815, the call at 1818, `Runtime::new` at 1820) and `browser::warmup` (early return at `crates/browser/src/lib.rs:103`). Not verified by running `t`.

The only other in-process `Helper` is `memory.rs:76` (a second `Helper::new`, so a second documents thread and isolate, as it was a second child before). It is reached from the same `t` process after main's init.

## Tests

New helper unit tests against the real in-process runtime:

- `the_embedded_bundle_speaks_our_protocol_version`: the old `--version` handshake as a unit test (`DocumentsJs::version()` == `theuth-documents <protocol.ts PROTOCOL_VERSION>`).
- `requests_pipeline_and_events_arrive_before_the_reply`: open, then `import` and `markdown` queued together. The `Update` event (doc, req = the import's id, ratio, touched) is already in the channel when the import reply resolves. Then outline, and an `edit` error decoded by `EditError::from_helper`.
- `a_timeout_fails_the_request_then_restarts_and_documents_reopen`: 1.5 s deadline; `for(;;){}` through document-js's hidden `eval_raw`, plus a queued `markdown`, which fails with `Error::Timeout`. Then `Restarted` arrives, the mirror is gone ("not open"), and after a reopen the document is imported and read back.
- `a_panic_on_the_runtime_thread_is_a_crash_then_a_restart`: `panic_on_thread` → `Restarted` → reopen → readable.
- `an_oversized_request_is_refused_before_the_runtime`: `Error::Protocol("… the helper accepts …")`.
- `shutdown_stops_the_runtime`: after the token is cancelled, requests fail with `Crashed`.
- `protocol_version_is_read_from_protocol_ts` (kept).

The skip gates are removed, so these suites now really run without a binary:

- `helper_or_skip!` in `tests/{actor,index_search,ingest_job,insights}.rs` now just yields `helper_path()`; it never skips. The macro name and the `bin` plumbing are phase-3 removals.
- The `if !bin.is_file() { return }` blocks in `crates/artifact/tests/{document,read}.rs` and `crates/meeting/tests/note.rs` are gone.

`relay.rs` tests: `actor()` uses the real in-process helper (formerly a missing binary). `actor_with(dir, respond)` uses `Helper::fake` with the same canned `edit`/`import` replies the old `sh` script gave.

### Commands and results

All run with `CARGO_TARGET_DIR=<worktree>/target CARGO_BUILD_JOBS=4 THEUTH_DOCUMENTS_HELPER=/nonexistent GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=commit.gpgsign GIT_CONFIG_VALUE_0=false`, under `/tmp/theuth-build.lock`, one command per hold.

(a) `cargo test --offline -p document`:
- Run at 18:13 failed in the `opusic-sys` build script: the seeded `target/` carried a CMake cache from another checkout. I deleted `target/debug/build/opusic-sys-60eaa978b973c9eb/out/build`; no code change.
- Rerun at 18:19–18:21:
  - unit tests: `test result: ok. 114 passed; 0 failed`
  - `tests/actor.rs`: `ok. 12 passed`
  - `cite_check_links`: `ok. 4 passed`
  - `index_search`: `ok. 3 passed`
  - `ingest_audio`: `ok. 1 passed`
  - `ingest_job`: `ok. 5 passed`
  - `insights`: `ok. 2 passed`
  - `replicate`: `ok. 3 passed`
  - doc-tests: `ok. 0 passed`

(b) `cargo test --offline -p artifact --test document --test read` (lock held 18:46–18:52, after ~25 min waiting on another session): `test result: ok. 2 passed; 0 failed` (document), `test result: ok. 7 passed; 0 failed` (read).

(c) `cargo check --offline -p theuth`: the first two attempts failed in third-party build scripts because the seeded `target/` held Swift module caches from another checkout (`apple-metal`, then `screencapturekit`: "PCH was compiled with module cache path '/Users/sky/Develop/skymoore/theuth/target/…'"). I removed `target/debug/build/*/out/swift-build*`; no code change. Third run (19:07–19:08): `Finished dev profile … in 53.64s`, exit 0. Every `Helper::new` call site compiles.

(d) `cargo clippy --offline -p document --all-targets` (19:09): failed before reaching `document`, on a pre-existing lint in a dependency I did not touch: `crates/voice/src/history/mod.rs:465` `clippy::chunks_exact_to_as_chunks`, new in clippy 1.99 and denied by the workspace `clippy::all`.

Re-run as `cargo clippy --offline -p document --all-targets -- -A clippy::chunks_exact_to_as_chunks` (19:17): one error in my code, `clippy::type_complexity` on the test-only `Backend::Fake(Arc<dyn Fn…>)`. I fixed it with the `Respond` type alias. A second run at 19:18 was wasted: it started before the fix landed.

**The final clippy run after the fix did NOT run.** Its lock waiter was still queued behind another session at 19:22, and I stopped it to report within budget. Still to run: `cargo clippy --offline -p document --all-targets -- -A clippy::chunks_exact_to_as_chunks`.

## Deviations

- `Helper::new` keeps its `path` parameter (ignored). That keeps all ~20 call sites (hub/notebook/tools/note_cells/relay/memory/lib, `crates/agent`, `crates/artifact`, `crates/meeting`, `crates/document/tests`) untouched. It goes together with `helper_path` in phase 3.
- `Error::Missing`/`Error::Version` are removed (no producer left; nothing outside helper.rs matched on them).
- `HelperStatus.found` is always `true`, and the field docs no longer describe it literally (kept verbatim for the wire snapshot).
- The process-global status is shared by every `Helper` in the process, as before (`memory.rs` builds a second one; its shutdown resets the status to `starting`, as the old worker did).
- `tests/*` keep the `helper_or_skip!` name although it no longer skips.

## Phase 3 deletion list

Produced with `git grep -n -I -E "theuth-documents|helper_path|THEUTH_DOCUMENTS_HELPER|build-helper\.ts|documents-helper|DOC_HELPER|build_doc_helper"`, excluding `crates/document-js/js/*`, `docs/design/*`, `bun.lock`, `crates/app/*` and `crates/agent/tests/fixtures/*`:
- `crates/app` has a `helper_path` of its own (the app parser); it is not this.
- The fixture is one old transcript line.

Notes on items that must not simply be deleted:
- `crates/document-js` and `packages/documents/src/inproc.ts` hits are the in-process bundle's own name and version string: keep them.
- `justfile:113-142` is the new `documents-bundle` recipe set: keep it.
- `crates/sandbox/src/release.rs` and `crates/platform/src/process.rs` only mention the name in tests/comments.
- `crates/update/src/lib.rs` must keep accepting (and should start pruning) old `theuth-documents-*` files beside installed binaries.

- `README.md`: 32,34
- `crates/artifact/tests/document.rs`: 5,60,232
- `crates/artifact/tests/read.rs`: 292
- `crates/desktop/companion-windows/w5-gate.ps1`: 42
- `crates/desktop/ui/src/components/settings/App.tsx`: 97
- `crates/document-js/src/lib.rs`: 34 (keep)
- `crates/document-js/src/runtime.rs`: 237 (keep)
- `crates/document-js/src/tests.rs`: 80 (keep)
- `crates/document/src/helper.rs`: 55,56,57,59,60,68,75,85,93,97,131,383,734
- `crates/document/src/lib.rs`: 70,295
- `crates/document/src/memory.rs`: 76
- `crates/document/src/rpc_documents.rs`: 57
- `crates/document/tests/actor.rs`: 29,31
- `crates/document/tests/index_search.rs`: 6,28
- `crates/document/tests/ingest_job.rs`: 5,27
- `crates/document/tests/insights.rs`: 6,31
- `crates/meeting/tests/note.rs`: 86
- `crates/platform/src/process.rs`: 108
- `crates/sandbox/src/release.rs`: 122,132
- `crates/tasks/src/thread.rs`: 200
- `crates/theuth/tests/fixtures/wire/openrpc-v1.json`: 77996,78008,79739,79751 (HelperStatus descriptions; changing them is a wire-snapshot update)
- `crates/update/src/lib.rs`: 422,423,439,441,446,817,917,925,933,934
- `deploy/update/Dockerfile`: 2,5,95,96,97,101
- `docs/plugins.md`: 310
- `justfile`: 81,83,90,92,103,105,106,110,111,166 (113-142: `documents-bundle*`, keep)
- `packages/apps/package.json`: 23 (the app parser's own build-helper.ts; keep)
- `packages/documents/package.json`: 17
- `packages/documents/scripts/build-helper.ts`: 7,8,97,111,118,122 (whole file)
- `packages/documents/scripts/build-inproc.ts`: 5,18 (keep)
- `packages/documents/scripts/parity-inproc.ts`: 3
- `packages/documents/src/helper.ts`: 2,10,11,138,142 (whole file, after folding into inproc.ts)
- `packages/documents/src/inproc.ts`: 171 (keep)
- `packages/documents/test-inproc/serve-bundle.ts`: 5,10
- `packages/documents/test/helper.test.ts`: 9,39,43
- `scripts/app-parser-env.sh`: 13,14,16,22,23,56,57,59,60,61,67,68,71,75
- `scripts/install-macos.sh`: 17,19
- `scripts/package-linux.sh`: 6,8,126,127,129,131,134,142,174,176
- `scripts/package-macos.sh`: 6,11,85,86,125,126,128,154,155,188
- `scripts/package-update-macos.sh`: 6,41,54
- `scripts/package-windows.ps1`: 10,12,72,84,120,159,162
- `scripts/release-s3.sh`: 11,183,192,374,378,384,387,388,393,394,401
- `scripts/setup-ubuntu-build.sh`: 135,136,143,146,149
- `scripts/stage-linux.sh`: 5,65,69,73,80,81,86
- `scripts/tap/theuth.rb.formula.tmpl`: 33,35,42,43
- `site/content/docs/getting-started/installation.mdx`: 8,77,127,157
- `site/content/docs/reference/cli.mdx`: 92
- `site/content/docs/reference/development.mdx`: 21,109,125,126
- `site/content/docs/reference/troubleshooting.mdx`: 155
- `site/content/docs/workspace/documents.mdx`: 202

jsdom / server-util / React in `packages/documents` (`git grep -n -I -E "jsdom|server-util|\breact\b|react-dom|@blocknote/react" -- packages/documents ':!packages/documents/bun.lock'`):
- `convert.ts:6` imports `@blocknote/server-util`. Phase 3 should import `src/inproc/editor.ts` directly and drop the build-time alias.

- `packages/documents/package.json`: 23 (`@blocknote/server-util` dependency)
- `packages/documents/scripts/build-helper.ts`: 10,13,17,55,56,59,63,69,70,72,76,81
- `packages/documents/scripts/build-inproc.ts`: 8,21,23,39 (the alias and the forbidden-module check)
- `packages/documents/scripts/golden-inproc.ts`: 3 (default peer `bun src/helper.ts`)
- `packages/documents/scripts/parity-inproc.ts`: 3
- `packages/documents/src/convert.ts`: 6
- `packages/documents/src/inproc/editor.ts`: 2,3,4,37 (comments)
- `packages/documents/test-inproc/preload.ts`: 2,14,15
- `packages/documents/test-inproc/serve-bundle.ts`: 3

## Not done / not verified

- `t` was not run, so the V8 init order is verified by reading the code only.
- No end-to-end check through the desktop UI.
- No parity run against the compiled binary in this phase. The phase-1 golden test covers converter output.
- `crates/meeting/tests/note.rs` is edited (skip removed) but not run (not in the brief's test list).
