# 171 CELLS — file ownership for W1 CHAT-SHELL + W2 PYTHON-NOTES

Status: committed FIRST by contract worker C (draft by the lead, 2026-11-03 18:30; every path verified against the tree at a0e6ef08 by C on 2026-11-03; corrections marked *(C)*; owner assignments unchanged). Design: `171-cells.md` §15, §17.

## Rules
1. A worker edits only the files its row owns, plus NEW files it creates inside its owned directories/prefixes. A new file outside them needs the lead (say so in DONE).
2. **Frozen after the SKELETON** (everything under C): public types, trait and function signatures, RPC method shapes, grammar, fixtures. Do not change or remove anything frozen. If a frozen item is wrong, build a local adapter in your own file and report `STUB-DEVIATION <file> <what>` in your final message; as a LAST resort you may ADD (never change/remove) a variant/field/fn to a frozen file, marked `// 171:<TAG>-ADD`, and list it in your final message (Z resolves adjacent additions as a union, the lead relays to dependents).
3. **Registration lines**: C puts one marked line per worker in every shared file (`// 171:E1`, `// 171:A1`, `// 171:U1`, `// 171:D2`, `// 171:E2`, `// 171:U2`; `{/* 171:U1 */}` in TSX). A worker fills only its own marked line(s).
4. `Cargo.lock` and workspace `Cargo.toml`: C only. C declares every Rust dependency the workers plausibly need in `crates/cells/Cargo.toml` up front; a worker needing one more adds ONE line there marked `# 171:<TAG>` and says so in DONE. Never hand-edit `Cargo.lock` (mbx regenerates it; Z takes main's and regenerates).
5. Tests: each worker owns the tests of its files (new test files, inline `#[cfg(test)]`, `*.test.ts(x)`), named after the module.
6. Each worker writes `docs/design/171-notes-<tag>.md` (deviations, decisions, test list) — its own file; nobody else edits it.
7. Never touch another design's work: 172/173/174 seams are listed in each brief.
8. `bindings/` and `crates/desktop/ui/src/rpc/bindings/` are gitignored: never commit them.
9. Dependency direction: `cells → agent, tools, surface, document, platform, secrets, runtime, link`; NEVER `agent|tools|document|surface → cells`. `rpc` and `theuth` depend on `cells`. The prompt block reaches the agent through `tools::RegisterPrompt{plugin:"cells"}` (precedent `crates/app`), the doc_read run lines through a hook trait defined in `document` (cells registers an impl).

## C — contract (frozen after SKELETON)
- `Cargo.toml` (`cells = { path = "crates/cells" }`), `Cargo.lock`
- `crates/cells/Cargo.toml`, `src/lib.rs`, `src/types.rs`, `src/config.rs`, `src/kernel.rs` (the `Kernel` trait, `ExecuteRequest`, `KernelEvent`, `KernelCtx`, `start_kernel` dispatcher with one marked arm per implementer), `src/error.rs` (`CellsError`, `NotBuilt(tag)`), `migrations/0001_cells.sql` (every table/column of §8.4), `migrations/0002_cells_e1.sql` and `0003_cells_e2.sql` (empty, reserved; registered in the migration list)
- creates as neutral stubs, ownership passes to the worker named: `src/actor.rs` (E1; the `Cells` handle's public method set is frozen), `src/shell.rs`, `src/run.rs`, `src/render.rs`, `src/redact.rs`, `src/store.rs`, `src/deliver.rs` (E1), `src/prompt.rs`, `src/tools.rs`, `cells.md` (A1), `src/python.rs`, `src/monty.rs`, `src/interp.rs`, `src/note.rs`, `shim.py` (E2)
- `crates/surface/src/cell.rs` (grammar + validation + lint + digest, FULLY implemented), `crates/surface/tests/cells.json` (+ the Rust test reading it), the `pub mod cell;` line in `crates/surface/src/lib.rs`
- `crates/rpc/src/api.rs` (all §15.2 method declarations + the `cell_attach` subscription, AND the delegating server lines — *(C)* the `impl RpcServer for Server` block lives in `api.rs` itself, precedent `autoresearch_*` at ~:7361; `crates/rpc/src/engine.rs` is the engine lock file and is NOT touched), `crates/rpc/src/export.rs` (all new wire types, one contiguous block marked `// 171:C`), `crates/rpc/src/lib.rs` (`mod cells; mod cells_latest;`), `crates/rpc/src/bridge.rs` *(C)* (one line subscribing `cells::CellEvent` onto the wire, precedent `autoresearch::StudyEvent`), `crates/rpc/Cargo.toml` *(C)* (`cells` dep + `cells/ts` in the `ts` feature); creates stubs `crates/rpc/src/cells.rs` (E1) and `crates/rpc/src/cells_latest.rs` (E2)
- `crates/desktop/ui/src/rpc/api.ts` (`Api` interface + `METHODS`)
- `crates/link/src/types.rs` (`RpcPeer::is_internal`, implemented) and the `GUEST_METHODS` entry `cell_latest` in `crates/link/src/conn.rs` (`// 171:E2`)
- neutral field additions: `tools::pty::StartPty.program/.cell` + `Mark`/`MarkKind` (`crates/tools/src/pty.rs`) with every construction site updated — *(C)* the sites are `crates/tools/src/pty.rs` (test), `crates/agent/src/durability/jobs.rs`, `crates/sandbox/src/actor.rs` (C touches only the struct literal in each); `agent::supervisor::HarnessDeliver.wake` (serde default true) with every construction site updated (`crates/agent/src/supervisor.rs`, `crates/sessionsync/src/sync.rs` ×2)
- `crates/document`: the `CodeBlockText` message + reply type, `RunAnnotations` hook trait + setter, `CellLatest` type and stub accessors, marked stub arms `// 171:D2`
- `crates/desktop/ui/src/components/cells/types.ts` + one stub component file per UI worker (below)
- registration lines in the shared files named in the other rows; *(C)* `crates/theuth/src/main.rs` lists plugins TWICE (`autoresearch::AutoresearchPlugin` at ~:1815 and ~:1939): C adds `cells::CellsPlugin` to both lists (marked `// 171:E1`), and `crates/theuth/Cargo.toml` (`cells` dep)

## E1 — engine shell + run store + render + redact + delivery + RPC handlers + press guard + login env + quit support
- `crates/cells/src/{actor.rs, shell.rs (+ shell/**), run.rs, render.rs, redact.rs, store.rs, deliver.rs}` and new modules declared from those files; `ShellConfig` (defined in `shell.rs`, `#[serde(flatten)]`ed by C's `config.rs`)
- `crates/tools/src/pty.rs`, `crates/tools/src/jobs.rs`: mark scan in `Log::push`, `program` honoured, `cell` jobs omitted from `jobs()`, refused by `send`/`cancel_job`
- `crates/platform/src/env.rs` (`login_env`)
- `crates/rpc/src/cells.rs` (+ `crates/rpc/src/cells_guard.rs`): the press guard (invariants 1, 14), `authorize`, `forward::route`, every handler body except `cell_latest`
- `crates/theuth/src/main.rs` lines marked `// 171:E1` (start the plugin, wire `cell_live` to the quit path); `crates/update/**` and the `t update` CLI path only for the F6 "list running cells / wait / stop / --stop-cells" behaviour
- tests: invariants 1-5, 7-11, 13-15 of §12.1 and the §17.4 engine unit tests (mark scan across chunk splits, render of `\r` progress and a pulumi spinner, credential masks, digest guard, `after` with edited/old previews, idempotent run ids, danger refused for line/Run all, internal/guest peers refused, `send`/`cancel_job` refusal, `jobs()` omission)

## A1 — agent
- `crates/agent/src/supervisor.rs` (parking for `wake: false`; skip `cell` jobs: never `<job_result>`/`<job_idle>`), `crates/agent/src/loop_.rs` (`prompt_blocks` unattended arm `"app" | "cells" => None`; the `check_fences` cell pass + `<cell_error>` once per turn, hidden like `<surface_error>`), `crates/agent/src/lib.rs` and other agent files only as those two need
- `crates/cells/src/{prompt.rs, tools.rs}`, `crates/cells/cells.md` (the `## User cells` block of §10.1, the skill of §10.2-10.5, `RegisterPrompt`, the three read-only runtime functions `cell_runs`/`cell_output`/`interpreters` — `Risk::ReadOnly`, registered by the plugin start line `// 171:A1`)
- `crates/surface/src/prompt.rs` (`cell_error`, the sentence at ~:289) and `crates/surface/visualize.md` (sentence at ~:42)
- `crates/surface/src/cell_link.rs` (NEW; pure helper that appends the "Run in theuth: theuth://c/<session>?cell=…" line after each cell fence) and ONE call-site line in each of `crates/slack`, `crates/telegram`, `crates/signal` where outbound markdown is composed (164's picture fences untouched) — assigned by the lead; design §17 names no owner
- `crates/theuth/src/main.rs` line marked `// 171:A1` (write `cells.md` in `first_run`)

## U1 — chat UI
- `crates/desktop/ui/src/components/cells/{Cell.tsx, CellFence.tsx, RunbookBar.tsx, KernelStrip.tsx, ResultCard.tsx, QuitSheet.tsx, grammar.ts, theme.ts, vendor/**}` + tests
- `crates/desktop/ui/src/components/chat/MessageAssistant.tsx` (`CellFence` plugin + `CellOrdinals` provider), `crates/desktop/ui/src/lib/injected.ts` (EXACTLY two map entries: `CARD.cell_result`, `HIDDEN.cell_error`), `crates/desktop/ui/src/components/chat/TerminalViewer.tsx` (`source` prop for `cell_attach`/`cell_input`, 167's keys kept), `crates/desktop/ui/src/routes/c.$session.tsx` (kernel strip on the composer; `?cell=` scroll), `crates/desktop/ui/src/App.tsx` (pre-quit sheet), `crates/desktop/ui/src/styles.css` (`--ansi-*` tokens), `crates/desktop/ui/package.json` + `crates/desktop/ui/bun.lock` *(C)* (the three new deps: `@uiw/codemirror-extensions-langs`, `anser`, `escape-carriage`)
- `crates/desktop/src/lib.rs`: ONLY the `deep_link_route` region (keep `?cell=`); never the `tauri_nspanel::init` line (174's)
- tests: grammar fixtures shared with Rust (`crates/surface/tests/cells.json`), ordinary fences still render as code, states/chips, phone key row

## D2 — documents
- `crates/document/**` only: the `CodeBlockText` handler (yrs read of the live Y.Doc), `doc_read` markdown with the `output` fence + run line through the `RunAnnotations` hook (§4.4), the `cell_latest` table beside `sources` in `store.rs` + its read, tests; `packages/documents` untouched

## E2 — python + monty + interpreters + note scope
- `crates/cells/src/{python.rs, monty.rs, interp.rs, note.rs}`, `crates/cells/shim.py`, `migrations/0003_cells_e2.sql`; `PythonConfig`/`MontyConfig` defined in their own files and flattened by C's config
- `crates/rpc/src/cells_latest.rs` (the `cell_latest` handler), the `GUEST_METHODS` test in `crates/link/src/conn.rs`; the `RunAnnotations` impl is registered by `note::install(&cells)`, already called from `CellsPlugin::start` (`crates/cells/src/lib.rs`, `// 171:E2`) — *(C)* not a `crates/theuth/src/main.rs` line (the impl needs the `Cells` handle, which exists only in the plugin start); main.rs has no E2 line
- the note-scope arms in E1's actor are the marked `// 171:E2` calls into `note.rs` (C places them)

## U2 — notes UI (cut from U1's DONE sha)
- `crates/desktop/ui/src/components/cells/{NoteCell.tsx, CellDefaults.tsx, NoteKernelPill.tsx, GuestCell.tsx}` + tests
- `crates/desktop/ui/src/components/editor/CodeBlock.tsx` (the `cellOf` branch), `crates/desktop/ui/src/components/editor/Editor.tsx` (side-menu items + slash "Cell"), `crates/desktop/ui/src/components/notes/NotePage.tsx` and `crates/desktop/ui/src/components/notebooks/NotePane.tsx` (header pill), `crates/desktop/ui/src/components/share/GuestApp.tsx` and `crates/desktop/ui/src/components/notebooks/NotebookGuest.tsx` (guest view of replicated latest outputs)

## Z — closer (integrate/171)
- Everything at integration: merges in owner order C → E1 → A1 → U1 → D2 → E2 → U2, the fold of origin/main, conflict resolution (union for adjacent lines), bindings regeneration, gate fixes, live-proof fixes with fail-before tests, `docs/design/171-as-built.md`, the "Status: BUILT" paragraph of `docs/design/171-cells.md`.

## Frozen API (C, SKELETON)

Signatures below are frozen; bodies of files owned by a worker are theirs. `git grep -n NotBuilt crates/cells | wc -l` = 21: code sites E1 12 (actor.rs 9, shell.rs 2, deliver.rs 1), E2 5 (note.rs 2, interp.rs 1, python.rs 1, monty.rs 1); doc lines 2 (actor.rs, lib.rs); the `CellsError` variant + its `code()` arm 2 (error.rs).

*(C)* files C also touched beyond the rows above: `crates/surface/Cargo.toml` (sha2, hex, regex), `crates/document/src/{cells.rs (new; D2 owns it after the skeleton), lib.rs}`, `crates/desktop/ui/src/rpc/api.test.ts` (STREAM list += `cell_attach`/`cell_detach`), `docs/design/171-notes-c.md`.

| file | frozen public names | implementer |
|---|---|---|
| `crates/cells/src/types.rs` (C) | `CellRef{Chat{session,entry,ordinal},Note{doc,key}}`, `Scope{Session,Doc}` (TS `CellScope`), `ShellKind`, `KernelSpec{Shell{kind,path},Python{path},Monty}`, `KernelId{scope,spec}`, `KernelPhase`, `KernelState`, `RunMode{Cell,Line{line},Selection{from,to}}` (TS `CellRunMode`), `RunId(String)` (TS `CellRunId`), `RunState` (TS `CellRunState`), `ExitStatus{Code,Signal}`, `ExpectResult`, `CellRun`, `CellOutput` (nbformat, tag `output_type`), `Armed`, `CellState`, `InterpreterKind`, `Interpreter`, `StateReply`, `RunGet`, `OutputPage`, `CellEvent{RunQueued,RunStarted,RunFinished{run,scope},KernelChanged{kernel},CellsChanged{scope}}`, `RunRequest`, `RunAllRequest`, `Scope::key`, `CellRef::scope`; re-exports `surface::cell::*` grammar types, `document::{CellLatest,DocId,NoteKey,Principal}`, `tools::pty::{Mark,MarkKind,PtyChunk}` | — |
| `crates/cells/src/error.rs` (C) | `CellsError{NotBuilt,NotRunning,NotOwner,Changed,Superseded,Disarmed,Danger,MemberAck,Invalid,FolderNotFound,KernelBusy,RunIdConflict,NotFound,Private,NoInterpreter,Failed}`, `CellsError::code()`, `Result` | all |
| `crates/cells/src/config.rs` (C) | `CellsConfig` (§15.4 keys + flattened `shell`/`prompt`/`python`/`monty`), `CellsConfig::from_runtime` | — |
| `crates/cells/src/kernel.rs` (C) | `trait Kernel{spec,phase,execute,input,interrupt,kill,shutdown,live_cwd}`, `ExecuteRequest{run,code,mode,cwd,env,timeout}`, `KernelEvent{Started{log_from},Bytes,Mark,Output,InputRequest{prompt,password},Finished{exit,log_to,kernel_died}}`, `KernelSink`, `KernelCtx{id,owner,login_env,paths,config,jobs,root}`, `CellsPaths{root}` (+`db,shim,rc_dir,kernel_dir,run_dir`), `start_kernel` (arms `// 171:E1` Shell, `// 171:E2` Python, `// 171:E2` Monty), `needs_prologue(mode,this,prev)` | E1 shell, E2 python/monty |
| `crates/cells/src/lib.rs` (C) | `PLUGIN`, `DIR`, `SKILL_PATH`, `CellsPlugin` (start: `actor::start` `// 171:E1`, `note::install` `// 171:E2`, `tools::register` `// 171:A1`) | — |
| `crates/cells/src/actor.rs` (E1) | `CELLS_ACTOR`, `CellsActor`, `Handle`, `start(rt,cfg)->Cells`, `Cells::{lookup,config,paths,publish,run,run_all,stop,input,ask,state,run_get,output,output_file,runs,interpreters,kernel_restart,kernel_close,env_refresh,live,latest,attach,detach}`; `// 171:E2` calls into `note::source`/`note::latest`/`interp::interpreters` | E1 |
| `crates/cells/src/{shell,run,render,redact,store,deliver}.rs` (E1) | `shell::{ShellConfig,start,resolve}`, `run::{check_expect,summary,armed}`, `render::{Rendered,render(bytes,cols,rows,scrollback)}`, `redact::mask(text,vault)->(String,u32)`, `store::{MIGRATOR,Store{pool},Store::open}`, `deliver::{cell_result,deliver}` | E1 |
| `crates/cells/src/{prompt,tools}.rs`, `cells.md` (A1) | `prompt::{PROMPT,SKILL,PromptConfig}`, `tools::register(rt,cells)` | A1 |
| `crates/cells/src/{python,monty,interp,note}.rs`, `shim.py` (E2) | `python::{SHIM,PythonConfig,start}`, `monty::{MontyConfig,start,sufficient}`, `interp::{interpreters(cwd,env),resolve(choice,cwd,env,monty_ok)}`, `note::{Route,NoteSource,kernel_session (implemented),install,route,source,origin,deliver,latest}` | E2 |
| `crates/cells/migrations/` | `0001_cells.sql` (C, frozen: `cell_runs` + 2 indexes, every §8.4 column), `0002_cells_e1.sql` (E1), `0003_cells_e2.sql` (E2) | — |
| `crates/surface/src/cell.rs` (C, implemented) | `Lang`, `lang_of`, `is_runnable`, `HeaderLine`, `Cell` (TS `CellSpec`), `CellOptions`, `Expect`, `PythonChoice`, `NoteDefaults`, `Severity`, `Problem{code,severity,message,key}`, `ValidateCtx`, `ChipKind`, `Chip`, `parse`, `parse_defaults`, `digest`, `validate`, `lint`, `lint_scripts`, `monty_scan`, `parse_duration`; fixtures `crates/surface/tests/cells.json` | U1 mirrors in `grammar.ts` |
| `crates/tools/src/pty.rs` (C fields) | `Mark{kind,exit,cwd,offset}`, `MarkKind{Ready,Start,End,Cwd}`, `StartPty.program`, `StartPty.cell` | E1 |
| `crates/link/src/types.rs` / `conn.rs` (C) | `RpcPeer::is_internal()` (implemented); `GUEST_METHODS` += `cell_latest` | — |
| `crates/agent/src/supervisor.rs` (C field) | `HarnessDeliver.wake` (serde default true), `HarnessDeliver: Deserialize` | A1 |
| `crates/document/src/cells.rs` (C stubs → D2) | `NoteKey{Name,Block}`, `CodeBlockText{doc,key}` → `Result<CodeBlockTextReply{block,language,text,last_author},String>` (`// 171:D2`), `trait RunAnnotations::note_cell(doc,block,language,text)->Option<CellAnnotation{outline,output,run_line}>`, `set_run_annotations`, `run_annotations`, `CellLatest`, `Store::{cell_latest,put_cell_latest}` (`// 171:D2`) | D2 (E2 implements `RunAnnotations`) |
| `crates/rpc/src/api.rs` (C) | the 16 methods + `cell_attach`/`cell_detach` (names/params of §15.2), one delegating line each; `authorize`/`authorize_opt`/`peer`/`ASK_FAILED`/`NOT_RUNNING`/`UNAUTHORIZED` are `pub(crate)` | — |
| `crates/rpc/src/cells.rs` (E1) | `CELL_REFUSED = -32020`, `cells_err`, `handle`, one `pub(crate) async fn` per method (`run, run_all, stop, input, ask, attach, state, run_get, output, output_file, runs, interpreters, kernel_restart, kernel_close, env_refresh, live`), `Latest` | E1 |
| `crates/rpc/src/cells_latest.rs` (E2) | `latest(server, ext, doc)` | E2 |
| `crates/desktop/ui/src/components/cells/types.ts` (C) | `CellViewState` (14 states of §11.2), `HostChip`, `CellCallbacks`, `CellProps`, `OutputViewProps` | U1, U2 |
| `crates/desktop/ui/src/components/cells/*.tsx` stubs | U1: `Cell`, `CellFence` + `CELL_LANGUAGES`, `RunbookBar`, `KernelStrip`, `ResultCard`, `QuitSheet`, `grammar.ts` `cellOf`, `theme.ts` `cellTheme`; U2: `NoteCell`, `CellDefaults`, `NoteKernelPill`, `GuestCell` (+ `<Name>Props`) | U1, U2 |
| `crates/desktop/ui/src/rpc/api.ts` (C) | `Api.cell_*` (16, positional), `METHODS` += 16 names; `cell_attach` via `client.stream<PtyChunk>('cell_attach', [run], 'cell_detach', …)` | — |
