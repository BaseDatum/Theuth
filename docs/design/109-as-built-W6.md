# 109 as-built — W6 (the crash-injection suite, `just durability`)

Branch `feat/109-durability`, on top of the Phase I integration (`109-as-built-I.md`). §11.1 made
exhaustive over every `crash::*` point the scripted turn reaches, the `kind = Suspended` runs,
the reaper's hand-off to a live jobs actor, and a `just durability` recipe. One file of tests,
extended rather than duplicated: `crates/agent/src/loop_/durability_tests.rs` (W1's harness, fake
provider and fake tools), from the `// W6` banner at `:824` on. No CI config exists in the repo
(no `.github/`, `.gitlab-ci.yml` or similar), so "`just durability` in CI" (§11.4) is the recipe.

## 1. The harness, generalised

- `FakeTool` records hits per **session** (`<hits>/<session id>`, `durability_tests.rs:114-138`) so
  parallel scenarios share the one process-global registry; every W1 scenario now reads
  `hits_file(hits, h.id)`. With `registry(…, crash: true)` each body carries its own
  `call.<i>.mid` point (`:185-211`), so `call.0.mid` (inside the read-only `probe`) is reachable,
  not only W1's `call.1.mid`.
- Two provider shapes (`:41-105`): `BatchFake` — `probe` (read-only) + `poke` (mutating) — and
  `BatchFake3`, the same plus the loop-local `todo` as a third call, whose `started`/`finished`
  rows the loop writes itself (§3.3 steps 6–7; registry tools write theirs inside the registry,
  where the loop has no crash point).
- The crashing life (`crash_child`, `:1298-1325`) sets `durability.partial_checkpoint_ms = 0` so
  `stream.after_partial` fires in a one-chunk reply.
- `crash_matrix` (`:1232-1266`) runs the crashing lives as children of the test binary,
  `CHILDREN_AT_ONCE = 6` at a time (`futures::stream::buffer_unordered`), then the second lives in
  sequence (the registry's journal is one store handle at a time). Every failed check is
  collected and reported together with its `shape/point`.

## 2. The matrix (`matrix()`, `:891-987`; checks in `second_life`, `:1022-1230`)

Per row the second life: `Harness::attach` on the same store, `unresolve_open_calls` (boot step
3), `open_turns`, `resume_plan`, `Turn::resume`, then the §11.1 checks — (1) transcript equals the
uninterrupted run's modulo the `<engine_restart>` line and the interrupted note; (2) `poke` ran
at most once across both lives and, when it did not run, the report names it interrupted; `probe`
once or twice; (3) every `tool_calls` row terminal, `turn_steps` ends `turn_finished`, no
`assistant_partial` row, `last_stop` set, no open turn; (4) for a re-issued model call the last
two request bodies the fake provider received are byte-identical (I5); (5) at most one
`request_id` has two attempts `n`, `n+1` — the first `unresolved`/`settled`, the second
`settled` — and none when nothing was re-issued.

| shape | point | life 1 ends with | second life | probe | poke | interrupted | re-issued |
|---|---|---|---|---|---|---|---|
| two | `input.before_persist` | nothing journaled, no turn | no open turn; the input is sent again (the ticket was never acked) → full turn | 1 | 1 | — | — |
| two | `input.after_persist` | `input_accepted` | §4.2 row 1: ordinary round | 1 | 1 | [] | no |
| two | `model.after_reserve` | `model_call` (never dispatched) | row 2: re-issued attempt 1 of the same request | 1 | 1 | [] | yes |
| two | `model.after_dispatch` | `model_call` (dispatch marker, no request sent) | same | 1 | 1 | [] | yes |
| two | `batch.after_tool_use_persist` | `tool_batch`, rows `planned` | row 4: both run, note prepended | 1 | 1 | [] | no |
| two | `call.0.before_started` | rows `planned` | both run | 1 | 1 | [] | no |
| two | `call.1.before_started` | rows `planned` | both run | 1 | 1 | [] | no |
| two | `call.0.mid` | `probe` body ran; `poke` planned or started (race) | `probe` again (read-only); `poke` run or interrupted | 2 | 0–1 | [] or [poke] | no |
| two | `call.1.mid` | `poke` body ran, row `started` | `poke` interrupted, note; `probe` reused or re-run | 1–2 | 1 | [poke] | no |
| two | `call.0.after_finished` | `probe` finished; `poke` planned/started/finished | as the row says | 1 | 0–1 | [] or [poke] | no |
| two | `call.1.after_finished` | `poke` finished; `probe` finished or still `started` | reuse / re-run `probe` | 1–2 | 1 | [] | no |
| two | `batch.before_message` | every row `finished` | row 3: message assembled from the rows, note prepended | 1 | 1 | [] | no |
| two | `batch.after_message` | batch message in the frame | ordinary round | 1 | 1 | [] | no |
| two | `stream.mid` | second `model_call` open (first text delta) | row 2: re-issued; request bytes identical | 1 | 1 | [] | yes |
| two | `stream.after_partial` | same + `assistant_partial` | `PartialDropped` published, partial deleted, re-issued | 1 | 1 | [] | yes |
| two | `stream.before_end_persist` | same, stream settled | re-issued | 1 | 1 | [] | yes |
| two | `turn.before_finished` | final reply not persisted | re-issued | 1 | 1 | [] | yes |
| two | `turn.after_finished` | `turn_finished` | nothing to resume; transcript already complete | 1 | 1 | — | — |
| todo | `call.2.before_started` | registry calls finished, `todo` planned | `todo` runs | 1 | 1 | [] | no |
| todo | `call.2.after_started` | `todo` `started` | §4.2 `todo` row: run again (its snapshot was never written) | 1 | 1 | [] | no |
| todo | `call.2.before_finished` | `todo` body ran, row `started` | run again | 1 | 1 | [] | no |
| todo | `call.2.after_finished` | every row `finished` | row 3: message from the rows | 1 | 1 | [] | no |

22 rows; "0–1" / "1–2" / "[] or [poke]" are the rows that race with the sibling call (the fake
`poke` waits for `probe`'s *body*, not its row) and admit both outcomes — the invariant checked
is the same in both: `poke` never ran twice, and when it did not run the model was told. Three
consecutive runs: `children 4.8–6.2 s, total 8.4–12.9 s`, all green.

Not in the matrix (why, and what covers it):

- **`python` cell steps.** The loop-local journal writes are the same four points as `todo`'s
  (`loop_.rs:4989-5010`, one code path for both); a cell needs a Monty worker pool in the child
  process and the fake tools exposed to the runtime index, which the harness does not do. The
  python rule itself (re-run when every inner call is read-only, the cell note otherwise, U's rows
  closed) is W1's `python_rule_both_ways` (`:592`), driven at the `Turn` level.
- **question / wait steps.** Driving a fake approval end to end through a *crash* needs a crash
  point inside the registry gate (the `tools` crate has none) — an in-process abort of the turn
  leaves the registry's suspended `invoke` alive, and on its later timeout it would close the
  replayed row. Covered by composition: the matrix replays `planned` rows through the registry
  gate on every `batch.after_tool_use_persist` / `call.*.before_started` row; W3's
  `superseded_on_reask_and_answered_row_honoured` drives `rebuild_waits` → `HonourAnswer` →
  `RequestApproval` (honoured / re-asked) and `wait_rearmed_with_remaining_deadline`,
  `replied_exchange_delivered_on_resume` the wait rows; `ask_user`'s republish is
  `ask::tests::republish_same_id` (`tools`). The glue `resume_question_call` → `ResumeQuestion` →
  `republish` → `Invoke` needs the `AGENT_ACTOR` registered — the manual smoke (§5) is its check.
- **`suspended` from a real `Drain`** — below, at the loop level; the `Drain` handler's own
  window/report logic is W5's `supervisor::drain::tests`.

## 3. `kind = Suspended` (`drain_suspends_and_the_next_start_resumes`, `:1328-1506`)

- `draining_session_opens_no_model_call`: `Shared.draining` set (what the drain's `Draining` tell
  does), `Turn::run` → journal `[input_accepted, suspended]`, no request reached the provider,
  the turn is open, `resume_plan` = `(Suspended, InputAccepted)`; the resume publishes
  `TurnResumed { kind: Suspended }` and the transcript equals the uninterrupted run's.
- `shutdown_token_leaves_the_batch_open`: `poke` held, `probe` finished; the drain's `suspended`
  step, then `rt.shutdown.cancel()` (the turn's token is its child). The turn returns; journal
  `[input_accepted, model_call, tool_batch, suspended]`, **no** batch message, rows
  `finished`/`started`. A fresh runtime resumes: report `interrupted = [poke]`, the note reads
  `kind="shutdown" replayed="0" interrupted="1"`, `probe done` reused, `poke` ran once across
  both lives, transcript equal modulo the note, `turn_finished` last. This is the regression test
  for the Phase I finding (the drain used to close the turn as `cancelled`).

## 4. Reaper hand-off (`reaper_hands_lost_rows_to_the_live_jobs_actor`, `:1508-1562`)

A `running` `jobs` row with a pgid no process has and a log file; `reap_jobs(&store)` (the real
entry, not W2's `reap` half) → report `(killed 0, lost 1, orphaned 0)`, row `lost`/`lost`,
and the registry's live `Jobs` actor (`JOBS_ACTOR`) receives `Reaped`: `jobs()` lists `j7 lost`,
the command, the log path, `secs ≥ 5` and the tail with the log's last line; a second pass is
empty. Together with W2's `reaper_kills_leaked_group` (a real `sleep 600` group outliving its
engine's runtime, killed by the reaper, `<job_result exit="lost">` body) and `tools`'
`reaped_jobs_list_lost_and_deliver_through_the_ledger` (the `Reaped` → ledger → `Settle` path
one `Seen::Result` per delivered job) this is the §11.2 reaper end to end. PTY snapshot + banner:
W2's `screen_snapshot_on_screen_and_timer` / `screen_after_restart_serves_banner` drive the real
PTY actor against a recording journal and the stored screen; not duplicated.

`fail_interrupted` narrowing + spared child result delivered exactly once: W1's store-level
`spared_child_results_are_listed` (`:777`, `result:` row spared, plain row failed) and W3's
`spared_child_results_delivered_once` (`supervisor/waits.rs`: one `enqueued` entry after two
passes, a result awaited by an open turn left for `ResumeWait`) — both in `just durability`.

## 5. `just durability` (`justfile:24-32`)

```
cargo test -p agent --lib -- loop_::durability_tests durability:: store::durability \
    supervisor::waits supervisor::drain supervisor::resume supervisor::acp_resume
cargo test -p tools --lib -- --test-threads=1 jobs:: pty::
```

Result: agent `24 passed; 0 failed; 1 ignored` (10.5 s), tools `49 passed; 0 failed` (60.6 s,
single-threaded). Wall 5 m 14 s on a cold target under `just`'s toolchain (rustup `stable`,
1.96.1 — the recipe prepends it; my shell's 1.98.1 target was not reused), of which ~71 s is the
tests. `#[ignore]`: only `crash_child` (the spawned life; a no-op without `THEUTH_109_*`).

## 6. Bugs found and fixed

None by the matrix itself: the 22 rows passed on the first run and on three repeats. The one
real bug of this wave — the drain closing turns as `cancelled` — was found in Phase I while
wiring `KillAll` and fixed there (`109-as-built-I.md` §2, commits 42ab3f94 and fdfe43d7); §3's
second scenario is its test.

## 7. Gates

All with `CARGO_TARGET_DIR=$HOME/Develop/theuth-109-integ-target`:

1. `cargo test -p agent --lib loop_::durability_tests` — `2 passed; 1 ignored`, ×4 (matrix
   timings above).
2. `just durability` — as §5.
3. `cargo test -p agent` — `414 passed; 0 failed; 1 ignored` (lib) + `0` (doc).
4. `cargo clippy -p agent --tests` — no warnings in `loop_/durability_tests.rs`, `loop_.rs`,
   `supervisor/{drain,waits}.rs`.
5. `rustfmt --edition 2024 crates/agent/src/loop_/durability_tests.rs`.

## 8. Manual smoke for Sky (§11.3; not run here)

Rebuild and run the engine on a scratch data dir:

```
export CARGO_TARGET_DIR=$HOME/Develop/theuth-109-integ-target
cargo build -p theuth
export THEUTH_DATA_DIR=/tmp/theuth-109-smoke && mkdir -p $THEUTH_DATA_DIR
$CARGO_TARGET_DIR/debug/t --headless &          # or `just desktop` with THEUTH_BIN set
```

1. In a chat, in one turn: `shell("cargo build -p agent 2>&1 | tail -20")` (long enough to be
   running), `start("bash -l")`, `remind_me_in(300, "x")`, and `ask_user` with one question. Do
   not answer the question.
2. `kill -9 $(pgrep -f 't --headless')`.
3. Start the engine again (same command). Expect, in order:
   - log `109: reaper: ReapReport { killed: 0|1, lost: 2, .. }` (1 if the `cargo build` group
     survived the kill), then `109: 1 open turn(s); durability.resume = Auto`, `109: resuming turn`;
   - the session shows the restart banner; the resumed batch's tool message starts with
     `<engine_restart … kind="crash" …/>`; the `shell` call's result is the job note
     (`ERROR: interrupted: … (job j1, started …); the job died with the engine …` + `Log:` + tail);
     the `start` call's result is the interrupted note plus the last screen;
   - the `ask_user` card is back with the **same** question and id (attention item `ask:<id>`);
     answer it — the turn continues with your answer as the call's result;
   - `jobs()` lists both jobs `lost` with their log paths; `screen(<pty id>)` shows the banner over
     the last prompt; `ps -ef | grep 'cargo build'` shows nothing leaked;
   - the reminder fires at its original time.
4. Repeat step 1, then `kill -TERM $(pgrep -f 't --headless')`: log `109: drain window closed`,
   `109: drained suspended=1`; exit within `drain_secs` (10 s). Start again: the banner says
   paused/stopped (`kind="shutdown"` in the note), both jobs `lost` with `killed at shutdown
   (engine stopped at …)`. Variant: have a `write_file` in flight at the kill — the file is
   complete and its call `finished` (reused, no note).
5. (W4 §8) Spawn an omp child on a worktree (`spawn_subagent` with `agent: omp`), let it start
   editing, `kill -9` the engine, start again: log `109: ACP child … respawned via session/load`,
   the child continues and its result reaches the parent. Then `rm` omp's session file for that
   child and repeat: the child restarts with the `<resume_handoff>` naming the uncommitted files.
6. `durability.resume: ask` in the config, repeat steps 1–2, start: `attention_list` shows a
   `resume:<session>:<turn>` item; press Resume (rpc `resume_turn`) → the turn resumes as in 3.

Files changed: `crates/agent/src/loop_/durability_tests.rs`, `justfile`,
`docs/design/109-as-built-W6.md` (this note).
