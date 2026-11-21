# 201 as built — P2 (tail, trim, generations)

D2 D3 D4, §3, §4.1–§4.4, §6 "frozen compactor context per generation", §13 log lines.

## Landed

- **`endless/tail.rs`** (new, pure): `bytes(history)` (= `compact::est` × 4, the request measure; binaries by their flat estimate), `turn_starts(history)` (a user message right after an assistant reply with no tool call), `trim(history, low, high, cold) -> Option<Trim {keep_from, cut, kept}>` (over high, or cold and over low → the first turn start whose suffix ≤ low, else the last turn; never index 0, never between a call and its results). Unit tests in the module.
- **`migrations/0071_endless_generations.sql`**: `endless_generations(session_id, n, t_cut, first_seq, blocks JSON, created_at)`. Node-local (not in `capture::ATTACHED`), so `SESSIONS_WIRE_SCHEMA` stays 70. Store: `GenerationRow`, `endless_generation`, `endless_generation_put`, `endless_generations_clear`.
- **`endless/pump.rs`**: `Generation {n, t_cut, first_seq, render}`; `generation()` (cache `Live.current_gen`, else the stored row); `start_generation(first_seq, max_bytes, again)` — ingests, `t_cut` = first unit with `seq ≥ first_seq`, `render_upto` = the fold rebuilt over `[base, t_cut)` (the only place a chat's fold runs for the frame) fitted to `max_bytes` (≤ 3 renders), persists, freezes; `turn_opened()` pushes the generation's frozen text as the turn's `<chat>` compactor context (so `ctx_for` serves the generation's bytes; the `frozen` deque is still keyed by turn) and records `tail_bytes` and the allowance. `turn_allowance_bytes` (PC seam) now returns the allowance the last turn measured. `memory()` fills `tail_bytes` and `generation`. `set_mode(Endless)` from another mode clears the generations (generation 1 again). `Handle::tail()`.
- **`endless/mod.rs`**: `Endless::{generation, start_generation, turn_opened, tail_marks}` (`tail_marks` = config `with_tail(session override)` → `tail_marks(allowance)`). VIEW_DOC rewritten per §4.4.
- **`loop_.rs`**: `SessionState.tail_gen`. `endless_start` no longer clears `head`/`history`: `endless_view` → `endless_tail` loads the generation (none → generation 1 with the current view and an empty tail), brings `history` to `seq ≥ first_seq` when `tail_gen` differs (fresh process, rebuilt state), computes marks (allowance = `view_allowance()`), cold = `prev_call` older than `cold_after_secs` (0 → 300 s, every transport; no `prev_call` = cold), trims, starts the next generation, drains the cut rows. Per-epoch sets (`hinted`, skills, `used`…) reset only on a new generation; `reset_this_round` only when the view changed. View budget at a trim = allowance − high mark. Logs: `endless: trim` (`cut_bytes`, `kept_bytes`, `generation`, `cold`, `forced`, marks, `view_bytes`); `view_rebase` gains `generation`, `tail_bytes`, `trimmed`. `shrink_view` (G5) re-renders the same generation smaller (`again`). The G10 note no longer says "0 older unsummarized lines".
- **`frame.rs` `view_messages`**: on Anthropic the view's last block is always marked plus the latest marked deep block (2 marks); system 1 h + newest message 5 min = 4. Test updated.

## Byte stability (item 4)

Every per-turn item (the opener's `<time>` stamp, knowledge hints, notes, notebook block) is persisted as a `messages` row and pushed into `history` by `persist`, so the carried tail is the sent bytes, and a restart rebuilds it from rows. Not verified end-to-end after a restart against a live provider. Vision projection (`projected` with images) is applied per request as before.

## Deviations

- Turn boundaries are inferred from roles (no turn id on rows): a salvage note after a partial reply also counts as a start (splits no tool call).
- A D11 overflow head mid-generation forces a new generation at the next turn (`forced`), tail = the rows after the cut. After a restart with such a head, rows between `first_seq` and the cut are absent from `store.frame` and are not carried.
- The test hook (`Shared::test_view`) path carries history and never trims (generation 1).

## Unfinished

- Item 8 `/endless off`: the head is still the live render from `set_mode_within` and the tail is not kept as history.
- Item 10: `raw_bytes`/unbuilt-verbatim still feed the fold (`render_upto` scales both).
- Per-agent `tail_*` keys are still not applied to the desk session (roster/lib.rs seam untouched); only session override → config.
- Tests not written: loop-level trim through the real service (cut ≤ low, cold early), refinement mid-generation leaving the frozen bytes unchanged, epochs/meeting frame-unchanged (existing `epochs_session_keeps_…` covers epochs only).
