# CELLHIST — note window freezes when a cell's history is opened (findings)

Worker notes for `fix/cell-history-freeze` (base `de3e6bcc`). Sky's report: the popped note window froze (WebContent at
~100 % CPU, RSS growing, sample dominated by layout and JS strings) when he opened `history ▾` on an earlier cell while a
later cell was running.

## Reproduction (base)

Scratch engine (`/Applications/theuth.app/Contents/MacOS/t` 2.18.5, data dir `/tmp/cellhist/eng`, RPC `127.0.0.1:18342`),
`bunx vite` from this worktree on `127.0.0.1:18343`, headless Chrome (CDP `9342`), the popped note URL
(`/notes/<id>?window=popout-note-<id>`) at Sky's popout size 880 × 1000 @2x. Cells A (`echo hello`), B
(`printf 'x%.0s' {1..300}; echo`), C (the 3000-line ANSI loop of the brief), all with `#| cwd: /tmp/cellhist`. Scripts
(scratch, not committed): `/tmp/cellhist/freeze.ts` (Chrome) and `/tmp/cellhist/pw/measure.ts` (Playwright WebKit).

- The brief's note alone (a title and the three cells) does **not** freeze, in Chrome or in WebKit (headless and
  headed): opening A's or B's history while C streams costs the same as having it closed (Chrome: 4–6 % main-thread
  busy, ~480 layouts/20 s, 20 React commits/20 s, no RPC but `doc_awareness`, in both states).
- A note shaped like a release note (40 headed sections of prose, lists and tables, 204 BlockNote blocks, the three
  cells after the first third) freezes **at once** when B's history opens: the renderer never answers again.
- C running is not needed (`before-open-b-idle` below), and the number of runs does not matter: see "Nothing running,
  one run vs several" below (Sky, 19:15: "even if nothing is running … only one run may be the cause").

Base, big note, popped window, Chrome, 20 s after opening (metrics through CDP `Performance.getMetrics` where the page
still answered, renderer CPU/RSS through `ps`, a 1 s `Runtime.evaluate` ping):

| run | renderer CPU | renderer RSS | pings answered | TaskDuration | layouts | JS heap |
|---|---|---|---|---|---|---|
| `before-closed` (C running, history closed) | 10 % | 553 → 562 MB | 20/20 | 1.3 s | 384 | 101 → 112 MB |
| `before-open-b` (C running, B's history open) | **167 %** | **538 → 2221 MB** | **0/7** (3 s timeouts) | **31.0 s** | **9680** | 106 → 207 MB |
| `before-open-b-idle` (C idle, B's history open, 12 s) | **208 %** | **505 → 1500 MB** | **0/4** | 22.0 s | 2368 | 101 → 123 MB |

The page stays wedged until the tab is closed (Sky: "close and re-open the note and it's fine").

### Nothing running, one run vs several (Sky's 19:15 report)

Base, nothing running anywhere, the history of a finished cell opened, 10–12 s, same scripts. "medium" = Sky-like:
`# Sprint 425 release note`, one paragraph, the three cells, one paragraph.

| note | cell (runs) | renderer CPU | renderer RSS | pings | layouts |
|---|---|---|---|---|---|
| small (title + 3 cells) | A (1) | 1.5 % | 514 → 513 MB | 12/12 | 5 |
| small | C (10) | 1.7 % | 544 → 556 MB | 12/12 | 5 |
| medium | A (1) | **206 %** | **497 → 1720 MB** | **0/4** | 5365 |
| medium | B (1) | **205 %** | **488 → 1569 MB** | **0/4** | 4191 |
| medium | A (2) | **206 %** | **488 → 1691 MB** | **0/4** | 5202 |
| big | A (1) | **208 %** | **505 → 1500 MB** | **0/4** | 2400 |
| big | C (12) | **208 %** | **505 → 1510 MB** | **0/4** | 2354 |

One run or many makes no difference; nothing has to run. What matters is a block of the note after the cells: one
paragraph is enough. In the small note `hideOthers` still writes `aria-hidden` on 4 editor elements (measured), but the
redraw range does not reach the opened cell, so the loop never starts. The history menu has no single-run code path,
no measuring effect, no ResizeObserver and no selected-run state (`HistoryMenu`, `Cell.tsx:284-312`: a query, a filter,
the items); the layout and rope-string time in Sky's sample is the node-view rebuild loop of the cause below.

### Pop-out window, window sizes, dropdown at the bottom edge (19:16 report)

Every run above already used the pop-out route: `openPopout({ kind: 'note', doc })` (`src/lib/popout.ts:53`) opens
`/notes/<id>?window=popout-note-<id>` in a Tauri window that `open_window` sizes 880 × 1000 for `popout-note-*` labels,
min 480 × 360 (`crates/desktop/src/commands.rs:39-51`); the page is the ordinary `NotePage` (`routes/notes_.$id.tsx`)
with the `POPOUT` shell. Script `/tmp/cellhist/placement.ts`: for each viewport size, scroll the note so the history
button's bottom sits N px above the bottom of the visible note area (the composer panel covers the window's last
~160 px), or N px above the window's own bottom edge (button under the composer, opened from the keyboard: focus +
Enter); 5 s per condition. It counts style writes on the popper wrapper and `data-side` changes on the menu, reads
`--radix-popper-available-height`/`max-height`, the menu's box and whether the menu is in this page's `document`.

Sizes: 880 × 1000 (the pop-out), 880 × 600, 880 × 400 (short), 480 × 360 (the pop-out's minimum), 1100 × 720.
Positions: 300, 80, 44, 20, 4 px above the note area's bottom; 60, 40, 20, 8, 2 px above the window's bottom.

- With the fix, big note, B (1 run), nothing running — 50 conditions: renderer 0.8–5.4 % CPU, every ping answered,
  **8–9 popper style writes per open, at most one `data-side` change**. The menu opens below while ≥ 61 px remain under
  the button (`max-height` = the space left, e.g. 56.5 px), and flips to the top once at ≤ 41 px (`max-height` = the
  space above). Always in this page's `document.body` (one JS realm per webview; Radix portals to the realm's
  `document`).
- Base, small note (no rebuild loop there), C with 10 runs — a menu taller than the room, so `max-height` clips it and
  it scrolls — 12 conditions at 880 × 1000, 880 × 400 and 480 × 360: the same, 1.2–7 % CPU, one flip at most, 8–9
  style writes, every ping answered.
- Conclusion: no flip, size or available-height loop, and no portal into another document, at any of these sizes or
  positions. A short window or a bottom-edge dropdown is not a condition of the freeze; the freeze appears at every
  size (880 × 1000 throughout the base runs above) whenever the note has a block after the cells.

## Root cause

Paused the wedged page with the debugger (`Debugger.pause` with the domain enabled before the click): the main thread
loops in ProseMirror — `DOMObserver` (MutationObserver callback) → `flush` → `updateState` → `updateChildren` →
`addNode`, i.e. ProseMirror re-creating node views. A breakpoint on `registerMutation`'s hit showed the mutations it acts
on: **1581 attribute mutations `data-aria-hidden` / `aria-hidden` on BlockNote's `.bn-block-outer` elements** inside the
editor.

The chain:

1. `crates/desktop/ui/src/components/cells/Cell.tsx:621` — every cell renders its §11.9 announcer
   `<p aria-live="polite" className="sr-only">` inside its own DOM, which in a note is a BlockNote node view inside the
   ProseMirror editor. A running shell cell adds a second one: the terminal status line
   `crates/desktop/ui/src/components/chat/TerminalViewer.tsx:376`.
2. `history ▾` is a modal Radix `DropdownMenu` (`Cell.tsx:291`). Opening it calls `hideOthers(content)`
   (`@radix-ui/react-menu` 2.1.24, `dist/index.mjs:137`). `aria-hidden` 1.2.6 keeps every `[aria-live]` node visible
   (`dist/es2015/index.js:133`, "we should not hide aria-live elements"), so it walks down to each cell's live region and
   sets `aria-hidden="true"` + `data-aria-hidden` on every sibling along the way — hundreds of ProseMirror-owned block
   elements.
3. ProseMirror (`prosemirror-view` 1.42.5 `registerMutation`) only ignores attribute changes on the editor root,
   `contenteditable` and some `style` changes: these count as DOM edits, so `flush` marks the whole range dirty and
   redraws it, re-creating the BlockNote node views in it — every cell in the range remounts.
4. The remounted cell whose history is open mounts its menu open again (the open state is store-backed,
   `useDisclosure(disclosureKey(key, 'history'))`, `Cell.tsx:287`, by design so a remount keeps it); the unmount undoes
   `hideOthers` and the new mount runs it again → more attribute mutations → step 3 → forever. Layout of the whole note
   per pass (Sky's `RenderFlexibleBox`/`RelayoutChildren`) and node-view DOM/text building (his `JSString`/rope
   frames), with memory growing per pass.

Measured on base (a MutationObserver counter installed before load, read through `Debugger.evaluateOnCallFrame` while
the page was wedged): 4 s after the click, **1083 `[data-note-cell]` insertions and 362 `[role=menu]` mounts** (3 and 0
before the click).

Whether a pass re-creates the opened cell depends on where the hidden siblings sit (ProseMirror redraws from the first
to the last mutated block, [INFERENCE] from `markDirty`'s range): measured, a single paragraph after the cells is enough
to start the loop with nothing running (medium note), while the title-plus-cells note never loops. "While a later cell
runs" is not required.

Not the cause (checked): the history query is per cell (`['cells','runs',refKey]`) and is fetched once; no output chunk
invalidates or refetches any query (0 RPCs but `doc_awareness` in 20 s); the store gets no per-chunk events (output
reaches only the xterm through `cell_attach`); no O(n²) string work on the output path. Nothing in the engine: no
`crates/cells`/`crates/rpc` change.

## Fix

Root: no live region inside a cell's DOM, so no modal overlay anywhere (history ▾, the python chip, the note's own
menus and dialogs) walks into the editor.

- `Cell.tsx`: the §11.9 announcements go through `announce(text)` and one `<p class="sr-only" aria-live="polite">`, a
  direct child of `<body>` (`ensureLiveRegion`, re-created if something removed it). R1: every cell mount ensures it
  exists **empty** before the first message (a region that appears already filled is not spoken by some screen
  readers); `announce` empties it and writes the text on the next `requestAnimationFrame`, so a repeated identical
  message is spoken again, and clears it 5 s later (`ANNOUNCE_CLEAR_MS`) or at the next message. The per-cell
  `<p aria-live>` and its `useState` are gone. As a direct child of `<body>` it is "kept" by `hideOthers` without any
  walk below `<body>`.
- `TerminalViewer.tsx`: the status line keeps `aria-live="polite"` for an agent PTY (outside any editor) and drops it
  for a cell source (inside the note's editor); the cell's own announcement already says running/finished/exit.
- `docs/design/171-cells.md` §11.9: where announcements live, one sentence.

Decisions (simplest option that keeps the contract):
- Kept the history menu modal and its store-backed open state: both are right once nothing inside the editor is
  "kept"; `modal={false}` on one menu would leave every other modal overlay (python chip, note toolbar menus, dialogs)
  re-creating all note cells once per open.
- One shared region instead of one per cell: two cells changing state in the same tick announce only the last; a
  screen reader read both before. Accepted (announcements are polite status, the footer shows the rest).
- No ProseMirror/BlockNote patch (ignoring `aria-hidden` mutations would need a node-view `ignoreMutation` on every
  BlockNote block type); removing the cause in our code is smaller and covers chat as well.

## After the fix (same scenario, same scripts)

Chrome, big note, popped window, 20 s from the click:

| run | renderer CPU | renderer RSS | pings answered | TaskDuration | layouts | React commits | RPCs | `aria-hidden` in editor |
|---|---|---|---|---|---|---|---|---|
| `after-closed` (C running) | 9.6 % | 537 → 540 MB | 20/20 | 1.18 s | 243 | 22 | `doc_awareness` 1 | 0 |
| `after-open-b` (C running, B's history open) | 7.3 % | 553 → 569 MB | 20/20 (worst 4 ms) | 0.79 s | 286 | 33 | + `cell_runs` 1 | 0 |
| `after-open-a` (C running, A's history open) | 9.6 % | 535 → 539 MB | 20/20 | 1.16 s | 370 | 33 | + `cell_runs` 1 | 0 |
| `after-open-b-idle` (C idle, B's history open) | 1.8 % | 504 → 466 MB | 20/20 | 0.14 s | 5 | 20 | + `cell_runs` 1 | 0 |
| `after-small-open-a` (brief's 3-cell note) | 9.9 % | 521 → 524 MB | 20/20 | 1.17 s | 364 | 33 | + `cell_runs` 1 | 0 |

Nothing running (Sky's 19:15 case), 12 s from the click, with the fix:

| note | cell (runs) | renderer CPU | renderer RSS | pings | layouts | RPCs | `aria-hidden` in editor |
|---|---|---|---|---|---|---|---|
| medium | A (2) | 0.7 % | 491 → 498 MB | 12/12 | 5 | `cell_runs` 1 | 0 |
| medium | B (2) | 0.8 % | 469 → 477 MB | 12/12 | 5 | `cell_runs` 1 | 0 |
| big | A (1) | 0.9 % | 502 → 511 MB | 12/12 | 5 | `cell_runs` 1 | 0 |
| big | C (12) | 1.7 % | 502 → 498 MB | 12/12 | 5 | `cell_runs` 1 | 0 |
| small | A (1) | 1.3 % | 488 → 482 MB | 12/12 | 5 | `cell_runs` 1 | 0 |
| medium, C running (second case) | A (2) | 9.4 % | 494 → 510 MB | 12/12 | 323 | `cell_runs` 1 | 0 |

The 11 extra commits of the open runs are the menu opening inside the window (one `cell_runs`, then idle): per chunk,
an open history now costs what a closed one does. `aria-hidden` lands on 5 elements, all outside the editor.

WebKit (Playwright WebKit 2370 = Safari 27.2, headless, popped window size; WebContent CPU via `ps`): base
`wk-big-closed` 10.0 % CPU; base with B's history open could not be measured — the page stopped answering Playwright
right after the click and the run hit its 300 s limit twice; after the fix `after-wk-big-open-b` 10.2 % CPU, menu open,
page answering. WebContent RSS grows ~3–6 MB/s while C streams with the history closed or open, before and after the
fix (Playwright WebKit baseline, not this bug).

Checked as asked:
- The running cell's own history: not reachable while it runs — the footer holding `history ▾` renders only for an
  ended run (`Cell.tsx` footer), so C shows Stop/Kill and no history; a history left open before a re-run reappears
  open when the run ends and, with no live region in the editor, costs one menu mount.
- A chat cell (`CellFence`): chat cells are not in ProseMirror; `hideOthers` there stamps the transcript (CodeMirror's
  own `cm-announced` live regions are in the cells) but nothing re-renders from it. The cells fixture
  (`/src/dev/cells-fixture.html`, real `MessageAssistant → CellFence → Cell`): history ▾ open, 8/8 pings answered.

## Tests

- `src/components/cells/Cell.test.ts` — `CELLHIST: no cell renders a live region of its own, whatever its state or open
  history` (ready, running, done, failed; history disclosure open): fails on base (`ready: expected '<div id="cell-…'
  not to contain 'aria-live'`), passes with the fix.
- `Cell.test.ts` — `CELLHIST: announcements go through one empty-at-first live region on <body>, filled a frame later,
  cleared after 5 s` (R1; a stubbed `document` slice and fake timers/`requestAnimationFrame`): the region exists empty
  before the first message, is a direct child of `document.body` and the only one, the text appears one frame later, a
  repeated message is emptied and written again, the text is gone after 5 s.
- `src/components/chat/TerminalViewer.test.ts` — `CELLHIST: a cell terminal (inside a note editor) has no live
  region; a pty terminal keeps its polite status line` (R1).

The aria-hidden → ProseMirror loop itself needs a real DOM and an editor (the UI tests run in node with
`renderToStaticMarkup`); the live runs above are its proof. R1 smoke in Chrome (`/src/dev/cells-fixture.html`): after
mount `<body>` holds one empty `p.sr-only[aria-live=polite]`; `announce('smoke message')` leaves it empty in the same
tick and filled after a frame; still one region.

## Screenshots (`/tmp/cellhist/shots/`)

- after: `after-big-open-b.png` (B's history open while C streams), `after-big-open-a.png`, `after-big-open-b-idle.png`,
  `after-big-closed.png`, `after-small-open-a.png`, `after-wk-big-open-b.png` (WebKit), `after-chat-history-open.png`.
- before: `before-big-closed.png`; the frozen page itself cannot be captured (`Page.captureScreenshot` times out from
  10 ms after the press on; the renderer never paints again). `before-small-open-a.png`, `before-small-pop-open-b.png`,
  `before-small-open-b-late.png`, `before-wk-small-*.png`: base with the history open while C runs in the brief's small
  note, which does not freeze.

## Gates

1. Bindings: `BUILD_SLOTS=1 build-lock mbx test -p rpc --features ts --lib --quiet export_bindings` → `test result: ok.
   8 passed`, copied into `crates/desktop/ui/src/rpc/bindings/`; `build-lock bunx tsc -b --noEmit` (TypeScript 7.0.2;
   also with `--force`) → 0 `error TS`.
2. `build-lock bunx vitest run src/components/cells/ src/components/chat/TerminalViewer.test.ts
   src/components/disclosure.test.ts` → `Test Files 20 passed (20)`, `Tests 122 passed (122)`.
3. `BUILD_SLOTS=1 build-lock bunx vite build` → `✓ built in 3.48s`.
4. No engine change.

## Deviations

- The brief's 3-cell note does not freeze on base (Chrome, WebKit headless and headed); the freeze reproduces as soon
  as the note has a paragraph after the cells (medium note) or more (204-block note), with nothing running, one run or
  several, and with C running.
- `crates/desktop/ui/node_modules` lacked `anser`/`escape-carriage` (the clone predates CELLHL's lockfile):
  `build-lock bun install --frozen-lockfile` in `crates/desktop/ui`, no lockfile change.
- BUILD RULES call `vite build` heavy and say never; gate 3 asks for it under `BUILD_SLOTS=1 build-lock`: ran it once
  that way, as the gate says.
- WebKit runs used Playwright's cached WebKit (`~/Library/Caches/ms-playwright/webkit-2370`, `playwright-core` installed
  under `/tmp/cellhist/pw`) in addition to the brief's headless Chrome, to match Sky's WebContent; one run was headed
  (a Playwright window for ~40 s).
- No "before" screenshot of the frozen state (see Screenshots).
