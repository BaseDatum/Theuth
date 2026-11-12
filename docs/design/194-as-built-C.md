# 194 as-built C — meeting session model, routines, autoresearch (rows 16–20)

Worker C, branch `feat/194-c` (base 1ded15b6, S1 merged at 2587a9ae). Contract: `194-effort-everywhere.md`.

## Shared engine helper (agent crate, `lifecycle.rs`)
- `agent::lifecycle::settle_effort(model, info, effort, model_changed) -> Result<Option<String>, String>`: D4b–d for a
  model setting. Blank/`default` → `None`; no model → `None` (D4d); a catalog model checks the effort like
  `models::Model::validate_effort` with the same message (`<id> accepts off|low|high`, `… accepts no reasoning effort`);
  an unknown/pass-through model, or no running agent, accepts any `effort_of` word (`unknown effort <x>` otherwise);
  a refused effort becomes `None` when `model_changed` (D4c), else `Err` (D4b).
- `agent::lifecycle::effort_model(model) -> Option<ModelInfo>`: `ResolveModel` on the registered agent.
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
  `SetModel` (fire and `BindSession`); files are re-read per fire, so a pick is live (D4f).
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
- `autoresearch.settings.set` settles each against its role model (`config::settle_role`; `off`/Default carry none).
- Resolution (`AutoresearchConfig::{executor,reviewer,advisor,target}_effort`): an effort rides only with the model it
  was picked with — the study's `(model, effort)` pair, else the section's; a Default model carries none.
- Applied (D4e): executor → the study schedule file's `effort:` (so the study session starts on it); reviewer →
  `Backend::review(.., effort, ..)` → `ModelRole::NamedWithEffort`; advisor → `Backend::advise(.., effort, ..)`
  (the `advisor.model` fallback brings `advisor.effort`); report prose → executor effort; cases/judge target and
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
- Draft leaves check only the effort word; the catalog check happens at the side call (S1 drops an effort the model
  does not take, with a warning) and in the UI (chips are the model's levels).
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
