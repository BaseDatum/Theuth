# 194 as-built — B2 (rows 11–15, `localFirst` at two call sites)

Worker B2, branch `feat/194-b2` (base 1ded15b6), S1 merged at 2587a9ae (`Merge 194-S1`).

## Rows built

| # | Picker | Model → effort storage | Applied at | RPC |
|---|---|---|---|---|
| 11 | Settings › Memory › Compactor | `agent.endless.model` → `agent.endless.effort` (`EndlessConfig.effort`) | every compactor request whose model came from the settings link | none: `endless_settings_get/set` carry the field (D3) |
| 12 | Chat › Memory panel › Summary model | `sessions.endless_model` → `sessions.endless_effort` (migration `0063_endless_efforts.sql`) | that session's compactor | `set_session_compactor_effort(session, effort) -> SessionMemory` (new, after its sibling in `TurnsApi` and api.ts); the read path is `SessionMemory.effort` |
| 13 | Agents › Memory summary model | agent file `memory_model:` → `memory_effort:` (`AgentFile.memory_effort`, `#[serde(default)]`); per desk session `sessions.endless_agent_effort` | its desk's compactor | none (agent file via `agents_put`/`PutAgent`); `SetAgentCompactorModel` gained `effort` |
| 14 | Settings › Meetings › Summary model | `meeting.summary_model` → `meeting.summary_effort` (`MeetingConfig`/`MeetingSettings.summary_effort`) | the summary turn (a session switch, not a side call: `SetEffort` beside the existing `SetModel`, both put back after) | none: `meeting_settings_get/set` carry the field |
| 15 | Settings › Dreaming › tier models | `dreaming.models.<tier>` → `dreaming.efforts.<tier>` (`DreamConfig.efforts`, `DreamSettings.efforts`) | judge: the replay side call (`ModelRole::NamedWithEffort`, S1); tidy: the routine's `effort:` front matter | none: `dreaming.settings/set_settings` carry the field |

### Engine pieces
- `ModelCompactor.chosen: bool`: `effort` is the user's pick, sent on every transport (no `anthropic_off`, no local thinking-off kwargs), with its thinking budget added to the reply cap. `chosen: false` keeps R1 Q1 (thinking off) byte-for-byte.
- `pump::chosen` returns `(model, source, effort)`: the chat's own effort (`endless_effort`) wins over any link; otherwise the choosing link's effort (agent → `memory_effort`, settings → `agent.endless.effort`); the title and session links carry none (thinking off, D4a).
- `pump::usable_effort`: an effort the resolved compactor model does not take is dropped at use with a `warn!` (the chat's own effort under a changed inherited model).
- `pump::check_effort` (`models::Model::validate_effort`, else `effort_of`) is the single check; `Agents::check_effort` and the new `agent::CheckEffort { model, effort }` message wrap it for the roster, meeting and dreaming.
- `agent::SessionEffort(SessionId) -> Option<String>` beside `SessionModel` (the meeting switch's put-back).
- `Endless::set_effort` (chat), `Endless::set_agent_model(session, model, effort)`; `Endless::set_model` unchanged in signature.
- `Store::set_endless_effort(id, effort, agent)`; `EndlessMeta.effort_override` / `agent_effort`.
- **`SESSIONS_WIRE_SCHEMA` 61 → 63** (store.rs, its snapshot test retaken): 0063 adds two columns to the replicated `sessions` table, so the bump rule applies. Session sync holds between engines on 61 and 63 until both run 63.

### D4 per setting
- **11**: `SetEndlessSettings` trims the effort; `model: None` → effort `None` (D4d); an effort the model refuses → refused (`effort: <validate_effort words>`) when the model is unchanged (D4b), dropped and persisted `null` when the model changed (D4c). Written quoted (`effort: "high"`) with `write_nested_config_value`; live through `Endless::set_config` → re-resolve (D4f).
- **12**: `set_session_compactor_effort` is checked against the compactor model in force (unattached: the chain's model, else the session's); `set_session_compactor_model(None)` clears it (D4d); a new chat model that does not take it drops it (D4c). Live via re-resolve.
- **13**: `PutAgent` runs `settle_memory_effort`: cleared with `memory_model`, dropped when a changed `memory_model` does not take it, refused (`memory_effort: …`) on a new file or an unchanged model. Unchecked while the agent actor is down; the compactor drops it at use then. Desk open and every change send `SetAgentCompactorModel { model, effort }`; reconcile and `PutAgent` compare `(memory_model, memory_effort)`.
- **14**: `SetSettings` runs `checked_summary_effort` (same rule set, `summary_effort: …`). The summary switch reads the prior effort first, `SetModel` when the model differs, `SetEffort` when the effort differs; `take_summarizing` restores the model, then the effort (`Restore { model, effort }`).
- **15**: `SetSettings` runs `tier_effort` per tier (same rule set, `efforts.<tier>: …`).

## Decisions where the contract was silent
- **Chat effort vs inherited model (row 12).** The chat's own effort applies over whichever link chose the model (the panel shows the model in force, so its chips must act on it). Other links' efforts travel with their own model only. Consequence: the panel's chips show `SessionMemory.effort`, the effort in force; the Default chip clears only the chat's own pick.
- **Pickers with `none` selected show no chips** (Compactor, Summary model ×2, Memory summary model, dreaming tiers): `none` means "use the fallback, effort included" (D4d), and `current` (the selected `ModelInfo`) is undefined.
- **D4c vs D4b on whole-struct setters**: a whole-struct write whose model changed drops an incompatible effort; with the model unchanged it refuses. This lets a single picker write (`{...s, model}`) succeed.
- **Meeting summary** is a session model switch, not a side call, so row 14 did not need S1.
- **Dreaming tiers that run nowhere.** `triage` and `propose` models run in the night's own turn in v1 (no separate call); their efforts are stored and validated like their models, and apply where those models are honoured (nowhere yet, as for the models). `target` is `agent.model` (no setting).
- **Tidy effort → routine `effort:`** (quoted). `update_routine_file` gained `effort` in `ROUTINE_KEYS`; a cleared effort removes the line. The routine parser (`schedule::file::FrontMatter`, `deny_unknown_fields`) learns `effort:` in C's row 18: **on this branch alone a set tidy effort makes the routine file unparseable until C merges.** Nothing writes the line while the effort is unset.
- **`memory_effort:` is written quoted** in agent files so a budget (`"8000"`) stays a string.
- **Tool descriptions (D7)**: `memory_effort` is named in the param doc of `agent_propose`/`agent_put` (`AgentProposeArgs`) and `agent_draft` (`AgentDraft`), beside `memory_model`. The tool description texts do not list `memory_model` either, so they were left unchanged. The draft card shows `memory effort \`x\`` after `memory model`.
- **Migration number 0063** (`0063_endless_efforts.sql`). If another branch adds a 0063, Z renumbers one.
- **`localFirst`** was removed from settings/Memory.tsx and chat/MemoryPanel.tsx. The two 177 U4 assertions that pinned `localFirst === true` were deleted (D5 removes the ordering); the Local-tag assertions stay.

## Tests
- agent: `endless::compactor::tests::a_chosen_compactor_effort_rides_every_transport`; `endless::tests::compactor_efforts_follow_the_chain_live` (rows 11–13, D4e/D4f, the real chain); `endless::tests::the_chats_own_compactor_effort_is_checked_kept_and_dropped` (row 12 D4b–d, persistence); `endless::tests::an_endless_section_without_effort_loads`; `supervisor::tests::endless_settings_effort_follows_its_model` (row 11 D4b–d, config.yaml, live, `CheckEffort`); `store::endless::tests` (extended: both effort columns round-trip).
- roster: `file::tests::render_parse_round_trip` (extended: `memory_effort`, old file without it, quoted budget); `tools::tests::file_of_checks_reports_to_and_keeps_the_fields` (extended).
- app: `agent_draft::tests::card_buttons_…` (extended: `memory effort` on the card).
- meeting: `summary_effort_persists_and_clears_with_its_model`, `summary_effort_is_checked_against_its_model`, `the_summary_switch_carries_the_summary_effort`.
- dreaming: `night::tests::tier_efforts_follow_their_picks`, `night::tests::a_tier_effort_is_checked_against_its_pick`, `replay::tests::the_judge_call_carries_the_judge_effort`, `config::tests::partial_section_keeps_other_defaults` (extended: no `efforts:` loads).
- UI: `chat/MemoryPanel.test.ts` (chips at the effort in force; `effortMutation`), `settings/Memory.test.ts`, `agents/RuntimeAccessFields.test.ts` (memory effort chips, D4c/D4d in the draft), `settings/Meetings.test.ts`, `lib/dreaming.test.ts` (`effort` change).
- `store::tests::sessions_wire_schema_tracks_the_replicated_columns`: snapshot retaken at 63.

## Gates (final tree, feat commit 0c8d468a)
- `cargo fmt -p agent -p roster -p app -p meeting -p dreaming -p rpc`, unowned hunks reverted.
- `mbx check` and `mbx check --tests` on `-p agent -p roster -p app -p meeting -p dreaming -p rpc`: 0 errors, 0 warnings. `mbx clippy … --tests --no-deps -- -D warnings`: clean.
- `mbx test` on the same set, `--no-fail-fast`: 1412 passed, 6 failed. None is in code B2 touched, and each fails alone too: `sensor::tests::{fs_exists_through_monty, monty_budgets_and_allowlist, pr_watch_type_checks, sensor_http_follows_the_private_network_flag}` and `loop_::runtime_bridge_tests::guessed_direct_calls_are_bridged_or_refused` fail because the installed monty worker speaks protocol 5 and the client speaks 6. `meeting` `tests/calendar.rs::fastmail_floating_time_is_local_and_unknown_hosts_are_not_links` is one hour off (10:00 against 09:00) through `chrono::Local`. [inference] The cause is daylight saving: the event date (2026-10-16) is in summer time and today is not. The new and extended tests passed twice (26/26 each run).
- Bindings regenerated (`export_bindings`, 1037 files) and copied. `bunx tsc -b --noEmit`: 0 errors. vitest on the touched tests plus `meeting-model-offer.test.ts` and `src/rpc/api.test.ts`: 52/52. A wider run over settings, chat, agents, meeting, dreaming, lib and dev: 1603/1604. The one failure is `lib/injected-wrappers.test.ts`, a source scan that expects a `contact_message` builder in `crates/agent/src/loop_.rs`. Base 1ded15b6 has no such builder either, and B2 did not touch the files it scans.

## Left
- The roster's `settle_memory_effort` with a live agent actor has no roster-level test: the roster's live-engine rig needs a model catalog with efforts; the rule is the same `CheckEffort` covered in the agent tests, and the file-level clear runs without an actor.
- Meeting and dreaming D4b/D4c are tested through their pure `settle_*` functions; the async glue (`CheckEffort` ask) is not tested in those crates (no `agent/test-support` dev-dep there; adding one would change their feature set).
