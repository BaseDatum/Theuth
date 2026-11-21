# 201 as built — P6b (ageing by use, prefetch, tool-list batching)

D11, D12, §10, §11, §12.1 `age_by_use` / `prefetch_bytes`, phase row P6.

## Landed

- **Ageing by use.** `fold.rs`: `Fold.pinned: Vec<Part>`, `Fold::rebuild_pinned` (`rebuild` = no pins); `most_due` skips a pair holding a pinned part (the fit may end over its low mark; the hard-cap prefix of `folded_runs` still applies). `pump.rs`: `Live.zooms: Vec<(generation, Part)>`; `zoom` records `(current generation, (l, i))` for every resolved address and logs `endless: zoom` (level, id, n, generation, zooms). `start_generation` takes the zooms of the generation just ended (`prev`; `again`: `prev − 1`) as pins when `age_by_use`, passes them to `render_upto` → `Fold::rebuild_pinned`, and drops older zooms. `Generation.zooms` (not persisted, 0 from a row) = that count; the loop's `endless: trim` log line carries `zooms`. `SessionMemory.zooms` = zooms in the generation in force (`Live::zoom_count`).
- **Prefetch.** `guards::rare_words` (the G2 rule: `tokens`, ≥ 4 chars, not in `STOP`, distinct). `Store::endless_recall_any` (same FTS5 query as `endless_recall`, words OR-ed; shared `endless_fts`). `pump::prefetch` / `Endless::prefetch(session, input)`: off at `prefetch_bytes` 0, in a meeting, or before a generation; hits with unit `i ≥ t_cut` (the tail) skipped; best rank first, a line that does not fit is skipped; body `"{i}+1|{line}"` lines. Log `endless: prefetch`.
- **Notes.** `endless_ext::input_note` (`<recall>` / `<tools>`); `units.rs` makes neither a unit. `loop_.rs` `Turn::input_notes(&texts)`, called in `start_turn` right after `task.ledger_note()`: the `<tools>` announcement (below), then the `<recall>` note (header line + the body). Both only with `self.view` (endless frame), persisted after the input like `<ledger>`.
- **Tool-list batching.** `endless/tool_pin.rs` (pure): `ToolPin {generation, index, tools, names, announced, removed, pending}`, `Fresh`, `ToolPin::{new, reconcile, announce, refusal}`. Stored per session in `Inner.tool_pins` (process-local); `Endless::{tool_pin, set_tool_pin, tool_refusal}`. `loop_.rs`:
  - `rebase`: the `## Runtime` block is pushed through `self.pinned_index(fresh)` (the frozen block while the pin holds; keeps `fresh` in the new `Turn.tool_index`), and the last line is `self.batch_tools(registry).await`. Nothing else in `rebase` changed.
  - `batch_tools`: no endless frame or no runtime index → pin dropped (classic behaviour, every change at once). Pin missing or not holding (`tool_pin_holds`: `view.generation == pin.generation` and `!cache_cold()`, the trim's `cold_after_secs` rule) → freeze the fresh tools/index/names. Otherwise `reconcile`: a name gone from the session's list that is still registered (`ListTools(None)`, asked only when something is gone) is a **hide** → leaves the pin now, the index block is taken fresh (`state.baseline` patched, `reset_this_round`); a name gone from the registry is a **removal** → stays defined, `refusal`; additions never enter the pin. `self.tools` = the pin's.
  - Dispatch: before the direct/bridge check, `Endless::tool_refusal` answers a removed tool's call with a plain error.
  - `input_notes`: while the pin holds, names in `ListTools(Some)` not in the pin and not yet announced → one `<tools>` note of `repl::signature` lines ("call them from the python runtime now; they join the direct tool list at the next memory trim"). Additions found by a mid-turn rebase are announced at the next turn's input.
- **Status:** `SessionMemory.zooms: u32`, `SessionMemory.tools_pending: bool` (pin `pending`: unannounced/announced additions, removals, or a differing index).

## Deferred vs immediate (§11 and the pending `hidden_epoch` rebase)

Deferred to the next generation or a cold cache: added direct tools and index lines (MCP server/skill added), removals from the registry (defined, refusing), any other index text change. Immediate: per-session hides (risk cap, communication off, `SetHidden`) — the tool leaves the direct list and the index is rebuilt fresh at that rebase (so index additions pending at that moment land with it). If the other branch's round-top check calls `rebase` mid-turn, the same rules apply (cache warm → held; hides immediate). Classic/epochs and meeting sessions (no `view`): no pin, unchanged; sessions without the runtime index: unchanged.

## Deviations

- Zoom records and tool pins are process-local (a restart loses one generation of grace; the cache is cold after a restart anyway, so the tool part is rebuilt).
- Prefetch matches ANY rare word (OR); `recall` keeps its AND query. The `<recall>` header line is outside the byte cap; only the hit lines count.
- Locally listed tools that are not in the registry (todo, consult…) are pinned too; additions among them are deferred but not announced (no `ToolSpec`).
- Removed runtime-only functions are not intercepted: their python call fails with the registry's own error.

## Not done

- No loop-level tests (announce-then-join-at-trim, removed-refuses-until-trim through a real turn, epochs rebase unchanged, prefix byte-identity with a prefetch note); covered by `tool_pin::tests` and the pump test only.
- Carried item C (`describe_left` through the local `Gate`, `model_calls` row, skip while paused by spend): not built.
- Pump-level test of the fold grace across real trims: only the pure fold test (`a_pinned_part_stays_unmerged`) and the zoom count across generations.
