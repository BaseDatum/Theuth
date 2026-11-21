# Documents helper in-process — phase 3 as built: the sidecar is gone

Branch `feat/documents-inproc`. Phase 3 of 3. Earlier phases: [P1](documents-inproc-as-built-P1.md) built the `document-js` crate, and [P2](documents-inproc-as-built-P2.md) put `Helper` on it. This phase deletes the stdio helper `theuth-documents`, its build, packaging and shipping, and the jsdom/React dependency tree. The only documents helper left is the bundle `t` embeds.

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
