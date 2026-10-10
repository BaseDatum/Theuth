# 158 as-built — W4 (speaker-name suggestions + notes): the wave

Closed on `feat/158-w4e` (closer W4E). Base main 23ea03c5 (contract, W1, W2, W5); main not merged (W3 fold pending — the
orchestrator sends it). Per-worker detail: `158-as-built-W4E.md` (engine), `158-as-built-W4U.md` (UI). Design §10, §11, §15 W4 rows, §16.

| Worker | Branch / head | Scope |
|---|---|---|
| W4E | `feat/158-w4e` `44ce2a27` (pre-close) | `post/suggest.rs`, `speakers.rs::assign`, `templates.rs`, `note.rs`/`summary.rs` forms + rules, `post/transcribe.rs` dictionary pass, `meeting_suggest_speakers` tool, W4 RPC bodies, `Template.body`, `meeting_template_reset` |
| W4U | `feat/158-w4u` `1a51a364` | `Review.tsx` suggestions (Suggest names, Accept / Not them), `settings/MeetingTemplates.tsx` + one line in `Meetings.tsx`, `StartForm.tsx` note-template select, `lib/meeting-templates.ts`, `lib/rpc.ts` keys |
| close | this branch `180c08ce` (merge) → `6e72918e`, `7b1973e0`, + this file | hand-offs decided below, `Template.edited`, UI body/reset/gated, calendar test lock, bindings, gates |

## Merge
`git merge --no-ff feat/158-w4u` at `180c08ce`: no conflicts (W4U touched no Rust; W4E touched no UI).

## Decisions taken at close (orchestrator 17:48)
1. **`Template.body`** (raw file incl. front matter) on every template; `meeting_template_put(slug, body)` parses first, then writes
   `body` exactly. UI `MeetingTemplates` seeds the editor from `t.body` and saves it verbatim (`templateBody()` now only builds the
   "New template" draft). **`meeting_template_reset(slug) -> Template`** (api.rs + `templates::reset`): shipped body written back and
   recorded as ours; refused for user templates. **`meeting_template_delete` refuses built-ins** ("reset it instead of deleting it").
   Plus **`Template.edited: bool`** (built-in whose file ≠ shipped body) so the UI shows "Reset to built-in" only when it does something;
   Delete stays disabled for built-ins.
2. **Start selects the template** (`lib.rs` StartMeeting): `manifest.template = templates::select(store, StartMeeting.template,
   manifest.event, manifest.project, cfg.template)` after the calendar seed — start pick → `tpl:uid:<uid>` → `tpl:title:<norm>` →
   `tpl:project:<p>` (`cal_project_memory`) → `meeting.template` → `meeting`. `meeting_set_template` remembers the pick for the
   meeting's event and project.
3. **Enrollment gates wired**: `EnrollTarget::New { name, entity, gated: bool }` (`#[serde(default)]`; TS `gated: boolean`);
   `Message<EnrollSegments>` embeds through `post::suggest::embed_spans_gated` when `gated` (≥ 5 s, clipping ≤ 5 %, SNR ≥ 8 dB,
   3 s-window agreement ≥ 0.70), else `post::embed_spans` as before. `Review.tsx` Accept sends `gated: true`; the manual selection
   enrol (`EnrollDialog.tsx`, design 156) sends `gated: false`. A gate refusal is the `Err` string the UI already shows as
   "<name> named; voice not enrolled — <reason>" (name kept).
4. **Preferred spellings in the live head** (`head.rs`): `HeadInput.preferred` = `note::preferred_names(data_dir)` (user + learned
   `Term|Mention|Abbrev` from `<data_dir>/voice/dictionary.json`, ≤ 40 — the user file only, never the packaged dataset: this runs on
   every head render), one line `Preferred spellings: …` under `Speakers heard:`; absent when empty; a change re-pushes through the
   existing `changed()` compare. `note::preferred_names` switched to the same cheap read for the forms.
5. **`tests/calendar.rs` race**: `matcher::choose` is process-global; `example_3` picks and clears for `us.zoom.xos` while `example_1`
   / `example_5` score that app in parallel. Smallest fix: a test-local `static ZOOM: Mutex<()>` taken by those three tests.
   Proof: 5 consecutive `mbx test -p meeting -p rpc --test calendar` → 15 passed each.

## What landed (wave)
- **Engine**: `suggest()` per §10.2 (owner rule, attendee candidates, intro/vocative cues, voiceprints with floor 0.6 + margin 0.08 +
  mutuality via `Profiles::assign`, 1:1, elimination, conflicts, warning); `meeting_suggest_speakers` tool + RPC; templates (5
  built-ins, gray_matter parse, upgrade-only-unedited install, select/remember, put/delete/reset/set RPCs); forms compiled from the
  template with the seven §11.2 rules and the preferred-spellings line; dictionary post pass (user entries only) in `transcribe::words`.
- **UI**: Review *Suggest names* rows (confidence meter, evidence, Accept → rename + gated enrol, Not them); Settings › Meetings ›
  Templates (list, built-in badge, Edit on the raw body, New, Save, Delete, Reset to built-in); start form *Note template* select
  (Automatic = engine order). `rpc/api.ts` gained `meeting_template_reset`.
- **Bindings**: `MeetingTemplate` (+`body`, `edited`), `EnrollTarget` (+`gated`), `methods.json` (+`meeting_template_reset`);
  exported (8 passed) and copied to `crates/desktop/ui/src/rpc/bindings/` (gitignored).

## Gates (merged tree, each its own command, `BUILD_LABEL=158w4e`, `CARGO_BUILD_JOBS=4`)
- `cargo fmt -p meeting -p rpc` (bare) → clean.
- `build-lock mbx test -p meeting -p rpc --no-run` → exit 0, no warnings.
- `mbx test -p meeting -p rpc` → meeting lib 112 passed (1 pre-existing ignored + 1 new ignored real-copy test), calendar 15, note 1,
  oracle 1; rpc 56 + alerts 1 + prompts 1; 0 failed. First run tripped the pre-existing `rpc::engine::tests::
  second_acquire_in_one_process_is_running` flake (`engine.lock is held but engine.json is missing`; 3 of 8 runs this session, green on
  rerun) — open item below.
- Calendar binary ×5 → 15/15/15/15/15 passed.
- `build-lock mbx clippy -p meeting -p rpc` → exit 0, 0 warnings.
- `BUILD_SLOTS=1 build-lock mbx test -p rpc --features ts --lib --quiet export_bindings` → 8 passed; copied.
- `build-lock bun install` (ui, packages/apps) → ok. `build-lock bunx tsc --noEmit -p .` → 0.
- `build-lock bunx vitest run` Review, StartForm, MeetingTemplates, Meetings, lib/meeting-templates, rpc/api, lib/segment-select →
  7 files, 27 tests passed.
- `BUILD_SLOTS=1 build-lock mbx check --workspace --tests` → exit 0 (only the pre-existing `block v0.1.6` future-incompat note).

## Live proof
Memory gate at 18:11: free + speculative 6.46 GB (> 4 GB), load 24. A scratch engine (`t` build ≈ 15 min heavy + Tauri for the Review
click path) was not started: the workspace check held the heavy slot and the §16 proof needs the UI click path. What ran instead
(engine half, real data shapes): `post::suggest::tests::suggest_on_a_real_meeting_copy` over COPIES of two real meetings'
`manifest.json` + `transcript.json` + `embeddings.json` (`/tmp/158w4/meetings/<id>`, no audio, no profiles — none are enrolled on this
Mac); labels/sources/confidences only, no names or text printed:
`THEUTH_TEST_MEETING_DIR=/tmp/158w4/meetings/<id> mbx test -p meeting -p rpc --lib suggest_on_a_real_meeting_copy -- --ignored --nocapture`
→ `01a0e8f9…`: labels 5 · segments 298 · embeddings 5 (dim 512) · event false · profiles 0; `01a12243…`: labels 8 · segments 167 ·
embeddings 8 (dim 512) · event false · profiles 0. Both parse and run through `suggest_for` (1 passed each); with no event, no enrolled
profile and no intro cue in either, zero suggestions — the honest output for those two.
**Owed (orchestrator's pass)**: build `t`, scratch data dir with the latest meeting copied + `models` symlinked, open Review → *Suggest
names* → Accept one → `speakers/<uuid>.json` written (or the gate's refusal toast); Settings › Meetings › Templates → edit a built-in →
"Reset to built-in" appears → reset; start form picks a template → `manifest.template`.

## Deviations (wave; per-worker lists in their files)
- W4E 1–6 (`158-as-built-W4E.md`): built-in-ness from the slug list; `tpl:project` memory row; voiceprint↔attendee by name; no-note
  form says `## Summary`; `templates::attach` data-dir handle for the sync `form_for` / `words()` / `head::of`.
- W4U 1–5 (`158-as-built-W4U.md`): one Accept button (rename, then enrol if the gate passes); every segment of the label sent;
  Templates as the last Settings › Meetings section. Its deviations 3–4 (no raw body, no Reset) are closed by decision 1.
- `Template.edited` is an extra field beyond the frozen §15 shape (additive, `#[serde(default)]`).
- `EnrollDialog.tsx` touched for one field (`gated: false`) — the TS union requires it.

## Open items
- `rpc::engine::tests::second_acquire_in_one_process_is_running` flakes under parallel load (not W4's file; the first `acquire`'s
  `try_lock` is refused while `engine.json` is unlinked — a `Lease` drop order / fd race worth a look by whoever owns `rpc/engine.rs`).
- Floor/margin 0.6/0.08 are §10.2's starting values; retune with §14's eval.
- `is_me` unknown for ICS feeds (W2) → the owner can be an attendee candidate on those events.
- W3 fold: `lib.rs` StartMeeting (W3 sets `manifest.project` — W4's `templates::select` reads it, so the select call must stay after
  W3's project line), `StartForm.tsx` `startOptions`, `api.rs` method order.
