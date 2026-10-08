# 141 — ACP SUBAGENT VISIBILITY, as built

Status: AS-BUILT (2026-10-08). Design: `141-acp-subagents.md` (body + Revision 1; the design stays the reference for *why*, this file records *what landed* and where it differs). Branch `feat/141-acp-subagents`, cut from main `450dbf5c`; C landed first, W1–W5 merged `--no-ff`, then the Closer. Live smoke: `141-smoke.md` (Z). The six per-wave hand-offs were folded in here and removed.

## 1. What landed, per wave

### C — contract (`63281cc4`)

`Cargo.toml` (`agent-client-protocol = "=3.1.0"`, features `process, stdio, unstable_end_turn_token_usage, unstable_subagents`), `Cargo.lock` (exactly `agent-client-protocol`, `-derive` 2.2.0→3.1.0, `-schema` 1.9.1→1.10.2; `schema→schemars`/`→tracing` edges dropped), `crates/agent/{Cargo.toml, migrations/0041_observed_children.sql, src/{lib.rs, cost.rs, store.rs, store/advisor.rs, store/dreaming.rs, session_read.rs, acp_child.rs, supervisor.rs}}`, `crates/acp/src/{lib.rs, harness.rs, omp/mod.rs}`, `crates/rpc/src/{api.rs, acp.rs}`, `crates/desktop/ui/src/rpc/extra.ts`, compile-only `observed: None` literals in `roster/desk.rs`, `slack/{board,channel,ui}.rs`, `telegram/{commands,ui}.rs`, `tui/{attach,lib,model}.rs`.

- Types (§8.1): `SessionKind::Observed`, `ObservedSource`, `ObservedState` (+`is_terminal`), `ObservedMeta`, `AgentEvent::ObservedChildChanged{parent, child, name, model, observed}` (`session()` → child), `observed`/`kind` on `AgentInfo`/`SessionSummary`/`Loaded`, `CostScope::Observed`. acp: `Update::Session{session, update}`, `Update::Subagent(ChildAnnounce)`, `ChildSource`, `ChildState`, `CostRule` (lives in `acp`, re-exported by `agent`), `Permission.session`, working `Harness::cancel_session`.
- Store (§11.2): `ObservedUpsert`, `upsert_observed` (identity `(parent, observed_source, acp_session)`), `observed_children`, `observed_child`, `observed_cursor`/`set_observed_cursor`, `mark_observed_lost`, `default_cost_rule`; `kind_str`/`kind_of` know `observed`; indexes `sessions_parent`, `sessions_observed_identity`. Q7: `Store::ledger_since` skips `kind = 'observed'` (dreaming).
- rpc `admitted`/`wanted`: `announces_child()` treats `ObservedChildChanged` like `SubagentSpawned` (test `observed_child_changed_enrols_like_subagent_spawned`). Bindings regenerated; `extra.ts` `AgentInfo.observed?`.

### W1 — transport (`244d78d1`)

`crates/acp/src/{agents.rs, harness.rs, harness/{rfd,opencode,process}.rs}`, `crates/rpc/src/acp.rs`, fixtures `crates/acp/fixtures/{opencode-2.0.22/*, opencode-1.18.30/task.*, rfd/**}`.

- `Link::spawn`: typed `SubagentUpdate` → `Update::Subagent(rfd::announce(..))`; every other update → `Update::Session{session: <wire id>}`; untyped `opencode/session/child_update` → `opencode::Children::child_update` (status → announce, update → implicit announce + child-keyed traffic, **no flattening, no chunk→thought rewrite**); `Children::spawn_join` binds the parent's `subagent` call to the announced child via `rawOutput.metadata.sessionID`; `task_v1::{is_task_call, announce}` for opencode 1.x; `initialize_request()` advertises `subagents: {}`.
- §4.3: `ChildProcess`/`ExitSlot`, `EXIT_GRACE = 1500 ms`, waited in `main` after `incoming_closed` and in `Link::explain` on any failed request. R2A G1 settled: the `exit 17` test failed 10/10 before, 0/10 after (`process_exit_status_and_stderr_reach_the_text`). §4.4: `t --acp` serves with `Builder::connect_to`.
- D14: `Entry::mcp_enabled()` (`mcp.unwrap_or(true)`); `session/new|load` with the shim retried once without it + warning (`mcp_back_channel_is_retried_without_on_failure`).

### W2 — ingest (`f5a20a4a`)

`crates/agent/src/{acp_child.rs, acp_child/observed.rs (new), supervisor.rs, store/accounting.rs}` (+ a 3-line clippy fix in `store.rs`).

- `observed.rs`: `observed_upsert` (announce → row → `ObservedChildChanged` immediate on create/state/name/model/`can_cancel`, detail-only coalesced 250 ms; child turn opens on `running|requires_action`, closes with `end_turn|error|cancelled|unknown`); `observed_traffic` (child-keyed updates → `translate()` on the child's turn, rows appended before publish; unknown child buffered 2 s; parent replay gate applies); `observed_directed` (RFD `session_message*` → `→ name:` / `← name:` lines, `MessageDelivered`); `observed_permission` (`p.session ≠ own` or `<child>:<id>` prefix → `ExternalApproval{session: child}`, `requires_action` ↔ `running`, Q5 both sources); `CancelObserved{child}` → `cancel_session` or `Error "<harness>: cannot stop this subagent; stop <parent>"`; `observed_seed` on `Connected`; `observed_lost` on actor stop; `OmpTick` tail poll (500 ms) with cursors persisted, `sidecar_done`/`settled_jobs` → `completed`, `.json` → `detail.result`; `counted` → `apply_observed_meter` (`CostScope::Observed`).
- `supervisor.rs`: §5.4 guard `OBSERVED = "watch-only: the harness runs this subagent; open or message its parent"`; `OBSERVED_PEER` refusals in `resolve`/`enqueue_peer`; `directory()` `send: false`; `state_of_row`; `Cancel` forwarded up to the nearest live `LiveActor::Acp`; boot `mark_observed_lost` sweep.
- `accounting.rs`: `apply_meter_scoped`, `apply_observed_meter`; `session_cost` reads each node's stored scope (was hard-coded `OpaqueRemoteInclusive`).

### W3 — desktop (`088e217c`)

`crates/desktop/ui/src/`: `lib/{session-tree.ts, money.ts, rpc.ts}`, `store/{reduce.ts, sessions.ts}`, `components/chat/{ObservedBar.tsx (new), RosterCard.tsx (new), MessageTool.tsx, SubagentStrip.tsx, Header.tsx, ModelMenu.tsx, CostPill.tsx, Composer.tsx, slash.ts}`, `components/sidebar/{AgentTree.tsx, SessionItem.tsx}`, `components/library/Skills.tsx`, `routes/c.$session.tsx`; tests `lib/session-tree.test.ts`, `store/reduce.test.ts`, `components/chat/{RosterCard,ObservedBar,slash}.test.ts`, `components/sidebar/SessionList.test.ts`.

- §9 end to end: `ObservedChildChanged` → child slice + ancestor rows (no `↳ spawned`); tree rows eye · source tag · `WatchDot` (Q1 idle greyed, no check); `RosterCard` replaces the spawn tool body (Q4 disclosure, Q9 progress bar, cost per rule); strip dots titled "watching"; watch-only `LiveSession` with `ObservedBar` instead of `Composer`; "Watching" chip + `inside <parent> · <source>` crumb; `ModelMenu` gated; cost breakdown eye + rule line.
- D15 a/b: Skills panel harness rows send `/<command>` raw (`sendRaw`); slash menu merges harness commands with source labels; typed `/skill:<name>` keeps parking theuth's skill.

### W4 — TUI + channels (`5136ed9d`)

`crates/tui/src/{model,ui}.rs`, `crates/slack/src/{board,channel}.rs`, `crates/telegram/src/{channel,commands}.rs`.

- TUI: `◎ <name> <state> watch · <src>[ · <lastIntent>]` rows; `watching · <state>` line on the shown child; header `◎ <name>  watching · <model>`; `input_focused()` false; status hint `watch-only — <harness> runs this subagent · Esc stops it | cannot be stopped here`; Esc/`/cancel` → `Command::Cancel(child)` only when `can_cancel` and a turn is open; D15 c `slash_rows` lists harness commands `/<name>  [<harness>] <desc>` and sends them raw.
- Slack: `route()` → `Out::Observed` board rows on bound ancestors (`◎ ReadA  running  watch · omp task · …`; failed/lost `error · <stop|lost contact>`), no thread/root post/Cancel. Telegram: `ChildState::observed` rows, no topic/marker; `/cancel <name>` refuses unless `can_cancel`.

### W5 — omp (`3c45076a`)

`crates/acp/src/omp/{mod,progress,tail}.rs`, fixtures `crates/acp/fixtures/omp-18.3.1/{task.{tool_call,completed_stale,mid,final}.json, ReadA.{jsonl,md,json}, parent.jsonl}`.

- Feed 1: structural `progress_rows` (call status ignored, F18); `Progress::announce` with the full §6.4 detail (+ Q9 `completionPercent`, `currentToolIntent`, `recentTools[].isError`), nested `a.b → a`, background → `Counted`, sync → `InsideParent`, `detail.batch` on every row.
- Feed 2: `Tailer::{locate, child_path, read, sidecar_done, settled_jobs, sidecar_text, result_json}`, `Entry` parser, `to_updates` → SDK `agent_message_chunk` / `tool_call` / `tool_call_update`.

### Closer

- `crates/agent/src/acp_child/observed.rs` `observed_task_frame`: opencode < 2 (`Harness::usage_is_lower_bound`) frames go through `acp::harness::opencode::task_v1::announce` beside omp's `progress_rows` — the one call path neither W1 nor W2 owned. Test `acp_child::tests::opencode_task_frames_announce_and_rekey_one_child` drives the recorded 1.18.30 `task.*` frames through the `Fake` (replaces the announce-fed re-key test); a 2.x harness ignores them.
- Un-ignored `acp_child::tests::raw_child_update_frame_reaches_the_observed_child` (§4.5 test 2) — green on W1's adapter.
- `supervisor.rs` reads `entry.mcp_enabled()`; the last `141 C stub` comment (slack `Thread::apply`) reworded — `grep -rn "141 C stub" crates` is empty.
- Docs: `06-acp.md` status line, §3.8, §8.4 pointer, new §8.5; `75-acp-deep.md` §9 D14 bullet; this file; hand-offs removed.

## 2. Deviations from the design

| where | design | as built | why |
|---|---|---|---|
| migration 0041 | eight `observed_*` columns | nine: `+ observed_cost_rule TEXT NULL` | `ObservedMeta.cost_rule` is per row (omp background vs sync) and not derivable from `observed_source`; NULL reads as `default_cost_rule(source)` |
| `CostRule` home | `crates/agent/src/lib.rs` | `crates/acp/src/harness.rs`, re-exported by `agent` | `ChildAnnounce.cost_rule` needs it and `agent` depends on `acp` |
| acp modules | `crates/acp/src/{rfd,opencode,process}.rs` | `crates/acp/src/harness/{rfd,opencode,process}.rs` (`acp::harness::rfd`, `acp::harness::opencode`) | `lib.rs` was C's; no re-export added — nothing needs `acp::rfd` |
| omp tail `tool_execution_start` | `ToolCallStarted` if not yet seen | emits nothing (`Entry::ToolStart` parsed, unused) | omp writes the assistant `toolCall` part first; a stateless mapper would duplicate the card across cursor batches |
| omp `.md` sidecar text | final assistant message when the jsonl had no yield | not wired (`Tailer::sidecar_text` exists) | the final assistant text is already in the jsonl as an assistant entry; the `.md` is a copy — wiring it would duplicate the message. `.json` → `detail.result` **is** wired |
| §7.2 item 6 omp after close | keep tailing 10 s after the parent closes | children → `unknown` at once when the actor stops; settle via tail/sidecars on the next `Connected` | the actor stops with the harness (`closed()` → `ctx.stop()`); keeping it alive for one source was more state than the 10 s bought |
| stale `completed` rows (F18) | ignored when a later snapshot exists | applied in wire order as `pending`; the next snapshot overwrites | the transport is ordered; the stale frame's rows are `pending`, never `completed`, so nothing regresses |
| RFD directed messages | `HistoryEntry::Info`-class row | persisted once as an assistant row | the agent crate has no `Info` history entry |
| TUI typing on a watch-only view | swallowed | swallowed, except `/` opens one slash command | the TUI has no non-slash way to `/parent`/`/agents`/`/sessions` |
| TUI `requires_action` tool label | `Approval{tool}` | `Approval{tool: detail.currentTool ?? "permission"}`; desktop `approval{tool:'permission'}`; engine `state_of_row` `Approval{tool: description}` | the real tool name is not on `ObservedMeta` |
| D14 "today `mcp: true` opts in" | — | the engine already defaulted on (`883449cf`); W1 made it the stated contract + retry-without | brief fact was stale |
| `OrgChart.StripDot` | not in §9 | left plain (green dot, no eye/state colours) | not listed; open item |
| `crates/agent/bindings/*.ts` | — | stale tracked copy from 2026-09-26 left alone | nothing reads it; `just ts-desktop` writes `crates/desktop/ui/src/rpc/bindings/` |

## 3. Gate results (Closer, this tree)

All through `BUILD_LABEL=141-closer build-lock`, stable toolchain, `CARGO_BUILD_JOBS=4`, `CARGO_INCREMENTAL=0`, `CARGO_TARGET_DIR=$PWD/target`.

| gate | result |
|---|---|
| `cargo test -p agent --lib` | ok — 540 passed, 0 failed, 2 ignored (pre-existing `crash_child`, `measure_dump_knee`) |
| `cargo test -p acp` | ok — 45 passed |
| `cargo test -p rpc --lib` | ok — 55 passed |
| `cargo test -p tui` | ok — 149 lib + 1 bin passed |
| `cargo test -p slack` | ok — 146 passed |
| `cargo test -p telegram` | ok — 124 passed |
| `cargo test -p roster` | ok — 68 passed |
| `cargo build -p theuth` → `t` | ok — `target/debug/t` (`t 2.12.3`), copied to `~/.theuth/briefs/2026-10-08/141/smoke/t`; `INTEGRATED` = `c03ce741` + that path |
| `just ts-desktop` | ok — `export_bindings` 8 passed; `ObservedMeta.ts`, `ObservedSource.ts`, `ObservedState.ts`, `CostRule.ts`, `SessionKind.ts` present |
| desktop `bun run check` (tsc) | ok — clean |
| desktop `bunx vitest run` (full) | ok — 203 files, 1400 tests passed |
| desktop `bun run build` (vite) | ok — `✓ built in 10.13s` (chunk-size warning only, pre-existing) |
| `cargo clippy --workspace --all-targets -- -D warnings` (under `/tmp/theuth-build.lock`, `BUILD_SLOTS=1`) | ok — `Finished`, no warnings (only the pre-existing `block v0.1.6` future-incompat note) |
| `cargo fmt --all -- --check` | 26 files differ, **none touched by 141** (pre-existing on main under this rustfmt; not fixed — outside the wave) |
| `git diff $(git merge-base main HEAD) HEAD -- Cargo.lock` | 15 lines: `agent-client-protocol` + `-derive` 2.2.0→3.1.0, `-schema` 1.9.1→1.10.2, dropped `schemars`/`tracing` edges — nothing else |
| `grep -rn "141 C stub" crates` | empty |

## 4. Open items

- Z smoke findings (`141-smoke.md`) — any ✗ is a Closer ticket.
- `OrgChart.StripDot` for observed rows (eye/state colours).
- Per-child stop for omp/opencode; theuth-as-harness RFD emission over `t --acp`; claude/codex draft names; opencode HTTP route (D12).
- D13 upstream proposals (omp `modes/acp` RFD emitter, opencode RFD frames) — after Z, on Sky's go (Q8).
- Screenshots of the four desktop surfaces (W3 could not run `vite dev` under build-lock) — Z.

## 5. R1 — live smoke tickets (Z, 2026-10-08 17:33) and fixes

Z ran the §13.3 matrix against the `INTEGRATED` binary (28/40 pass, 5 fail, 7 not run; `141-smoke.md`). Each ticket below has a fixture-driven test that failed before and passes after.

| ticket | symptom | fix | test |
|---|---|---|---|
| T1 omp tail stopped at the first terminal observation | `omp_tick` filtered `!terminal`; the yield call/result/`session_exit` written in the same second as feed 1's `completed` were never read (`observed_cursor` < file length) | `ObservedChild::tails()`: an omp child is tailed while non-terminal **and** for `OMP_LINGER` (10 s) after its last change; `omp_poll`/`omp_tick` use it, so a terminal transition from any feed is followed by drains to EOF (late entries land as a one-shot turn) | `acp_child::tests::omp_tail_drains_after_completion_and_meters_counted_cost` |
| T2 `counted` omp children metered 0 USD when the parent answered first | only feed 1's `detail.cost` drove `apply_observed_meter`; omp drops post-turn frames (F20) | the tail's per-turn `usage.cost.total` is summed per child (`ObservedChild.tail_cost`) and applied through the same high-water meter key (`<acp sid>/<external>`); feed 1 and the tail measure the same money, so the higher wins and nothing doubles (`meter_child`). Not persisted: after a restart the sum restarts at 0 and only exceeds the stored high-water once it catches up (`ponytail:` note in code) | same test (0 → 114 902 500 → 121 280 250 nanos) |
| T3 spawn-ack stale rows flipped `running → pending → running`, leaving empty `Assistant ""` rows | the `completed` acknowledgement's `pending` rows were applied after the live `running` snapshot; each flip closed/reopened the child turn, and `Turn::close("")` ends on an empty assistant row by the parent's invariant | `observed_upsert`: `pending` is accepted only for a new or still-pending row (never a regression); `close_child_turn` drops empty assistant rows (no tool calls, no text) — a watch-only child's history has no reader of that invariant | `omp_task_frames_announce_two_children_through_progress_rows` (re-sends the stale frame after `mid`; asserts no `Pending` event, one child `TurnFinished`, no empty assistant rows) |
| T4 opencode 1.x child had an empty transcript | the `<task_result>` text was only in `observed_detail.result` | on a terminal `opencode_task` announce with `detail.result`, the text goes through `apply_child_update` (`agent_message_chunk`) into the child's turn before it closes — one `TextDelta`, one persisted assistant row | `opencode_task_frames_announce_and_rekey_one_child` (asserts the delta and `[(Assistant, "alpha line one")]`) |
| T5 aborted omp children read `completed` after an engine restart | omp aborts its jobs on exit and writes a **zero-byte** `.md`; `sidecar_done` took any `.md` as completion; the boot sweep skipped children whose harness was respawned, so the stored `running` survived | `Tailer::sidecar_done` requires a non-empty `.md`; `Entry::Assistant` gains `stop_reason`, and `stopReason: "aborted"` → `cancelled` / `observed_stop = "aborted"` after the entries (incl. `Command aborted`) landed in the turn; `observed_seed` marks every stored non-terminal child `unknown` / "lost contact" on a new connection (D11) until a real observation moves it | `omp::tail::tests::aborted_child_entries_carry_the_stop_reason`, `sidecar_and_async_result_settle_children` (0-byte `.md`), `acp_child::tests::aborted_omp_child_is_cancelled_not_completed_after_a_restart` (unknown first, cancelled, never completed) |
| T6 `resume_child` on a watch-only row said "not an external harness…" | the observed check came after the `agent` check | `ResumeChild` refuses with the §5.4 `OBSERVED` text first | `supervisor::tests::resume_child_refuses_native_node_and_unstored` |

New fixture: `crates/acp/fixtures/omp-18.3.1/SlowA.aborted.jsonl` (recorded 2026-10-08 after SIGTERM of the smoke engine; `title`/`session_init`/`model_usage` entries dropped, long strings trimmed to 60 chars). Test hook: `AcpChild.omp_home` (the tail root; `std::env::home_dir()` in production, a scratch dir in tests).

Gates after R1 (`BUILD_LABEL=141-closer build-lock`): `cargo test -p acp` ok — 46 passed; `cargo test -p agent --lib` ok — 542 passed, 0 failed, 2 ignored (pre-existing); `cargo clippy -p acp -p agent --all-targets -- -D warnings` clean; `cargo fmt -p acp -p agent --check` clean; `cargo build -p theuth` ok → `smoke/t` replaced, `FIXED-R1` written.
