# DICTATION as built — a dictation mic on every text input

Branch `feat/dictation-everywhere`, worktree `theuth-dictation`, base `ae467a8`. Two commits plus this report.

## Approach

The composer's pipeline (`lib/dictation.ts`: store, `dictation_start/stop/audio` RPCs, `Dictation*` events, browser-mic streaming for relay peers, `splice`) is reused unchanged; no second STT path. Instead of touching ~60 call sites, the mic lives **inside the shadcn `Input` and `Textarea` primitives** (`components/ui/input.tsx`, `textarea.tsx`), so every text field in desktop and mobile gets it by default. Bare `<input>`s and the BlockNote editor use the standalone `DictationButton`.

## Commits

### `81de24b` desktop: dictation mic on every Input/Textarea; DictationButton for bare inputs and the editor

- `lib/dictation.ts`
  - `useBox(api: Api | null, value, onChange(v, caret), ref, mic)` — the per-field hook the brief calls `useDictation()` (that name already belongs to the store hook; kept). Now: `api === null` makes the toggle a no-op (fields rendered before a connection: pair screen, guest app); the anchor is read from the field's **DOM value and selection** so a selection is replaced and uncontrolled fields work; the finish callback also receives the caret.
  - `anchor(value, start, end)` — the insert-at-caret helper: `before`/`after` around the selection; `null` → the end.
  - `useDictation()` passes a server snapshot (SSR-safe; `Peers.test.ts` renders `Input` via `renderToStaticMarkup`).
  - exported `Box` type.
- `components/ui/dictation.tsx` (new)
  - `useFieldDictation(ref, value | undefined, onText?)` → `Box & { hidden }`. Controlled field: `shown` replaces the value while dictating. Plain field (`value === undefined`): an effect mirrors the live splice and a `readOnly` lock into the DOM. `hidden` = no connection, or model `missing`/`failed` (STT unavailable). `loading` / "dictating elsewhere" dim the button with the reason as `title`.
  - `setFieldValue(el, value, caret)` — writes the finished text the way typing would: points React's value tracker elsewhere, sets the real value through the prototype setter, dispatches a bubbling `input` event (reaches the caller's `onChange`), focuses, places the caret. Works for controlled and uncontrolled fields; no call site had to change its `onChange` shape.
  - `MicButton` — `type="button"` (never submits the surrounding form), `size="icon-xs"`, `aria-label` Dictate / Stop dictation, `aria-pressed`, `aria-disabled` + `title` reason (tooltip, never a modal), `onPointerDown → preventDefault()` so blur-committing rename inputs survive the press. Dictating: pulsing stop square in the accent colour.
  - `DictationButton({ field?, onText?, className? })` — standalone: `field` for a bare `<input>`, `onText` for rich editors.
  - `splitLayout(className)` — width/flex/margin/min-max-height/grid-span classes (with variants) move to the wrapper `div` so the box sizes as the bare field did.
  - `TEXT_TYPES` — `text`, `search` (and no `type`) take dictation; password, number, datetime-local, url, file… stay plain.
- `components/ui/input.tsx` / `textarea.tsx` — `dictate?: boolean` prop (default true). A text-typed, non-read-only field renders inside `<div class="relative flex w-full min-w-0 …">` with the mic at the trailing edge (Input: vertically centred right; Textarea: bottom-right); `pr-7`/`pr-8` (`pointer-coarse:pr-11`) only while the mic is shown; hidden while `disabled`. Read-only / non-text fields render the bare element exactly as before.
- `chat/Composer.tsx` — `dictate={false}` on its Textarea (keeps its own Ctrl+D / meter mic).
- `chat/AskCard.tsx` `OwnAnswer`, `meeting/StartForm.tsx` `ContextBox` — their hand-rolled `useBox` + button code deleted; the plain `Input`/`Textarea` now carries the mic (−60 lines).
- `notes/NotesList.tsx` (rename), `notes/NotePage.tsx` (title), `editor/FrontmatterBlock.tsx` (tags) — bare `<input>`s get `<DictationButton field={ref} />` beside them.
- `editor/Editor.tsx` — BlockNote: `<DictationButton onText>` in the editor footer (visible on desktop and mobile, no selection needed): `editor.insertInlineContent(text)` at the cursor, then `editor.focus()`. Only while `editable`.
- `App.tsx` `loadInfo` — a `dictation_prewarm` refusal (voice plugin disabled, relay peer) now sets the model `failed` instead of leaving it `loading` forever, so every field mic hides (and the composer's dims with a real reason). One line + its doc comment.
- Tests: `lib/dictation.test.ts` + `anchor`; `components/ui/dictation.test.ts` for `splitLayout`.

### `9f0a91d` desktop: dictation finish reaches React onChange when the field already shows the final text; editor skips empty inserts

Found by the web smoke: while dictating the field already renders the spliced text, so React's value tracker saw no change on finish and `onChange` never fired (the field snapped back). `setFieldValue` now sets a sentinel through the instance setter first. `Editor` ignores an empty finish.

## Coverage — one pass over `grep -rn "<Input\|<Textarea\|<input\|<textarea" src`

Covered automatically (controlled or uncontrolled `Input`/`Textarea`, text/search type): tasks `NewTaskSheet` title + brief, `TaskDetail` comment + hand-off notes, `HireWizard` title/name/capabilities, `AgentDetail` currency, `RoutineDetail` name/cron/every/project/model/cwd/until/prompt, `Skills` search/name/description/body/repo URL, `Secrets` **name and note**, `Tools` config textarea, knowledge `SearchBox` + `NewDocDialog` id/title, `BriefSearch`, `BriefEditor` title/bullets/aliases/tags, settings `Channels`, `Connections` name + URL, `Communication` root alias (uncontrolled), `Mesh` pairing string / key / rename, `Link` rename (uncontrolled), `Peers` filter + answer/resolve textareas, `BroadcastDialog` text, `SessionMenu` rename, `HeldStrip` JSON textarea, `MessageUser` edit textarea, `ProjectPicker`, `SessionItem` rename, `Review` label rename / speaker name + entity / segment text (uncontrolled), `ControlCenter` ask box, `StartForm` title + context, `Hud` decision answer, `AskCard` own answer, artifacts `Explorer` + `ArchiveView` filters, `PairScreen` key textarea + `GuestApp` name + `ShareDialog` preset (mic hidden there: no connection), `FrontmatterBlock` key/value/fixed-row inputs, `SidebarInput`.
Bare inputs given a `DictationButton`: `NotesList` rename, `NotePage` title, `FrontmatterBlock` tags. BlockNote `Editor`: footer mic.
Already had a mic, kept: `Composer` (own pipeline), `SearchDialog` (cmdk `CommandPrimitive.Input`, own `useBox` button).

Deliberately skipped: every `type="password"` (`Secrets` value, `Connections` API key, `SecretRequestCard` value), `type="number"` (`Loop`, budgets, max runs), `type="datetime-local"` (routine `at`), `type="url"` (`MobileToolbar` link), `type="checkbox"`/`type="file"`, read-only fields (`shared.tsx` chord recorder, `AskCard` locked answer), `lib/injected.ts` (string literal), `meeting/Island.tsx` composer (the island webview does not install the dictation listeners — `App.tsx` `ISLAND` gate — so the store never sees events there).

## Deviations

- Brief header names `/tmp/67/…` and `docs/design/67-as-built-DICTATION.md`; its own tail names `/tmp/69/report-DICTATION.md` and `docs/design/70-as-built-DICTATION.md`. Followed the tail (task-specific).
- `useDictation()` already exists as the store hook; the per-field hook is the existing `useBox` (generalised) and the component is `DictationButton` / the built-in adornment rather than `<DictationButton onText onPartial />`. Partials stream through `shown` into the field (read-only, like the composer); `onPartial` is not a separate callback — the BlockNote mic inserts the final text only (streaming partials into ProseMirror would need repeated inline replacement).
- Adornment built into `components/ui/input.tsx`/`textarea.tsx` (the brief's allowed alternative) instead of `<TextInput>`/`<TextArea>` wrappers: one diff covers every current and future field.
- "Hidden when STT unavailable" = no connection or model `missing`/`failed`; `loading` and "dictating elsewhere" show a dimmed mic with the reason, matching the composer.
- Errors from `dictation_start` still surface as the pipeline's existing sonner toast (non-modal); the disabled reason is the button's `title`.
- `StartForm` context box used `mic=false` (engine mic always); it now follows the shared rule (browser mic on a relay peer), like `AskCard` and `SearchDialog`.
- `App.tsx` one-line change (prewarm refusal → `failed`) outside the UI primitives; disclosed above.
- Did not touch `AppSidebar.tsx`, `tasks/Board.tsx` or the Attention card (UI-FIXES-3 owns them); they gain the mic through `Input` anyway.
- Silence-stop is the daemon's (max length / finish); no client-side VAD added.

## Not built

- Streaming partials into BlockNote (final insert only).
- Mic for the meeting Island composer (needs the island webview to subscribe to `Dictation*` events).

## Tests written

- `lib/dictation.test.ts`: `anchor` — caret split, selection dropped, `null` → end, backwards `end` never duplicates.
- `components/ui/dictation.test.ts`: `splitLayout` — box vs field classes incl. variants.
- The hook's state machine is `applyEvent` (existing tests, unchanged and green).

## Verification

- Bindings: `cargo test -p rpc --features ts export_bindings` did not finish in 30 min on this cold worktree target; copied `crates/desktop/ui/src/rpc/bindings` from the `theuth` main worktree at the same base commit (`ae467a8`). No `cargo check` (no Rust touched).
- `bunx tsc --noEmit` → 0 (after `bun install` here and in `packages/apps`).
- `bun run build` (`tsc -b && vite build`) → 0, 13173 modules.
- `bunx vitest run` → 76 files, 476 passed, 1 skipped (pre-existing).
- Smoke, web dev build against the running `/Applications/theuth.app` daemon (`VITE_THEUTH_URL=ws://127.0.0.1:53703`, headless Chromium): New task title — typed `hello world`, selected `world`, mic → field read-only, shows `hello `, stop icon, focus kept; stop → `hello ` committed to React state, caret 6, mic back to Dictate. Brief textarea: caret mid-text, start/stop → value and caret unchanged (nothing said; daemon mic heard silence). Settings → Link relay rename (uncontrolled, blur-commits): DOM value read at start, `readOnly` locked and released, caret restored, field stayed mounted through the press. Secrets: name + note carry the mic, value (password) does not. Routine editor, knowledge search, knowledge doc (editor footer mic) render with layout intact. No console errors. Real speech was not exercised (no audio source in the headless run).

DONE DICTATION
