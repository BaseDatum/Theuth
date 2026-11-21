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

## Continuation: the 14 red tests (verdicts)

Code fixes:
- **Stale-merge race (real bug)**: a child refined while its draft parent was being rebuilt lost the rebuild (the landing node cleared `stale`). `Live.redo` marks a running ancestor; its node lands still stale. Ancestor marking is now limited to drafting sessions (meeting trees as in 137).
- `settle` ingests (no model call) before waking the pump and returning `true`.
- `fold_all` cancels level-0 calls only when it has units to fold (a drafting session has none; its refinement goes on).
- `quiet_for_tests`: a given-up draft no longer counts as "failing".
- **Backfill ended only at refinement (real regression of D5)**: `Live::backfill` counted a draft as not done, so `active()` (the endless frame) waited for the model. It now counts built units (drafts included); the backlog still counts drafts. This reverses the earlier P1 deviation "Backfill … count a draft as not yet lined" for backfill.
- **Turn view over budget (real regression of G1)**: drafts are built lines, so with merges not built the fold caps at `high() + raw_bytes`, above `view_bytes + raw_bytes`; and the fold sizes lines without their `id+n|` prefix and newline (~1.6 KB over on 5,000 units). `render_within` now scales against `high() + raw_bytes` and re-renders smaller (at most 3 renders) while the text exceeds `max_bytes`.

Verdicts (R = real regression, fixed in code; N = asserted removed 137 behaviour, test rewritten to the §5/§6 behaviour):
- `refined_child_rebuilds_its_draft_parent` — R: the race above; test unchanged.
- `context_length_refusal_retries_once_smaller` — N: "every unit built" held at ingest before any call; now waits for every draft refined. Retry-smaller assertions unchanged.
- `fold_turns_the_backlog_into_mechanical_nodes_at_once` — N: units already have draft excerpts; asserts the view is built with ≤ node_bytes drafts before any call, fold adds nothing, no failure; no cancel of the refinement call; the stored-draft count is level 0 only (draft merges carry the flag too).
- `an_error_failing_every_unit_stalls_and_folds_nothing` — N: drafts carry the `mechanical` flag; asserts every unit keeps its draft, is never given up, stalled shown.
- `three_hard_failures_over_ten_minutes_fold_the_unit` — N: asserts the bad unit keeps its draft, unit 0 refined meanwhile, still retried under 10 min, given up (draft kept, no model line) after.
- `transient_failures_never_fold_a_unit` — N: asserts the draft stays and the unit is never given up.
- `over_cost_batch_splits_and_only_an_over_cost_unit_folds` — N: waits for four refined units; the over-cost unit keeps its draft; batch/solo counts unchanged.
- `batches_settle_200_units_in_parallel_and_retry_a_bad_line_alone` — N: paused view has drafts and the hard-cap prefix run (no unsummarized run); waits for 200 refined nodes; batch/solo/parallel assertions unchanged.
- `zoom_refuses_an_aligned_folded_run_but_opens_a_lone_unit` — N: the folded run is now the hard-cap prefix ("older lines folded to fit the view"); alignment/zoom assertions unchanged.
- `endless_off_with_5000_unbuilt_writes_a_bounded_head` — N: bound unchanged; head text is the prefix line.
- `endless_turn_view_is_bounded_and_shrinks_after_a_refusal` (loop_) — R + N: the view exceeded its budget (117,607 B; fixed in `render_within`); settle now returns at once (timed); prefix line text.
- `pump_builds_tree_and_tools_answer` — R: `active()` false after attach (backfill waited for refinement); fixed in `Live::backfill`, test unchanged.
- `settings_model_switch_is_live_and_resets_backoff`, `switch_cancels_the_hung_call_and_rebuilds_on_the_new_model` — R: model-less draft merges left behind by the stale race; tests unchanged (green after the race fix).

Not done: `Fold::folded` counts only unbuilt parts, so the G10 turn note reads "0 older unsummarized lines" when the view folds drafts into the prefix; `endless_fold` in a drafting session does nothing.

Result: `cargo test -p agent endless` — `test result: ok. 96 passed; 0 failed; 0 ignored; 0 measured; 1000 filtered out`.
