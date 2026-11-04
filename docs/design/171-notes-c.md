# 171 CELLS — notes of contract worker C

Decisions taken where the design or the brief was silent or contradictory (simpler option, contract intact). Every item is also in the SKELETON message.

## Deviations from §15 / the brief

1. **`CellState` lives in `cells::types`**, not `surface::cell` (brief item 2a): it holds `CellRun`/`CellRef`, which `surface` cannot see (dependency direction). `Problem`, `Chip`, `CellOptions`, `Expect`, `PythonChoice`, `Cell` are in `surface::cell` as specified and re-exported by `cells`.
2. **TS binding names**: five bare names were already bindings, so the ts-rs names are prefixed — `Scope` → `CellScope`, `RunId` → `CellRunId`, `RunMode` → `CellRunMode`, `RunState` → `CellRunState`, `surface::cell::Cell` → `CellSpec`. Rust names unchanged.
3. **`NoteKey` and `CellLatest` are defined in `document`** (`crates/document/src/cells.rs`) and re-exported by `cells`: `document` needs both (`CodeBlockText`, the `cell_latest` row) and must not depend on `cells`.
4. **`CellOutput`** is internally tagged by `output_type` and `ExecuteResult.count` is serialised as `execution_count` (nbformat v4 wire shape, so nteract's `OutputArea` consumes it unchanged). Rust field names are §15.1's.
5. **`CellEvent` variants carry `scope`** besides `run` (`RunQueued/RunStarted/RunFinished {run, scope}`) so the bridge/clients can filter without a read.
6. **`render::render(bytes, cols, rows, scrollback) -> Rendered {ansi, text, dropped_lines}`** instead of `-> String`: §8.3 needs both `out.ansi` and `out.txt` from one parse.
7. **E2's `RunAnnotations` registration** is `note::install(&cells)` called from `CellsPlugin::start` (`crates/cells/src/lib.rs`, marked `// 171:E2`), not a `crates/theuth/src/main.rs` line: the impl needs the `Cells` handle, which only exists inside the plugin start. main.rs carries no E2 line.
8. **Shell resolution** (`§6.5`, which bash) is `shell::resolve` in E1's `shell.rs`; `interp::resolve` (E2) is python-only and `interp::interpreters` lists shells through `shell::resolve`. W1 needs shells before E2 lands.
9. **`RunRequest` / `RunAllRequest`** wrap the RPC params plus `pressed_by` (filled by E1's RPC layer from the peer); internal RPC → actor types, not ts-exported (`dreaming` already owns the `RunRequest` binding name).
10. **`Cells::interpreters(cwd)`** has no `engine` param: routing to another engine is the RPC layer's (E1, `crate::cells::interpreters`).
11. **Error mapping**: `CellsError::NotRunning` → `-32001`, `NotOwner` → `-32002`, every other refusal → `CELL_REFUSED = -32020` with `data: {"reason": CellsError::code()}`; the message is the text the cell shows.
12. **`crates/rpc/src/api.rs`**: `authorize`, `authorize_opt`, `peer` and the `ASK_FAILED`/`NOT_RUNNING`/`UNAUTHORIZED` consts widened to `pub(crate)` so E1's `rpc::cells` can admit peers without editing `api.rs`. `pub mod api;` in `crates/rpc/src/lib.rs` carries `#[allow(clippy::too_many_arguments)]`: jsonrpsee drops method attributes and `cell_run` has §15.2's six params plus the extensions.
13. **A1's `first_run` line is a marker comment**, not live code: `if !cells::prompt::SKILL.is_empty()` on a const trips `clippy::const_is_empty`.
14. **Reserved migrations** `0002_cells_e1.sql` / `0003_cells_e2.sql` hold a comment and `SELECT 1;` (sqlx runs every file in `./migrations`; a worker replaces the `SELECT 1;`).
15. **StartPty construction sites** include `crates/sandbox/src/actor.rs`, so the check gate adds `-p sandbox`.
16. **`api.test.ts`**: its STREAM list gains `cell_attach`/`cell_detach` so the `methods.json` comparison stays satisfiable.
17. **`CellFence` is not registered** in `MessageAssistant.tsx` (marker only): streamdown's default path passes copy/download controls as `CodeBlock` children, so a fall-through to a bare `CodeBlock` would drop them — U1 wires it with the controls.
18. **`CellsPlugin` is registered** in both plugin lists of `crates/theuth/src/main.rs` (`// 171:E1`); its start spawns only the idle actor (no files, no events, no agent registration), which the RPC stubs look up.
19. **`HarnessDeliver`** gained `#[derive(serde::Deserialize)]` (it had none) for the `wake` default test.

## Grammar decisions (`surface::cell`, mirrored by U1's `grammar.ts`)

- A `#|` line that is not `key` / `key: value` stays in the header as `HeaderLine{key: <raw text>, value: None}` and in `options.unknown` → warning `unknown_option`.
- Repeated non-repeatable key: last wins. `#| key:` (empty value) on a valued key = `bad_option`.
- A value failing its type leaves the typed option at its default; `validate` reports `bad_option` (or `python_option`) from the raw header.
- `python:` values: `monty`, `auto`, `.venv` (serde `venv`), `uv`, an absolute or `~/` path.
- Warning codes (not in §3.6 by name): `unknown_option`, `on_not_yet`, `cwd_not_found`, `second_defaults`, `changing_without_label`, `monty_timeout`.
- `validate` never runs `monty_scan`; the engine runs `monty_scan` + the pool check and passes `ValidateCtx.monty_ok`.
- Secret names use the vault charset `[A-Za-z0-9_.-]+`.

## Tests I own

`surface/tests/cells.rs` (104 fixture cells + 5 note-defaults cases, digests filled, table tests), `cells::kernel::tests::cwd_rule_table`, `cells::types::tests::wire_shapes_are_stable`, `cells::config::tests`, `cells::store::tests::migrations_create_every_section_8_4_column`, `cells::note::tests::kernel_session_is_deterministic_per_doc`, `rpc::cells::tests`, `link::types::internal_tests`, `agent::supervisor::harness_deliver_tests`, `components/cells/CellFence.test.tsx`.
