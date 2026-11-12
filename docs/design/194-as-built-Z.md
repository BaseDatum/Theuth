# 194 as-built — Z (closer)

Branch `feat/194-effort` from `1ded15b6` (origin/main `1784059b` + the contract).

## Merges

All by sha, `--no-ff`, in relay order. S1 (`2587a9ae`) arrived inside B2 (`58ff271e`) and C (`97ef88ac`), never on its own.

| # | Worker | Merged sha | Merge commit | Conflicts |
|---|---|---|---|---|
| 1 | T (row 22) | `93bdffd7` | `49dc1023` | none |
| 2 | T R1 fix | `6aab0d80` | `069dd5b2` | none |
| 3 | B1 (S1, rows 7–10) | `56d4846b` | `87045236` | none |
| 4 | A (rows 3–6, D4h) | `298ac48e` | `af817374` | 2 (below) |
| 5 | B2 (rows 11–15) | `fe0cf447` | `bf29cd2f` + fix `4ee2fbe9` | 1 + a duplicate message (below) |
| 6 | C (rows 16–20) | `abc1839f` | `1f910da6` | 4 (below) |
| 7 | B1 R1 fix | `16ce3134` | `1eabee6a` | none |
| 8 | C R1 fix | `680d62de` | `52a505e2` | 1 (below) |
| 9 | A R1 fix | `528bcc77` | `7dc35b1f` | 1 (below) |
| 10 | B2 R1 fix | `72aa6ac9` | `f4470c98` | 5 (below) |

## Conflicts and resolutions

Every one was additive or a worker's own rewrite of its own code meeting Z's rename; no worker's change was dropped.

- **A ← B1, `crates/agent/src/loop_.rs`**: B1's `Session::with_role_efforts` and A's `with_subagent_effort` builders at the same spot → both, B1's first. **`supervisor.rs`** `Agents::session` builder chain: `.with_role_efforts(..)` and `.with_subagent_effort(..)` → both.
- **B2 ← A, `crates/agent/src/lib.rs`**: A's `ValidateEffort { model: Option<String>, effort: String }` and B2's `CheckEffort` (same fields, same rule) at the same spot, and each branch added its own `impl Agents { fn check_effort }` (a duplicate method). Integration fix `4ee2fbe9`: one message, A's name `agent::ValidateEffort` (doc names both users), handled by one `Agents::check_effort` = B2's body over `endless::pump::check_effort` (the single rule); B2's `CheckEffort` handler and A's duplicate body removed; B2's callers (supervisor test, `dreaming::night::settle_effort`, `meeting::settle_summary_effort`, `roster::settle_memory_effort`) renamed.
- **C ← B2, `crates/meeting/src/lib.rs`** `SetSettings` handler: B2's `checked_summary_effort` then C's `settle_effort` for `meeting.effort` → both, in that order on one `m`. Tests block: B2's three summary tests and C's two meeting-effort tests → all five. **`settings/Meetings.test.ts`**: the `ModelCombobox` mock records both ways (B2's `chips[label]`, C's `handlers[\`${label} effort\`]` + `data-levels`/`data-effort`); B2's and C's 194 tests both kept.
- **C R1 ← A/B2, `supervisor.rs`**: the shared `Agents::check_effort` impl and C R1's new `Message<lifecycle::ModelMeta>` handler at the same spot → both.
- **A R1 ← B2, `crates/roster/src/lib.rs`** `PutAgent`: A R1's `old_model = (model, effort)` pair and B2's `old_memory = (memory_model, memory_effort)` → both (the merged body already used both names).
- **B2 R1 ← the merged tree** (5): B2 R1 rewrote its own effort checks fail-closed (`check_effort` helpers in `dreaming/src/night.rs`, `meeting/src/lib.rs`, and `roster::settle_memory_effort`) against B2's pre-fix code, which on this branch carried Z's `CheckEffort` → `ValidateEffort` rename. Resolution: B2 R1's code in all four doc/code hunks, with the rename re-applied to the three `agent::CheckEffort` uses it brought back (inside the merge commit `f4470c98`). Meeting tests: C's two (C R1 extended `the_session_starts_on_the_picked_effort`) and B2 R1's two summary put-back tests → all kept.

Effort validators left as they are (not trivially one): `agent::ValidateEffort` → `Agents::check_effort` → `endless::pump::check_effort` (A rows 4–5, B2 rows 11–15, roster), `agent::lifecycle::settle_effort` + `ModelMeta` (C rows 16–20; same messages, plus the D4c drop and blank/`default` handling), and B1's `Agents::follow_role_model` / `loop_::side::checked_effort` (rows 7–10). All three apply `models::Model::validate_effort`'s rule and words.

## Z's own changes

- `5b40f15b` fix: D1 parity in `chat/ModelMenu.tsx` — the header picker (and the notebook column) opens the top-of-band dialog on touch, like `ModelCombobox` (the dialog body is shared with the compact header's `ModelDialog` as `PickerDialogContent`, whose sr-only title now sits inside `DialogContent`); the header trigger and the compact Model row show the Local tag.
- `2e4a6d6a` fix: the harness picker's Favorites from `5b40f15b` taken out again (ruling below).
- `54d77687` fix: D5 `localFirst` removed from `ModelCombobox`, `ModelPicker`/`viewFor` and `ModelPicker.test.ts` (its 177 U4 ordering assertions; the Local-tag assertions stay).
- `2327ab3a` test: `chat/effortEverywhere.test.ts`, the D5 guard. Scans every non-test `.tsx` under `src/`, reads each `<ModelCombobox` / `<ModelPicker` element to its own closing `>` (skipping `{…}` and strings, so arrows and nested JSX do not end it), and fails with `file:line` when `effort=` is missing. Exempt only `components/chat/ModelCombobox.tsx:ModelPicker` (forwards `effort`). Red on the base tree with exactly the 17 contract sites; red on the merged tree with `Voice.tsx:199` when its `effort=` line was removed (not committed); green on the merged tree.
- `ba921022` test: `crates/rpc/tests/method_names.rs` (wire-name uniqueness).
- `4ee2fbe9` fix: integrate B2 (above). `7d8e9415` test: `dreaming` `tidy_effort_rides_only_with_its_model_and_the_routine_still_parses` (B2 × C on the merged tree: the tidy effort reaches `schedules/dreaming.md` only beside its model, and the file parses with `schedule::file::parse` fresh, after a patch with the effort, and after the pick is cleared). Green twice.
- `eeb692ce`, `caefa758` test: the live-proof fixture (Proof). `21be656b` fix: rustfmt of two resolution hunks.
- `6a5069d4` + this file: docs (124 §8, 148 "Effort beside the model").

## Parity audit (D1, row 21 plus every row)

Features: Fav = Favorites with ★ / `Alt+Enter`; Srch = word search; Cur = the Current row; Chips = the chip row under the selected model, with Default (unless required); Keys = ↑/↓, Enter, Backspace back, chips reachable; Touch = the top-of-band dialog on coarse pointers; Trig = trigger reads `name · effort`; Local = the Local tag (rows, and the trigger).

All surfaces render the one `ModelPicker` body, so Srch, Cur, Keys and the row Local tag are shared code; the table records what each call site passes and its trigger.

| Surface | Fav | Srch | Cur | Chips | Keys | Touch | Trig | Local | Fixed in 194-Z |
|---|---|---|---|---|---|---|---|---|---|
| Chat header `ModelMenu` | ✓ | ✓ | ✓ | ✓ Default | ✓ | was popover → ✓ dialog | ✓ | rows ✓; trigger was missing → ✓ | touch dialog, trigger Local tag |
| Compact header `ModelRow` + `ModelDialog` | ✓ | ✓ | ✓ | ✓ Default | ✓ | ✓ (always a dialog) | ✓ (`Model · name · effort`) | row was missing → ✓ | Local tag on the row; dialog body shared with the header (`PickerDialogContent`), its sr-only title moved inside `DialogContent` |
| Notebook column `ModelMenu small` | ✓ | ✓ | ✓ | ✓ Default | ✓ | was popover → ✓ dialog | ✓ | as header | as header |
| Session ⋯ `SessionMenu` Model › | opens the header / compact picker (`theuth:open`) | | | | | | `Model: name · effort` ✓ | | — |
| Advisor pill → `AdvisorCommand` | ✓ | ✓ | ✓ | ✓ Default; `Off` row | ✓ | via header/dialog | pill names the advisor model | ✓ | — |
| ACP `HarnessPicker` | none, by ruling (see below) | ✓ | ✓ | ✓ no Default (D1 exception) | ✓ | via header/dialog | `agent · model · level` ✓ | n/a (harness values) | — |

Harness Favorites: Z first gave `HarnessPicker` Favorites (`5b40f15b`), then removed them again (`2e4a6d6a`) when T's review (T R1, `2bdcd7fb`) ruled the TUI harness picker star-free. Its rows are the harness's own option values, not catalog ids, so a star would land in the engine-wide favorites where no catalog picker shows it. Desktop and TUI now agree: harness = no Default chip, no Favorites.

TUI (row 22, T): parity with desktop verified from T's as-built and its R1 fixes: Favorites (⇥ ★), word search, Current, the chip row (Default first, ←→ between chips), the Local tag (` · local`), harness without Default or stars. T's `updates.rs` edits are confined to the picker-opening arms (Models, ModelSet, AdvisorStatus, HarnessOptions), which lost the chained effort step D6 removes; no `events.rs` edit.

## New RPC methods and struct fields (per api file)

For the wire orchestrator: the full rpc diff is `/tmp/194z/194-rpc.patch` (`git diff 1ded15b6 HEAD -- crates/rpc`). `crates/rpc/src/api.rs` is untouched. Every new method sits immediately after its model sibling in both the trait and the impl; none takes or returns a new struct (all `Option<String>`, or the existing `SessionMemory` / `AdvisorDefaults`), so no new `schemars::JsonSchema` derive is needed. No new workspace dependency in rpc or tui (`dependency_limit` green); `crates/rpc/tests/method_names.rs` (new) checks wire-name uniqueness across `src/**` and passes (proved red with a planted duplicate).

### New methods

| api file | Method (after) | Params → result | Agent / roster message |
|---|---|---|---|
| settings.rs | `title_effort` (after `set_title_model`) | `() → Option<String>` | `agent::GetTitleEffort` |
| settings.rs | `set_title_effort` | `(effort: Option<String>) → Option<String>` | `agent::SetTitleEffort` |
| settings.rs | `polish_effort` (after `set_polish_model`) | `() → Option<String>` | `agent::GetPolishEffort` |
| settings.rs | `set_polish_effort` | `(effort) → Option<String>` | `agent::SetPolishEffort` |
| settings.rs | `vision_effort` (after `set_vision_model`) | `() → Option<String>` | `agent::GetVisionEffort` |
| settings.rs | `set_vision_effort` | `(effort) → Option<String>` | `agent::SetVisionEffort` |
| settings.rs | `escalate_effort` (after `set_escalate_model`) | `() → Option<String>` | `agent::GetEscalateEffort` |
| settings.rs | `set_escalate_effort` | `(effort) → Option<String>` | `agent::SetEscalateEffort` |
| settings.rs | `subagent_effort` (after `set_subagent_model`) | `() → Option<String>` | `agent::GetSubagentEffort` |
| settings.rs | `set_subagent_effort` | `(effort) → Option<String>` | `agent::SetSubagentEffort` |
| settings.rs | `set_advisor_default_model` (after `set_advisor_default_effort`) | `(model: Option<String>) → AdvisorDefaults` | `agent::SetAdvisorDefaults { model: Some(model), .. }` (new field `model: Option<Option<String>>`) |
| settings.rs | `advisor_on` (after `set_advisor`, `with_extensions`; A R1) | `(session: SessionId) → AdvisorStatus` | `agent::AdvisorOnDefault` (the engine reads the live `advisor.model` / `advisor.effort`) |
| boards.rs | `theuth_agent_effort` (after `set_theuth_agent_model`) | `() → Option<String>` | `roster::TheuthEffort` |
| boards.rs | `set_theuth_agent_effort` | `(effort) → Option<String>` | `roster::SetTheuthEffort` |
| turns.rs | `set_session_compactor_effort` (after `set_session_compactor_model`, `with_extensions`, forwarded like its sibling) | `(session: SessionId, effort: Option<String>) → SessionMemory` | `agent::SetSessionCompactorEffort` |

### New fields of RPC param / result types (from the regenerated bindings, diffed against the base export)

| api file (carrier methods) | Type | Field(s) |
|---|---|---|
| boards.rs (`agent_get`, `agent_put`, `agents_list`) | `roster::AgentFile` | `memory_effort: Option<String>` (`#[serde(default)]`, skipped when none) |
| settings.rs (`endless_settings_get` / `endless_settings_set`) | `agent::EndlessConfig` | `effort: Option<String>` |
| turns.rs (`session_memory`, `set_session_compactor_*`) | `agent::SessionMemory` | `effort: Option<String>` (the effort in force), `effort_override: Option<String>` (the chat's own pick; B2 R1) |
| meetings.rs (`meeting_settings_get` / `_set`) | `meeting::MeetingSettings` | `effort: Option<String>`, `summary_effort: Option<String>` |
| meetings.rs (`meeting_start`) | `meeting::StartMeeting` | `session_effort: Option<String>` |
| meetings.rs (`meeting_get`, `meeting_list`) | `meeting::Manifest` | `session_effort: Option<String>` (skipped when none) |
| dreams.rs (`dreaming.settings` / `dreaming.set_settings`) | `dreaming::DreamSettings` | `efforts: DreamModels` (per tier, optional on the wire); `DreamConfig.efforts` in config |
| research.rs (`autoresearch.settings.get` / `.set`) | `autoresearch::config::AutoresearchConfig` | `models.executor_effort`, `models.advisor_effort`, `review.effort` (`Option<String>`); **removed**: the reserved `effort: {executor, review, advisor}` block and its type `Effort` (TS `AutoresearchEffort`) — never applied; old configs with it still load |
| research.rs (`autoresearch_draft`, draft edits) | `autoresearch::ModelsSpec` | `executor_effort`, `review_effort`, `advisor_effort`, `target_effort`, `judge_effort` (`Option<String>`, skipped when none) |
| routines.rs (`get_schedule` / `put_schedule`) | `schedule::file::ScheduleFile` | `effort: Option<String>` (front matter `effort:`) |

Not on the wire but persisted: `agent.title_effort`, `agent.vision_effort`, `agent.escalate_effort`, `voice.polish_effort`, `agent.subagents.effort` (existing key, now live), `roster.theuth_effort`, `advisor.model` setter, `sessions.endless_effort` / `endless_agent_effort` (migration `0063_endless_efforts.sql`, `SESSIONS_WIRE_SCHEMA` 61 → 63), agent files `memory_effort:`, routine files `effort:`.

## Gates (fully integrated tree, after all ten merges)

- Bindings regenerated (`BUILD_SLOTS=1 … export_bindings`: 10 passed) and copied into `ui/src/rpc/bindings`; 13 binding files and `methods.json` (+15 methods) differ from the base export, as tabled above.
- `cargo metadata --locked`: passes; `Cargo.lock` unchanged (the new dev-deps — `agent` with `test-support` in schedule, meeting, autoresearch — were already locked).
- `rustfmt --check` on every file a resolution touched: clean in Z's hunks (`21be656b` fixed two).
- `BUILD_SLOTS=1 mbx check --workspace --exclude desktop --tests`: 0 errors; 2 warning lines, both the pre-existing `codeindex/src/map/export.rs:627` unused import (not a 194 file). `mbx check -p desktop`: 0 errors, 0 warnings.
- `mbx clippy -p runtime -p agent -p roster -p rpc -p voice -p app -p dreaming -p meeting -p autoresearch -p schedule -p cursor -p tui --tests --no-deps -- -D warnings`: stops only on the pre-existing `voice/src/history/mod.rs:465` `chunks_exact_to_as_chunks` (file unchanged since `1ded15b6`); with that one lint allowed, clean.
- `mbx test` on the same 12 crates (`--no-run` first, then the same set, `--no-fail-fast`): **2132 passed, 7 failed, 11 ignored**. Re-run alone: `supervisor::tests::a_waiting_first_message_leaves_the_chat_working` passes (load). Still red alone, in files 194 did not touch (`git diff 1ded15b6 HEAD` empty for them): `agent` `sensor::tests::{fs_exists_through_monty, monty_budgets_and_allowlist, pr_watch_type_checks, sensor_http_follows_the_private_network_flag}` and `loop_::runtime_bridge_tests::guessed_direct_calls_are_bridged_or_refused` ("monty worker crashed: unsupported protocol version 6 (server supports protocol version 5)", the machine's monty worker), and `meeting/tests/calendar.rs::fastmail_floating_time_is_local_and_unknown_hosts_are_not_links` (`"2026-10-16 10:00"` vs `"09:00"`, DST through `chrono::Local`). Workers A, B1, B2 and C reported the same set.
- New Z tests: `rpc` `method_names` (and `dependency_limit`) green; `dreaming` `tidy_effort_rides_only_with_its_model_and_the_routine_still_parses` green twice.
- UI: `bunx tsc -b --noEmit`: 0 errors. Full `BUILD_SLOTS=1 bunx vitest run`: **343 files, 2347 passed, 1 failed** — `lib/injected-wrappers.test.ts` (expects a `fn contact_message(..) -> String` builder in `crates/agent/src/loop_.rs`; base `1ded15b6` has none either, only a test of that name; neither file touched by 194). The guard `chat/effortEverywhere.test.ts` and `rpc/api.test.ts` green.
- Disk: a single-crate `mbx test -p tui --lib --no-run` ran during the 19:07–19:43 DISK HOLD (for the TUI proof text) before Z saw the hold line; nothing else.

## Proof (`/tmp/194z/shots/`, not committed)

- Fixture (`eeb692ce`, `caefa758`): `dev/local-fixture.ts` answers every new 194 effort RPC from memory (`*_effort` / `set_*_effort`, `set_advisor_default_model`, and `info()` carrying the stored advisor defaults), so a chip pick round-trips; `dev/endless-fixture.ts` stores the chat's compactor effort as `effort_override`. `dev/effort-fixture.html?row=N` renders the real component of contract row N (3–20, plus 21 = the header `ModelMenu`) against no engine, with the endless, routines, dreaming and autoresearch fixtures on. Driven by headless Chrome over CDP (`--remote-debugging-port=9294`), vite on :5794; both stopped.
- `row<N>-open.png` / `row<N>-picked.png` for rows 3–21 at 1280×900 light: the picker open on a model with efforts, its chip row under the selected model (Default first and active), then the closed trigger after a chip pick. Every row's trigger read `name · <level>` after the pick (`results.txt` lists before → chips → after per row). Rows 12 (Memory panel) and 17 (Meeting start) first pick Opus in the UI: their chips belong to the chat's / start's own pick (B2 R1, C R1), and row 18 opens the routine's Advanced sections and picks Opus (the fixture routine runs on the engine default).
- Dark: `row3-dark-*`, `row7-dark-*`, `row14-dark-*`. Phone 390×844 with touch emulation (`(pointer: coarse)` true): `row4-phone-*`, `row11-phone-*`, `row16-phone-*` and `row21-phone-open` — all open the top-of-band dialog, row 21 being the header picker Z moved off the popover.
- `contact.png`: the 52 shots on one grid page. `tui.txt`: the TUI chip-row render (`ui::tests::effort_chips_render_under_the_picked_model`, `focused_chip_stays_visible_at_narrow_widths`, frames printed by a temporary `eprintln!`, reverted).

## Left

- The TUI's `/advisor on` still uses its cached advisor default (A R1 added the engine-side `advisor_on` RPC; the orchestrator keeps the TUI as a follow-up and said not to touch `crates/tui`).
- Three effort validators stay separate (listed under Conflicts); folding them is a refactor, not an integration fix.
- Pre-existing reds above (monty worker protocol, the DST calendar test, `injected-wrappers`, `voice` clippy lint, `codeindex` warning) are not 194's.
