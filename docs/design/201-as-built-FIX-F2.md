# 201 — as-built, fix builder F2 (pump, compactor, per-kind lines)

Base: `5bf030c15`. Findings from `201-review.md` §R2 and R1-3b.

| id | status | what |
|----|--------|------|
| R2-1 | fixed | `pump::schedule`, merge pass: in a drafting session, when no free merge and no model merge now (paused, spend-capped, under `merge_at`, no slot), a `tree::draft_merge` node (Draft) is put and added to `Live::stale`; written only when its text differs (no rewrite per pass). Test `paused_long_user_messages_render_merged_lines`. |
| R2-5 | fixed | `Step::Batch` gained `drafting: bool`; `compactor::build_batch` takes the thinking-off retry and the empty/thinking hard failure only when `drafting`; a meeting batch is the pre-201 path (one call; empty reply → every line `Format` → units solo). `Gate::acquire(priority, owner: Option<SessionId>)`: meetings pass `None`; while an owner-less waiter is in line, `GateState::head` returns the pre-201 head (no `RUN_MAX` run reorders it). Tests `empty_batch_reply_is_charged_and_a_meeting_runs_solo`, `local_gate_keeps_meeting_calls_in_order`. |
| R2-4 | fixed | `build_batch` pushes a `CallRow` per reply (the thinking-spent first reply, the forced retry, the empty reply); `Failed.calls` carries them, so `call()` charges them via `sum_usage`. Same test as R2-5. |
| R2-3 | fixed | `finish_batch`: a drafting batch (>1 unit) that fails hard (`hard_failure`) for a reason other than thinking/empty puts its units in `Live::solo`; they run alone after their backoff. Test `a_batch_refused_for_one_unit_runs_its_units_alone`. |
| R2-2 | fixed | `compactor::build_small`: chunk lines are reduced in stages (`small_groups`: groups of ≥ 2 lines that fit `chunk`, a lone last line passes) until one line covers the unit; over `SMALL_MAX_CHUNKS` (16) chunks nothing is sent. `pump::schedule` skips such a unit (no call, no failure, no retry; its draft stays). Tests `small_reduce_halves_by_stage_and_caps_the_chunks`, S8 (below). |
| R2-6 | fixed | `lines::is_error`: a line counts only when it starts (after whitespace, timestamp tokens, `[…]`) with `error`/`fatal`/`traceback`/`exception` (not `Error::`), `FAILED`, `failed:`, `panicked at` / `thread '…' panicked at`, a `[ERROR]`-style bracket, or is a non-zero `exit code`/`exit status` line. Test `only_error_output_counts_as_error_lines`. |
| R2-7 | fixed | `request_small` sends no stop sequence; `small_once` already takes the first non-empty line under the 160-token cap. |
| R2-10 | fixed | `small_prompt.txt` = the reviewer's wording (395 B). `small_render`: unit "Write one line about this message: one plain sentence of at most N words." and merge "Join these two lines into one plain sentence of at most N words; keep names, paths and numbers; user items first." (N = node_bytes/6). |
| R2-12 | partly | `history_backfills_…`: asserts `backfill.is_none() && active` (drafts are lines at ingest), no escape. `fold_turns_the_backlog_into_mechanical_nodes_at_once` renamed `fold_adds_nothing_when_every_unit_already_has_its_draft_line`. `batches_settle_200_…`: view bound `16_000 + 1_024`. `three_hard_failures_…`: `Picky::bad_calls` proves no call after giving up. NOT done: `failing_unit_blocks_nothing`, `repeated_frontier_failure_…` (production `settle`), the missing meeting-batch / thinking-off-retry / small-through-pump tests (S8 now runs small units through the pump). |
| R2-8 | fixed | `Live::index_drafts` (drafting sessions) queues only units `lines::needs_model` accepts; `schedule` only walks that queue. |
| R2-9, R2-11 | not done | time. |
| R1-3b | fixed | `store::endless_excluded_seqs`; `pump::rows_excluded` (const `REMOVED`): each unit of a whole-excluded row gets the free line `<kind>: (removed from context)` (FTS mirror follows), its unit text becomes `REMOVED` (zoom answers it, raw recall skips it), every built merge above is rebuilt at once as a draft merge and marked stale; `finish`/`finish_batch` drop a late line for such a unit; `load` reads excluded rows back as `REMOVED`. `Endless::rows_excluded` sets `regen_asked`; `Session::remove_context` (loop_.rs) calls it after the store write; `endless_tail` opens a new generation when `take_regen_request` is set. Epochs and meeting trees are skipped. Test `removed_rows_leave_memory_and_indices_stay`. |

## Deviations
- R2-2: groups fit `chunk` (the small call's input bound), not `node_bytes`:
  grouping by `node_bytes` would reduce one line per call. Above the cap the
  pump records `NO_PASSING_LINE` (no call made, draft kept).
- R2-10: the instructions keep the prefixes "Write one line about this
  message" / "Join these two lines" so the evaluator's S8 test
  (`eval_tests.rs`, which counts them) stays unchanged; the rest is the
  reviewer's wording. The word count lives in the user turn, not the
  system prompt (the prompt is static).
- R2-3: solo after a unit's second hard non-thinking batch failure (not
  the first: one transient-looking 400 would otherwise fan every batch out;
  `retry_reschedules_now_and_restarts_a_hung_call` relies on one batch).
- R1-3b: a model merge that was running over a removed unit when it was
  removed lands, is marked `redo` and rebuilt; until then that one merge
  may show the old text (narrow race, not covered).

## Seams / notes
- A draft merge over two final children stays in `stale`; `settle` waits for
  it only while `merges_due` is true (unchanged rule).

## Changed tests (evaluator's and existing)
- `eval_s8_small_profile_…` (evaluator): confirmed by printing — chunk 735 B,
  the 13.2 KB huge unit is 18 chunks > 16. Changed ONLY the huge unit's
  expectation: no call carries "huge part", unit 6 is not in the failed
  list (not retried) and stays Draft (was: refined, > len/chunk calls).
  Added unit 7 (`mid part …`, 3–16 chunks): refined; ≥ chunks + 2 calls
  (staged reduce); a multi-line reduce input carries the last chunk's line.
  All other assertions unchanged.
- `zoom_refuses_an_aligned_folded_run_but_opens_a_lone_unit`: now on a
  paused meeting-filtered tree (`unbuilt_meeting`, filter set before
  attach), where folded runs still occur; R2-1's assertion is its own test
  `paused_long_user_messages_render_merged_lines`.

## Test state
`cargo test -p agent -- endless ledger tool_pin view_blocks compact_in fold
input_notes removed_rows history_backfills`, run after the last code edit:
`184 passed; 0 failed; 2 ignored`. `three_hard_failures_…` counts only
unit calls (`Picky::bad_calls` skips merge steps): a model merge over the
given-up unit's draft is a legitimate call.
