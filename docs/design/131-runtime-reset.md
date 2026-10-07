# 131 RUNTIME-RESET — as built

Branch `fix/131-runtime-reset` on main 22c87abb. Brief: `omp-RUNTIME-RESET.md`.

## Problem

A schedule loop (`every: 0s`, `max_runs: 800`) runs every iteration in one session, clearing the
frame but never the python runtime. After 144 runs the session held 887 names and a 168 MB dump.
Dumping that heap copies it and crosses `MAX_MEMORY`; monty-pool kills the worker with a
`MemoryError`. `Repl::dump` then kept the dead checkout, wrote `names.json` anyway, and the next
cell hit `Finished`, restored an hours-old dump, and told the model "state restored to after the
previous cell". Every later turn restored the same 168 MB blob, so the loop sustained itself.

## As built

- **D1** `runtime: clear` (default) | `keep` front-matter key (`file::Runtime`, `parse_runtime`,
  error `runtime: expected clear|keep`), mirrored at every `memory` site in `file.rs` (Spec,
  FrontMatter, FileArgs, ScheduleFile, render). Unlike `memory` it is not watcher-only. The
  `schedule()` tool takes `runtime` and writes it only when `keep`. Fire path
  (`Schedule::fire`): inside the clear block, after the `Compact { Clear }` ask, when
  `resets_runtime(spec)` → `agents.ask(agent::ResetRuntime { session })`; a failure is
  `tracing::debug!` and never blocks the fire. A watcher with `memory: continue` never enters the
  block, so its runtime is kept whatever `runtime:` says. Prompt text and `routines.md` updated.
  `ScheduleInfo`, `RoutineDraft` and the TUI/desktop were left alone: neither mirrors `memory`
  in a one-line table (`RoutineDraft` is a field copy with a ts_rs type, the TUI hit is a test
  literal); `routine_draft.rs` passes `runtime: None` (file default). The key is settable via the
  tool and the file.
- **D2** `agent::ResetRuntime { session }` (lib.rs, beside `Compact`) → `Agents` routes to the
  session's native actor (`loop_::ResetRuntimeNow`) like `SetCompaction`. The session actor
  refuses with `Err("a turn is running")` when `inflight.is_some() || state.is_none()`; the
  `Repl` lives on `Turn`, which only exists inside a running turn, so that is the whole guard
  (the brief's "or `self.repl` is `Some`" has no separate field on `Session`). Otherwise
  `repl::reset_session(data_dir, session) -> io::Result<ResetReport { dump_bytes, names }>`
  removes `<session>.monty`, `.names.json`, `.lost`; never `.stale`, other sessions, the spill
  dir. One `info!` line; the report is the reply.
- **D3** `Repl::dump` on `Err(e)`: `checkout = None`, no `names.json` write, inventory rolled
  back to `last_dump`'s (`roll_back()`), `dead_cause = Some(e)`, and the lost line queued.
  `names.json` is written only after `.monty` was written, from the same inventory the dump
  was taken with. The dump now runs *after* the cell result is sealed, so the line opens the
  next cell; at turn end `Repl::close` writes it to `<session>.lost` (atomic), which
  `Repl::open` reads once and deletes.
- **D4** `Checkout::worker_id()` is `None` once the worker was taken (`poison`, `poison_timeout`,
  `discard_worker`; `pid()` is also `None` for websocket workers, so `worker_id` is the
  observable). `ensure_checkout` treats such a checkout as absent: drop, roll back, check out
  again, restore the snapshot.
- **D5** `last_dump: Option<Snapshot { bytes, names, imports }>`; `candidates` is derived.
  `Repl::open` builds it from the files; every restore site (`ensure_checkout`, the crash arm,
  D3, D4) resets the inventory to the snapshot. The crash message appends
  `; names bound since the last save are gone: …` when the rollback dropped names.
- **D6** `DUMP_HARD_BYTES = 64 MiB`, `DUMP_SOFT_BYTES = 32 MiB`. Measured with
  `repl::tests::measure_dump_knee` (`#[ignore]`, 8 MiB `bytes` chunks, real worker): under
  `MAX_MEMORY` = 512 MiB a 136 MiB dump succeeded and a 144 MiB dump killed the worker
  (`MemoryError`), so HARD ≈ knee/2 = 64 MiB (the default the brief named; the largest healthy
  dump on the machine was 46 MB, the failing one 168 MB). Under a 24 MiB limit the knee is
  16/17 MiB — the test uses 18 MiB objects. `Repl::open` checks the metadata length before
  reading; above HARD it renames `.stale` and clears the inventory. A dump above SOFT appends
  the footer once per turn (`size_warned`).
- **D7** The crash arm renders `runtime crashed (worker was lost earlier: <cause>; this call:
  <e>)` for `Finished` with `dead_cause` set; `dead_cause` is cleared by a fresh checkout. With
  D3 + D4 a dead checkout is never reused, so this path is defensive; the `MemoryError` reaches
  the model through the D3 line (asserted in `dump_failure_keeps_heap_and_inventory_in_step`).
- **D8** `max_checkouts_per_worker`, `MAX_MEMORY`, interactive `/clear`, pruning and debounce
  untouched.

## Invariant

`.monty` and `.names.json` are written together or not at all; `self.names` always describes
the heap in the worker (restore, rollback and dump move bytes and inventory as one snapshot).

## What the model sees

- `<runtime> the heap could not be saved after the previous cell (<e>); state is as of the last
  successful save — names bound since then are gone: a, b` (suffix omitted when empty).
- `<runtime> saved state (<N> MB) exceeds the restore cap (<M> MB) and was set aside as
  <file>.stale; the runtime is empty. It held: <names>.`
- `runtime heap is <N> MB saved (above <M> MB it is set aside at the next turn): del the large
  bindings you no longer need, or write them to a file first` (cell footer, once per turn).
- `RuntimeError: runtime crashed (<e>); state restored to after the previous cell — this cell's
  effects are lost[; names bound since the last save are gone: …]`.

## Open question

Should interactive `/clear` (`CompactStyle::Clear` from a person, design 11 §2.4) also reset the
runtime? Today it keeps it; the reset is wired to the schedule fire, not to the style.

## Tests

repl.rs: `dump_failure_keeps_heap_and_inventory_in_step`, `oversize_dump_is_set_aside_on_open`,
`soft_cap_footer_once_per_turn`, `reset_session_removes_dump_names_and_sidecar`,
`lost_sidecar_is_read_once_on_open`, `measure_dump_knee` (ignored). `monty_pool()` honours
`THEUTH_MONTY_WORKER=<binary>` before the PATH search; with `$CARGO_TARGET_DIR/debug/t` all 20
`repl::` tests run (9 need the worker and skip without it), 1 ignored.
schedule: `file::tests::runtime_key`, `tools::tests::render_args_runtime_key_only_when_keep`,
`fire_tests::resets_runtime_follows_the_clear_and_the_key` (no agent-actor double exists; the
decision is the pure `resets_runtime`). No routing test for `ResetRuntime`: `Compact` /
`SetCompaction` have none to copy.

## Gates (final head, rust 1.98.1, `CARGO_INCREMENTAL=0`)

- `cargo check -p agent -p schedule` — ok.
- `cargo test -p agent -p schedule --no-fail-fast` — agent 456 passed / 2 ignored, schedule 72
  passed. (A first run of this gate aborted the agent test binary with no failing test named;
  two reruns were green — not reproduced.)
- `THEUTH_MONTY_WORKER=$CARGO_TARGET_DIR/debug/t cargo test -p agent --lib repl::` — 20 passed,
  1 ignored (`measure_dump_knee`); 9 of the 20 need the worker.
- `cargo clippy -p agent -p schedule --all-targets -- -D warnings` — ok.
- `cargo fmt -p agent -p schedule` — clean for the touched files; it also reformats the
  untouched `agent/src/store/durability.rs` and `schedule/src/validate.rs` (pre-existing), that
  churn was reverted.
- `cargo check --workspace` — ok (pre-existing future-incompat note for `block v0.1.6`).
