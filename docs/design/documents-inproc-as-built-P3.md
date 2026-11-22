# Documents helper in-process — phase 3 as built: the sidecar is gone

Branch `feat/documents-inproc`. Phase 3 of 3. Earlier phases: [P1](documents-inproc-as-built-P1.md) built the `document-js` crate, and [P2](documents-inproc-as-built-P2.md) put `Helper` on it. This phase deletes the stdio helper `theuth-documents`, its build, packaging and shipping, and the jsdom/React dependency tree. The only documents helper left is the bundle `t` embeds.

2026-11-22: `crates/document-js` has since been merged with `crates/app-js` into `crates/js-runtime` (bundle `DOCUMENTS`); see [js-runtime-unified-as-built.md](js-runtime-unified-as-built.md). Paths below are as of this phase.

## Deleted

### `packages/documents`

| what | why |
|---|---|
| `src/helper.ts` (stdio main, `--version`, line framing, `MAX_LINE_BYTES`) | `src/inproc.ts` was already a transport-agnostic copy of `parseRequest`/`handle`; nothing of helper.ts was still needed |
| `scripts/build-helper.ts` (the `bun build --compile` binary, the jsdom embed patches, the compiled-binary self-test) | no binary |
| `test/helper.test.ts` (spawned the compiled binary or `bun src/helper.ts`) | replaced by `test/inproc.test.ts`: the same six protocol cases against `call(line)` in-process. `--version` → `version`; "EOF exits 0" → "a request that changes nothing answers exactly one line" |
| `scripts/parity-inproc.ts` (jsdom helper vs in-process peer) | its jsdom side no longer exists; the golden transcripts are the baseline now |
| `test-inproc/preload.ts`, `test-inproc/serve-bundle.ts` | `bun test` needs no module aliasing any more; the bare-vm peer existed only for parity |
| `src/inproc/dom-happy.ts` | the happy-dom candidate |
| dependencies `@blocknote/server-util` (and through it `jsdom`, `@blocknote/react`, `react`, `react-dom`, their `@types`), devDependency `happy-dom` | `bun install`: "3 packages removed"; `bun.lock` has no `jsdom`/`react` entries left (`grep -c "react\|jsdom" bun.lock` → 0) |
| `package.json` script `build:helper` | replaced by `bundle` (`bun run scripts/build-inproc.ts`) |

Changed:

- `src/convert.ts` imports `ServerBlockNoteEditor` from `./inproc/editor.ts` directly. There is no build-time or test-time alias any more, so `scripts/build-inproc.ts` lost its resolver plugin; it keeps the "no React / no node: / no require(" bundle checks.
- `linkedom` moves from devDependencies to dependencies, since it is now runtime code.
- `src/inproc/editor.ts`: a type cast fix in the constructor; `tsc --noEmit` reports 0 errors.
- The stale test `markdown round trip per block type > code: link` now expects `[x](theuth://notes/1)` to survive, because `schema.ts` allowlists `theuth:` (144 §6).
- `scripts/golden-inproc.ts` no longer spawns a peer. By default it replays the corpus through `src/inproc.ts` in-process and compares with `crates/document-js/tests/golden.json`, exiting 1 on any difference. `--write` re-records. `golden.json` itself is unchanged: it was recorded from the jsdom helper in P1 and remains the regression baseline.
- Package exports are unchanged (`.`, `./schema`, `./protocol`, `./yaml`). The desktop UI imports only `@theuth/documents/schema` and `@theuth/documents/yaml` (grep of `crates/desktop/ui/src`).

### Build and packaging

- `justfile`:
  - recipe `documents-helper` deleted.
  - `install` no longer depends on it and no longer copies `$DOC_HELPER`.
  - `desktop` no longer depends on it.
  - `check` now depends on `documents-bundle-check`.
  - There is no CI workflow directory in the repo, so `just check` is the check entry point.
- `scripts/app-parser-env.sh`: `DOC_VERSION`, `DOC_HELPER`, `DOC_HELPER_SRC`, `build_doc_helper`, `check_doc_helper` and their header comment removed. The app parser part is untouched.
- `scripts/package-macos.sh`: no documents build, copy into `Contents/MacOS`, codesign (`-i …documents`) or `check_doc_helper`.
- `scripts/package-linux.sh`: no documents build or staging; `externalBin` has `t` + the app parser only; the deb check lists `t`, the app parser and `theuth-desktop`.
- `scripts/package-update-macos.sh`: CLI tarball = `t` + app parser; no documents presence check.
- `scripts/package-windows.ps1`: no `$DocVersion`/`$DocHelper`, no documents `Build-Helper` + `--version` + `Test-StdinEof` block, no staging; `externalBin`, zip contents and expected zip members are `t.exe` + app parser.
- `scripts/release-s3.sh`:
  - gate 2 signs/checks only the app parser.
  - gate 7 smokes `t` + app parser.
  - gate 8 expects `t $HELPER` (Windows: `t.exe $HELPER.exe`).
  - no `check_doc_helper`.
- `scripts/setup-ubuntu-build.sh`, `scripts/stage-linux.sh`: no documents build; tarball = `t` + app parser.
- `scripts/install-macos.sh`: requires only `theuth-app-parser-*` in the bundle.
- `scripts/tap/theuth.rb.formula.tmpl`: installs and tests only `t` + app parser.
- `deploy/update/Dockerfile`: the documents `RUN` stage and its `COPY` glob removed; the output holds two files.

Kept: `deploy/relay/Dockerfile` copies `packages/documents` into the web build stage. The browser UI consumes `@theuth/documents/schema` and `/yaml` from source (vite `documentsDeps`) and never ran a helper step, so nothing was removed there.

### Rust

- `crates/document/src/helper.rs`:
  - `helper_path()` deleted, along with the `THEUTH_DOCUMENTS_HELPER` env, the debug-build fallback to `packages/documents/dist/theuth-documents` and the `theuth-documents-<version>` beside-the-binary name.
  - `Helper::new(path, events, shutdown)` → `Helper::new(events, shutdown)`.
  - `helper_status()` answers `path: "in-process"`, `found: true`.
  - `Helper::fake(events, respond)` (canned responder) is now `#[doc(hidden)] pub` instead of `cfg(test)`, so tests outside the crate can use it.
  - `PROTOCOL_TS` (`include_str!` of protocol.ts) stays: `protocol_version()` feeds `HelperStatus.version` and the unit test that compares it with the embedded bundle's `version`.
- `crates/document/src/lib.rs`: the `helper_path` re-export and the plugin's `let path = helper_path()` are gone.
- Call sites updated:
  - `crates/document/src/{hub,memory,note_cells,notebook,relay,tools}.rs` (via an ast-grep codemod).
  - `crates/agent/src/rpc_documents.rs`.
  - `crates/document/tests/{actor,index_search,ingest_job,insights}.rs`: the `helper_or_skip!` macro and every `bin` parameter/argument are removed.
  - `crates/artifact/tests/{document,read}.rs`, `crates/meeting/tests/note.rs` (`start_documents(rt)`).
  - `crates/tasks/src/thread.rs`: `headings_reach_the_document_actor` used a `sh` fake helper. It now uses `Helper::fake` with the same canned outline.
- No doctor/diagnostic code looked for the binary apart from `helper_status` (`documents_status` RPC). There is nothing else to delete.
- `crates/platform/src/process.rs` (comment) and `crates/desktop/companion-windows/w5-gate.ps1` (process list) no longer name `theuth-documents-*`.
- Not changed on purpose:
  - The `HelperStatus`/`HelperProcess` doc comments and `rpc_documents.rs`'s `documents_status` description, because they are in the OpenRPC snapshot.
  - `crates/update` still accepts an optional `theuth-documents-<version>` in an update payload (old payloads; harmless; a later cleanup can drop it and prune the old file beside `t`).
  - `crates/sandbox/src/release.rs` uses the name only as tar test data.

### Docs

- `docs/design/59-documents.md`: dated notes at §0 decision 1 and §3.2; history otherwise untouched.
- `docs/design/00-architecture.md`: the `packages/documents/` workspace line.
- `README.md`: `just documents-helper` gone; `just install` description updated.
- Site:
  - `getting-started/installation.mdx`, `reference/development.mdx`, `reference/cli.mdx` (no `THEUTH_DOCUMENTS_HELPER`), `reference/troubleshooting.mdx`: the "helper missing" rows replaced by the in-process failure modes.
  - `workspace/documents.mdx`: the same "helper missing" / "speaks protocol" entries replaced.

## Vestigial wire fields

`HelperStatus` (RPC `documents_status`, `crates/theuth/tests/fixtures/wire/openrpc-v1.json`) keeps its shape. Its doc comments, which the fixture embeds, are verbatim. A later change can drop these together with the Settings UI row (`crates/desktop/ui/src/components/settings/App.tsx`):

| field | now |
|---|---|
| `HelperStatus.path` | constant `"in-process"`; the description still says "Where [`helper_path`] points" |
| `HelperStatus.found` | always `true` |
| `HelperProcess::Missing` | never produced |
| `HelperProcess::Crashed.stderr` | carries the last fatal runtime error, not stderr |
| `HelperStatus.version` | still meaningful: `PROTOCOL_VERSION` |

## Bundle

`just documents-bundle` after removing the dependencies: `inputs` hash changed (`c1edcf76…` → `c879e3cd…`); `bundle` hash unchanged (`a9ae5753f7d7fa03349f67a8e6cabc46d00fb87bd9ba043dd0033b4de562bb9e`). The bundle bytes are identical: the removed packages were never in the bundle. This also shows the build is reproducible across two runs. `just documents-bundle-check` passes.

## Grep proof

Command (tracked files only, so `node_modules`/`target` are excluded; `docs/design` is history):

```
git grep -n -I -E "theuth-documents|helper_path|THEUTH_DOCUMENTS_HELPER|build-helper|jsdom|server-util" -- ':!docs/design'
```

Run after the Rust commit (fd6acb4cf). Every remaining hit is listed here by class; nothing refers to the removed sidecar's build, shipping or lookup.

| hit | why it stays |
|---|---|
| `crates/app/**`, `packages/apps/**` (`helper_path`, `build-helper.ts`, `jsdom`), `scripts/package-windows.ps1:71,83` (`build-helper.ts`) | the **app parser** sidecar (`packages/apps`), which stays |
| `crates/desktop/ui/**` (`jsdom` in UI tests, `bun.lock`) | the desktop UI's own test DOM; out of scope |
| `crates/desktop/ui/src/components/settings/App.tsx:97` (`Helper: theuth-documents …`) | the Settings row over the vestigial `HelperStatus` (see above); goes with the wire fields |
| `crates/document/src/helper.rs:64,68`, `crates/document/src/rpc_documents.rs:57`, `crates/theuth/tests/fixtures/wire/openrpc-v1.json:77996,78008,79739,79751` | doc comments frozen by the OpenRPC snapshot (wire stability) |
| `crates/document/src/helper.rs:700`, `crates/document-js/src/{runtime.rs:244,tests.rs:80}`, `packages/documents/src/inproc.ts:167`, `packages/documents/test/inproc.test.ts:17,19` | the protocol identity string `theuth-documents <PROTOCOL_VERSION>` the bundle reports |
| `crates/document-js/js/theuth-documents-inproc.js`, `crates/document-js/src/lib.rs:34`, `justfile:103,109,130`, `packages/documents/scripts/build-inproc.ts:5,8,18` | the embedded bundle's file name |
| `crates/document-js/tests/golden.rs:2,140` | the golden baseline's provenance (recorded from the jsdom helper in P1) |
| `crates/update/src/lib.rs:422-446,917-934` | updater still tolerates an old documents helper in a payload (see above) |
| `crates/sandbox/src/release.rs:122,132` | tar test data |
| `crates/agent/tests/fixtures/endless/B.main.jsonl:243` | a recorded transcript (fixture) |
| `docs/plugins.md:310` | plugin overview paragraph still describes the subprocess helper: **not updated** (one very long line; follow-up) |
| `deploy/vm/Dockerfile:49`, `deploy/vm/bootstrap.sh:22` | false positive (`x11-xserver-utils`) |

## Commands and results

Non-Rust:

- `bun install` (packages/documents): `3 packages removed`.
- `bun test` (packages/documents, no preload): `84 pass, 0 fail, 258 expect() calls, Ran 84 tests across 4 files` (before and after all edits).
- `bunx tsc --noEmit` (packages/documents): 0 errors.
- `bun run scripts/golden-inproc.ts`: `28 cases, 28 match …/crates/document-js/tests/golden.json`. The in-process helper on JSC reproduces the jsdom baseline.
- `just documents-bundle`, then `just documents-bundle-check`: passes. With an extra line appended to `src/yaml.ts` (P1) it fails.
- `bash -n` on every edited shell script: ok (`app-parser-env.sh`, `install-macos.sh`, `package-linux.sh`, `package-macos.sh`, `package-update-macos.sh`, `release-s3.sh`, `setup-ubuntu-build.sh`, `stage-linux.sh`). `just --list`: ok. `package-windows.ps1` and `w5-gate.ps1`: read only, not executed.

Rust: each command in its own hold of `/tmp/theuth-build.lock`, `CARGO_TARGET_DIR=<worktree>/target CARGO_BUILD_JOBS=4`, `THEUTH_DOCUMENTS_HELPER` unset, commit signing off, at commit fd6acb4cf:

- `cargo test --offline -p document-js` (19:38–19:39):
  - `test result: ok. 13 passed; 0 failed` (unit)
  - `test result: ok. 1 passed; 0 failed` (golden)
  - doc-tests `ok. 0 passed`
- `cargo test --offline -p document` (19:39–19:41):
  - unit `ok. 114 passed`
  - `actor` `ok. 12 passed`
  - `cite_check_links` `ok. 4 passed`
  - `index_search` `ok. 3 passed`
  - `ingest_audio` `ok. 1 passed`
  - `ingest_job` `ok. 5 passed`
  - `insights` `ok. 2 passed`
  - `replicate` `ok. 3 passed`
  - doc-tests `ok. 0 passed`
  - all 0 failed
- `cargo check --offline -p theuth` (19:42–19:45): `Finished dev profile … in 3m 06s`, exit 0.
- `cargo clippy --offline -p document -p document-js --all-targets -- -A clippy::chunks_exact_to_as_chunks` (19:46): `Finished`, no warnings, exit 0. The `-A` is for a pre-existing lint in `crates/voice` (P2 note).
- Gate run by the orchestrator before phase 3 (`/tmp/docs-inproc2-verify-p2.log`): P2's clippy finished clean, and `cargo test -p document --lib helper` gave `ok. 7 passed`. Its `cargo test -p meeting --test note` printed no result line in that log.

- `cargo test --offline -p tasks --lib headings_reach_the_document_actor` (19:48–19:51): `test result: ok. 1 passed; 0 failed` (the test moved from a `sh` fake to `Helper::fake`).
- `cargo test --offline -p meeting --test note` (19:52–19:53): `test result: ok. 1 passed; 0 failed` (it now runs on the in-process helper with no binary anywhere).

## Not verified

- No packaging, release, install or docker build was run. That covers `package-macos.sh`, `package-linux.sh`, `package-update-macos.sh`, `package-windows.ps1`, `release-s3.sh`, `stage-linux.sh`, `setup-ubuntu-build.sh`, `install-macos.sh`, the formula, and both Dockerfiles, including the relay image.
- Shell scripts were checked with `bash -n` only. The PowerShell script was checked by reading only.
- No real `t` run, desktop UI run or UI check.
- The updater (`crates/update`) still accepts `theuth-documents-*` names beside `t` (its tests name them); older installs keep their now-unused helper file. Pruning it is a follow-up.

## Review fixes

Code: `0ca80bbf3`. Docs: `f2b02d52d`. Bundle inputs: `107e1a233`.

| finding | outcome | proof |
|---|---|---|
| **M1** reply not matched to its request | **fixed, both sides.** (a) document-js: an event-loop error (`poll_event_loop` Err) is now `Error::Crashed` (fatal). So is a request that ends with an idle loop and no reply. The runtime is dropped and rebuilt and the generation bumps. `serve` clears the request slot after every request; the reply slot was already cleared per `drive`. A rebuilt runtime has fresh JS globals (`sink`, `current`), so nothing crosses a rebuild. (b) helper.rs: `parse_reply(line, id)` checks `reply.id == pending id`. On a mismatch the forwarder calls the new `DocumentsThread::restart()` (epoch bump: queued and in-flight requests of the old runtime fail as `Crashed`, the next one runs fresh), answers `Crashed`, drops that reply's events and emits `Restarted` | document-js `an_event_loop_error_mid_request_is_fatal_and_the_next_request_gets_its_own_reply` (a hidden hook rejects an unrelated promise and schedules a late reply 100 ms on; the next two requests, 200 ms apart, get their own ids). It **failed** with the old `Error::Js` mapping (`tests.rs:460`) and passes with the fix. Also `a_request_that_ends_without_its_reply_is_fatal`, `restart_fails_the_queued_requests_and_the_next_one_runs_fresh`, document `helper::tests::a_reply_for_another_request_is_fatal_and_rebuilds` |
| **M2** `Restarted` can overtake `Update`s | **fixed.** The separate `restarts` task (generation watch) is deleted; the forwarder is the only writer to the events channel. Tickets resolve to `Settled { result, rebuilt }`. `rebuilt` is true on exactly one job per lost runtime: the job whose failure was fatal, or the first job of a respawned thread. The jobs queued behind it fail with the same error and `rebuilt: false`. The forwarder sends `Restarted` right after answering that ticket, so every earlier ticket's updates come first. A mismatch-driven restart is reported by the forwarder itself. Nothing reports a rebuild twice: the thread's build-after-failure does not report. The `watch` generation channel and `subscribe()` are removed; `generation()` is an atomic | document `helper::tests::restarted_follows_the_updates_of_earlier_requests` (A import → B runaway → C queued: A's `Update`s, then `Restarted`, then nothing for 300 ms); document-js `only_the_failing_request_reports_the_rebuild`; the existing timeout and panic restart tests now also assert that only one `Restarted` arrives |
| **m1** requests after a fatal failure served before the reopen | **partly closed, rest documented.** Jobs carry the epoch they were enqueued under. The thread bumps the epoch *before* sending the fatal job's reply, and a job from an older epoch fails with the epoch-ending error instead of running on the fresh runtime. Still open: a request enqueued *after* the failure but before the actor handles `Restarted` runs on the fresh runtime before the reopen and fails with `document X is not open`. Closing it would need the actor to gate requests on its reopen; that is left. It is documented in the helper.rs module docs | covered by the epoch tests above |
| **m3** dead runtime thread never restarted | **fixed.** When `send` fails (closed queue) and shutdown is not cancelled, `send` clears `tx`, spawns a new thread and resends once. The new thread's first job reports `rebuilt` (its mirrors are gone). `alive` is now a counter, because the old and new threads can overlap | document-js `a_dead_runtime_thread_is_respawned`: a test-only `Work::Exit` makes `serve` return; the next request reports `rebuilt` and "not open", a round trip works, generation 2 |
| **m5** V8 init fragility | **fixed, and it was a real bug.** The old `init_platform` built and dropped a current-thread tokio runtime. tokio panics when one is dropped inside an async context ("Cannot drop a runtime in a context where blocking is not allowed"), and `t` calls it inside `run()`, which `main`'s `block_on` drives. It is now `JsRuntime::init_platform(None)`: the same `init_v8(None, false, false)` that `JsRuntime::try_new` performs, without an isolate or a tokio runtime. It is a no-op after `browser::warmup` (process-wide `Once`), so no throwaway runtime is built any more. Comments at `init_platform` and the `main.rs` call site explain why the call must sit synchronously in the startup future before `Runtime::new` and not in a spawned task. No debug assertion was added: deno_core exposes no cheap, reliable "already initialised" or "main thread" probe | document-js `init_platform_is_safe_inside_an_async_context` (a `#[tokio::test]`). It **panicked** in tokio `blocking/shutdown.rs:51` with the old body and passes now |
| **m6** test timing | **fixed.** `DocumentsJs::new` loads polyfill, bundle and pump under `max(deadline, DEFAULT_DEADLINE)` and only then applies the configured deadline. The 1.5 s deadline therefore covers requests only, and the runaway tests still prove termination at that deadline. `wait_until` now waits 60 s instead of 10 s, and the elapsed-time bound is 20 s instead of 10 s. The deadline test polls `generation()` instead of the removed watch | document-js 19/19 |
| **m7** broken intra-doc link `helper_path` | **fixed without changing text.** The sentence "Where [`helper_path`] points." is in `openrpc-v1.json` (lines 77996, 79739), so it stays verbatim. An `#[allow(rustdoc::broken_intra_doc_links)]` on the field silences the link; schemars reads only doc comments, so the snapshot text is unchanged [INFERENCE: not run; the orchestrator runs the wire snapshot] | clippy clean |
| **m8** user-facing helper-binary text | **fixed:** `site/.../desktop.mdx` (troubleshooting), `site/.../meetings.mdx` ("no note"), `docs/plugins.md` (`document` paragraph), `scripts/package-windows.ps1` comment | text only |
| **m9** bundle hash inputs | **fixed.** `_documents-bundle-hash` (shared by `documents-bundle` and `documents-bundle-check`) hashes `tsconfig.json` too. `inputs` changed `c879e3cd…` → `8583ad75…`; `bundle` is unchanged (`a9ae5753…`) and the bundle file has no diff | `just documents-bundle`, `just documents-bundle-check` ok |
| crate docs: uncontainable cases | **done** (`document-js/src/lib.rs`): a V8 internal fatal error, a single allocation larger than the 64 MB headroom, memory outside the V8 heap (ArrayBuffer backing stores), a panic in a deno_core builtin op | — |

### Commands and results (after the fixes)

Each cargo command ran in its own hold of `/tmp/theuth-build.lock`, with `CARGO_TARGET_DIR=<worktree>/target CARGO_BUILD_JOBS=6 --offline` and commit signing off.

- `bun test` (packages/documents): `84 pass, 0 fail`.
- `just documents-bundle`, then `just documents-bundle-check`: ok.
- `cargo test --offline -p document-js` (20:31): unit `ok. 19 passed; 0 failed`, golden `ok. 1 passed; 0 failed`, doc-tests `ok. 0 passed`.
- `cargo clippy --offline -p document -p document-js --all-targets -- -A clippy::chunks_exact_to_as_chunks` (20:35): `Finished`, no warnings. An earlier run flagged a `while let` in a new test, which was fixed.
- `cargo test --offline -p document` (20:36–20:37, at `0ca80bbf3`):
  - unit `ok. 116 passed`
  - `actor` `ok. 12 passed`
  - `cite_check_links` `ok. 4 passed`
  - `index_search` `ok. 3 passed`
  - `ingest_audio` `ok. 1 passed`
  - `ingest_job` `ok. 5 passed`
  - `insights` `ok. 2 passed`
  - `replicate` `ok. 3 passed`
  - doc-tests `ok. 0 passed`
  - all 0 failed
- The two red runs were taken against a temporary revert of the old code: `cargo test -p document-js --lib -- an_event_loop_error init_platform` gave `FAILED. 0 passed; 2 failed`.
- A first `cargo test -p document` failed before compiling our code: `mlx-sys` `out/bindings.rs` was missing, because 181 of 359 `target/debug/build/*/out` directories were empty (wiped outside these commands). `cargo clean -p mlx-sys` fixed it.

Not run here: `-p theuth` (check and the wire snapshot), the `artifact`, `tasks` and `meeting` tests.

### Follow-ups

- UI strings over the vestigial status (a later wire + UI change):
  - `crates/desktop/ui/src/components/settings/App.tsx:97-98` (`Helper: theuth-documents …` / `Helper missing at …`).
  - `crates/desktop/ui/src/components/meeting/ControlCenter.tsx:95` (`no meeting note: the documents helper is missing`, quoted verbatim in meetings.mdx).
  - The Document.tsx strings.
- Vestigial wire fields (see above): `HelperStatus.path/found`, `HelperProcess::Missing`, `Crashed.stderr`, and the frozen doc sentences (including "Where [`helper_path`] points").
- Updater: prune old `theuth-documents-*` files beside `t` and stop accepting them in payloads.
- m1 remainder: gate requests in the actor until its reopen after `Restarted`.
