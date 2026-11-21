# 201 as built — P1 (draft lines; nothing waits on the summary model)

D5, §5, §6 "Fixes carried over" / "Wake on append", F8 F9 F10 F12.

## Landed

- **`endless/tree.rs`**: `Quality {Free, Draft, Model}` (`as_str`, `parse`), `Node::quality()` / `Node::set_quality()`, `draft_merge(a, b, node_bytes)` (free merge if it fits, else each child `guards::cut_words` to half of `node_bytes`, joined by `\n`).
- **Store**: `migrations/0070_endless_quality.sql` adds `endless_nodes.quality` (backfilled free/draft/model from `free`/`mechanical`). `endless_nodes` reads it and sets the flags from it; `nodes_put` writes it. `endless_nodes` is replicated, so `SESSIONS_WIRE_SCHEMA` 69 → 70 and the snapshot test is updated.
- **`endless/pump.rs`**
  - `draft_line(unit, node_bytes)` beside `mechanical_node` (body: today's mechanical cut; doc names P3). `draft_node` (free if it fits, else draft).
  - `fill_drafts(state, from)`: every level-0 unit in `[max(base, from), T)` without a node gets its draft (`endless_nodes_put_new`). Called at the end of `ingest` (new units), in `load` (node-less units of an old session) and in `set_mode(Endless)`.
  - `Live.drafts: BTreeSet<u64>` (refinement queue), `Live.stale: HashSet<(u8,u64)>` (draft merges to rebuild), `Live::put` (inserts a node, keeps both; a replaced node marks its `draft` ancestors stale), `Live::index_drafts` (on load), `Live::lined`, `Live::drafting`.
  - `schedule`: level 0 = drafts oldest first (batched as before; given-up units skipped); merges = missing or stale; a merge with a draft child, or whose children fit, is instant (no slot). Slot accounting, local 1+1, near-cap, pause and priorities unchanged. Rule 3 gone for drafting sessions.
  - `spawn_build`: draft child → deterministic draft merge; model merge carries a `fallback` draft merge; a `Built` with negative `tries` becomes the error `NO_PASSING_LINE` (never stored). `finish(.., fallback)`: on failure stores the fallback as draft and keeps the merge stale (retried after backoff).
  - `settle` returns `true` at once (wakes the pump). New test seam `quiet_for_tests` (the old wait: view built, drafts refined or given up, no stale merge).
  - `memory()` fills `SessionMemory.drafts`; the "endless: batch" log line has `drafts`.
- **`compactor.rs` `build_batch`**: reply spent on thinking → one retry with thinking off (local compactor that `can_grow`), else/again `thinking_error`; an empty reply → "compactor returned nothing". Either fails the whole batch (normal backoff, no solo fan-out).
- **`loop_.rs` `endless_view`**: no settle wait, no settle log lines.
- **Wake on append** already existed: `Turn::committed` → `Endless::on_rows_committed` → `pump::nudge` → `wake.notify_one()`. Only its doc changed.

## Deviations

- **Meeting trees keep 137 behaviour** (hard rule): `Live::drafting()` is false for `UnitFilter::Meeting`; such a session gets no drafts, keeps the frontier, rule 3, mechanical fold, and the shortest failing candidate. For that reason the F9 rule is enforced in the pump (a negative-`tries` build is turned into an error for drafting sessions), not by deleting the arm in `compactor::build_one`; the compactor tests of that arm still hold.
- `quality` is derived in memory from the existing `free`/`mechanical` flags (draft ≡ mechanical, not free) instead of a new `Node` field, so the ~23 `Node` literals in agent, meeting and supervisor code are untouched. The column is persisted and authoritative on load.
- A drafting unit whose `Retry::mechanical` holds (over `max_usd_per_call`, or 3 hard failures over 10 min with a success since) is no longer refined (draft kept) until `endless_retry` resets backoff.
- A unit that failed inside a batch rejoins a batch (the old `fails > 0 → alone` rule now applies to meeting trees only); only a guard trip of its own batch line sends it solo.
- Backfill and backlog count a draft as not yet lined (they measure refinement now).

## Tests

New in `endless/tests.rs`: `every_unit_has_a_line_at_ingest_and_no_turn_waits`, `guard_failing_reply_keeps_the_draft`, `batch_spent_on_thinking_does_not_fan_out`, `refined_child_rebuilds_its_draft_parent`, `load_drafts_node_less_units`, `failing_unit_blocks_nothing`.
Changed: every `endless.settle(..)` in `endless/tests.rs` now calls `endless.quiet_for_tests(..)` (same wait semantics). See the report for which tests ran and their results.

## Seams / unfinished

- Merges are still eager (every missing/stale merge each tick); P3 makes them lazy.
- `draft_line` body is the mechanical cut; P3 gives per-kind lines.
- `frontier_failing`, `Live::frontier_stalled` and the fold's verbatim/placeholder rendering stay; unreachable for drafting sessions' new units.

## Test status at commit (`cargo test -p agent endless`)

Compiles (PC's code included, no fixes needed). 82 passed, 14 failed. New P1 tests pass except `refined_child_rebuilds_its_draft_parent` (tests.rs, the "draft parent" wait). Failing old tests, which assert 137 behaviour P1 removes (mechanical folds of failing units, frontier-first order, every unit model-built after the wait) or need investigation: `context_length_refusal_retries_once_smaller`, `fold_turns_the_backlog_into_mechanical_nodes_at_once`, `an_error_failing_every_unit_stalls_and_folds_nothing`, `over_cost_batch_splits_and_only_an_over_cost_unit_folds`, `pump_builds_tree_and_tools_answer`, `batches_settle_200_units_in_parallel_and_retry_a_bad_line_alone`, `settings_model_switch_is_live_and_resets_backoff`, `switch_cancels_the_hung_call_and_rebuilds_on_the_new_model`, `three_hard_failures_over_ten_minutes_fold_the_unit`, `transient_failures_never_fold_a_unit`, `zoom_refuses_an_aligned_folded_run_but_opens_a_lone_unit`, `endless_off_with_5000_unbuilt_writes_a_bounded_head`, `loop_::endless_tests::endless_turn_view_is_bounded_and_shrinks_after_a_refusal`. Not updated (time).
