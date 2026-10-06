# 109 as-built — W1 (journal writes, `ResumeTurn`, safe replay)

Worktree `theuth-109-w1`, branch `feat/109-w1` off the contract commit 8a5f8b75. Built 2026-10-06.
Scope: the write points of §3.3 in the loop, the batch message assembled from the journal (D13),
`Turn::resume` with the §4.2 state machine (minus the job / question / wait rows, which route to
the W2/W3 stubs), `resume_scan` / `ResumeTurn` at boot, `repair_dangling` demoted, `last_stop` /
`finished_at`, the spared `result:` delivery (§7.2), the §3.2 cut treatment, and the tests.

Line numbers are of this branch's last commit.

## 1. Write points (§3.3) — `crates/agent/src/loop_.rs`

| step | where | store fn (`store/durability.rs`) | one transaction with |
|---|---|---|---|
| 1 `input_accepted` | `Turn::take_input` LOOP_TAKE_INPUT | `Store::append_input` STORE_APPEND_INPUT | the user row (`insert_in`) |
| 1 (peer) | `Turn::consume_peer` LOOP_CONSUME_PEER | `Store::input_accepted` | *its own* tx after `consume_exchange` (deviation 1) |
| 2 `model_call` | `Reserved::open` LOOP_RESERVED_OPEN | `Store::reserve_journaled_call` STORE_RESERVE | `reserve_call_in` (`accounting.rs` ACCOUNTING_RESERVE_IN) — `Round` calls of a turn only |
| 3 `assistant_partial` | `Turn::stream_once` LOOP_STREAM_ONCE (`checkpoint_now`) | `Store::partial_upsert` (contract) | its own small tx; at a block boundary (reasoning→text, text→tool-use) or when `partial_checkpoint_ms` passed and the text grew; never per token |
| 4 `turn_finished` + §4.5 | `Turn::round` text reply LOOP_FINAL | `Store::append_final` STORE_APPEND_FINAL → `turn_finished_in` STORE_TURN_FINISHED_IN | the final assistant row, the partials' DELETE, `sessions.last_stop/finished_at` |
| 4' `turn_finished` (cancel / error / landing / after a continuation) | `Turn::drive` end LOOP_DRIVE_FINISH | `Store::finish_turn` | its own tx |
| 5 `tool_batch` + `planned` rows | `Turn::round` LOOP_TOOL_USE | `Store::append_tool_use` STORE_APPEND_TOOL_USE | the assistant `tool_use` row, the partials' DELETE, `calls_planned_in` (`risk` from `Turn::risks`, filled in `rebase` LOOP_RISKS; `read_only` for `todo`/`python`/unknown names) |
| 6–7 registry calls | `tools::registry::invoke` hook (contract) | `StoreJournal` | — |
| 6–7 `python` / `todo` / refused names | `Turn::run_batch` LOOP_RUN_BATCH (`journal_started` / `journal_finished`) | `call_started` / `call_finished` | their own tx, immediately before / after the body |
| 7 spill cap | `Turn::finish_batch` LOOP_FINISH_BATCH | `Store::append_batch` STORE_APPEND_BATCH (`BatchRow.output`) | the batch message: every row's `output` is overwritten with the capped text the model sees (addendum: batch-end cap, not in `StoreJournal`) |
| 8 batch message | `Turn::finish_batch` | `Store::append_batch` | the plan it changed (`put_todos`), the rows' capped `output`, the `skipped` rows |
| 9 cancel / landing | `Turn::finish_batch` (`skipped[i]`) / `Turn::run_batch` landing arm | `Store::append_batch` `BatchRow { skipped: true }` | the batch message |

The model-facing message (step 8) is built from `calls_of(session, turn)`: for every call the terminal
row's `output` / `is_error` / `files`; the in-memory result only when no terminal row exists (a
refused store write), the cancel text when the call never ran. On a normal run rows and memory are
the same bytes; on a resumed run they differ by the §4.3 notes.

Crash points (`durability::crash_point`, right after each commit, the contract's constants):
`input.before_persist` / `input.after_persist` (take_input, peer path after-only), `model.after_reserve`
/ `model.after_dispatch` (Reserved::open), `stream.mid` (first text delta), `stream.after_partial`,
`stream.before_end_persist`, `turn.before_finished` / `turn.after_finished` (both around `append_final`
and around `finish_turn`), `batch.after_tool_use_persist`, `call.<i>.before_started` (registry calls:
before the `Invoke` is sent; local calls: before `call_started`), `call.<i>.after_started` /
`call.<i>.before_finished` (local calls only), `call.<i>.after_finished` (registry calls: after the
`Invoke` returned; local: after `call_finished`), `batch.before_message` / `batch.after_message`.
`call.<i>.mid` is a tool's own (the test's fake tool calls it).

## 2. Resume (§4) — `loop_.rs`, `supervisor/resume.rs`

- `Session` message `Resume { plan, slots }` LOOP_RESUME_MSG: claims the state like `start_turn`,
  spawns `Turn::resume(plan, slots)`; `false` when a turn is in flight.
- `Turn::resume` LOOP_TURN_RESUME: adopts `plan.turn`, takes one `resume_parallel` slot (held to the
  end), then `drive(turn, Some(plan))`. `drive` LOOP_DRIVE publishes `TurnResumed` first, runs
  `rebase`, then `resume_prelude` LOOP_PRELUDE decides the first step:

| newest step | built |
|---|---|
| `input_accepted` | ordinary `round` |
| `model_call` | `PartialDropped` if a partial exists, `partial_delete`, `Turn::resume_call = (request_id, attempt + 1)`; `Turn::call` LOOP_CALL takes it for the first round call (same `request_id`, accounting `attempt + 1`; this process's backoff index is separate), no note to the model |
| `tool_batch`, frame still ends in the `tool_use` row | `resume_batch` LOOP_RESUME_BATCH: per-call rule → `run_batch` with presets → `finish_batch` with the restart note |
| `tool_batch`, batch message already in the frame (`batch.after_message`) | ordinary `round` |
| `turn_finished` | not resumed (`resume_turn` returns) |
| `suspended` | `kind = Suspended` → `kind="shutdown"` in the note; otherwise identical |

Per-call rule (`resume_started` LOOP_RESUME_STARTED), tool-specific rows first:
`shell`/`start`/`send`/`wait` → `resume_calls::resume_job_call` (W2 stub); `ask_user`/`desktop_handoff`/
`secret_request*` → `resume_question_call` (W3); `spawn_subagent`, `send_message{await_reply}` →
`resume_wait_call` (W3); `python` → python rule (`resume_python` LOOP_RESUME_PYTHON); `todo` → run again;
`remind_me_in` → a `reminders` row of this session with `set_at ≥ started_at` is the result (`reminder_since`),
else run again; then by risk: `read_only` → `call_replan` (planned, `attempt + 1`) and run; `mutating` /
`destructive` → `call_interrupted` with the interrupted note. A stub/rule result with `is_error` is
written as the row's `interrupted` note (`ERROR:` stripped, re-added by the batch), a plain result as
`finished`. `finished` / `skipped` / `interrupted` rows are reused; `planned` rows run.

Python rule: `inner_calls_of(session, cell)`; every inner call `read_only` by this turn's `risks` →
re-plan and run (the fresh `Repl::open` restores the previous dump); else the cell note, `inner_calls_interrupt`
closes U with `interrupted by an engine restart; effect unknown`, row → `interrupted`.

`ResumeReport { replayed, interrupted, lost_jobs: [], children_*: 0 }` is published after the resumed
batch's message (or right after the prelude for the other rows).

Lazy path (`resume: off|ask`, or an input that beat the scan): `Turn::settle_open_turns` LOOP_SETTLE runs
in `Session::start_turn` before the opener's row and in `Turn::run` before its inputs. A dangling `tool_use`
whose ids match an open `tool_batch` is answered from the rows — finished reused, every other row
`interrupted` with the note, nothing re-run (the model has moved on) — and every other open turn of the
session gets `turn_finished { stop_reason: "superseded" }`. `repair_dangling` LOOP_REPAIR is reached only
when no journal answered the dangling row (pre-0033 transcripts); its text is unchanged.

Boot (`supervisor/resume.rs`): `resume_scan` → `open_turns()` (most recent first); `auto` → `resume_turn`
per row with one shared `Semaphore(resume_parallel)` the turn tasks acquire (the supervisor never waits on it);
`ask` → nothing (the open set is `OpenTurn`'s; W5's item); `off` → nothing. `resume_turn` = `resume_plan`,
`Agents::session(Some(id), …)` (the lazy load), `actor.ask(Resume)`; an ACP actor is logged and left to W4.
`ResumeTurn` (tell) = `resume_turn(session, turn, None)`. Then `deliver_spared_results` (§7.2): every
`accepted` `result:` exchange from a `session:*` sender (`Store::spared_child_results`,
`store/communication.rs` STORE_SPARED) → `enqueue_peer` under the `enqueued` dedup set, `fail_exchange`
+ `Error` on refusal. `Agents::session` reads `Store::last_stop` into `Agents.last_stop` (§4.5) SUP_LAST_STOP.

## 3. §4.3 wording as shipped

- Restart note (first response of a resumed batch, before the clock stamp), `loop_.rs` `restart_note`:
  `<engine_restart at="HH:MM:SS" after="Ns" kind="crash|shutdown" replayed="R" interrupted="I"/>`
  (`replayed` = `started` rows run again; `interrupted` = rows marked `interrupted` by this resume).
- Interrupted note: the contract's `resume_calls::interrupted_note` —
  `ERROR: interrupted: the engine restarted at HH:MM:SS while this call was running (started HH:MM:SS); its effect is unknown — check before repeating it.`
  (stored on the row without `ERROR: `; the batch re-adds it.)
- Cell note, `loop_.rs` `cell_note`:
  `ERROR: RuntimeError: the engine restarted at HH:MM:SS while this cell ran; state restored to after the previous cell — this cell's effects are lost from the runtime, not from the world. Inner calls that finished before the restart: name (ok), name (error). Started, outcome unknown: name.`
  (`none` when a list is empty; `ERROR:` from the batch, as every error result.)
- U's inner rows: `interrupted by an engine restart; effect unknown`.
- Lazy close of a superseded batch: the interrupted note per non-terminal row + the restart note.
- Pre-0033 repair: `ERROR: not run: the harness exited before this tool completed` (unchanged).
- Landing: rows `skipped` with `not run: tool rounds exhausted`; cancel: `cancelled by user`.

## 4. Store additions (`store/durability.rs`, `store.rs`, `store/accounting.rs`, `store/communication.rs`)

`append_input`, `input_accepted`, `reserve_journaled_call`, `append_tool_use`, `append_final`,
`finish_turn`, `append_batch` (+ `BatchRow`), `partial_delete`, `call_replan`, `last_stop`,
`inner_calls_of`, `inner_calls_interrupt`; free `turn_finished_in`, `cut_journal_in`.
`Store::commit_cut` STORE_COMMIT_CUT deletes `waits` (by the cut calls), `tool_calls`, `turn_steps` with
`at_seq >= cut_at` (§3.2). `reserve_call` is now a wrapper over the new `reserve_call_in`.
`Store::spared_child_results` (communication.rs). No existing signature changed.

## 5. Tests — `crates/agent/src/loop_/durability_tests.rs` (one `#[test]`, scenarios in sequence)

Fake provider (`BatchFake`): first main call → a batch `probe` (read-only) + `poke` (mutating); any call
carrying a tool message, or a prompt saying `just talk` → the final text. Fake registry tools record
each invocation in a hits file (cross-process), `poke` runs after `probe`, can block (in-process
interruption) or abort at `call.1.mid`.

1. `normal_turn_journals_every_write_point`: steps `input_accepted, model_call, tool_batch, model_call,
   turn_finished`; both rows `finished` with the tool text, attempt 1, risks `read_only`/`mutating`;
   `last_stop = stop`; no open turn; message built from the rows.
2. `interrupted_mutating_call_is_reported_not_rerun`: the turn task is aborted with `poke` `started`;
   resume → `poke` `interrupted` with the exact note, `probe` reused, `poke` ran once across both
   lives, restart note `replayed="0" interrupted="1"`, `ResumeReport.interrupted == [poke]`, transcript
   equals the reference modulo the tool message.
3. `interrupted_read_only_call_runs_again`: `probe` `started` → re-run as attempt 2, `replayed == [probe]`,
   transcript equals the reference.
4. `open_model_call_drops_the_partial_and_reissues`: a journaled `model_call` with a partial →
   `PartialDropped`, `TurnResumed { partial_dropped: true }`, `model_calls` = `(req-1, 0, unresolved)`,
   `(req-1, 1, settled)`, steps end `model_call, turn_finished`, no tool ran.
5. `python_rule_both_ways`: two `started` cells; all-read-only inner calls → `planned`, attempt 2;
   a mutating unfinished inner call → the cell note, U closed, row `interrupted`.
6. `stale_batch_closes_from_the_journal_and_pre_0033_repairs`: a fresh turn over an open batch →
   restart note + reused `probe` + interrupted `poke`, nothing re-run, old turn `turn_finished`; the
   same dangling row without a journal → the verbatim repair text.
7. `spared_child_results_are_listed`: `fail_interrupted` fails a plain exchange and spares the
   `result:` one; the boot reader lists `(parent, child, id)`.
8. `crash_matrix` (§11.1, subprocess): the test binary is spawned as a child (`crash_child`, an
   ignored test driven by `THEUTH_109_*` env) with `THEUTH_CRASH_AT` at `batch.after_tool_use_persist`,
   `call.1.mid`, `batch.before_message`, `turn.before_finished`; the child must die; the parent runs
   `unresolve_open_calls`, finds the open turn, resumes, and asserts: `poke` ran exactly once, `probe`
   once or twice, interrupted ids per point, every row terminal, steps end `turn_finished`, no
   partial rows, `last_stop` set, transcript equals the uninterrupted run modulo the notes, and for
   `turn.before_finished` the lost attempt `unresolved` with a settled `attempt + 1` of the same request.
   The contract offers no in-process hook (`crash_point` only aborts), so the child process it is.

Harness changes (`loop_/vision_fallback_tests.rs`): `Harness::new` takes any `Respond`,
`Harness::attach(dir, endpoint, session)` for a second life / process, `endpoint()`, `pool()`-based reads;
`sse` / `body` / `FINAL` / `MAIN_USAGE` `pub(super)`.

## 6. Deviations from the design / brief

| what | design says | built | why |
|---|---|---|---|
| peer input step | `input_accepted` in `take_input`'s transaction | after `consume_exchange`, own tx | `consume_exchange` is 50 §7's transaction in `communication.rs` (not owned); a kill between the two leaves a consumed exchange and no step |
| `input_accepted.payload.input_id` | the caller's `InputId` | `null` for user/replay rows, the exchange id for peer rows | `Input` does not carry the `InputId` into `take_input`; `seen` stays process-local |
| `model_call` step | every `reserve_call` | `Round` calls of a turn only | a compaction/vision/title call interrupted mid-turn resumes correctly from the previous step; re-issuing a non-round request under its id would be wrong |
| `assistant_partial.content` | text and tool-call arguments | the text so far | arguments stream as partial JSON; the UI renders text |
| restart note | "the first model input after a resume" (brief) / the resumed batch (design) | the resumed batch only | §4.2 row 2 / I5: the re-issued call must be byte-identical |
| `tool_batch` row 3 (all finished) | assemble and persist the message | only when the frame still ends in the `tool_use` row | `batch.after_message` leaves the same journal shape with the message already persisted |
| cancel path rows | planned → `skipped`, started → `finished` with the cancel text | both `skipped` with `cancelled by user` | one `BatchRow` shape; the model text is identical |
| `turn_finished` for Yield-then-continue | one row per turn | written with the final reply; a continuation that keeps the turn alive appends further steps and a second `turn_finished` at the end | the newest-row semantics of `open_turns`/`resume_plan` stay right |
| lazy resume (`off`/`ask` + input) | the per-call rule runs then | rows answered without running (finished reused, the rest `interrupted`) | the new input's row would follow the `tool_use`; running tools under a turn the model has moved past is not what the user asked for |
| `resume: ask` | publishes an Attention item | logs; `OpenTurn` answers W5 | the contract has no event for it; W5 owns `roster/attention.rs` |
| rounds counter after a resume | — | restarts at 0 | the plan does not carry the round; `max_tool_rounds` counts from the resume |
| landing | — | not journaled; a crash inside the landing batch resumes the planned rows and runs them | the landing call's rows are `planned`; low frequency |
| `remind_me_in` row | `{id, fires_at, seconds}` | the tool's own reply text + `structured` from the `reminders` row | same bytes as a fresh call |

## 7. Cross-seam touches

- `crates/agent/src/store/accounting.rs`: `reserve_call` split into `reserve_call_in` + wrapper
  (no signature change) so the `model_call` step shares its transaction.
- `crates/agent/src/loop_/vision_fallback_tests.rs`: harness generalised (above).

## 8. Not done / open items

- `ask` mode publishes nothing (W5 renders from `OpenTurn` / `open_turns`).
- `seen` dedup by `input_accepted.payload.input_id` (§4.5 `(call)`) — `input_id` is not recorded.
- The spared-result delivery is exercised at the store level only; `Agents::deliver_spared_results`
  itself has no test (no supervisor harness without a registry in this crate's tests).
- `fail_interrupted` narrowing was already in the contract; verified by its test and scenario 7.
- `Reserved::open` for a `Round` call outside a turn (`turn: None`) journals nothing.

## 9. Gates

GATES
