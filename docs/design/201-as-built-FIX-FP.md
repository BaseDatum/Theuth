# 201 — as-built, fix builder FP (polish)

Base: `9c452db6a`. Items from the FP brief (review ids where they exist).

| item | status | what |
|------|--------|------|
| 1 `await_holding_lock` | fixed | `pump::memory`: the `live` guard lives in a block that returns `(m, full_cfg, tail, profile, reason)`; `ledger_status(..).await` runs after it. (The explicit `drop(live)` was already there; clippy flagged it anyway.) |
| 2 clippy | fixed | `ledger.rs` (Section/State variants, Entry/Suggestion/Artefact/Ledger/Scan fields, `ALL`, `title`, `from_json`, `to_json`) and `mod.rs` `LedgerStatus` fields documented; `tail.rs` `int_plus_one`; `compactor.rs` `useless_vec`; `pump.rs` test `field_reassign_with_default`; `loop_/endless_tests.rs` needless ref; also `units.rs` `unnecessary_lazy_evaluations`, `lines.rs` `comparison_to_empty`, `store/endless.rs` `type_complexity` (local `type Row`), `endless_eval_tests.rs` regex compiled once outside the loop, dead helper `endless::tests::unbuilt_session` removed. |
| 3 rustfmt | fixed | `loop_.rs` (the `pinned_index(repl::prompt_block(..))` hunk) and `supervisor.rs` (`EndlessFold` handler signature, `endless.memory(..)` chain) by hand; `pump.rs`, `compactor.rs`, `tests.rs`, `endless_eval_tests.rs` formatted. Left alone: `rpc_turns.rs`, `roster/src/tools.rs` (pre-existing), `repl.rs:2334` (pre-existing hunk, not in the build diff), `crates/tui/src/model/{history,tests}.rs` (UI builder's files). |
| 4 notices test | not a build bug (evidence) | Run with the rows printed (throwaway edit, reverted): every error row is `tool registry not running`, written by `Turn::drive` (base code, unchanged) — the admitted notice opens a real turn on the idle session actor and this `#[tokio::test]` has no `ToolRegistry` (it is not in `with_test_registry`). The session's ~100 rows are all `<message_file …>` notice inputs; no endless/ledger/pump row and no 201 code on the path. Whether it is red at the base is unconfirmed (the base cannot be built here); left as is. |
| 5 grace test | done | `a_zoomed_line_gets_one_generation_of_grace` and its helpers `zoomed_line_across_trims`, `alone` removed. |
| 6 R1-10 | not done | `endless_start` (which calls `view_allowance`) must run before the turn's input rows are logged (fresh turn: `loop_.rs` ~L1404; resume: `resume`), while `rebase` needs the tool registry and runs inside `drive` after them, with knowledge sync side effects. Moving either reorders turn start (and touches the spot the other branch edits); not a small change. |
| 7 R2-12 | fixed | `failing_unit_blocks_nothing`: 40 units (fold over `merge_at` × `view_bytes`), asserts the merge `(1, 1)` over the failing unit's neighbours is `Quality::Model`. `repeated_frontier_failure_is_stalled_and_settle_stops_waiting`: production `Endless::settle` returns `true` within 5 s at one and at two failures, `stalled.failures == 2` stays visible, cleared after success. New: `meeting_long_units_run_through_the_batch_path` (meeting filter, 6 ~600 B units: ≥ 1 batch call, 0 unit calls, all model-built) and `compactor::tests::a_local_batch_spent_on_thinking_retries_once_with_thinking_off` (drafting: one plain + one forced call, lines pass, 2 call rows; meeting batch: no forced call, every line trips). |
| 7 R2-9 | fixed | `compactor::largest_call_bytes(step, node_bytes, small: Option<usize>)`: on the small profile the small prompt (`SMALL`), `small_render` and the small chunk size. `pump::call` computes the small chunk before the over-cost check; the dropped-call estimate of a small call uses it too (standard path unchanged). Test `small_profile_sizes_the_largest_call_by_its_chunk`. |
| 7 R2-11 | fixed | `compactor::build`, standard map-reduce: the loop breaks at the first chunk whose line failed its guards (negative tries returned; no further chunk, no reduce). Test `map_reduce_stops_at_the_first_tripped_chunk`. |
| 8 removed-row merge race | fixed | `Live::discard`: `rows_excluded` puts every ancestor `(l, i >> l)` of a removed unit that is in `running`; `finish` removes the key, drops the landing result (success or failure, no fallback stored, no failure recorded), clears its `redo` and marks it stale when built so it is rebuilt from the current children. Test `a_merge_running_over_a_removed_row_is_dropped_when_it_lands` (seam `pump::mark_running_for_tests`). |
| 9 timing test | test fixed (no product bug) | A job dispatched before the give-up (`running` holds its key) can reach `compress` after `quiet_for_tests` returned; `schedule` skips a given-up unit (`mechanical` check), so no call is *started* after the give-up. The test now waits until `(0, 1)` leaves `running_for_tests` before taking the count. |

## Wire schema note
`LedgerStatus` derives `JsonSchema` and `ts_rs::TS`: the new field docs become
descriptions in `bindings/` when they are next generated (`just ts`); not
regenerated here (generated files are never edited by hand).

## Test state
Run after the last code edit (commit `103139430`):
- `cargo clippy -p agent -p roster --all-targets -- -D warnings -A clippy::chunks_exact_to_as_chunks`: clean (RC=0).
- `cargo test -p agent -- endless ledger tool_pin view_blocks compact_in fold input_notes notices`:
  `203 passed; 1 failed; 1 ignored` — the one failure is `notices_are_automatic_and_stop_at_the_chain_limit` (item 4).
- `cargo test -p agent -- endless::tests::three_hard_failures_over_ten_minutes_fold_the_unit --exact`, 10 runs: 10 × `1 passed; 0 failed`.
