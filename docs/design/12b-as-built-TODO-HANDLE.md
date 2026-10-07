# 12b as-built — to-do grabber above the composer (TODO-HANDLE)

Branch `fix/todo-handle` (from main cb6d94b9). Files: `crates/desktop/ui/src/components/chat/TodoCard.tsx`,
`TodoCard.test.ts`. No store, RPC, bindings, Composer, route or TUI changes.

## The ask (Sky, 2026-10-07 18:32)

"on desktop/mobile there should be a little short thick line centered on top of text input below where the
subagent dots would be where clicking it hides/shows the todos like in hermes desktop, they should be hidden by default."

## Decisions as built

| Decision | Where |
|---|---|
| D1 grabber: `button`, 64×20 hit area (`h-5 w-16`), 36×4 line (`h-1 w-9 rounded-full bg-fg-tertiary/40`, `group-hover`/`group-active:bg-fg-secondary`, `transition-colors`), `touch-manipulation select-none cursor-pointer`, `focus-visible:ring-[3px] focus-visible:ring-ring/50`, `aria-expanded`, `aria-controls` → body id `todos-<session>`, `aria-label` = `title` = `handleLabel(...)`; no text, icon or badge | TodoCard.tsx:133–143; `handleLabel` :50 ("Show to-do list · 2/5 done" / "Show to-do list · loading…" / "Hide to-do list") |
| D2 one flag: visibility is `todosExpanded` (default false). Hidden → only the grabber. Shown → header row (ChevronDown · Todos · counts · tags) + open list; the header is a button that sets the flag false. `/todos` slash toggles the same flag, unchanged listener. Collapsible replaced by a plain conditional (`body`) | TodoCard.tsx:60–127; slash listener :67–77 |
| D3 grabber exists iff the card would have rendered today: items, or a note state (loading… / unavailable / empty · Stale). `note() === null` with no items → `return null` (nothing at all). Hidden + note: the grabber reveals the note row | TodoCard.tsx:79–93 |
| D4 order: dots · [body] · grabber · Composer. TodoCard returns `[body?] + grabber`; route order untouched. Card container lost `pb-2`; the 20 px grabber row gives 8 px line→composer and sits directly under the dots row (`h-7`) | TodoCard.tsx:129–145; route `c.$session.tsx:145–147` unchanged |
| D5 same component for desktop and phone; nothing in TUI/engine; no new fields/RPC/bindings | — |
| D6 plain mount/unmount, no animation; persistence as before (per-session slice) | — |

## Gates (this worktree, after setup; `~/.theuth/bin/build-lock`)

- `bun run check` (tsc): 0 errors.
- `bunx vitest run src/components/chat src/store`: 51 files, 556 tests, all green (TodoCard.test.ts 4/4 incl. the new `handleLabel` case).
- `bunx vite build`: ok (built in 3.4 s).
- No biome/eslint configured for the UI (package.json scripts: dev/build/preview/test/check) — skipped.

Setup caveat: the setup script's `tsc-baseline` FAILED (159 errors, all `@openuidev/*` unresolved from `packages/apps`
and `AppActionEvent` members) because `packages/apps/node_modules` was not cloned into the worktree, and the clone left a
stray absolute symlink `crates/desktop/ui/node_modules/node_modules → …/theuth/crates/desktop/ui/node_modules` that
loaded a second React into vitest (134 "Cannot read properties of null (reading 'useEffect')" failures). Fixed locally by
APFS-cloning `packages/apps/node_modules` from the user's checkout (read-only on the source) and deleting the stray
symlink; both are gitignored, the user's checkout was not touched. Gates above are after that repair.

## Visual proof (vite dev on :1433 against the live engine 127.0.0.1:59231, read-only; headless Chrome via raw CDP)

Session `01a116a6-6631-7293-ae91-066d18e9e318` (reads 2/5 done at shoot time; one live subagent, so the dots row shows).

- `/tmp/todo-handle/desktop-hidden.png` (1600×1000) — dot · line · composer; no "Todos" text on the page (`todoText:false`).
- `/tmp/todo-handle/desktop-shown.png` — dot · card (header + 5 items) · line · composer; grabber rect identical in both states (`[898,916,64,20]`).
- `/tmp/todo-handle/phone-hidden.png`, `/tmp/todo-handle/phone-shown.png` (430×932 @2, touch) — same; grabber `[183,848,64,20]`, line `[197,856,36,4]` (centred: 183+32 = 215 = 430/2).
- `/tmp/todo-handle/desktop-hidden-closeup.png`, `phone-hidden-closeup.png` — line centred under the dot, 8 px above the composer's top edge.

## Not verified

- Hover/active colour and the focus ring: Tailwind classes per D1, not screenshotted (headless, no pointer hover).
- Real Tauri phone build: emulated viewport + touch only.

## Revision 1 (20:55)

Sky: "you didn't get it quite right. clicking or tapping the line should show or hide the collapsed todo block, not display it expanded. clicking on the todo block itself when it's shown expands it like before. small tweak." Branch `fix/todo-handle-r1` (from main d5f76242).

Two per-session flags. `todosShown` (new, default false) is the grabber's state: hidden → only the grabber; shown → the pre-d7b22af0 Collapsible block above the grabber (header `ChevronRight` rotating 90° · Todos · counts · tags · truncated `Now: …` when collapsed; `CollapsibleContent` with the bounded list). `todosExpanded` (unchanged) is the block's disclosure, toggled only by its header (`data-testid="todos-header"`); the header never hides the block. `/todos` toggles `todosShown` (keyboard twin of the grabber), never `todosExpanded`, so re-showing restores the previous expand state. Labels: `handleLabel(shown, detail)` → "Show to-dos · 2/5 done" / "Show to-dos · loading…" / "Hide to-dos"; `aria-expanded={shown}`, `aria-controls` → `todos-<session>` (the Collapsible root, or the note `<p role="status">`).

- `store/sessions.ts:287` field, `:348` action decl, `:616–628` `setTodosShown` (no-op when unchanged); `store/reduce.ts:65` `empty()` default.
- `components/chat/TodoCard.tsx:51–53` `handleLabel`; `:69–81` `/todos` listener; `:90–135` block (note row / Collapsible); `:139–151` container + grabber.
- Tests: `TodoCard.test.ts:29–33` label texts; `store/reduce.test.ts:1041–1056` `setTodosShown` per session, `empty().todosShown === false`, repeated value returns the same state object.

Gates (`~/.theuth/bin/build-lock`): `bun run check` 0 errors; `bunx vitest run src/components/chat src/store` 51 files / 557 tests green; `bun run build` ok (5.1 s). Setup's tsc-baseline passed (packages/apps/node_modules cloned, stray nested node_modules removed by the script).

Visual proof (vite :1434 against the live engine 127.0.0.1:53287, read-only, session `01a116a6-…`, 1/4 done at shoot time): `/tmp/todo-handle-r1/{desktop,phone}-{hidden,shown-collapsed,shown-expanded}.png` (1600×1000; 430×932 @2 touch). Hidden: no "Todos" text. Collapsed: one-line row `› Todos · 1/4 done  Now: R1 worktree + bindings setup`, 0 list items. Expanded: chevron rotated, 4 items, `Now:` gone from the header. Grabber rect identical in all three states: desktop `[898,916,64,20]`, phone `[183,848,64,20]`.
