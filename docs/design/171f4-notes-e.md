# 171F4-E — notes (engine: Run all race, fragment results, repeated results, line echo)

Worker E, branch `fix/171f4-e`, base `a76092c4`. Decisions where the brief left a choice.

## T2-1 — Run all stopped on a spurious "failed step"

- `Live.done` is one `oneshot::Sender<CellRun>` created in `enqueue` with the live entry;
  `enqueue` returns `(CellRun, Receiver)`. `cell_run` drops the receiver; `drive_pass` awaits
  it. The old "finished before we could wait" fallback (`find_run` on the stored row) is gone.
- `finish()` no longer takes the live entry first: it snapshots record/expect/stop, writes the
  files and `store.finish`, and only then removes the entry, publishes `RunFinished` and tells
  the waiter. Only the executing run's owner (`execute_one`) removes an executing run, so no
  other path races this. A reader (`find_run`, `run_get`, a second press) now sees the live
  record until the row is final, never an ended run's unfinished row.
- `end_unstarted()` keeps take-first: that take is the claim between a Stop and a kernel
  close; its waiter already heard only the final record (sent after the row).
- Test hook `State.race` (cfg(test)): `finish` sleeps 500 ms before `store.finish`; the pass
  waits until the run has left the live set before it waits on it. On the old ordering both
  new tests failed (`stopped at py0 "failed", 1/5` with the hook; `py1 "failed", 2/5` without
  it — Sky's repro); with the fix both pass. `State.delivered` (cfg(test)) records every
  `<cell_result>` handed over, built from the delivered record.

## T2-2 — fragment runs

- `finish()` judges `expect` only for `RunMode::Cell`; a line/selection run's state is by exit
  (and Stop/timeout) only, no verdict.
- `<cell_result>` gains `mode="line"` / `mode="selection"` after `secs`; whole-cell runs carry
  no `mode` (unchanged).

## T2-5 — repeats of one cell

- Where: `agent::loop_::Session::join_quiet`, the one place parked results meet the turn
  they join (the next user message, a pass-end wake, a review wake). `fold_cell_repeats`
  keeps each cell's newest result at the newest's position and adds `runs="N"`.
- Cell identity from the tag: `name`, else `cell` + `message`, plus `in` (the note). A result
  naming no cell (no `name`/`cell`) is never folded. A ticketed result (Ask theuth, a review
  wake) is owed and never dropped; it is folded into only as the newest.
- `earlier="…"`: the earlier runs whose `exit` differs from the newest's, counted as
  `k failed` (any exit but `0`) and `k ok` (exit `0`), e.g. `earlier="2 failed"`,
  `earlier="1 ok"`, `earlier="2 failed, 1 ok"`. Same exit as the newest: not mentioned.
  `exit` alone decides (a missed expect with exit 0 counts as ok) — simple, per the brief.
- Consumption: the dropped runs' ids are not in the turn's text, so `Store::consumed` now
  also consumes, for each named run, the earlier (lower rowid) parked runs of the same `cell`
  or `lineage` in the scope; they would otherwise be re-parked at every engine start. Earlier
  runs stay in `cell_runs`.
- Not folded: results a running turn takes at a batch boundary (the passive steer queue,
  `drain_steer`); the brief names parking and pass-end/review batching only.

## T2-6 — line run of a bare expression

- shim.py: the last bare expression is shown in every mode (the `mode` filter is gone); a
  statement line prints nothing. Monty: the `Line` exclusion is gone; a statement's value is
  `None`, which shows nothing. Selection and cell runs unchanged. `171-cells.md` §5.8 frame 1
  updated.

## For the other worker (D)

- `<cell_result>` may now carry `mode="line"|"selection"`, `runs="N"` and `earlier="…"`
  (all after `run=`, which stays first). The transcript card / user-bubble row reads the
  turn's text, so a folded result shows once; if the card wants to say "×5", read `runs`.
- A line/selection run of a cell with `expect` is `done` on exit 0 with `expect: []` — the
  gutter/fold should not show an expect verdict for it.
- T2-6: a line run of a bare expression now has an `execute_result` output; a statement line
  still has none (the "ran" mark is yours).
