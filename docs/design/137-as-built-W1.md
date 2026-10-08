# 137 — As-built, W1 TREE (contract + tree + compactor + store)

Branch `feat/137-w1`, worktree `~/Develop/theuth-137-w1`, base a0046c22. Skeleton commit `3955ab2b` ("137 W1: contract + skeleton"). Contract: `docs/design/137-contract.md` (signatures there are the ones in the code).

## What was built (file level)

| path | what |
|---|---|
| `crates/agent/src/endless/mod.rs` | `EndlessConfig` (+ `fold_cfg()` clamps), wire types `MemoryMode`, `ViewLine`, `GuardTrips`, `EndlessStats`, `Backfill`, `SessionMemory`, `EndlessProgress` (`#[serde(tag="type")]`), `SetMode`, `Attached`, the `Endless` handle (API of contract §4), `VIEW_DOC` verbatim. |
| `crates/agent/src/endless/units.rs` | §4 unit derivation from genai `ChatMessage` parts (`derive`, `cap` head ⅔ / tail ⅓ in chars, `WORK_MARKER` = the peer wrapper `<message from="session:`). |
| `crates/agent/src/endless/tree.rs` | `Kind`, `Unit`, `Node`, `Tree`, `id_n`/`addr`/`exists`, `free_unit`, `free_merge`. |
| `crates/agent/src/endless/fold.rs` | §7.1 slack fold (`Fold::rebuild/append/fit`, `most_due`), D10 `part_text`/`PLACEHOLDER`/`VERBATIM_MAX`, §7.2 `render` (≤ 4 marked blocks cut at the three deepest band boundaries + end), `render_lines`. |
| `crates/agent/src/endless/guards.rs` | §6 G1–G5 + size as `check() -> Result<cleaned, Trip>`, `correction()`, `ruler()`, `PROMPT_PHRASES`, 200-word stoplist, `cut_utf8`. |
| `crates/agent/src/endless/compactor.rs`, `compact_prompt.txt` | §5.3–5.4: COMPACT prompt (`{agent}` substituted), `Step::render` (earlier lines / ruler / instruction), `Compactor` trait, `ModelCompactor` over `backend::Backend` with genai `CacheControl` marks (system 1 h + context block ephemeral on Anthropic; `prompt_cache_key = endless:<session>` on OpenAI-like), `max_tokens` 600 + thinking budget, `build()` retry loop (same conversation, best candidate, `tries = -n` flag), `estimate_usd`/`observation` (the `loop_::estimate` formula). |
| `crates/agent/src/endless/pump.rs` | §5.2 per-session task: row ingestion → units (store-cached), rule-3 frontier scheduling with ≤ `jobs` tasks, free nodes without a call, `model_calls` row per node (`CallKind::Endless`) + `endless_calls` per attempt, retry after `retry_secs` with one `warn!`/event per node, `EndlessProgress` throttled to 500 ms, frozen contexts (last 8 turns), `settle`, `render_for_turn`, §9 `zoom`/`recall`/`date`, 8.6 `set_mode` (+ `off_head`), `memory`, `resume_all`, D14 backfill accounting. |
| `crates/agent/src/endless/tests.rs` | End-to-end over a real store with a scripted compactor; wire-type round trip. |
| `crates/agent/src/store/endless.rs` | `Store` accessors of contract §5 (`session_memory`, `set_session_memory`, `endless_sessions`, units/nodes/calls/stats, `endless_recall` FTS5, `endless_units_like` raw fallback, `endless_messages_from`, `endless_fts_sync`, `delete_endless_in`). |
| `crates/agent/migrations/0037_endless.sql` | `sessions.memory`, `endless_units`, `endless_nodes`, `endless_calls`, FTS5 `endless_lines_fts` + AI/AU/AD triggers on level 0. |
| `crates/agent/tests/fixtures/endless/B.haiku.tree.jsonl`, `B.main.jsonl` | lab fixtures (structure only; never asserted on content). |
| edits outside | `lib.rs` (`pub mod endless`, re-exports, `AgentConfig.endless`, `SessionSummary.memory`), `store.rs` (`pub mod endless`, `memory` in `SessionRow`/`session_cols!`/`summary`, `delete_endless_in` in `delete_session`), `store/capture.rs` (`ATTACHED` += 3 tables), `lifecycle.rs` (`CreateSessionOptions.memory`), `runtime/src/side_call.rs` (`CallKind::Endless`), `rpc/src/api.rs` (4 trait methods + `Err("not yet")` stubs), `rpc/src/export.rs` (7 new TS exports), test literals of `SessionSummary` in `session_read.rs`, `slack/ui.rs`, `telegram/commands.rs`, `tui/attach.rs`, `tui/model.rs` (`memory: Default::default()`). |

## Revision 1 (design §17, brief message 03:09) — commit "137 W1: revision 1 interfaces"

Added: config fields/defaults (view_bytes 128 KB, default_desk true, default_meeting, meeting_view_bytes, backfill_max_units), `resolved_compactor_model`, the R1-D3 chain in `compactor_for` (`Endless::new(.., title_model)`), `UnitFilter` (+ `keeps`), `BackfillEstimate`, `set_view_bytes`, `render_snapshot`, `render_with_budget`, `backfill_estimate`, the R1-D4 base in the fold/render/schedule/zoom (`sessions.endless_base`), the filter at ingestion (`sessions.endless_filter`), `set_mode(session, mode, filter, epoch)` usable on any idle session with a transcript. Tests: `backfill_cap_sets_base_and_keeps_raw_history_reachable`, `compactor_chain_and_meeting_filter`, fold base case in `fixture_fold_invariants_and_refold_equality`, store meta round trip. Contract §8 lists the exact semantics.

Revision 1b (03:10): `set_view_bytes(session, Option<u32>)` persisted in `sessions.endless_view_bytes` (migration 0037), loaded at attach, `SessionMemory.view_bytes` = the budget in force; test extension in `backfill_cap_sets_base_and_keeps_raw_history_reachable` (persist, survive detach/attach, clear). Deviation: a larger budget re-folds purely at the new budget rather than "stops merging" — contract §8 explains and names the one-line alternative.

Addendum (03:38, W4 gap): `active()` false for `UnitFilter::Meeting`; `render_with_budget -> Vec<ViewLine>` (was blocks), live fold untouched; first switch-on with a different filter re-derives the unit cache (`Store::endless_units_clear`). Test `meeting_filter_session_is_served_but_never_active`.

W3 hand-off (03:42): `ViewLine.at: Option<DateTime<Utc>>` (`ts(optional = nullable)`), set for `n == 1` lines in the view, zoom and recall from the unit's row time.

Gates after the revision (R1 + R1b + addendum): endless filter → `22 passed; 0 failed`; clippy clean.

Gates after the revision (R1 + R1b): `cargo test -p agent --lib --no-fail-fast endless` → `test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 463 filtered out; finished in 0.54s`; `cargo clippy -p agent -p rpc --no-deps -- -D warnings` → `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 36.75s`; unfiltered `cargo test -p agent --lib --no-fail-fast` after R1 → `481 passed; 1 failed (pre-existing prompt_parts_in_order_with_blank_lines); 2 ignored`; `cargo fmt -p agent -p rpc` clean.

## Gates before the revision (exact commands, result lines)

- `BUILD_LABEL=137-W1 build-lock cargo check -p agent -p rpc` → `Finished \`dev\` profile` (no errors; skeleton commit and every checkpoint).
- `BUILD_LABEL=137-W1 build-lock cargo test -p agent --lib --no-fail-fast endless` → `test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 463 filtered out; finished in 0.54s`.
- `BUILD_LABEL=137-W1 build-lock cargo clippy -p agent -p rpc --no-deps -- -D warnings` → `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 43.02s` (clean).
- `cargo fmt -p agent -p rpc` → run (no diff left).
- `BUILD_LABEL=137-W1 build-lock cargo test -p agent --lib --no-fail-fast` (unfiltered) → `test result: FAILED. 479 passed; 1 failed; 2 ignored; 0 measured; 0 filtered out; finished in 68.89s`. The one failure is `loop_::tests::prompt_parts_in_order_with_blank_lines` (`loop_.rs:7802`, `assert!(!full.contains("rules"))` on `frame::baseline`'s prompt text) — pre-existing: `git diff a0046c22 -- crates/agent/src/frame.rs crates/agent/src/loop_.rs` is empty, nothing of mine feeds the baseline prompt.
- `BUILD_LABEL=137-W1 build-lock cargo test -p runtime --lib --no-fail-fast call_kind` → `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out`.

## Deviations from the design / brief (decided, see contract §7)

1. **Fold purity.** The live view is `Fold::rebuild(tree, T)` (replay of `append`+`fit` against the current tree), recomputed when the tree or `T` changes. The lab's incremental fold (fit on each `mark_built` against the tree *as it was*) is not reproducible from rows after a restart, so the design's "pure function of (tree, T)" is taken literally. Measured on the fixture at 32 KB: identical part-prefix share > 0.7 turn to turn (test `fixture_slack_fold_is_stable_turn_to_turn`); the lab's 84–96 % figure was for the incremental variant.
2. **`on_rows_committed(session)` takes no rows**: units are derived from `messages` beyond the last cached `seq` (restart-safe, excluded rows skipped, `cut_at` respected).
3. **Frozen contexts are in memory** (last 8 turns); after a restart an unbuilt node uses the live view as `<chat>`.
4. **Pricing**: `loop_::estimate/observe` are private to `loop_.rs` (not mine to edit); the formula is duplicated in `compactor.rs` (`estimate_usd`, `observation`) and documented as such. W2 may `pub(crate)` the originals and drop the copy.
5. **Ledger**: one `model_calls` row per node build (attempt usage summed, `kind = endless`); `endless_calls` has one row per attempt.
6. **`CallKind::Endless`** added to `crates/runtime/src/side_call.rs` (shared crate, not in the ownership list; three lines + the round-trip test).
7. **`Endless::new` is `pub(crate)`** (`supervisor::Auth` is crate-private); `#[cfg_attr(not(test), allow(dead_code))]` until W2 wires it.
8. **Backfill mark** = units with `turn == 0` (committed outside an endless turn). `active()` is false while any of them lacks a level-0 line. Fresh endless sessions and restarts never backfill.
9. **Two consecutive `user` messages** carry the context block and the step (genai cache marks are per message). Anthropic merges consecutive same-role turns; if a provider rejects it, fold into one message with two text parts (loses the per-block mark).
10. **`INSERT … ON CONFLICT DO UPDATE`** for nodes, not `INSERT OR REPLACE`: REPLACE's implicit delete skips the FTS delete trigger (found by the store test).
11. RPC stub bodies return `ASK_FAILED "not yet"` after `authorize`; `EndlessProgress` is **not** registered in `rpc/src/bridge.rs` (W2's file: `forward!` list + `rt.subscribe::<Bridge, EndlessProgress>`).
12. `SessionSummary.memory` and `CreateSessionOptions.memory` are types in `agent` (that is where the RPC types live); the `sessions.memory` column is read through `session_cols!`.

## Hand-offs

**W2 (loop/rpc/roster)** — everything is on `agent::Endless` (contract §4):
- Supervisor: `Endless::new(store, rt, client, auth, catalog, cfg.endless.clone())` once; `endless.resume_all().await` at engine start; per session actor start `endless.attach(id, Attached { model, effort, agent: desk-or-"theuth", store: private_store_or_None }).await`; `endless.detach(id)` on actor stop.
- `create_session`: `options.memory.unwrap_or(if cfg.endless.default_chat { Endless } else { Epochs })` → `store.set_session_memory(id, mode)` after `create_session`.
- Turn start when `endless.active(id)`: `settle(id, Duration::from_secs(cfg.endless.settle_secs))`, then `render_for_turn(id)` → `ViewRender.blocks` as the first user message's blocks (mark where `mark`), then the input block; system = prompt + `Endless::view_doc()`; no head, no history. After every commit (any role, steering included): `on_rows_committed(id)`. Auto-compaction off; D11 overflow head = the view.
- Tools (endless sessions only): `zoom` → `endless.zoom(id, id_, n)`, `recall` → `endless.recall(id, &query, limit.unwrap_or(8))`, `date` → `endless.date(id, id_)`; descriptions verbatim in design §9.
- `/endless on|off` and `set_session_memory`: `endless.set_mode(id, mode, next_epoch)` — for `Epochs` write `SetMode.head` as the epoch head (`compact::style_of_head` must accept `style="endless"`, compact.rs is yours).
- RPC bodies: `session_memory` → `endless.memory(id)`; `endless_zoom`/`endless_recall` → the tool fns; register `agent::EndlessProgress` in `bridge.rs`.
- Roster: `agents/<name>.md` `memory:` → `set_session_memory` on the desk row; skip the 60 % episode trigger when `endless.active(desk)`.

**W3 (UI)** — bindings after the closer runs `just ts-desktop`: `MemoryMode`, `ViewLine`, `GuardTrips`, `EndlessStats`, `Backfill`, `SessionMemory`, `EndlessProgress`, `SessionSummary.memory`, `CreateSessionOptions.memory`, `CallKind` gains `"endless"`.

**Closer** — `just ts-desktop` regenerates bindings (CallKind, SessionSummary, CreateSessionOptions changed); the live smoke needs W2's wiring; `cargo test -p runtime` covers the `CallKind` round trip.

## Known ceilings (`ponytail:` marks in code)

- Whole-session unit text is held in memory per attached session (`pump.rs` `Live.units`); page from the store if a session outgrows RAM.
- `schedule()` scans every level each wake (O(T) per level); fine to ~10⁴ units.
- The FTS mirror on a replica is rebuilt by `endless_fts_sync` at engine start only.
