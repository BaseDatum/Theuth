# 141 hand-off — W2 (INGEST)

Branch `feat/141-w2`, cut from C `0b5f8aa2`; merged `feat/141-acp-subagents` (now carrying W3 + W5) before the first gate and again before DONE. Files owned and touched: `crates/agent/src/acp_child.rs`, new `crates/agent/src/acp_child/observed.rs`, `crates/agent/src/supervisor.rs`, `crates/agent/src/store/accounting.rs`. Outside the brief's list: one 3-line clippy fix in `crates/agent/src/store.rs` (`observed_cursor`: `needless_question_mark`, C's code — `cargo clippy -p agent --tests` fails on `-D clippy::all` without it). `store/turn_usage.rs` needed no change: `record_remote_turn` already works for any cost owner, and an observed row is one.

## 1. What landed

### 1.1 `crates/agent/src/acp_child/observed.rs` (new; §7.2–§7.8)

State on `AcpChild`: `observed: Observed { by_external: HashMap<external, ObservedChild>, pending_traffic, directed, omp: Option<OmpFeeds> }`; `ObservedChild { id, parent, name, model, meta: ObservedMeta, turn: Option<Turn>, dirty: Option<AbortHandle>, last_emit }`.

| entry point | behaviour |
|---|---|
| `Update::Subagent(a)` → `observed_upsert` | `ChildSource` → `ObservedSource` (4-arm); parent = `a.parent` resolved through `by_external` (unknown → this row, §5.3); `ChildState` → `(ObservedState, stop, usage)`: `Idle{stopReason:"cancelled"}` → `cancelled`, `Idle.usage` → `detail.usage` (display-only) + `TurnFinished.usage` + `record_remote_turn(child)`, `Failed{error}` → `stop`, `Unknown` → `stop = NULL`; `Store::upsert_observed`; the row is re-read (`Store::session`) so `name`/`model`/`meta` are exactly what is stored; **opencode 1.x re-key** (§6.3): an `OpencodeTask` announce with `tool_call = Some(tc)` and `child != "task:<tc>"` while `task:<tc>` is known → `Store::set_acp_session(row, child)` + map re-key, same row. |
| publication | `ObservedChildChanged` immediately when created or state/name/model/`can_cancel` changed; detail-only → coalesced to one event per 250 ms per child (`ObservedDirty` timer, tokio clock). Then `TitleChanged`/`HarnessModelChanged` on the child when those changed. Then the turn transition. |
| child turn (§5.2) | opens (`TurnStarted{session: child, text: "<runtime>watch-only: the harness runs this subagent</runtime>", model: child model ∥ harness model ∥ harness name, turn_seq: 0}`) when the state becomes `running`/`requires_action` and none is open; closes (`Turn::close`, rows persisted on the child, `TurnFinished{stop_reason}`) on leaving them: `end_turn` for idle/completed/pending, `error` for failed, `cancelled`, `unknown` for lost contact. No `TurnEntered`/`TurnReturned` ledger claims (observed rows hold no permits, §5.3). |
| `Update::Session{session ≠ own, update}` → `observed_traffic` | gated by the parent's replay rule (`!worked` → dropped) in the handler; `session_message*` → `observed_directed`; only `tool_call`/`tool_call_update`/`agent_message_chunk`/`agent_thought_chunk` are child transcript (plan/usage/mode bookkeeping of a child is ignored); known child → pending/idle/unknown → `running` first (published), turn opened if none, `translate()` on the child's `Turn`, rows appended on the child id before the events; a **terminal** child receiving late content gets a one-shot turn (open → events → `TurnFinished end_turn`); unknown external → buffered 2 s (`ObservedSweep`), replayed when its announce arrives, else dropped with a warn. |
| `Update::Session{update: SessionMessage ∣ SessionMessageChunk}` (own or child) → `observed_directed` | one directed line per `messageId`: `→ <recipient>: text` when the session is the sender, `← <sender>: text` otherwise (`harness` names the driven session; a child by its row name); published as `MessageDelivered{session, from: "→ name" ∣ "← name", text}` on every chunk (accumulated text), **persisted once** as an assistant row when a whole `session_message` arrives. There is no `HistoryEntry::Info` in the agent crate, so the stored form is an assistant row. |
| own-session `tool_call(_update)` → `observed_task_frame` | `is_omp()` (agentInfo `oh-my-pi`/`omp`) && `acp::omp::progress_rows(&update)` → every row's `Progress::announce(&frame)` (W5's real adapter; `detail` incl. `cost`, `batch`) through `observed_upsert`; then the frame falls through to the parent's tool card as before. Proven end to end by `omp_task_frames_announce_two_children_through_progress_rows` on W5's `task.{tool_call, completed_stale, mid, final}.json`. **`opencode::task_v1` (opencode 1.x recogniser) is W1's and does not exist on this branch** — the Closer adds one line next to the `progress_rows` call (or W1 emits `Update::Subagent` from the transport; either shape is ingested as-is: the `task:<toolCallId>` re-key is tested with announces). |
| `Update::Permission(p)` → `observed_permission` | `p.session ≠ own` → the RFD child; else `toolCallId` `"<child>:<id>"` whose prefix is a known child (opencode); the ask is `ExternalApproval{session: child_row}` through the shared `ask_permission`; the child is set `requires_action` (published) and goes back to `running` when the responder fires (`PermissionAnswered`) — Q5, both sources. The child turn's cancel token (else `alive`) bounds the ask. Otherwise the parent path is unchanged. |
| `CancelObserved { child: SessionId }` (`pub`, re-exported `crate::acp_child::CancelObserved`) | `meta.can_cancel` → `Harness::cancel_session(external)` (a `session/cancel` with the child's own id); else `AgentEvent::Error{session: child, turn: None, message: "<harness>: cannot stop this subagent; stop <parent name∣title∣"its parent">"}`. |
| `Connected` → `observed_seed` | `Store::observed_children` recursively (nested rows under observed parents); omp + local (`!remote`) → `OmpFeeds` with cursors from `Store::observed_cursor`, poll armed. |
| actor `on_stop` → `observed_lost` | every non-terminal, non-`unknown` child → `unknown` / `"lost contact"` (written, published), its turn closed `unknown`. Covers `Closed`, `Shutdown`, `KillTimer`, connect failure. **Deviation from §7.2 item 6**: omp children do not keep tailing 10 s after the parent closes — the actor stops with the harness (`closed()` → `ctx.stop()`), so they are marked `unknown` at once and settle on the next `Connected` through the tail (`sidecar_done`/`settled_jobs` run on `unknown` rows too, which are non-terminal). |
| `OmpTick` (§7.3) | every 500 ms while any omp child is non-terminal: `Tailer::locate` (retried until found), `settled_jobs(parent_cursor)`, per child `read(id, cursor)` → `Entry::User` opens the turn with the assignment text, `Entry::Model` fills an unknown model, `Entry::Assistant{usage}` → `record_remote_turn(child, TurnId::new(), …)`, then `to_updates(entries)` through the traffic path; `set_observed_cursor` after each applied batch; `sidecar_done ∥ settled` → `detail.result` from `Tailer::result_json` (when present) then `completed`. Q6: W5's `read` resets the cursor on truncation; the actor stores whatever it returns. `Tailer::sidecar_text` (".md text → final assistant message when no yield") is **not** wired — the yield's `result` already lands in `detail` from the live rows. Not exercised by a Rig test (needs a real `~/.omp` tree; W5's unit tests cover the tailer). |
| cost (§7.6) | `cost_rule == counted` && `a.detail.cost` numeric → `meter_update(harness, "<acp sid>/<external>", cost, "USD", confirmed_new_session: true)` → `Store::apply_observed_meter(child, …)` (high-water, once). `inside_parent`/`display_only`/`unreported` write no money. |

### 1.2 `crates/agent/src/acp_child.rs`

`mod observed; pub use observed::CancelObserved;` — field `observed: Observed`; `Message<Update>` arms in this order: `Subagent` → `SessionMessage*` → `Session{session ≠ own}` (child traffic, `worked`-gated) → the existing bookkeeping arms → generic own-session arm (now calls `observed_task_frame` first) → `Permission` (child first, then parent) → `Peer` → `Closed`. `Connected` seeds after `self.harness = Some(h)`. `on_stop` calls `observed_lost`. `permission()` was split into `permission()` (parent) + `ask_permission(p, session, cancel, answered)` (shared).

### 1.3 `crates/agent/src/supervisor.rs`

- §5.4 guard in `Agents::session()` right after the row read: `kind == Observed || row.observed.is_some()` → `anyhow::bail!(OBSERVED)`; `pub const OBSERVED: &str = "watch-only: the harness runs this subagent; open or message its parent"` — covers SendInput/Steer/UseSkill/LateAnswer/Undo/Redo/Compact/`actor()`/replay/ResumeChild.
- `resolve()` and `enqueue_peer()`: Observed → `Err("`<to>`: watch-only harness subagent — message its parent")` (`pub const OBSERVED_PEER`).
- `directory()`: observed rows `send: false, reply: false`.
- `state_of_row()` (new; used by `agents()` and `activity_rows()`): an approval keyed to the child wins; else `running → Running`, `requires_action → Approval{tool: description}`, `failed → Error`, `cancelled → Cancelled`, `pending∣idle∣completed∣unknown → Idle` (UI labels pending/idle/lost contact from `observed.state`, Q1).
- `Cancel`: `Root` on an observed id with no live actor → `cancel_observed(row)`; `Tree` → every observed descendant with `can_cancel` → `cancel_observed`; the forward walks `parent` up to the nearest live `LiveActor::Acp` (≤ 8 hops) and tells `CancelObserved{child}`.
- Boot (`ActivityRegistered`, after `respawn_acp_children`): `mark_observed_lost()` → `Store::mark_observed_lost(live ACP sessions)` and one `ObservedChildChanged` per lost row.

### 1.4 `crates/agent/src/store/accounting.rs`

`apply_meter` → `apply_meter_scoped(.., CostScope::OpaqueRemoteInclusive)`; new `apply_observed_meter(owner, update)` → `CostScope::Observed`; `MeterRow` reads `scope` and `session_cost` sets each node's scope from the stored column (was hard-coded `OpaqueRemoteInclusive`), so `SessionCost.scope` and `children[].scope` say `observed` for a counted child.

## 2. Event ordering actually emitted (for W3/W4)

Per announce on child C (all on C's id unless noted):

1. `ObservedChildChanged{parent, child: C, name, model, observed}` — on create and on every state/name/model/`can_cancel` change; detail-only changes ≤ 1 per 250 ms.
2. `TitleChanged{session: C}` when the name changed on an existing row; `HarnessModelChanged{session: C}` when the model changed.
3. `TurnStarted{session: C, turn, text: "<runtime>…</runtime>", model, turn_seq: 0}` when the state entered running/requires_action (or on first child traffic / first omp user entry — then `text` is the assignment).
4. Translated child events (`TextDelta`, `ReasoningDelta`, `ReasoningDone`, `ToolCallStarted`, `ToolCallFinished`) as traffic arrives; rows are appended before each publish.
5. `TurnFinished{session: C, turn, stop_reason, usage, context: 0}` when the state left running/requires_action (`end_turn`/`error`/`cancelled`/`unknown`).
6. `ModelUsageChanged{session, revision}` for C and its ancestors when a `counted` meter or RFD/omp tokens landed.
7. `Error{session: C, turn: None}` on a `CancelObserved` the harness cannot honour.
8. `MessageDelivered{session: S, from: "→ name" ∣ "← name", text}` for RFD inter-session messages (per chunk, accumulated).

Per source on this branch: RFD/opencode_ext/opencode_task/omp_task all follow 1–5 (tested by feeding `ChildAnnounce`s); omp live rows additionally produce 6 on every cost change. `Agents` sees 3/5 on its bus subscription (`running` set, `last_stop`) exactly as for any session; `child_finished` is a no-op for an observed id (not in `sessions`).

## 3. Assumptions about other waves

- **W1**: `Update::Subagent` + `Update::Session{session: child}` from `child_update`/RFD as C's types describe; `Permission.session` = the asking session; opencode 1.x `task_v1` either emits `Update::Subagent(ChildAnnounce{source: OpencodeTask, child: "task:<toolCallId>" then the real id, tool_call: Some(toolCallId)})` from the transport or exposes a function the Closer wires beside `observed_task_frame`. The verbatim-frame test `raw_child_update_frame_reaches_the_observed_child` is `#[ignore]`d until W1's adapter is merged (C's stub flattens the frame onto `rootSessionId`, which is not the Fake's `s1`).
- **W5** (landed, merged): `detail.cost` is the USD number the `counted` meter reads; `Tailer::read` resets on truncation (Q6); `to_updates` drops user entries (the actor opens the turn from `Entry::User`); `announce` already writes `detail.batch`.
- **W3/W4**: `ObservedChildChanged` is the only upsert; no `SubagentSpawned` for observed rows; `AgentInfo.state` for an observed row follows §1.3's mapping; `requires_action` without an actual approval keyed to the row shows `Approval{tool: description}`.
- **C**: `ObservedUpsert.model` writes `harness_model` (used for the child's own model); `Store::session` returns `observed` for observed rows; `default_cost_rule` applies when an announce never set one.

## 4. Gate output

All through `BUILD_LABEL=141-W2 build-lock`, stable toolchain, `CARGO_BUILD_JOBS=4`, `CARGO_INCREMENTAL=0`, `CARGO_TARGET_DIR=$PWD/target` (the warm clone never wrote `.ready`; built on the partial copy after `cargo clean -p sentencepiece-sys` cleared a foreign `CMakeCache.txt`).

```
cargo test -p agent --lib -- acp_child:: supervisor:: store::accounting store::turn_usage
running 137 tests
test acp_child::tests::raw_child_update_frame_reaches_the_observed_child ... ignored, 141: needs W1's opencode child_update adapter; Closer re-enables
test result: ok. 136 passed; 0 failed; 1 ignored; 0 measured; 405 filtered out; finished in 12.77s

cargo check -p rpc --all-targets      → Finished (4m 24s)
cargo clippy -p agent --tests         → Finished, no warnings
cargo fmt -p agent                    → clean
```

New tests: `acp_child::tests::{rfd_child_is_announced_watched_and_cancelled_on_its_own_id, opencode_child_updates_become_the_childs_transcript, child_permission_is_asked_on_the_child_and_flips_requires_action, opencode_task_provisional_row_is_rekeyed_to_the_real_id, omp_rows_count_their_cost_once_and_nest, omp_task_frames_announce_two_children_through_progress_rows, children_lose_contact_when_the_harness_stops, unknown_session_update_kinds_are_tolerated (§4.5 test 3), raw_child_update_frame_reaches_the_observed_child (§4.5 test 2, ignored)}`; `supervisor::tests::{observed_rows_are_refused_as_targets_and_listed_watch_only, cancel_of_an_observed_child_is_forwarded_to_its_harness}`; `store::accounting::tests::observed_meter_rows_keep_their_scope_and_roll_up`.

## 5. For the Closer

- Un-`ignore` `acp_child::tests::raw_child_update_frame_reaches_the_observed_child` once W1 lands; it reads the fixture's `childSessionId`/`toolCallId` so a fixture refresh does not break it.
- Wire opencode 1.x `task_v1` (W1) beside the `progress_rows` call in `observed_task_frame` if W1 did not emit it from the transport.
- Optional: `Tailer::sidecar_text` → a final assistant message on the child when the transcript had no yield (§6.4 `.md` row).
- §7.2 item 6's "keep tailing 10 s after close" is not implemented (see §1.1 `on_stop`); if wanted, keep the actor alive after `Closed` for omp while omp children are non-terminal.
