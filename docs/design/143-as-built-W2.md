# 143 NOTE-FLOW — as built, W2: the note chat is always there

Contract: `143-note-flow.md` D8–D14. Branch `feat/143-w2` off main `84548287`.

## What landed (file:line)

- `crates/desktop/ui/src/lib/note-chat.ts`
  - 28–29 `EMPTY.open = true` (D9). A saved `open: false` still wins through `set`'s `{ ...EMPTY, ...saved, ...patch }` merge. `DEFAULT_HEIGHT` 320 / `MIN_HEIGHT` 160 unchanged.
  - 31–41 `panelHeight(saved, pane, mobile)` (D10/D13): desktop `clamp(saved, MIN_HEIGHT, pane − 120)`; mobile cap `0.45 · pane`, start `min(320, 0.4 · pane)` when `saved === DEFAULT_HEIGHT`; floor `min(MIN_HEIGHT, pane)`.
- `crates/desktop/ui/src/lib/note-chat.test.ts` (new) — three tests: desktop clamp, mobile cap/start, default-open + `set` merge (localStorage stubbed with `vi.stubGlobal`).
- `crates/desktop/ui/src/components/notes/NoteChat.tsx`
  - 51–61 `useIsMobile()`, `pane` state read in a `useLayoutEffect` once a session exists, `height = panelHeight(state.height, pane, mobile)`; drags still clamp against the live `parentElement.clientHeight`.
  - 63 `popped = isPopout()` (D11).
  - 81–92 **input bar** (no session): `section.flex.shrink-0.border-t` with no `style.height` and no `ResizeHandle`; `h-8` header with the muted copy "Ask theuth about {about} — the first message starts a chat with it attached" (truncating) + Close; then the same `Composer compact pinned={doc}` (same `key`, `onCreated`, `sessionTitle`).
  - 94–138 **session panel**: today's panel — `ResizeHandle`, header (title · Open chat · New chat · Close), `Transcript compact`, `Composer compact`, `style={{ height }}`. Open chat: `openPopout({ kind: 'chat', session })` when popped, else `navigate('/c/$session')`; tooltip "Open in its own window" / "Open in the chat list".
  - `theuth:note-ask` listener, `useLoadSession`, `about` copy unchanged. CharterDoc untouched; it gets the bar shape via the same component (D12).
- `crates/desktop/ui/src/components/notes/NotePage.tsx`
  - 155 Chat action without the `!popped &&` guard; 161 `below={chat.open && !deleted ? <NoteChat …/> : null}` (D8). "Open in new window" keeps `!popped`; Delete in a popout still `closePopout()`.

## The two shapes

| shape | when | height |
|---|---|---|
| input bar | `state.session === null` | content: 32 px header + composer. Measured **133 px** desktop (1600×1000 and 960×720 popout), **137 px** at 390 px mobile |
| session panel | `state.session` set (after `onCreated`) | `panelHeight(saved, pane, mobile)`; measured 320 px at 1600×1000 with the default saved height |

The bar measures above the contract's "≈ 90–110 px" target: the compact Composer alone is ~100 px (pinned-chip row + 40 px textarea row + padding) and the header adds 32. Not shrunk — the Composer is off-limits (D12).

## Proof (read-only against the live engine, vite on 1437, headless Chrome 2×)

Note id `agqrwrzff52ifit22fl6tyu72u` (an existing "Untitled" plain note — the other notes on the engine are boards and render `BoardReader`, no `.bn-editor`). Nothing was created, typed or sent.

- `/tmp/143/w2/note-shell.png` — 1600×1000 shell: sidebar, paper (`.bn-editor` 800×412 at y 120; W1's portrait ratio is not in this tree), input bar at y 867, h 133, header text as above, Chat action present, `localStorage['note-chat']` null.
- `/tmp/143/w2/note-shell-paper.png` — same note, earlier run (identical facts). `/tmp/143/w2/note-shell.png` from the first runs showed board notes (`agqrd3o2q54mbcvruvc4bfhazq`, `agqrdx6ytn3qdieqbsiojwfs7i`) with the same bar at 133 px — the bar applies under `BoardReader` too.
- `/tmp/143/w2/note-popped.png` — `?window=popout-note-<id>` at 960×720: no sidebar, drag bar, paper 800×294, bar at y 587 h 133, `[aria-label="Chat"]` present, `[aria-label="Open in new window"]` absent.
- `/tmp/143/w2/note-phone.png` — 390×844 mobile: paper edge to edge (x 0, w 390), bar at y 707 h 137, header copy truncates with an ellipsis.
- `/tmp/143/w2/note-session.png` — session shape. No note on the engine has a `note-chat` binding in the headless profile, so the shot seeds the profile's `localStorage['note-chat']` with an existing session id (`01a10e31-47cf-7103-be29-f5a4983e45a3`, a read via `load_session` only) and reloads: panel 320 px at y 680, `ResizeHandle` on top, header "DR_FastDeploy unplanned deploy failback" with Open chat / New chat / Close, compact transcript, composer. Plus `note-chat.test.ts` for the clamp.

Measurement: `document.querySelector('section[aria-label^="Chat about"]').getBoundingClientRect().height` → 133 (desktop), 137 (mobile), 320 (session).

## Gates

- `BUILD_LABEL=143-W2 nice -n 10 build-lock bun run check` → exit 0, 0 errors (baseline `tsc-baseline-theuth-143-w2.log`: 0).
- `… build-lock bunx vitest run src/lib src/components/notes` → exit 0, 77 files / 480 tests passed (incl. `note-chat.test.ts`, `no-emoji.test.ts`).

## Deviations / decisions

- `panelHeight` floor is `min(MIN_HEIGHT, pane)` rather than the old raw `clamp(h, MIN_HEIGHT, pane − 120)`: identical for every pane ≥ 280 px; for a pane under 280 px the old code returned `pane − 120` (below MIN_HEIGHT or negative), the new one returns MIN_HEIGHT, or the pane itself when the pane is smaller than that — the contract's "never below MIN_HEIGHT unless the pane itself is smaller".
- The rendered height uses a `pane` state read in `useLayoutEffect` on `[session]` (the ref is empty on the first render); drags re-measure live, as before. `onStart` records the rendered `height`, not `state.height`, so a mobile first drag starts from what is on screen.
- `BoardCard` binds a board to its session via `set(id, { session })` on an unsaved note: with `EMPTY.open = true` that binding now also opens the panel (previously `open: false`). Consistent with D9 — a never-closed note shows its chat.
- Bar height 133 px, not 90–110: see above.
- Session screenshot taken by seeding the headless profile's localStorage instead of skipping (the contract allowed a unit-test-only proof).

## Only the real Tauri app can show

- `openPopout({ kind: 'chat', session })` from a popped note's Open chat → `open_window` focus-if-open; the browser gets a named `window.open`.
- The 960×720 pop-out window chrome itself; the mobile 45 % cap under a real soft keyboard (`pane` shrinks with the visual viewport).
