# 194 as-built — B1 (S1 + rows 7–10)

Worker 194-B1, branch `feat/194-b1` from 1ded15b6. Contract: `docs/design/194-effort-everywhere.md`.

## S1 — side calls carry an effort (commit 2587a9ae, marker `/tmp/194/S1.sha`)

- `runtime::ModelRole::NamedWithEffort { model: String, effort: Option<String> }` (crates/runtime/src/side_call.rs). `effort: None` resolves exactly like `Named`.
- `Shared::role_model(&ModelRole) -> Result<(Resolved, Option<String>), String>` (crates/agent/src/loop_/side.rs) now returns the model **and the effort of the setting that resolved it**. The only exhaustive `match` on `ModelRole` in the workspace is this one; no other crate needed an arm.
- `Side::effort(&Resolved, Option<&str>)` / `loop_::side::checked_effort`: the effort a side call sends. An effort the resolved model's catalog entry does not take (`Model::validate_effort`), or an unknown word on an uncatalogued model, is dropped with `warn!` ("side-call effort `x` dropped: …"); `None` sends no reasoning setting (D4a).
- `Side::call` sends the effort as the request's reasoning effort, floors `max_tokens` above the thinking budget on Anthropic (`Current::reply_floor`, 116) when an effort is set, and skips the local-server thinking-off kwargs (177 R2-6) when an effort was picked.
- Callers: the `SideCall` handler (loop_.rs) and the session title turn both go through `role_model` → `.effort(...)` → `.call(...)`.

## Rows built

| # | Setting | Config key (beside the model key) | RPC pair (after its model sibling) | Agent messages | Applied at |
|---|---|---|---|---|---|
| 7 | Settings › Helpers › Title | `agent.title_effort` (`AgentConfig.title_effort`) | `title_effort` / `set_title_effort` | `GetTitleEffort` / `SetTitleEffort` | title turn + `ModelRole::Title` side calls (notebook titles) |
| 8 | Settings › Helpers › Vision fallback | `agent.vision_effort` (`AgentConfig.vision_effort`) | `vision_effort` / `set_vision_effort` | `GetVisionEffort` / `SetVisionEffort` | the vision consultation's image call (`Turn::inspect`) + `ModelRole::Vision` |
| 9 | Settings › Helpers › Escalation | `agent.escalate_effort` (`AgentConfig.escalate_effort`) | `escalate_effort` / `set_escalate_effort` | `GetEscalateEffort` / `SetEscalateEffort` | tier-2 `ModelRole::Escalate` side calls |
| 10 | Settings › Voice › Polish | `voice.polish_effort` (`voice::VoiceConfig.polish_effort`) | `polish_effort` / `set_polish_effort` | `GetPolishEffort` / `SetPolishEffort` | `ModelRole::Polish` (polish + rewrite) |

Setters take `effort: Option<String>` and return the stored `Option<String>` (D3). All live in one shared slot `loop_::RoleEfforts { title, vision, escalate, polish }` (`Agents.role_efforts`, handed to each session by `Session::with_role_efforts`), read per call — a pick takes effect on the next call (D4f). Persisted with `runtime::write_config_value` (quoted value, `~` clears), as `set_effort` writes `agent.effort` (D4g). Startup reads the four keys into the slot.

Semantics, identical for all four (D4):
- **b** `set_*_effort` checks the setting's effective model: title → `agent.title_model`; vision → `agent.vision_model`; escalate → escalate model, else title model; polish → polish model, else title model. Refused with `validate_effort`'s message; with no model (or an uncatalogued one) only a known word passes (`unknown effort x`). A refusal or a failed config write changes nothing.
- **c/d** Each model setter (`SetTitleModel`, `SetVisionModel`/`set_vision_model`, `set_escalate_model`, `SetPolishModel`) calls `Agents::follow_role_model` after the swap: the `none`/Off row clears the effort; a new model that does not take it drops it. Both persist `~`.
- **e** The effort follows the setting whose model answered (`role_model`): Title → title effort; Escalate → escalate effort when `agent.escalate_model` answered, title effort when the title model did, none on the session's model; Polish likewise with polish effort; Vision → vision effort.

UI (D5): `settings/Helpers.tsx` (Title, Vision fallback, Escalation) and `settings/Voice.tsx` (Polish) pass `effort` built as ModelDefaults does; levels come from the catalog row of the model the picker shows (Escalation: `escalate_model().model`, never the inherited one). Model setters also invalidate the effort key; `/title`, `/escalate`, `/vision` in `chat/slash.ts` invalidate it too. Query keys `queryKeys.titleEffort`, `visionEffort`, `escalateEffort` (lib/rpc.ts) and `keys.polishEffort` (Voice.tsx). api.ts declarations sit after their siblings; the method-name list is alphabetical.

## Tests

Rust (crate `agent`):
- `loop_::tests::named_with_effort_carries_its_effort` — S1: `reasoning_effort` on the wire; `None` ≡ `Named`; a level the model does not list is dropped.
- `loop_::tests::role_effort_follows_the_answering_setting` — every fallback of D4e, live slot.
- `loop_::tests::role_side_calls_carry_their_effort` — Title, Escalate (title fallback), Polish each send their effort on the wire.
- `loop_::vision_fallback_tests::vision_effort_rides_the_vision_call` (inside `vision_fallback_contract_against_fake_provider`) — the image call carries `agent.vision_effort`.
- `supervisor::tests::role_efforts_follow_their_models` — per setting: persistence, D4b refusal (model and no-model), D4c drop, D4d clear, live slot.
- `supervisor::tests::role_effort_keys_load_old_and_new_configs` — old configs without the keys load.

UI: `settings/Helpers.test.ts` (chips per row, own-model levels, setter calls incl. Default = null, effort refetch after a model change/Off), `settings/VoicePolish.test.ts` (Polish chips, setter, refetch). `settings/Voice.test.ts` only gained the `polish_effort` mock.

## Decisions where the contract was silent

- **One slot, not four.** The four efforts share `RoleEfforts` behind one `parking_lot::RwLock`, passed by a builder like `with_polish_model`, so `Session::new` (called in many tests) keeps its signature.
- **Vision effort unset keeps today's behaviour.** The vision call already sent `frame::default_effort` of the fallback model; `None` still does (D4a "keeps today's behaviour"). Title/escalate/polish unset still send no setting.
- **Picked effort beats the local thinking-off.** A title or vision call to a local OpenAI-protocol server normally gets `enable_thinking: false` (177); with an explicitly picked effort it sends that effort instead. The question call of a vision consultation is unchanged.
- **Escalate/polish validation uses the effective model** (own → title), per D4b; D4c on a model change checks the role's own new model.
- **`VoiceSettings` unchanged.** Polish has a dedicated RPC pair, so `polish_effort` is not added to `voice_settings` (D3 "dedicated pair"); `crates/rpc/src/api/hub.rs` untouched.
- **Advisor** (`advisor.rs`) sends `ModelRole::Named(src.model)` and resolves through none of B1's roles: untouched.
- New agent messages are referenced as `crate::…`/`agent::…` paths instead of being added to the shared import blocks in supervisor.rs and rpc/api.rs, to keep merges with A/B2 conflict-free.

## Gates (final tree)

- `cargo fmt -p runtime -p agent -p rpc -p voice`; unrelated hunks reverted.
- `mbx check -p runtime -p agent -p rpc -p voice` and `--tests`: 0 errors, 0 warnings.
- `mbx clippy -p runtime -p agent -p rpc --tests --no-deps -D warnings`: clean. With `-p voice` clippy stops on a pre-existing `chunks_exact_to_as_chunks` in `crates/voice/src/history/mod.rs:465` (unchanged since 1ded15b6; B1 only added `VoiceConfig.polish_effort`).
- `mbx test -p runtime -p agent -p rpc -p voice --no-fail-fast`: agent 888 passed / 7 failed; rpc 98 + 10 integration, runtime 13, voice 74 green. Of the 7: `communication::network::tests::an_inbox_neither_sees_nor_reaches_another_contact` and `supervisor::tests::a_waiting_first_message_leaves_the_chat_working` pass alone (load); `sensor::tests::{fs_exists_through_monty, monty_budgets_and_allowlist, pr_watch_type_checks, sensor_http_follows_the_private_network_flag}` and `loop_::runtime_bridge_tests::guessed_direct_calls_are_bridged_or_refused` fail alone too with `monty worker crashed: unsupported protocol version 6 (server supports protocol version 5)` — the monty worker binary in this environment, untouched by B1.
- New Rust tests run twice: green both times.
- Bindings regenerated (`export_bindings`, 10 passed) and copied; `bunx tsc -b --noEmit`: 0 errors; `bunx vitest run` Helpers.test.ts, VoicePolish.test.ts, Voice.test.ts, chat/slash.test.ts, rpc/api.test.ts: 5 files, 33 tests green; the two new files run twice.

## Left

- Nothing in rows 7–10. The TUI `/title` and `/escalate` commands (crates/tui) set models only; TUI pickers are T's (row 22).
- No change needed in `ModelCombobox`/`ModelPicker`.
- Docs: `site/content/docs/reference/configuration.mdx` gained one row for the new keys.
