# 194 as-built C — meeting session model, routines, autoresearch (rows 16–20)

Worker C, branch `feat/194-c` (base 1ded15b6, S1 merged at 2587a9ae). Contract: `194-effort-everywhere.md`.

## Shared engine helper (agent crate, `lifecycle.rs`)
- `agent::lifecycle::settle_effort(model, info, effort, model_changed) -> Result<Option<String>, String>`: D4b–d for a
  model setting. Blank/`default` → `None`; no model → `None` (D4d); a catalog model checks the effort like
  `models::Model::validate_effort` with the same message (`<id> accepts off|low|high`, `… accepts no reasoning effort`);
  an unknown/pass-through model (`Ok(None)`), or no running agent, accepts any `effort_of` word (`unknown effort <x>`
  otherwise); a model that does not resolve (`Err`) refuses with that error (R1);
  a refused effort becomes `None` when `model_changed` (D4c), else `Err` (D4b).
- `agent::lifecycle::effort_model(model) -> Result<Option<ModelInfo>, String>`: `model_meta` on the registered agent
  (R1: `lifecycle::ModelMeta`, catalog metadata without the credentials gate).
- Lives in `lifecycle.rs` (no owner in contract §5) so `agent/src/lib.rs` is untouched.

## Rows 16 + 17 — meeting session model
Decision: **two settings**. `meeting.model` (Settings › Meetings) is the engine default; the start form's
remembered pick (`meeting-settings` store, client-only) overrides it per start. Each carries its own effort.
- Config: `meeting.effort` = `MeetingConfig.effort` / `MeetingSettings.effort` (`#[serde(default)]` via the struct),
  written by `MeetingSettings::write_keys` (`runtime::write_config_value`), applied live by `apply_to`.
- `meeting_settings_set` (no new RPC; the struct gained the field, D3) settles the effort against `meeting.model`
  (`settle_effort`; model change = D4c; `model: None` clears it = D4d).
- `meeting::StartMeeting.session_effort` (`#[serde(default)]`, ts optional). Name follows D2 (`session_model` →
  `session_effort`) rather than the brief's bare `effort`.
- Start resolution `session_pick`: the start's model with the start's effort, else `meeting.model` with
  `meeting.effort`; a start effort without a start model is not read. Stored as `Manifest.session_effort`
  (`skip_serializing_if` so old manifests re-serialize byte-identical) and sent as `CreateSessionOptions.effort`
  (the agent validates it against the session model at create). `None` → `agent.effort` (D4a).
- UI: `settings/Meetings.tsx` Meeting model row passes `effort` (chips → `set({ effort })`; the save refetches the
  block, so an engine-dropped effort disappears). `meeting/StartForm.tsx` passes `effort`; the store gained
  `effort` + `setModel(model, effort = null)` (`null` model clears it) + `setEffort`; a model pick keeps the old
  level only when the new model lists it; `startOptions` sends `session_effort` only with a picked model.

## Row 18 — routines
- Front matter `effort:` beside `model:`: `Spec.effort`, `FrontMatter.effort`, `ScheduleFile.effort`
  (`#[serde(default)]`, ts optional), `FileArgs.effort`, `render` writes `effort:` right after `model:`.
- Parse errors (style of the other keys): `effort: needs model:`, `effort: unknown effort <x>`. Old files without
  the key parse as before.
- `put_schedule` (`schedule::Put`) settles the effort against the file's model: refuses on the same model (D4b,
  `effort: <validate_effort message>`), drops it on an edit that changes the model (D4c), clears it with no model
  (D4d). A create with a bad effort is refused, not dropped.
- Run: `new_session` takes `(model, effort)` and sends `agent::SetEffort { session, effort, manual: false }` after
  `SetModel` (fire and `BindSession`); a reused session gets the same pair before every fire (`apply_pick`, R1).
- `schedule()` tool: `ScheduleArgs.effort`, description clause "`effort` sets the run model's reasoning effort (with
  `model`)", catalog check before the write, reply shows ` · effort <e>`.
- `routine_draft()`: `RoutineDraft.effort` → `ScheduleArgs`, description names it, card Run line shows `effort <e>`
  after `model <m>`. The bundled `routines.md` skill lists the key.
- UI: `routines/RoutineDetail.tsx` Model field passes `effort` (chips set `f.effort`; a model change keeps the
  level only if the new model lists it; `none` clears it); `effort` errors map to the Run section. `draft.ts`
  carries `effort` through `draftToFile` and `fileText`.

## Row 19 — Settings › Autoresearch role models
- The old `autoresearch.effort.{executor,review,advisor}` block (reserved, never applied, defaults
  `high/low/high`, TS `AutoresearchEffort`) is **removed** (clean cutover; applying its defaults would have changed
  behaviour, D4a). A config that still has it loads; the next save drops it.
- Per-role siblings (D2): `models.executor_effort`, `models.advisor_effort`, `review.effort` (`Option<String>`).
- `autoresearch.settings.set` settles each against its role model (`config::settle_settings` over `settle_role`;
  `off`/Default carry none), the disabled-plugin path too (`config::set_unloaded`, R1).
- Resolution (`AutoresearchConfig::{executor,reviewer,advisor,target}_effort`): an effort rides only with the model it
  was picked with — the study's `(model, effort)` pair, else the section's; a Default model carries none.
- Applied (D4e): executor → the study schedule file's `effort:` (so the study session starts on it); reviewer →
  `Backend::review(.., effort, ..)` → `ModelRole::NamedWithEffort`; advisor → `Backend::advise(.., effort, ..)`
  (no pick: `advisor.model`, live, with no reasoning setting — R1); report prose → executor effort; cases/judge target and
  judge → `NamedWithEffort`. `Backend::{review,advise,report_prose}` gained an `effort: Option<&str>` parameter.
- UI: each role's `ModelCombobox` passes `effort` bound to its own field.

## Row 20 — draft leaf models
- `ModelsSpec` gained `executor_effort`, `review_effort`, `advisor_effort`, `target_effort`, `judge_effort`
  (`#[serde(default, skip_serializing_if)]`; old drafts and stored specs load and re-serialize unchanged). The
  `autoresearch_draft` tool's spec carries them; its description names them in one clause.
- `draft_edit` leaves `models.{executor,review,advisor}_effort` (word check `unknown effort \`<x>\``); emptying a
  model leaf clears its effort (D4d). The Models row value reads `executor x/y · effort high`.
- Inner suite studies inherit the outer review/advisor efforts, and the executor's only with the outer executor.
- UI: `setup.ts` LEAVES lists the effort leaves; `DraftPanel` `LeafModel` renders each `models.*` leaf picker with
  its `_effort` leaf as chips (dropped when the new model does not list it) and no input of its own.

## Tests
- meeting: `meeting_effort_persists_with_its_model`, `the_session_starts_on_the_picked_effort`; the incident
  manifest test still re-serializes byte-identical.
- schedule: `effort_round_trips_beside_model` (file), `put_keeps_effort_with_its_model`,
  `the_run_session_starts_on_the_effort` (live model-less engine, `LoadSession`), `settle_effort_checks_the_catalog_model`.
- app: `effort_rides_with_the_model` (routine_draft).
- autoresearch: `role_efforts_ride_with_their_models`, `settle_role_refuses_drops_and_clears`,
  `the_executor_effort_rides_in_the_schedule_file`, `edit_parses_every_leaf` (extended), the cases fake backend
  asserts the target's `NamedWithEffort` effort on every call.
- UI: `Meetings.test.ts` (chips + whole-block writes), `meeting-settings.test.ts`, `StartForm.test.ts`,
  `Autoresearch.test.ts` (role chips → per-role field), `DraftPanel.test.ts` (`LeafModel`), routines `helpers.test.ts`.

## Decisions where the contract was silent
- Rows 16/17 are two settings (above). Field `StartMeeting.session_effort` (D2 naming).
- `effort:` without `model:` is a parse error for routines (D4d: no model = the fallback, effort included).
- Draft leaves check the effort word in `set_leaf`; R1 adds the catalog check in the engine before the save
  (`config::settle_models`), and the UI chips are the model's levels.
- Kind rows `target`/`scoring` got no effort leaves (the panel has no model picker there); the spec fields exist for
  the tool and are applied.
- `ScheduleInfo` (the routine list row) did not gain `effort`; the Overview "model:" line is unchanged.
- The one-time `meeting-model-offer` toast copies only the model (it predates the store effort).

## Gates
All on the final tree (code commit 9333d640), mbx 1.99.0:
- `cargo fmt -p agent -p meeting -p schedule -p autoresearch -p app -p rpc`; unrelated rpc reformatting reverted.
- `check -p agent -p meeting -p schedule -p autoresearch -p app -p rpc --tests`: 0 errors, 0 warnings.
- `clippy` same set `--tests --no-deps -D warnings`: clean (one pre-existing `needless_borrow` in
  `schedule/src/lib.rs` `SensorProfile::for_runtime(&rt)` fixed to pass).
- `test -p meeting -p schedule -p autoresearch -p app` (full crates, `--no-fail-fast`): app 50+1, autoresearch 307,
  meeting 167 (+2 ignored), meeting/tests note 1, oracle 1, schedule 98 — all pass; `meeting/tests/calendar.rs`
  `fastmail_floating_time_is_local_and_unknown_hosts_are_not_links` fails (expects 09:00 local, gets 10:00: a
  DST/clock-dependent fixture in files this branch does not touch). New tests run twice, green both times.
  `agent` and `rpc` were checked/clippied, not test-run (agent change = two free functions exercised through the
  schedule/meeting/autoresearch tests; rpc change = one line in the export list, exercised by `export_bindings`).
- Bindings: `test -p rpc --features ts --lib export_bindings` 10 passed; copied into `ui/src/rpc/bindings`.
- UI: `bunx tsc -b --noEmit` 0 errors; `vitest run` of the touched tests + `src/rpc/api.test.ts`: 10 files, 76 tests
  pass; the wider `routines`, `meeting`, `autoresearch`, `settings`, `lib/meeting*`, `lib/detection` run: 53 files,
  316 tests pass.

## Left
Nothing in scope. ModelCombobox/ModelPicker untouched. Merge notes for Z: B2 edits the same `MeetingSettings`
struct, its `write_keys`/`apply_to`, the `SetSettings` handler, `Meetings.tsx` and `Meetings.test.ts` (the
`block` literal and the ModelCombobox mock) — adjacent hunks, resolve by keeping both.

## R1
Review `review-194-c.md` (r194c), fixed on top of abc1839f by a fresh worker.
1. **Fixed — routine effort on every fire** (MAJOR). `schedule::apply_pick(agents, name, session, (model, effort))`
   runs before every fire of a reused session (the `Some(s)` branch of `fire_with`, after the clear): `SetModel` to
   the routine's `model:` else `GetDefaultModel`, then `SetEffort` to its `effort:` else `GetDefaultEffort`
   (`agent.effort`), the user's own messages, model first. An inherited effort the model refuses is cleared, as
   `SetModel` drops it on a fresh session. `rescan` and the restore still keep `e.session`; the pair is re-applied.
   Test `every_fire_applies_the_routines_model_and_effort`: one session across rescans — `effort:` set, cleared
   (back to `agent.effort` low), `model:` changed (effort dropped), `model:` cleared (engine default) — each equal
   to a fresh run session.
2. **Fixed — draft effort settles against its model** (MAJOR). `autoresearch::config::settle_models(models, was)`
   settles all five `*_effort` leaves with `settle_role` (catalog check D4b, drop on a model change D4c, clear
   with the model D4d). `setup::edit` is now async and settles before `validate_partial`/save (`was` = the draft's
   models before the leaf); the `autoresearch_draft` tool calls `setup::settle_draft(root, spec)` before
   `write_draft` (`was` = the saved draft, else the spec itself, so a new draft's bad pick is refused). Test
   `tests/effort.rs` `efforts_settle_against_their_models` (own binary, registered keyless engine): setter refusal,
   model change, clearing; tool refusal (target, judge), re-draft model change, re-draft without model.
3. **Fixed — disabled-plugin settings path** (MAJOR). `autoresearch::config::settle_settings(cfg, was)` is shared
   by `Studies::SetSettings` and `config::set_unloaded(data_dir, cfg)`, which reads the section saved in
   `config.yaml` as `was`, settles, persists. `research.rs` changed in the call only. Test
   `rpc/tests/autoresearch_unloaded.rs` `the_disabled_plugin_path_settles_efforts_too` (D4b refusal leaves the file
   unchanged, D4c against the saved section, D4d).
4. **Fixed — Default stays no-reasoning** (MAJOR). `review::advisor_role(model, effort, &AdvisorDefaults)`: a
   picked model carries its role effort (`NamedWithEffort`); no pick is `Named(advisor.model)` with no effort, else
   `Session`. `advisor.model` is read live (`agent::GetAdvisorDefaults`), not from `rt.section`. Test
   `the_advisor_default_sends_no_reasoning` (a non-null `advisor.effort` is never sent).
5. **Fixed — StartForm override vs fallback** (MINOR). The picker's value is the stored override only
   (`meeting-settings.model`, `null` = the `none` row **Meeting default**); no chips on that row; chips under a
   picked model call `setEffort` (that model's override effort); the none row clears model and effort. The New
   sessions model is no longer shown as the pick (`useConnection` dropped from the form). Test in
   `StartForm.test.ts` with a distinct agent default (`agent/new`, its own levels) and clearing.
6. **Fixed — metadata-only validation** (MINOR). New `agent::lifecycle::ModelMeta` (handler beside `ResolveModel`
   in `supervisor.rs`): `catalog.resolve` without the credentials gate, `Ok(None)` without a catalog row.
   `lifecycle::model_meta(agents, model)`; `effort_model` returns `Result<Option<ModelInfo>, String>`, and
   `settle_effort` takes it: `Ok(None)` = unknown (any `effort_of` word), `Err` = refuse (dropped on a model
   change). `settle_effort_checks_the_catalog_model` now goes through `model_meta` on the keyless `testp` provider
   (`ResolveModel` refuses it) and covers unknown and unresolved models.
7. **Declined — the `for_runtime(&rt)` nit.** Kept: clippy `-D warnings` on `schedule` fails without it.
8. **Fixed — test strength.** `the_session_starts_on_the_picked_effort` creates a real session from the meeting's
   own options (`meeting::session_options(&manifest)`, now what `start_task` sends) on a live engine and reads its
   effort back; removing `effort` there turns it red. The actor `Fake` backend records `(call, model, effort)`:
   `review_reverts_a_suspicious_keep_and_restore_queues_it` (reviewer), `plateau_explores_once_then_stops`
   (advisor), `finalize_writes_the_report_prose_before_the_tables` (report prose) assert their role's effort; the
   judge fake `ShorterWins` records each role and `toy_judged_runs_end_to_end_through_a_fake_judge` asserts the
   judge's `NamedWithEffort` on all 12 calls.

Decision: a draft edit settles every role, so a stored effort its model does not take (only possible in a draft
saved before R1, which checked the word only) refuses any leaf edit, naming the leaf, until it is cleared.

R1 gates (mbx 1.99.0, `BUILD_LABEL=194cf`): `cargo fmt` on the six crates, unrelated rpc reformatting reverted;
`check` and `check --tests -p agent -p schedule -p autoresearch -p rpc -p meeting -p app`: 0 errors, 0 warnings;
clippy same set `--tests --no-deps -D warnings`: clean; `test -p schedule -p autoresearch -p rpc -p agent -p meeting
--no-run`, then the 20 changed/added/edited-function tests filtered by name, green twice (autoresearch 11 + 1,
meeting 2, rpc 1, schedule 5). UI: `tsc -b --noEmit` 0 errors (no binding changes); `vitest run` StartForm,
meeting-settings, `src/rpc/api.test.ts`: 3 files, 14 tests pass.
