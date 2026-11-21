# 201 as built — P6c (backend leftovers)

Carried items from P4 (C), P6a and P6b.

## Landed

- **`pump::describe_call`** (new), used by `pump::describe_left` for every model description (a vision note's answer is no call):
  - a local describer (`Compactor::local()` = the `compactor::is_local` endpoint) takes the engine-wide `Gate` of its endpoint (`state.queues.get(endpoint, local_jobs)`) at priority 0, the lowest (compactor jobs use 0/1, so a frontier job still goes first; R1-8 ageing applies as for any waiter);
  - after the permit: skipped (`None`, nothing sent, no row) when `live.paused_by == "spend"` or `spend_capped` says the G11 day cap is reached now (which pauses settling, as a compactor job would);
  - otherwise a `model_calls` row: `reserve_call(session, "endless:describe:<unit>:<uuid>", CallKind::Endless, provider, model)`, `mark_dispatched`, `settle_call` (Settled with the reply's usage, Failed with none) through `compactor::observation`, so it counts in `endless_spend_since` / `max_usd_per_session_day`.
- **Tests** (`endless/tests.rs`): `image_descriptions_are_charged_as_endless_calls`, `a_local_describer_waits_for_the_gate`, `no_description_while_paused_by_spend` (`Seer` gained `local` and a usage-bearing reply; `Seer::at(vision, local)`).
- **Loop tests** (`loop_/endless_tests.rs`): `real_endless_with(h, model, attach, cfg)` (`real_endless` = default config); `CallOnce` responder (first main call = one tool call); `a_ledger_entry_is_noted_then_frozen_into_the_next_snapshot` (model's `ledger` add → `<ledger>` note right after the next input → after `/compact` trim the `<ledger_snapshot>` block has the entry, no `<ledger>` note in the tail, the block byte-identical over two turns of that generation); `with_the_ledger_off_there_is_no_tool_and_no_block`.

## Deviations

- The description call is not bounded by `attempt_secs` / cancelled by a compactor switch (it was not before either); a failed description is charged no usage (the describer returns none on error).
- The spend check comes after the gate wait, like `pump::call`.

## Not done

- Item 2 (loop-level tool-pin tests: announce-then-join-at-trim, removed-refuses-until-trim, epochs immediate rebase, prefix byte-identity with a `<recall>` note): not written. `batch_tools` holds a pin only with a runtime index, which `rebase` builds only when `shared.pool` (the python runtime pool) is set; the loop test harness passes `None`, so these need a pool (or a test seam on the index) first. Still covered only by `tool_pin::tests` and the pump test.

- Item 3, `a_zoomed_line_gets_one_generation_of_grace` (written, `#[ignore]`d): it picks a unit merged in generation 1, zooms it, and expects (alone in g2, merged in g3) with `age_by_use`, (merged, merged) without. Its setup (24 `seed_many` talk units, `view_bytes` 4 000, `Echo`, waiting up to 20 s on `render_for_turn`) never produces a merged run — every unit renders `i+1|`, so the merge build (lazy, P3) is not triggered by this setup. Not a proven code bug; the next builder needs a setup that yields level-1 nodes (e.g. insert them as `fold.rs`'s `a_pinned_part_stays_unmerged` does, or the `merge_at`/`cfg_137f` setup of `queued_merge_does_not_hold_back_the_frontier`) and then un-ignore.