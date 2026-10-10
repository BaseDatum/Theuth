# 158 as-built — W4U (desktop UI: speaker-name suggestions, templates)
Branch `feat/158-w4u` from main 23ea03c5. UI only (`crates/desktop/ui`); no Rust, Swift or C# touched. The W4 Rust side
(`post/suggest.rs`, `templates.rs`) is still the contract stubs on main, so against today's engine *Suggest names* returns an
empty list (toast "No name suggestions for this meeting") and the Templates list is empty with a one-line note.

## Built
- **Review (§10.3)** — `src/components/meeting/Review.tsx`: a *Suggest names* button after the speaker chips (Review only, not
  `readOnly`). One RPC, `meeting_suggest_speakers(id)`, into the query cache under `queryKeys.meetingSuggestions(id)`
  (`enabled: false`, the button `refetch()`es; `lib/rpc.ts`). One `SuggestionRow` per suggestion: `<speaker> → <name>`,
  the confidence (`0.84` + a `role="meter"` bar), the evidence line, **Accept** / **Not them**; the mock's caption under the rows.
  Rows for `me`, for labels no longer in the transcript, or whose name is already applied are skipped.
  - **Accept** → `meeting_edit(id, [Rename { speaker, name }])`, answer replaces the cached transcript, row dropped. Then, unless
    `profile` is set (a voiceprint match — the rename links it, `link_enrolled`) or `source == "Warning"`, →
    `meeting_speaker_enroll(id, <every segment of the label>, New { name, entity: null })`; success → transcript replaced,
    `meetingSpeakers` invalidated, `enrolledToast`; refusal (the §10.4 gates live in the engine) → `toast.message("<name> named;
    voice not enrolled", { description: <engine reason> })`, name kept. Nothing happens without a click (R6).
  - **Not them** → drops the row from the cached list, no RPC.
- **Templates (§11.1)** — new `src/components/settings/MeetingTemplates.tsx`, rendered as the last section of
  `settings/Meetings.tsx` (one render line + `TEMPLATES_KEY` now aliases `queryKeys.meetingTemplates` so the Defaults'
  Template select, the Templates list and the start form share one query). Rows: name, `built-in` badge, description (or
  `<slug>.md`) · section titles; **Edit** / **Delete** (disabled with a title for built-ins — the engine refuses anyway). **New
  template** / Edit open a `CharterEditor` (the existing CodeMirror markdown wrapper) on the front matter; new drafts have a
  slug field (typed freely, shown `slugify`d as `<slug>.md`, "already exists" blocks Save). Save → `meeting_template_put(slug,
  body)`; Delete → `meeting_template_delete(slug)`; both invalidate the templates query.
- **Start form (§11.1)** — `StartForm.tsx`: a *Note template* row under *Session model*: *Automatic* (default, sends nothing →
  engine order: remembered per event → `meeting.template`) or one of `meeting_templates()`. `startOptions` gained
  `template?: string | null`, emitted as `StartMeeting.template` only when picked.
- **lib** — `src/lib/meeting-templates.ts`: `NEW_TEMPLATE` (one Summary section), `templateBody(t)` (front matter from the
  parsed shape, every scalar JSON-quoted = valid YAML double-quoted), `slugify(name)`. `lib/rpc.ts`: `queryKeys.meetingTemplates`,
  `queryKeys.meetingSuggestions(id)`.

## Deviations from the design
1. §10.3 names two buttons (*Accept*, *Accept & enroll* → *Accept name only* after a failed gate). The brief and the mock have one
   **Accept** that renames and enrolls "when the engine says the sample passes": built that way. The UI cannot know the gate
   result before the call (`SpeakerSuggestion` carries no gate field), so the refusal is reported after the rename, name kept.
2. §10.3 "first ≤ 30 s of segments": the UI sends every segment of the label; the engine's `ENROLL_MAX_MS` cap keeps the first
   30 s (`Enrolled.available_ms > speech_ms` reports it). No client-side span trimming.
3. `MeetingTemplate` has no raw body, so Edit rebuilds the front matter from the parsed shape — comments, key order and any
   markdown after the front matter in a user's file are lost on save. Hand-off below.
4. "Reset" for an edited built-in (§11.1) has no RPC in §15; not built (Delete is disabled for built-ins instead).
5. Templates sit as the last section of Settings › Meetings (after Enrolled speakers), not beside the Defaults' Template select,
   so `Meetings.test.ts`'s section-order assertions hold unchanged.

## Hand-offs
- **W4 (Rust)**: consider `Template.body: String` (raw file text) on `meeting_templates` so Edit round-trips a user's file
  verbatim; the UI would then seed the editor from `t.body` instead of `templateBody(t)` (one-line change in
  `MeetingTemplates.tsx`). Optional: a `meeting_template_reset(slug)` for built-ins.
- **W3U**: `StartForm.tsx` `startOptions` and the form's `useAction` closure were touched (the template field only); the project
  field lands in the same lines — expect a trivial merge.

## Libraries (rules §"Libraries first")
- Markdown editing: `@uiw/react-codemirror` + `@codemirror/lang-markdown` via the existing `CharterEditor` wrapper (no new dep).
- YAML emission: none — the front matter is emitted with JSON-quoted scalars (a YAML 1.2 subset); a YAML library (`yaml`,
  `js-yaml`) was rejected as a new dependency for three keys. Parsing stays in the engine.
- No new dependencies.

## Open items
- No live engine for W4's RPCs in this worktree, so the suggestion rows and the template list were verified by vitest only
  (static markup + handler presses); no Vite/Tauri run (gate scope). Visual check of the confidence bar and the editor height
  (`h-64`) pending a desktop run once W4 Rust lands.

## Gates (BUILD_LABEL=158w4u, all through `~/.theuth/bin/build-lock`)
- `cd crates/desktop/ui && build-lock bun install` → 623 packages; `cd packages/apps && build-lock bun install` → 484 packages.
- `build-lock bunx tsc --noEmit -p .` → 0 errors.
- `build-lock bunx vitest run` on `meeting/Review.test.ts`, `meeting/StartForm.test.ts`, `settings/MeetingTemplates.test.ts`,
  `settings/Meetings.test.ts`, `lib/meeting-templates.test.ts`, `meeting/ControlCenter.test.ts`, `meeting/DetectionBanner.test.ts`,
  `rpc/api.test.ts` → 8 files, 29 tests passed.
- Proof left: `Review.test.ts` — rows render (name, `aria-valuenow`, bar width, evidence), Accept → `meeting_edit` Rename then
  `meeting_speaker_enroll` New over the label's segments; profile-set accept renames only; refused enroll → message toast;
  Not them → no RPC. `MeetingTemplates.test.ts` — list/badge/disabled Delete, Delete → RPC + invalidation, Save → put with the
  slugified slug and body, duplicate slug blocks Save. `StartForm.test.ts` — `startOptions` emits `template` only when picked;
  the Note template row renders under Session model.
