# 137 — As built, W2 LOOP (fresh-turn frame, settle, tools, `/endless`, RPC bodies, roster)

Branch `feat/137-w2`, worktree `~/Develop/theuth-137-w2`, from W1's skeleton `3955ab2b` + design R1 `9bf9f6cd`. Coded against `docs/design/137-contract.md` §4 (`agent::endless::Endless`); the five Revision-1 interfaces not yet in the skeleton sit behind `crates/agent/src/endless_ext.rs` (§3 below). Nothing under `crates/agent/src/endless/**`, the migration or `crates/desktop/**` was edited (one exception, reverted, §5.1).

## 1. What was built (file level)

### `crates/agent`
- `frame.rs` — `view_messages(blocks, transport) -> Vec<ChatMessage>` (one user message per view block; on Anthropic at most `VIEW_MARKS = 2` `cache_control: ephemeral` marks on the latest marked blocks *before* the final one — system 1 h + tail 5 min take the other two breakpoints; the final block grows every turn so a mark there is never read back; other transports: no message marks). `baseline_dated(…, dated: bool)` behind the unchanged `baseline(…)`: an endless turn renders no `Date:` line. `CHILD_VIEW_NOTE` (R1-D1 paragraph, verbatim §17). Test `view_blocks_become_marked_user_messages`.
- `loop_.rs`
  - `Shared.endless: Option<Endless>` (`Session::with_endless`), `Shared.parent_view` (`with_parent_view`), `SessionState.last_view: Option<Vec<String>>`, `Turn.view: Option<EndlessView>`, `Input.context: Vec<String>` + `OpenerContext` message (R1-D1), `SetMemoryNow` message.
  - **8.1** `Turn::endless_start` runs in `Session::start_turn` after `settle_open_turns`, before the opener row: `Endless::active` → `settle(settle_secs)` (raced with the turn's cancel token) → `render_for_turn`; then head `None`, history cleared, `used/last_after/pressure_sent/compaction_off` reset, per-epoch sets (`hinted`, `knowledge_read`, `files_written`, `skills`) cleared, `reset_this_round = true` (the rebuilt frame is the named event, not a 07 §7 miss). `changed_from_byte` / `blocks_hit` are computed against the previous turn's blocks.
  - **7.3** `rebase`: `baseline_dated(dated = !endless)`; VIEW_DOC appended after the runtime block; `CHILD_VIEW_NOTE` for a child of an active endless parent; `zoom`/`recall`/`date` specs added and the list re-sorted. The opener of an endless turn always carries the `<time …/>` stamp (date, weekday, time) — `take_input` resets `last_stamp` for it.
  - **D4** `round`: `frame::view_messages` precede the turn's rows; `system_tokens()` counts the view as fixed prefix (reply budget / calibration / fixed_tokens).
  - **D2** `round`: the auto trigger and the pressure note are gated `chat && !endless`.
  - **D11** `compact`: with a view, style is forced to `cut`, no session head / summariser; the cut body is the fold sentence + the view text; after the commit the view blocks are cleared (the view now rides in the head). Overflow then `Round::Retry` as today.
  - **8.3** `Turn::committed(seq, msg)` — the one loop-side funnel for every stored row (`record`, `consume_peer`, the vision note) → `Endless::on_rows_committed`.
  - **§7.3 log** `log_view_rebase` once per endless turn, after the first response: `view_rebase{session, turn, blocks, bytes, changed_from_byte, blocks_hit, settled, unbuilt, cache_read, cache_write, prompt}`.
  - **8.6** `Turn::set_memory` (mini-turn like `compact_now`): `endless_ext::set_mode(…, UnitFilter::All)`; `off` → `install_view_head(n, text)` writes one `epochs` row (`store.compact(session, n, at_seq, at_seq + 1, head)`), installs the head, empties the tail, publishes `Compacted`; logs `context_edit: endless on|off`.
  - `loop_/endless_tools.rs` — `zoom`/`recall`/`date` specs (descriptions verbatim §9) + `run` handler → `Endless::{zoom, recall, date}`; results are ordinary `ToolOutput`s capped by `finish_batch`.
  - `user_message` prepends `Input.context` blocks as their own text parts.
- `supervisor.rs` — `Agents.endless: Endless` constructed in `start()` beside the side-call slots; `resume_all()` after `register`; `attach` in `Agents::session()` (model, effort, agent = `sessions.desk` or `theuth`, private store when in a private tree); `detach` at `UnloadSession`, `DeleteSession`, private close; `CreateSession`: `options.memory.unwrap_or(default_chat)` → durable: `Endless::set_mode(id, Endless, 0)` after `create_durable`; private: `store.set_session_memory` before the actor attaches. Handlers for `SessionMemoryQuery`, `EndlessZoom`, `EndlessRecall`, `SetSessionMemory` (→ actor `SetMemoryNow`, `busy` while running), `SetSessionViewBytes`. R1-D1: `start_child_turn` sends `OpenerContext(render_snapshot(parent))` to a native child of an active endless parent before routing the brief. Test `memory_mode_is_created_queried_and_switched`.
- `lib.rs` — the `Agents` messages `SessionMemoryQuery`, `EndlessZoom`, `EndlessRecall`, `SetSessionViewBytes` and, per the orchestrator's 03:38 addendum (W4's interface), `SetSessionMemory { session, mode, filter: UnitFilter } -> anyhow::Result<SetMode>` and `RenderMemoryView { session, bytes } -> anyhow::Result<Vec<ViewLine>>`; re-exports `SetMode`, `UnitFilter` (`agent::UnitFilter` is `endless_ext`'s until W1's lands — a one-line swap); `pub mod endless_ext`.
- **Meeting filter guard (addendum)**: `SetMemoryNow { mode, filter }` sets `Shared.meeting_filter` on the session actor; `Turn::endless_view` returns `None` for it, so a `UnitFilter::Meeting` session never gets the fresh-turn frame, VIEW_DOC or the three tools in this process. Durable across restarts only once W1's `active()` returns false for Meeting-filter sessions (W1 is doing so). `RenderMemoryView` → `endless_ext::render_with_budget` (adapter: the live view's lines, oldest cut to fit `bytes`; W1's folds the tree to the budget).
- `endless_ext.rs` — §3.
- `loop_/vision_fallback_tests.rs` — `Fake::tool_call_then_overflow` (a main call with a `tool` message and no head → 400 overflow).
- `loop_/endless_tests.rs` — §4.

### `crates/rpc`
- `api.rs` — bodies of `session_memory`, `endless_zoom`, `endless_recall`, `set_session_memory` (forward-routed like `compact`, else `ask(Agents, …)`); new method `set_session_view_bytes(session, bytes) -> SessionMemory` (R1-D2b).
- `bridge.rs` — `EndlessProgress` in `forward!` and `rt.subscribe`.

### `crates/roster`
- `types.rs` — `AgentFile.memory: Option<MemoryMode>`, `AgentFile.view_bytes: Option<u32>` (serde default, ts optional).
- `file.rs` — parse (`memory: epochs|endless`, `view_bytes` 16 000..=256 000, user-facing errors) + render + `default_agent`; test `memory_forms`.
- `desk.rs` — `Desk.memory`, `Desk.pending_memory`.
- `lib.rs` — `Roster.endless: EndlessConfig` read from the `agent` section (`AgentSection`, the `handoff.rs` pattern); `desk_mode(f)` = file else `default_desk`; `open_desk` passes `memory: Some(mode)` to `CreateSession`, applies `view_bytes`, queries the session's mode and sets `pending_memory` when it differs; `apply_pending_memory` at open, at every desk `TurnFinished` and on `Tick` (idle only; `busy` keeps it pending; one `info!` with units/ETA/model/usd when it lands) — R1-D7 / D14, idempotent across restarts because the mode is a column. The 60 %-of-context episode trigger is skipped when the desk session's mode is `endless`; idle/lease/thread-end episodes unchanged.
- `tools.rs`, `attention.rs` — the new fields in `AgentFile` literals.

### `crates/tui` (outside the three gate crates; small)
- `/endless [on|off]` → `Command::Memory` → `set_session_memory` / `session_memory`; replies with `endless_ext::reply` / `endless_ext::status`. There is no engine-side slash table (each client has its own); the desktop's is W3's — it should call `set_session_memory` and render `reply`-equivalent text.

## 2. Gates (exact commands, result lines)

Shell for every call: `export PATH="$HOME/.theuth/bin:$(/opt/homebrew/bin/just toolchain):$PATH"; export CARGO_TARGET_DIR=$HOME/Develop/theuth-137-target; export CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0`, each cargo call wrapped in `BUILD_LABEL=137-W2 build-lock`.

| gate | result line |
|---|---|
| `cargo check -p agent -p rpc -p roster` | `Finished \`dev\` profile` — 0 errors (compile warnings only inside W1's `endless/**`: missing docs, never-read fields) |
| `cargo test -p agent --lib --no-fail-fast` | `test result: FAILED. 480 passed; 2 failed; 2 ignored` — the 2 are W1's skeleton tests (`endless::units::tests::derives_units_from_stored_row_shapes`, `store::endless::tests::endless_rows_round_trip_and_fts_follows_level0`); every W2 test passes. **Run with W1's `compactor.rs` test locally patched to compile (§5.1), reverted before commit; on the committed tree the agent lib test target does not compile until W1 fixes it.** |
| `cargo test -p rpc --no-fail-fast` | unit tests `test result: ok. 51 passed; 0 failed`; integration `tests/prompts.rs`: `test result: FAILED. 0 passed; 1 failed` — `prompts_resolve_everywhere_and_replay_only_while_pending` at prompts.rs:140, **pre-existing** since 132 ENG (`717708fa`): a second `answer_ask` now routes a `LateAnswer` to the `Agents` actor, which that test never starts (result `null`, expected `false`). Not touched by W2. |
| `cargo test -p roster --no-fail-fast` | `test result: ok. 67 passed; 0 failed; 0 ignored` |
| `cargo clippy -p agent -p rpc -p roster --no-deps -- -D warnings` | agent: **fails on 95 lints, all in W1's `crates/agent/src/endless/**` and `crates/agent/src/store/endless.rs`** (`missing_docs` ×~85, `dead_code` on `Inner.{rt,client,auth,catalog}`, `clippy::type_complexity`, `clippy::manual_is_multiple_of`); zero in W2's files. `cargo clippy -p rpc -p roster --no-deps -- -D warnings`: `Finished \`dev\` profile` (clean). |
| `cargo fmt -p agent -p rpc -p roster` | run; W1's `endless/**`, `store/endless.rs` and `store/durability.rs` reformatted by fmt were reverted (`git checkout`) — W1's files are not W2's to touch; everything else committed formatted. |
| `cargo check -p tui` (outside the gates, for the `/endless` command) | `Finished \`dev\` profile` |

Shared-target caveat for every line above: see §5.1.

## 3. The R1 adapter (`crates/agent/src/endless_ext.rs`) — hand-off to W1

Thin functions W2 calls today; each becomes a one-line delegation once `Endless` grows the method:

| adapter | today | W1 interface it stands in for |
|---|---|---|
| `render_snapshot(e, s) -> Vec<ViewBlock>` | one `<chat>` block rendered from `Endless::memory(s).view` (no frozen-context side effect) | `Endless::render_snapshot(session)` |
| `set_mode(e, s, mode, epoch, UnitFilter)` | logs a non-`All` filter, calls `Endless::set_mode` | `set_mode(…, UnitFilter)` |
| `backfill_estimate(&SessionMemory)` | units/ETA from `memory.backfill`, `usd: None`, `model` from `memory.model` | `Endless::backfill_estimate` (adds $ and the calibrated s/unit) |
| `set_view_bytes(e, s, bytes)` | range check + `info!`, returns `memory(s)` — **not yet applied** | `Endless::set_view_bytes` + `SessionMemory.view_bytes` |
| `UnitFilter` | local enum | W1's |
| `reply(mode, &SessionMemory)`, `status(&SessionMemory)`, `eta(secs)` | the `/endless` texts (shared by TUI; W3 may reuse) | — |

`render_with_budget` is not called by W2 (meetings are W4's).

## 4. Tests (137 §14, W2's share)
- `frame::tests::view_blocks_become_marked_user_messages` — marks per transport, cap, final block unmarked, Anthropic frame keeps system 1 h first / tail 5 min last with the view in front.
- `loop_::endless_tests::endless_turns_rebuild_the_frame_from_the_view` — two turns at the fake provider: tools and system byte-identical, no `Date:`, VIEW_DOC last, zoom/recall/date in the sorted list, `[system][view block][view block][stamped input]`, no head, turn 1 absent from turn 2's request, both turns' rows persisted.
- `loop_::endless_tests::epochs_session_keeps_the_dated_prompt_and_no_endless_tools` — the epochs path untouched (the existing frame prefix-oracle tests pass bit-for-bit too).
- `loop_::endless_tests::overflow_in_an_endless_turn_cuts_with_the_view_as_head` — tool round, overflow 400, `Compacted{epoch 1, cut}`, retry request's head is `<context_summary epoch="1" style="cut">` holding the view, no summariser call, view blocks no longer prepended.
- `loop_::endless_tests::endless_off_installs_the_view_head_as_the_next_epoch` — `install_view_head` writes epoch 1 with the given head and an empty tail; a row-less session gets none.
- `loop_::endless_tools::tests::*` — specs/descriptions, `render`.
- `endless_ext::tests::estimate_and_reply_follow_the_backfill_state` — estimate, reply and status texts.
- `supervisor::tests::memory_mode_is_created_queried_and_switched` — `session_memory` of an epochs session → `mode: epochs`, empty view, zero stats; `create_session.memory` lands; `/endless` flips an idle session both ways; `view_bytes` range; unknown session refused. (Design §14 places this under `cargo test -p rpc`; the rpc crate has no engine, so it lives where the actor does.)
- `roster::file::tests::memory_forms` — parse + default + render round trip + errors.

## 5. Deviations and decisions

1. **Settle runs on the session actor** (`start_turn`), bounded by `settle_secs` and the turn's cancel token. The opener row must be persisted *after* `render_for_turn` (contract §4) and before `TurnStarted` (whose `turn_seq` is the row's seq); moving the opener's persist into the turn task would free the mailbox during a slow settle. Marked `ponytail:` at the call site. In practice settle returns in well under a second once the pump trails the chat.
2. **Resumed turns (109) run the epochs-shaped frame.** A crash mid-turn in an endless session resumes over the stored frame (head + rows since the last epoch head), without the view and without the three tools; the next turn is fresh. Reason: the loop has no cheap "first seq of this turn" (the journal's `input_accepted` step carries no seq). Follow-up: a `turn_first_seq` journal query, then slice history and run `endless_start` in `Turn::resume`.
3. **Actor start loads the whole epoch** for an endless session (`Store::frame` as today): with no compaction ever, that is every row since `/endless on`. Bounded in time by the next turn's clear; a seq floor at the last `turn_finished` would bound memory. Noted, not built.
4. **R1-D1 child view** rides as separate text parts *inside the opener message* (view blocks, then the stamped brief), not as its own cache-marked row. Anthropic joins a text-only user message's parts into one block, so the view is not shared across children of the same parent in the cache (one extra write per child). A two-row opener would be the upgrade; `TurnStarted.text` and the title stay the brief alone. The `CHILD_VIEW_NOTE` is appended when the parent is `active()` at child start.
5. **`/endless` reply text lives in the engine** (`endless_ext::reply`/`status`) so every client says the same thing; the TUI command is wired, the desktop's is W3's.
6. **`view_rebase` is one line after the first response** (it carries the provider's `cache_read`/`cache_write`); a turn whose first call fails logs no `view_rebase` (the `debug!` at turn start still shows blocks/settled/unbuilt).
7. **D11 cut body** = the fold sentence + the view text inside the `<context_summary epoch=n style="cut">` wrapper (plus the usual harness parts). A one-message endless turn that overflows has nothing to cut and errors like today ("nothing to compact").
8. **R1-D7 default** — the roster reads `agent.endless.default_desk` from W1's `EndlessConfig`, whose skeleton default is `false`; R1-D7 says `true` at landing. W1/closer flip the default; the roster logic is in place either way.
9. **Desk file changes while a desk is open** (`memory:` edited) are applied at the next desk open/reconcile, not live; `/endless` on the desk session works at any idle moment.
10. **`memory_mode_is_created_queried_and_switched`** exercises the store column through the skeleton's `set_mode`; the head text of `off` and the backfill come from W1's implementation and are covered by `install_view_head` with a given head.
11. **R1-D5 `/endless import`** — not built (hand-off, §6).
12. **Fixed a pre-existing broken test** in my file: `loop_::tests::prompt_parts_in_order_with_blank_lines` asserted `!full.contains("rules")`, which the shared-hardware guidance ("Write the rules you are under…", commit `2a7253e4`) made false; it now asserts `!full.contains("- rules:")` (the skills index line) — the intent (an `always` skill is inlined, not listed).

### 5.1 Build-environment notes
- **Shared `CARGO_TARGET_DIR` collides across 137 worktrees.** Cargo hashes workspace-member artifacts relative to the workspace root, so `agent` from `theuth-137-w1` and `-w2` share one `.rmeta`/`.rlib`; `cargo check -p rpc` once linked against W1's `agent` (an error pointed at `endless/mod.rs:299`, a line only W1's tree has). Workaround used for every gate: `touch crates/agent/src/lib.rs` (and the other crate roots) right before the cargo call so my tree's artifact is rebuilt first. Reported to the orchestrator. An APFS clone of the 63 GB target as a private dir timed out at 52 GB and was removed.
- **W1's skeleton test code does not compile under `cargo test -p agent --lib`**: `endless/compactor.rs` test `request_marks_context_block_on_anthropic_only` calls `genai::Client::default()` and `supervisor::Auth::default()`, neither of which exists. To run the agent suite I patched those two expressions locally (`Client::new().unwrap()`, `Auth::stub(Path::new("/unused"))`), ran the gate, and reverted the file before committing — the committed tree still carries W1's version. **W1 must fix this for the agent test gate to compile.** With that patch, two W1 tests fail (`endless::units::tests::derives_units_from_stored_row_shapes` at units.rs:118 — JSON key order of `fn_arguments`; `store::endless::tests::endless_rows_round_trip_and_fts_follows_level0` at store/endless.rs:437); none of W2's.

## 6. Hand-offs
- **W1**: the five R1 interfaces (§3), especially `set_view_bytes` + `SessionMemory.view_bytes` (the RPC and roster already call the adapter) and `backfill_estimate` (`usd`); fix the compactor test compile (§5.1) and the two failing skeleton tests; `EndlessConfig.default_desk` → `true` (R1-D7); `render_snapshot` should not touch the frozen context.
- **W4 (meeting)**: `agent::SetSessionMemory { session, mode, filter: agent::UnitFilter::Meeting }` → `anyhow::Result<agent::SetMode>` (idle only, `Err("busy")`); `agent::RenderMemoryView { session, bytes }` → `anyhow::Result<Vec<agent::ViewLine>>`. Both handled by `Agents`; the meeting session is excluded from the fresh-turn frame on the switch.
- **W3 (desktop)**: `set_session_memory`/`session_memory`/`endless_zoom`/`endless_recall`/`set_session_view_bytes` are live RPCs; `EndlessProgress` arrives on the session event stream (`type: "EndlessProgress"`); `CreateSessionOptions.memory` is honoured; `AgentFile.memory`/`view_bytes` are in `agents_list`/`agent_put`; the `/endless` slash entry in `slash.ts` should mirror the TUI (`on|off|status`), text from `SessionMemory` as `endless_ext::reply`/`status` render it.
- **Closer**: `just ts-desktop` regenerates `AgentFile` (two optional fields) and `methods.json` (`set_session_view_bytes`); live smoke per §14 — the `view_rebase` line is `tracing::info!` target `agent::loop_` with `cache_read` as a field; the first endless turn of a session logs `changed_from_byte = None`, `blocks_hit = 0`.
- **R1-D5 `/endless import <session-id>`** (not built): append the other session's `user` texts and the final `talk` of each turn as `note` units prefixed `[imported from <title>, <date>]` through W1's unit-append path; the TUI `endless_cmd` has the parse point (`"import <id>"`), the actor side would be a `SetMemoryNow`-style mini-turn calling a W1 `append_notes(session, Vec<String>)`.
- **Follow-ups recorded** (§5): two-row child opener for cross-child cache sharing; `turn_first_seq` for endless resume; seq floor at actor start.
