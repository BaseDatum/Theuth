# 201 as built — P5 (in-turn trim)

§4.3 "In turn", F3, D12 "refuse rather than remove", §11.

## Landed

- **`endless/tail.rs`**: `round_starts(history)` — turn starts plus every assistant message that opens a round (not preceded by another assistant message), capped at the current round's start = the last assistant message (not the turn-start inference, so a recovery note after a partial reply is never a cut point). `trim_rounds(history, low) -> Option<Trim>`: the first position whose suffix ≤ low, else the current round's start; `None` when nothing precedes the current round. A tool row is never a position, so a call keeps its results.
- **`loop_.rs`**:
  - `Turn::trim_in_turn(why)`: marks from `tail_marks(view_allowance())`, `trim_rounds` to the low mark, `start_generation(first_seq, allowance − high)` (ingests the cut rows; their units have drafts), `describe_left` for the rows leaving, drains `history`, `tail_gen`, `since = len`, `hint_since` shifted, `generation_reset()` (the per-generation sets, extracted from `endless_start`), `turn_opened` again (compactor context and status follow the new generation), and swaps `self.view` to the new frozen render (`trimmed`, `logged = false`, `reset_this_round`). Log `endless: in-turn trim` (`why`, `cut_bytes`, `kept_bytes`, `generation`, `low`, `high`, `view_bytes`).
  - `round()`, after `prepare_images`, before the 07 §8.1 block: when `in_turn_trim_on()` (chat, endless frame up, no head, not meeting, service `active`, not the test hook) and view + tail > 90 % of `view_allowance()`, trim; if the frame is still over the allowance and the turn has not used its 07 §8.5 cut (`overflowed`), run the classic cut once (log `endless: in-turn trim cannot fit the frame; 07 §8.5 cut`).
  - `Abort::Model` overflow: an endless chat runs `trim_in_turn("refusal")` once (`Turn.refusal_trimmed`) before `shrink_view` and the §8.5 compaction; a trim → `Round::Retry`.
  - Tools (§11): `Turn.endless_listed` = `endless_tools_on() || stored_endless()` (stored memory mode `endless`, attached or not), set in `rebase` and used for listing and dispatch. Dispatch: not `served` → one-line refusal `"<tool>: nothing to search yet — this session's memory has no lines."`.
- **Tests** (`loop_/endless_tests.rs`, real loop + real service, paused compactor): `a_long_turn_completes_through_in_turn_trims_without_the_classic_cut` (40 rounds × 16 KB: no `Compacted`, every request's calls = its results, the previous round always sent, generation ≥ 3, the last request carries the newest frozen view); `one_round_too_large_for_any_trim_falls_back_to_the_classic_cut`; `a_turn_boundary_trim_through_the_loop_starts_a_generation` (item A for the boundary trim); `endless_tools_are_listed_from_creation_and_refuse_without_a_tree` (epochs: none; endless unattached and attached: identical `tools`). `endless::tail::tests::a_round_trim_keeps_calls_with_results_and_the_current_round`. Test model ids `inturn`, `onebig`, `listed`, `boundary` added to the harness catalog.

## Deviations

- "Allowance" = `view_allowance()` (turn target less system prompt and reserve), the same figure the tail marks are computed from; the frame measure is the frozen view's bytes + `tail::bytes(history)`.
- The fallback cut runs proactively when a trim cannot get the frame under the allowance (previously the endless path cut only on a provider refusal). In the one-big-round case the trim first cuts the rounds before the current one, then the cut runs.
- In-turn trims reset `state.used` like a boundary trim (the 07 trigger is off for endless turns anyway).
- The tool list reads the stored memory mode once per `rebase` (one store read).
- Meetings and `epochs`: `in_turn_trim_on` is false (no `view`, or `meeting_filter`); the test-hook view (`Shared::test_view`) also keeps the old D11 path, so `overflow_in_an_endless_turn_cuts_with_the_view_as_head` is unchanged.

## Not done

- B (`endless_fold` / `/compact` = "trim now"): not built; `compact_now` and `Endless::fold` unchanged.
- C (`describe_left` through the local `Gate` and the cost ledger): not built.
- No test of the refusal-triggered trim through the loop (the path is the same `trim_in_turn`; only covered by reading).
- The 200-tool-call gate is exercised at 40 rounds.
