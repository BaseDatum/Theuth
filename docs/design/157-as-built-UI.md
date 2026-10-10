# 157 as built — UI (w157-ui, branch `feat/157-nbui`, base 2de5c411)

Brief: `~/.theuth/briefs/2026-10-10/157/157-ui.md`. Rulings: knowledge `decisions/theuth-notebook-chat-context-rulings-2026-10-10`.
Paths below are under `crates/desktop/ui/src`.

## What changed

### D1 — the model picker in the notebook chat
- `components/notebooks/ChatColumn.tsx:108-157`: the header carries `<ModelMenu session={session} own small />` (the thread's picker, or the draft pick for `session === null`), between the column label and the thread controls. Same popover as the chat header: favorites, providers, effort, and the Advisor row for an existing thread.
- `components/chat/ModelMenu.tsx:96-100`: `Props.own?: boolean`; `:287` `slashTarget(session, focused, own)` (pure, exported) is the `/model` focus rule — `own || session === null || focused === session`; `:290` `useOpenOnSlash` reads it; `:399` `ModelMenu({ small })` renders the 28 px / text-xs trigger instead of the 36 px headline.
- `/model` typed in the notebook composer goes `slash.ts run → openSurface('model') → theuth:open`; the notebook's `ModelMenu` listens because `own` makes it a target whatever `usePanes.focused` is (no split pane ever focuses the notebook thread). The pop-out window is its own document and gets the same listener.
- `composerOnly` (the phone's Sources tab) returns before the header: no picker there, as allowed.

### D2 — a new chat's pick reaches both creation paths
- Composer path (unchanged): `Composer.tsx:819-822` → `createOptions(draftCreate, useDrafts.pick)`; `ChatColumn.tsx:102` now passes `draftCreate={{ ...PLAIN_DRAFT, endless }}` so the ∞ choice rides with the pick.
- Dial path: `components/notebooks/NotebookContext.ts:74-76` `ensureCreateOptions(notebook, pick, endless)` (pure) — `PLAIN_OPTIONS` + `notebook` + the draft's `model`/`effort` + `memory: 'endless'` only when on; `:86-105` `useEnsureNotebookSession` sends it, then `useDrafts.resetPick()` spends the pick exactly as `openDraftSession` does.
- Tests: `NotebookContext.test.ts` (5: the Z2 path, the existing-thread answer, pick + ∞ + spend, default-then-forget, the pure helper); `NewChatPane.test.ts` already covers `createOptions`.

### D3 — endless (∞) for a new notebook chat
- `NotebookContext.ts:36-52`: `useOpenThread` gains `endless: Record<notebook, boolean | undefined>` + `setEndless`; `set()` (a thread opened, created or **New chat**) forgets it — "remembered for that draft only". Memory only, never persisted.
- `:107-111` `useEndlessSettings()` — `endless_settings_get()` (same query key as NewChatPane); the default is `default_chat`, the compactor line uses `model` through `lib/endless.useCompactorLabel`.
- `ChatColumn.tsx:119-130`: with no session, a `Switch` + `∞` glyph (aria-label "Endless memory", tooltip) in the header; `:101` the compactor line `Endless memory · compactor: <model>` under the context line while on.
- `:117-118, :157`: an existing thread shows `MemoryBadge` (from the `notebook_chats` row's `memory`) and mounts `MemoryPanel`, exactly as `Header.tsx:262, :368`.
- `/endless on|off`: verified it did **not** reach the engine from any desktop composer — `/endless` was a TUI-only command (`crates/tui/src/model.rs:7266`); the desktop `isCommand` treated it as message text. Fixed per D3 ("fix that only if it does not"): `components/chat/slash.ts:48-52` adds `/endless on|off` to `COMMANDS`, `:389-397` runs `set_session_memory(session, mode)`, seeds `queryKeys.sessionMemory`, invalidates `sessions` and `['notebooks','chats']` so both headers' badges follow; a draft is refused with a pointer to the switch. Test: `slash.test.ts` "157 D3".

### D4 — notes open in the right-hand pane
- `routes/notebooks_.$id.tsx`: `validateSearch` gains `note?: string`; `tab`, `ask`, `new` unchanged.
- `components/notebooks/notebook-pane.ts:10` `paneMode(note, members, fresh)` (pure): list without `?note`; the note when it is a member, the one just created here (`fresh`), or while items are still loading; a foreign id falls back to the list. Test: `notebook-pane.test.ts`.
- `components/notebooks/NotebookPage.tsx:93-105`: `fresh` state, `openNote(doc, isFresh)` (push, so browser back closes it), `closeNote()` (replace), `pane = paneMode(...)`; `:156` renders `NotePane` or `NotesColumn` in the notes track.
- `components/notebooks/NotesColumn.tsx:35`: `onOpen?: (id, fresh) => void`. With it (the columns layout) `+ New note` opens the created note in the pane (`:64`) and a row is a button (`Row`/`RowText`, `:143-151`); without it (the phone's Notes tab, and `NotesColumn.test.ts` as written) the row stays `<Link to="/notes/$id" search={{ nb }}>` and a new note navigates with `?new`. The desktop auto pop-out (`noteOpenMode`) is gone from this column; `lib/popout.noteOpenMode` stays for `AppSidebar.tsx:186`.

### D5 — the pane shows the editor only
- `components/notes/NotePage.tsx:70-83` `useNoteTitle(id, provider)` (title following the provider + `doc_rename`), `:87-94` `useNoteBody(live, editing)` (deleted → error document; board → `BoardReader` unless editing; unknown kind → empty box), `:224` `TitleInput` exported. `NoteView` consumes all three (`:109, :140, :181-186, :212-213`) so the page's behaviour is unchanged — including its `NoteChat`.
- `components/notebooks/NotePane.tsx:20`: `useDocument` + `useNoteTitle` + `useNoteBody` + `Live` (from `DocumentPage.tsx`) — the same provider, the same BlockNote editor, the same saving/collab path as `/notes/$id`; no `NoteChat`. `fresh && live` selects the title for typing (what `?new` does on the note page). "Ask theuth…" from a block prefills the notebook composer's draft and fires `theuth:focus-composer`.

### D6 — pane header
- `NotePane.tsx:39-60`: `ChevronLeft` + "Notes" (aria-label "Back to notes") → `onBack` clears `?note`; the title (`TitleInput`, inline rename); `AppWindow` with aria-label and tooltip exactly "Open in new window" when `canPopout()` → `openPopout({ kind: 'note', doc })` then `onBack()`.
- The row menu's "Open in new window" item (`NotesColumn.tsx:159-163`) is unchanged.

### D7 — resizable pane
- `lib/notebook-columns.ts`: `notesWidth` persisted with `collapsed` under the same `notebook-columns` key (`load`/`toggle`/`setNotesWidth`); `clampNotesWidth(px, pageWidth, sourcesWidth)` (pure, `:53-56`) — `NOTES_MIN_PX = 280`, max = min(60 % of the page, page − sources − `CHAT_MIN_PX` 360); `layout(mobile, collapsed, notesWidth)` puts it in the third track. Tests: `notebook-columns.test.ts` (4).
- `NotebookPage.tsx:145-158`: the notes `SideColumn` is a flex row — `ResizeHandle` (vertical, aria-label "Resize notes pane", `aria-valuenow/min/max`, drag via the grid's right edge, `PX_STEP` nudge) then the pane. The collapse toggle keeps working (rail at 12 px; expanding shows the open note again since `?note` is URL state). The sources column is untouched.
- Widen-once: `openNote` lifts a pane narrower than `NOTE_OPEN_PX` (480) to 480 (clamped), never shrinks (`:99-103`).

### D8 — unchanged / out of scope
- Phone tabs still navigate to `/notes/$id?nb=`; popped notes keep their window; `?nb` back links untouched (`useNotebookBack` untouched).
- Citation chips (`Citations.tsx:52 routeOf(cite, nb)`) and the Save-answer toast's **Open** (`MessageAssistant.tsx:219`) still go to `/notes/$id?nb=`: both live in `components/chat/*`, which this brief allows only for a tiny export, so routing them into the pane is not a single-seam change here. Left as is.

## Decisions
1. **Bindings for tsc**: this worktree has no generated `src/rpc/bindings` (gitignored, never regenerated per the brief). I copied the directory from the sibling worktree `theuth-pickscroll` (same base 2de5c411) so tsc checks real types; it is stale for a few types (`Notebook`, `ContextEstimate`, `CreateSessionOptions.notebook`…), which is why the baseline has 447 errors. Gate 1 is baseline-relative: 446 after (one fewer: `ensureCreateOptions` types the `notebook` field explicitly).
2. **`ModelMenu` over `ModelRow`+`ModelDialog`**: the popover anchors to the header button and already carries the Advisor row; only a `small` trigger variant was needed. Tiny `chat/*` edits: `ModelMenu.tsx` (`own`, `small`, `slashTarget`), `slash.ts` (`/endless`), tests for both.
3. **Focus rule**: `own` on the notebook picker rather than mutating `usePanes.focused` from the notebook page (that would redirect window-level hotkeys/`document.title` to a thread no pane shows).
4. **∞ state in `useOpenThread`**, not component state: the dial path (`useEnsureNotebookSession`, used by the sources/notes columns) must read it, and `set()` is the natural "the draft ended" moment. Read at call time (`getState()`), not render time, so a switch flipped after a render still counts.
5. **`/endless` on the desktop** (tiny slash command) rather than a toggle: the brief forbids a new toggle for existing threads and the MemoryPanel already tells users to type `/endless on`.
6. **Pane body = `useNoteBody` + `Live`**, not `NoteView embedded` (156's meeting pane): the brief wants the editor only; `NoteView embedded` brings the whole action bar (Rename, Attach, Snapshots, Export, Delete). The title/rename and body rules are lifted into hooks so nothing is duplicated.
7. **Boards in the pane** open in their reader (`editing = false`); the Edit toggle stays on the note page (`ponytail:` note in `NotePane.tsx`).
8. **History**: opening a note pushes (browser back closes it); the back button and the pop-out close with `replace` so back after a close does not reopen the note.
9. **`fresh` lives in `NotebookPage` state**, not the URL: `?new` already means "notebook just created" on this route.
10. **Narrow chat column**: with the pane dragged wide the chat column can hit 360 px; the header is a `@container` and hides "Chat · this notebook" under 32 rem and the "New chat" text under 28 rem so the picker label stays readable (seen in the harness at 506 px).
11. **No lint/format gate**: the UI package has no lint/format script and the repo has no biome/prettier/eslint config; gate 3 is vacuous.

## Deviations from the brief
- The "144 W4 seeded dev fixture" the harness was to use no longer exists (`144-as-built-Z.md:171` deleted it). The harness stubs the `Api` by hand (`/tmp/157-ui-harness/main.tsx`, throwaway) with a notebook, two notes, one endless chat and a live-capable `doc_open`.
- `NotebookPage.test.ts` and `NotebookContext.test.ts` (existing tests of files I own) were extended, not just kept passing: the first to mock `NotePane` (its import chain reaches `Transcript`/`localStorage` at module load) and assert `?note` pane mode; the second because `useEnsureNotebookSession` now reads `useQuery`/`queryKeys`.
- `notebook-columns.test.ts`'s persisted-shape assertion now includes `notesWidth` (the stored object changed).

## Gates
- tsc (`build-lock bunx tsc -b --noEmit`): baseline 447 errors → 446; no new errors (diff of file+message sets is empty on the `>` side).
- vitest (`build-lock bunx vitest run …`, 22 files): 91 passed, 0 failed — ChatColumn, NotesColumn, NotebookContext, NotebookPage, notebook-pane, notebook-columns, ModelMenu, ModelPicker, slash, Composer, NewChatPane, MemoryPanel, DocumentPage, notes/*, popout, no-emoji, meeting/*.
- lint/format: no configured command.

## Harness proof (headless Chromium over CDP, throwaway under /tmp, not committed)
`/tmp/157-ui-harness`: Vite with the worktree's config (router + tailwind plugins dropped; Tailwind pre-built from the tree under test by `build-css.mjs`), the real route files registered by hand, a stub `Api` in `scope('local').connection`, `window.open` recorded as `window.__h.popouts`, every RPC recorded as `window.__h.calls`. Control: the same harness over `git archive 2de5c411` in `/tmp/157-ui-control`.
Screenshots in `/tmp/157-ui-proof/`:
1. `01-notebook-page.png` — picker `Claude Opus 4.8 · high` and the `∞` badge in the chat header (existing endless thread).
2. `02-picker-open.png` — the picker open from the header (providers, effort, Advisor row); `03-slash-model-opens.png` — `/model` typed in the notebook composer opens it (no `set_model` call).
3. `04-new-note-in-pane.png` — `+ New note` → `?note=new1`, pane with back / `Untitled` selected+focused / Open in new window, BlockNote live, 0 textareas (no chat); `05-back-to-list.png` — back clears `?note`, 3 rows.
4. `06-pane-dragged-wider.png` — drag 480 → 598 px (`aria-valuenow` 598, `localStorage.notebook-columns.notesWidth` 598); `07-width-survives-reload.png` — 598 after reload; ArrowLeft nudge → 614.
5. `08-existing-note-in-pane.png` — a row opens `?note=n2`, title not selected; `09-popout-returns-to-list.png` — Open in new window → `window.open('/notes/n2?window=popout-note-n2', 'popout-note-n2')`, pane back to the list, `?note` cleared. Browser back from an open note returns to the list.
6. `10-new-chat-endless-switch.png` — New chat: the switch (off by default, `default_chat: false`), on → `Endless memory · compactor: anthropic/claude-haiku-4-5`; `11-draft-pick-and-endless.png` — GPT-5.4 · xhigh picked for the draft; `12-first-send-create-args.png` — the send called `create_session {model: 'openai/gpt-5.4', effort: 'xhigh', memory: 'endless'}` then `rename_session`, `send_input`, `session_set_notebook`; `13-dial-path-create-args.png` — a dial touched on a new chat called `create_session {notebook: 'nb1', model: 'openai/gpt-5.4', memory: 'endless'}` then `notebook_context_set`.
7. `14-endless-badge-memory-panel.png` — the `∞` badge opens the Memory panel; `/endless off` in the notebook composer called `set_session_memory('sess1', 'epochs')`.
8. `15-advisor-pill-opens-advisor-level.png` (R1) — the normal chat header's `ModelMenu` for a session: `openAdvisorPicker(session)` (the Advisor pill's "Change model…") opens the popover on the Advisor level (Back · Advisor · Off ✓). R1 restored `useOpenAdvisor` in `ModelMenu`, dropped by the first edit.
9. Control (base code): `control-01-notebook-page.png` — header `Chat · this notebook | Chats · 1 | New chat`, no picker, no switch, no handle; `control-02-new-note-pops-out.png` — `+ New note` → `window.open('/notes/new1?new=true&window=popout-note-new1')`, no pane; `control-03-row-navigates-away.png` — a row click leaves for `/notes/new1?nb=nb1`.
