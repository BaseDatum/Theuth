# 171 CELLS — as built (integrate/171)

Status: merged and gated on `integrate/171`; the live proof (§17.4) has NOT run, because the memory gate held for 30 minutes (below). Nothing is pushed or merged into main. Per-worker detail: `171-notes-{c,e1,a1,u1,d2,e2,u2,z}.md`.

## What was built where

| owner | where | what |
|---|---|---|
| C | `crates/cells/src/{types,error,config,kernel,lib}.rs`, `migrations/0001`, `crates/surface/src/cell.rs` (+ `tests/cells.json`), the `171:C` RPC + export blocks, seams in tools/document/link, UI types | frozen contract: grammar (`parse`, `validate`, `lint`, `digest`, `monty_scan`), wire types, `Kernel` trait, 17 `cell_*` RPCs |
| E1 | `crates/cells/src/{actor,actor/chat,actor/tests,shell,run,render,redact,store,deliver}.rs`, `migrations/0002`, `crates/rpc/src/{cells,cells_guard}.rs`, `crates/tools/src/{pty,jobs}.rs`, `crates/platform/src/env.rs` | the cells actor, shell PTY kernels (`cell:` jobs, L1), run store, delivery (L2/L3), press guard, login env, L11 fields |
| A1 | `crates/agent/src/{supervisor,repl,…}.rs`, `crates/surface/src/{prompt,cell_link}.rs`, `crates/cells/src/{prompt,tools}.rs`, slack/telegram/signal one line each, `crates/theuth/src/main.rs` | `<cell_result>` delivery and quiet park, cell-job watcher (no `<job_result>`/`<job_idle>`), `<cell_error>` pass in `check_fences`, prompt block, `cell_runs`/`cell_output`/`interpreters`, run links (dropped from voice clips), `t --update --stop-cells`, HANDOFF-Z-1 in `Agents::place` |
| E2 | `crates/cells/src/{python,shim.py,monty,interp,note}.rs`, `crates/rpc/src/cells_latest.rs`, the `171:E2-ADD` block in `crates/document/src/cells.rs` | python shim + kernel, Monty kernel, interpreter discovery + chips, note scope helpers, `cell_latest` replication + guest RPC, `RunAnnotations` |
| D2 | `crates/document/src/note_cells.rs` (+ actor/store) | `CodeBlockText` with last author, `CheckCells` on agent writes, `reader_markdown`, `cell_latest` table |
| U1 | `crates/desktop/ui/src/components/cells/*` (chat), `App.tsx` pre-quit sheet, `crates/desktop/src/quit.rs` hold/cancel | chat cell, run card, kernel strip, live terminal, quit sheet |
| U2 | `components/cells/{NoteCell,GuestCell,CellDefaults,NoteKernelPill,note-edit}.*`, `editor/CodeBlock.tsx` routing | note cells, guest view, defaults line, Make it a cell |
| Z | wiring + fixes (below) | integration |

## Z's wiring and fixes (after the six branches)
- Note scope in `Cells::{prepare, state}` (one `inspect_note`): live block text with its WHOLE-text digest (= U2's `seen`), names and `after` edges from the note, `note::defaults_of` + `note::cell_cwd`, `ValidateCtx{chat:false, defaults, second_defaults, monty_ok}`.
- **Invariant 6 (security):** `note::member_guard` on every note press; `cell_state` fills `member_edit` and `stale`. Test `cells::actor::tests::member_edit_needs_the_members_ack` (fails before: the press ran).
- **Private note runs:** `deliver::cell_result` (the note path) took no `private`, so a private note cell's output reached the origin session. It now reads `name`/`private` from the record. Test `deliver::tests::cell_result_takes_name_and_private_from_the_record`.
- `cell_output_file` serves `code` (the stale and member views read it). Test `actor::tests::output_file_serves_images_and_code_only`.
- `note::with_in` replaces E1's bare `in="note <doc>"` with the titled one (test extended in `note::tests::result_attributes_and_run_line`).
- Note defaults reach `resolve` (`shell:`, `python:`); `auto → Monty` when no python is found and `monty::sufficient` passes (this cell only — ponytail-marked).
- One chip function: `interp::chip` (python/Monty), which calls `shell::chip` for shells.
- `cell_runs` `name`/`private` from L11; `CellState.host` = the peer's mesh name (else the id prefix).
- `shellscript` (BlockNote's stored language) = bash in Rust + TS, parity case `lang-shellscript-is-bash`.
- `doc_export` goes through `document::reader_markdown`, so `.md`/PDF exports carry the run lines.
- Tray: `desktop::tray_menu` restored on `quit-cancel`.
- UI: `CellEvent` in `WireEvent`; `cells/wire.ts` deleted (its importers use the generated types); gutter `▶` → `►` (no-emoji test).
- Removed: `CellsError::NotBuilt` and every stub path behind it (`git grep NotBuilt crates/cells crates/rpc` is empty).

## Deviations from the design (and why)
- **No agent migration** (§15.5). Note-kernel PTY jobs log journal errors because `sessions.sqlite` enforces foreign keys (E2 §18.3; the premise "FKs off" was wrong). Consequence: a kernel PTY left behind by an engine SIGKILL is not reaped at boot.
- **Quiet delivery** = in-memory park (`QuietInput`); toward a remote owner the outbox kind is prefixed `quiet `. `harness_outbox` has no wake column and no migration was allowed (A1).
- **Monty:** Stop/Kill/timeout kill the kernel and its variables (monty-pool cannot resume). The sufficiency check is a type check of the code wrapped in an uncalled `async def`. Two pool checks remain (`cells::monty::sufficient` caches and answers `None` when unchecked; `agent::repl::monty_sufficient` answers `Ok`); both use the one static rule `surface::cell::monty_scan`.
- `monty_sufficient` is async and takes the pool (A1 STUB-DEVIATION). E1 keeps a local copy of surface's crate-private fence scanner (`actor/chat.rs`).
- Note `.md`/`doc_read` annotations are synchronous from an in-memory cache, so the first `doc_read` after a restart can lack run lines.
- Note cells show their `#|` header lines (BlockNote text cannot hide lines). "Make it a cell" is desktop-only (phones have no block side menu).
- Agent authorship of a note block lives only on the engine that wrote it (sidecar `agents.json`). After a hub move, agent-written blocks count as the owner's.

## Lead pins
L1 `cell:` job names (E1 start/send/cancel/jobs refusals; A1 watcher) — built, tests `tools::pty::tests::cell_kernel_{kill,exit}_never_reaches_the_model`, `supervisor::tests::cell_jobs_are_never_job_result_or_job_idle`. L2 `run=` first — `deliver::tests::attributes_in_pin_order_and_body`. L3 `HarnessDeliver{wake}` — built. L4 `cell_error` in `check_fences` — built. L5 crate direction — holds (`cells` depends on agent/tools/surface/document…, nothing depends back). L6 note scope via `note.rs` hooks — wired by Z. L7 quit (`cell_live`, `t --update`, pre-quit sheet) — built. L8 temp hygiene — kept. L9 ADDs: `171:E2-ADD` (document), `171:A1-ADD` (surface `cell_link`), `171:U1-ADD`/`171:U2-ADD` (UI). L10 data-dir paths — every cells path is under the engine data dir. L11 wire fields — in the types, bindings and UI.

## Merge record
A1 25da2e85 → U1 ff9ddc76 → D2 2865d21f → E2 031ebd4c → U2 fe528654 → fold main 6f5e32cc → E1 ef30d5bb → fold main 7b9011aa (176).
- `Cell.tsx` footer (U1 R1 vs U2): union.
- `crates/document/src/cells.rs` ADD block: one copy.
- `supervisor.rs` at the `Agents` spawn (`cell_jobs` vs 176's `idle_stop`): union.
- Everything else merged clean. `cargo metadata --locked` resolves.

## Gates (after the 176 fold, d3d19490)
| gate | result |
|---|---|
| `mbx check` 13 crates `--tests`; `--workspace --exclude desktop`; `-p desktop` | ok |
| `mbx clippy --no-deps -D warnings` (13 crates + desktop, re-run for agent/rpc/theuth/cells/desktop after the fold) | 0 warnings |
| `mbx test -p cells -p surface -p tools -p platform -p link -p document -p rpc` | 642 passed, 0 failed |
| `mbx test -p agent -p cells` with `THEUTH_MONTY_WORKER=THEUTH_WORKER=/tmp/171-z/bin/t` | agent 678 passed, 1 failed: `repl::tests::capping_binds_out_name` (on main per 176, not ours); cells 82 passed |
| Monty, one by one (no skips) | `sensor::tests::fs_exists_through_monty` PASS · `sensor::tests::monty_budgets_and_allowlist` PASS · `sensor::tests::pr_watch_type_checks` PASS · `loop_::runtime_bridge_tests::guessed_direct_calls_are_bridged_or_refused` PASS · `cells::monty::tests::{static_scan_refuses_without_a_worker, names_persist_output_at_the_end_and_last_expression_is_plain_text, cwd_is_mounted_read_only, stop_kills_the_checkout_and_the_kernel_is_dead, type_check_decides_sufficiency}` PASS · `cells::interp::tests::no_python_falls_back_to_monty_only_when_sufficient` PASS · `agent::repl::tests::monty_sufficient_scans_then_type_checks_without_running` PASS |
| `mbx test -p desktop --lib` | 69 passed |
| `bunx tsc -b --noEmit` | 0 errors |
| vitest (cells, lib, chat, editor, notes, notebooks, share, api.test) | 187 files, 1292 passed |
| §17.5: diff of `tools/src/registry.rs`, `tools/src/approval.rs`, `crates/double` vs origin/main | empty |
| bindings | regenerated after each fold (`methods.json` has `ping`, `client_log`, the `cell_*` set) |

## Live proof (§17.4)
**NOT RUN.** The brief allows the scratch engine only while `vm_stat` (free + speculative) × 16 KiB > 4 GB. Readings, every 5 minutes for 30 minutes (22:50–23:20): 2.19, 2.09, 0.35, 1.75, 1.59, 1.17, 0.73 GB. Per the brief I reported instead of starting. Ready for a re-run:
- `/tmp/171-z/bin/t` (folded build) with `theuth-app-parser-5e864f05…` and `theuth-documents-59.5` beside it;
- `/tmp/171-z/eng/config.yaml` (block style, plugins off except what the steps need);
- the fake world `/tmp/171-z/{fakebin,home,dr-t6/…}`;
- the driver `/tmp/171-z/drv.ts`.

| step | result |
|---|---|
| 1 DR runbook · 2 zsh trap · 3 secrets · 4 agent cannot run · 5 double · 6 restart → lost · 7 graceful quit · 8 notebook note, add-ons a–f | not run (memory gate) |

Engine-level coverage of the same invariants (unit/e2e, green): `actor::tests::e2e_real_shells` (real bash/zsh: PIPESTATUS + trailing comment, input, Stop under a flood, masks, private), `press_refusals`, `run_all_stops_before_danger_and_blocking_lint`, `after_gate_with_edited_and_old_previews`, `restart_settles_lost_without_replay`, `idempotent_run_ids`, `member_edit_needs_the_members_ack`, `rpc` guard tests (guest/internal peer refused), A1 `cell_jobs_are_never_job_result_or_job_idle`.

**Safety incident (Z, 22:35):** while checking the fake stubs, a bare `aws s3 ls` ran with the shell's normal PATH and reached the real AWS CLI: a read-only bucket listing of the machine's default profile. Nothing was changed, and no credentials were read or printed. Since then every fake has been invoked by absolute path only.

## Open items
- The live proof (above) and its add-ons: (a) caret on typing `#|` into a `shellscript` block; (b) `t --update` listing/cancel/no-TTY refusal (code path read: both exits return before `update::run_once`); (c) member edit; (d) private run evidence; (e) `.md` export; (f) channel link vs voice clip.
- Stop and `cell_attach` for a Run-all pass forwarded to ANOTHER engine are not routed there. Keystroke counts are stored under `owner` (E1).
- The Telegram soft-cap split restarts the ordinal count. A quiet result to an ACP runner still wakes it (A1).
- The Monty sufficiency check can flag a cell that uses names from earlier Monty cells (E2). `auto → Monty` checks only the pressed cell, not the scope.
- The agent copy of a note python run carries its text stream only (no rich outputs).
- "Open as note" confirms with a toast (no chat link card); a notification click only focuses the app; python execution counts have no wire field (U1). The shell chip's "write note default `shell:`" is not built (U2).
- A note kernel PTY leaked by an engine SIGKILL is not reaped (no `sessions` row, FKs on).
- W3 REACH (§17.3) — JS kernels, the Jupyter backend, `.ipynb`, the TUI run key, `on:` targets, guest images, the detached PTY keeper — not started, as designed.
