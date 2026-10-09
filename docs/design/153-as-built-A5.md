# 153 as-built — A5 POLISH (`CallKind::Polish`, `ModelRole::Polish`, `voice.polish_model`, `polish` / `rewrite`)

Design: `153-voice-anywhere.md` §9, §13.6, §14.2 (frozen). Worktree `feat/153-a5` off PIN `cbaf4259`.

## Files
- `crates/agent/src/polish.rs` (new, pure): `POLISH_SYSTEM`, `POLISH_BUDGET = 4 s`, `REWRITE_TEMPLATE`, `destination_key`,
  `polish_system` (profile append), `polish_user` (JSON envelope), `rewrite_system` (template + `AppProfile.rewrite_prompt` + thread),
  `check_polish` / `check_rewrite` (fail-closed reply check). Tests inline.
- `crates/agent/src/loop_/side.rs`: `ModelRole::Polish` → `polish_model → title_model → session` (the `Escalate` shape).
- `crates/agent/src/loop_.rs`: `Shared.polish_model` slot + `Session::with_polish_model` (builder, so `Session::new`'s ten callers are untouched);
  test `polish_chain_prefers_polish_then_title_then_session`.
- `crates/agent/src/supervisor.rs`: `Message<Polish>` / `Message<Rewrite>` bodies (DelegatedReply; the supervisor never waits on the provider),
  `Agents::polish_target` (session → else theuth's desk via `store.desk_session("theuth")`, resumed through `Agents::session`), `voice_config()`,
  free fns `refused`, `polish_call`; sessions get the shared slot through `.with_polish_model(self.polish_model.clone())`;
  tests `set_polish_model_persists`, `polish_refuses_agent_prompt`.
- `crates/agent/src/lib.rs`: `pub mod polish;`.
- `crates/runtime/src/side_call.rs`: doc comments only (C's `CallKind::Polish` already labels ledger rows `polish`; `as_str`/`FromStr`/round-trip test present).
- `crates/rpc/src/api.rs`: no change needed — C's `polish_model` / `set_polish_model` / `polish` / `rewrite` bodies already forward to the
  `Agents` messages; `ask` resolves the DelegatedReply.

## Design said vs built
- §9.1: `Destination::AgentPrompt` → `refused: Some("agent prompt")` before any session lookup or model call. Built.
- §9.2: `SetPolishModel` validates through `catalog.resolve` and writes `voice.polish_model` with `runtime::write_config_value`
  (C's stub was already complete; `None` writes an empty value = YAML null). Boot reads it in `supervisor.rs` (`rt.section("voice")`).
- §9.3: chain guarded in `role_model`. Which session: `session` when given, else theuth's desk; a desk that is idle is resumed from the store
  (`Agents::session(Some(id), …)`), so the cost row lands on the desk even when no window has it open. No desk row at all → `refused: "no session"`.
- §9.4: system = `POLISH_SYSTEM` + `voice.polish_profiles.<kind>`; for `AnyApp{app}` a profile keyed by the app id wins over the `any_app`
  group text. User = `{"transcript","destination","app"?}` JSON. The `Polish` RPC carries no `AppProfile`, so `AppProfile.polish_prompt` is not
  appended by polish (it is by rewrite via `rewrite_prompt`); wave B/D can put the per-app text into `voice.polish_profiles.<bundle id>` instead.
- §9.4 output rule: trim, strip one surrounding code fence and one pair of quotes the input lacked; refuse empty, `"transcript"` in the reply,
  a `{`-led line the input did not have, or a length outside 0.3×–2× (+8 chars slack so short inputs like "ok" survive). Rewrite only refuses empty
  (shorter/longer is the point of a rewrite).
- §9.5: `Side::call`'s `timed out` → `refused: Some("timeout")`; `ms` = `SideReply.latency`, `model` = the answering model, both set whenever
  the model answered (also on a refused reply, so the debug row can say which model misbehaved).
- §9.6: no model / no credential / provider error / closed session → `refused: Some(<the error text>)`; `text` stays `None`.
- §13.6: instruction = user message; selection + `{app}` through `voice.rewrite_template` (default `REWRITE_TEMPLATE`); `AppProfile.rewrite_prompt`
  appended; `thread` rendered into the system prompt as "Earlier rounds…" (oldest first, newest reply = the text to refine) instead of
  assistant turns — `SideCall` is one user message by contract; charged to theuth's desk.

## Deviations from frozen shapes
- None. `Message<Polish>` / `Message<Rewrite>` reply types became `DelegatedReply<Result<PolishReply, String>>` (handler-internal; the
  `ask` result type seen by `rpc` is unchanged). `Agents::polish(&self, …) -> PolishReply` (C's private stub) is replaced by `polish_target`.

## Tests
- `polish::tests::{user_message_is_json_data_with_destination_and_app, system_prompt_appends_profile_app_id_over_group,
  rewrite_system_fills_template_tone_and_thread, check_polish_cleans_and_fails_closed}` (the brief's `polish_fails_closed_on_bad_reply`,
  as a pure validator — the agent crate's `fake_openai` would need the full session scaffold for one assertion).
- `loop_::tests::polish_chain_prefers_polish_then_title_then_session`.
- `supervisor::tests::{set_polish_model_persists, polish_refuses_agent_prompt}`.
- `runtime::side_call::tests::call_kind_round_trips_its_string_form` (C's; covers `Polish`).

## Gates (all `BUILD_LABEL=153-A5 build-lock mbx … -p agent -p rpc -p runtime`; tests run bare with `MBX_SCHEDULER_TESTS=0`)
- `mbx check --tests` → `Finished`.
- `mbx test --no-run` → `Finished test profile … in 12m 42s` (first), 2m 35s (last).
- `mbx test … polish` → `test result: ok. 7 passed; 0 failed` (agent lib).
- `mbx test` full agent lib → `590 passed; 2 failed` on a loaded machine: `sensor::tests::monty_budgets_and_allowlist` (wall-clock budget,
  13.99 s) and `supervisor::tests::human_input_releases_parked_items_as_steers` (`Elapsed`) — both pass on rerun (`2 passed; 0 failed`), both
  untouched by A5. agent lib minus those modules `413 passed`; `notebooks_binding` 2, `notebooks_block` 6, `prompts` 1; rpc `56 passed`; runtime `2 passed`.
- `mbx clippy --tests` → `Finished dev profile`, no warnings.

## Hand-offs (wave B desktop, wave D companion)
- Call `polish(text, destination, session)` once per finished utterance, after shaping, never on partials and never with `AgentPrompt`
  (it is refused, not an error). Pass the destination's session when it has one (document bound to a session, channel reply); `None` → theuth's desk.
- While waiting show "polishing…" on the stop glyph; deliver the shaped text immediately if the reply has `refused` (always within ≈4 s;
  `refused == "timeout"` is the budget). On `text: Some(t)` deliver `t`; history row (A3) stores `polished = t`, `polish_ms = ms`,
  model name for the debug row; `refused` goes to the debug row's reason. Undo = §6.7's "use the raw text" from the history row.
- `rewrite(instruction, selection, app, thread)`: `thread` = earlier `(instruction, result)` pairs of the same panel; pass the latest reply as
  the last pair when refining. Replace restores focus first (§13.6).
- `polish_model()` / `set_polish_model(id | null)` for Settings › Voice; `null` = "Chain (title model, then session)".
