# 218 — as built: B3 (canvas + ink)

Builder B3, 2026-11-25 night build. Owned paths only: `crates/desktop/ui/src/components/whiteboard/{canvas,ink}/**`, `crates/desktop/ui/package.json` + `bun.lock` (`perfect-freehand` 1.2.3, nothing else).

## Commits

| sha | subject |
|---|---|
| `54cb0756e` | canvas seam (model, store, api, empty Canvas) |
| `cc3f51ec5` | add perfect-freehand 1.2.3 |
| `471b38646` | pure whiteboard modules: pack, ids, model readers, ops, chunking, undo, rows, binding, LOD, input routing, anchoring, marks, life, colours, planSend/commitSend, fixtures, tests |
| `550133bd6` | canvas: React Flow binding, card chrome and renderers, LOD + snapshots, frames, links, pointer ink, eraser, palm rule, handle |
| `5171257a2` | canvas: a click after a stroke is not a canvas tap; the Pencil is a pointer in link mode; late first fit; free ink above cards |
| `65f2e18ee` | lasso, selection bar, mark long-press pin/unpin with labels, snapshot before demotion, off-screen placeholder chevrons; keyboard keys deferred to the shell |
| `43da2728e` | fixes found in the browser (listed under "Checks run") |
| `151c329c8` | placeholder cards are opaque enough to read over what lies under them |
| `7bfb036da` | the diff card uses the generated `DiffSpec` type, so it needs no temp cast |
| `17238ca2c` | session cards show the last turns; image cards load artifact, path and blob images; light green and amber ≥ 3 : 1 |
| `caaaf242e` | the Pencil over an entered live page inks, through a pen relay in page cards; fingers and mouse still reach the page |

## What is built (files)

`canvas/`
- **Seam, verbatim from the contract:** `model.ts` (§2.11), `store.ts` (§8.1), `api.ts` (§8.2).
  - `model.ts` also implements tolerant readers. They ignore unknown keys and enums, skip entries they cannot parse (never delete them), and read `bigint` numbers.
  - `model.ts` also exports `versionOf(card, scrub?)`, `readCard`/`readLink`/`readFrame`/`readStroke`/`readMark`/`readMeta`, `mapsOf` and `indexMarks`.
- `ids.ts` provides `ulid` and `isUlid`.
- `useBoard.ts` provides `useBoard(provider)`, backed by one shared source per `Y.Doc`. Each transaction re-reads only the entries it touched, so the board is structurally shared.
- `rows.ts` provides `rowsOf` (the §3.2 row rule).
- `ops.ts` contains every §8.3 mutation, `chunked`, and these extras:
  - `raise`, `addStrokes` (one stroke per transaction), `setSnap` (`wb-local`), and `moveItems` (a drag end in one owner transaction, riders included).
  - Constants: `IN_PLACE`, `STICKY_MAX`, `CHUNK_WRITES`, `CHUNK_BYTES`.
  - `chunked` measures the update as it builds it (the diff against the state vector) and stops a transaction before 60 writes or about 30 kB.
- `undo.ts` creates one `Y.UndoManager` over cards, links, frames, ink and marks. It tracks only `wb-owner`. Every owner action calls `stopCapturing()` afterwards.
- `binding.ts` is the xyflow-free Yjs ↔ React Flow binding:
  - Frames are parent nodes; positions are parent-relative in React Flow and absolute in the doc.
  - `dragEnd` works out the writes of a drag: frames carry their cards, membership follows the card's centre, and free strokes touching exactly one card ride with it.
  - `facing` picks the sides for links that have no ports.
- `lod.ts`: live / snapshot / chip, the live budget, and demotion after 30 s idle. Pinned cards are never demoted.
- `input.ts`: the §8.4 routing table as a pure function, the palm window, and `MultiTap` (two-finger tap = undo, three-finger tap = redo).
- `send.ts` contains `planSend`, `commitSend` and the caption geometry:
  - Page captions use the describe-point answer nearest the mark's centre.
  - Excerpt captions are `lines A–B`.
- `lasso.ts`: a freeform lasso over cards, frames, links, visible ink and marks.
- `compose.ts` (`composeImage`) draws each card's stored snapshot, or a labelled box, then the vector ink over it.
- `Canvas.tsx` is the lazy default export: `forwardRef<WhiteboardCanvasHandle, WhiteboardCanvasProps>`. It implements every handle method. Its input layer is a capture-phase pointer/touch/mouse filter that does:
  - pen and stylus filtering before d3-zoom
  - pointer ink with coalesced and predicted events
  - the whole-stroke eraser, palm rule and multi-finger taps
  - rubber-band frames, the lasso, and long-press Pin/Unpin on a mark
  - the connect-end menu ("New sticky linked here" / Cancel)
  - keyboard handling
  - snapshot scheduling, Follow (reacts to `meta.focus` when `follow` is on), the attribution panel, and `anchored`
- `CardNode.tsx` is the card chrome:
  - title bar, stacked edge, `vK of N` scrubber, dimmed state with a tag chip, placeholder shimmer
  - selected state (outline, resizer, toolbar with Open, Dim with a reason, Pin live, Archive), entered state, shield
  - double-tap enters, and a per-card error boundary keeps one failing card from taking down the canvas
- The rest of the UI:
  - `FrameNode.tsx`: rename, colour, delete, resize; the frame is grabbed by its title.
  - `LinkEdge.tsx`: label, arrow cycle, flip, dashed, delete.
  - `InkLayer.tsx`: free ink in a `ViewportPortal` above the cards, anchored ink inside the card scaled by `cs`, and "pinned" labels.
  - `Overlays.tsx`: lasso path, lassoed-ink halo, multi-selection bar, off-screen placeholder chevrons.
  - `context.ts` and `canvas.css` (all colours are theme tokens or light/dark pairs).
- `cards/PageCard.tsx`: the 120 sandboxed frame at the device viewport, CSS-scaled, themed, and registered for describe-point.
- `cards/bodies.tsx`: renderers for the other kinds (including session turns and artifact/path/blob images), plus `Chip`, `Snapshot` and `CardBoundary`.
- `turns.ts`: the session card's rows.
- `penRelay.ts`: the pen relay for entered pages (deviation 8).
- `fixtures/`:
  - `example.ts` builds the §2.10 doc. `example-v1.update.b64` is written and compared by `model.test.ts` with clientID 1, so the bytes are stable.
  - `generate.ts` generates 100 cards and 2 000 strokes (seeded).
  - `testing.ts` holds board builders for the tests.

`ink/`
- `pack.ts`: `pack`/`unpack` per §2.6, RDP, the 4 000-point split, and `bboxOf`.
- `stroke.ts`: captured points → `Stroke`s (RDP at the capture zoom, anchor, card-local coordinates, split, pack), and `strokeValue`.
- `anchor.ts`: the 60 % length rule (Liang–Barsky clipping), riders, `shifted`.
- `marks.ts`: single-link clustering (48 px or 4 s) and `freshStrokes`.
- `life.ts`: opacity per life, version-aware.
- `colour.ts`: `inkCss`, the ≥ 3 : 1 adjustment for free colours, and `nearestInk` (for B4's native bridge).
- `render.ts`: perfect-freehand options per §8.6, `strokePath` (cached per `pts` buffer), `pointsPath`.

## Deviations from the contract (with reasons)

1. **Cheap kinds go live from 60 px, not 240 px.** This covers `sticky`, `excerpt`, `diagram` and `image` (`lod.ts` `CHEAP`). Under the 240 px rule, a 220 px sticky at zoom 1 rendered as a chip. Seen in the browser; it was clearly wrong. Pages, terminals, notes, diffs and sessions keep 240 px and the budget.
2. **A terminal card never shows a live xterm on the canvas.** It is an info card (title, job, "open to watch it live"). `Terminal` resizes the agent's PTY to fit its container (`pty_resize`, then SIGWINCH), so a scaled card would reshape the job. The live view belongs in B4's sheet.
3. **Excerpt cards use a small renderer of their own:** monospace, numbered rows of a fixed 18 px (`send.ts` `EXCERPT_ROW`). The chat `Excerpt`'s row geometry is private and capped at 18 rows, and the `lines A–B` caption needs exact rows. The full `Excerpt` belongs in the sheet.
4. **Session cards show the session's last turns,** read-only: up to 6 user/assistant messages, each cut to 280 characters.
   - The source is the live session store when that session is loaded, else one `load_session(id, null, 40)` page.
   - **Image cards** load `blob` keys through `host.blobUrl`, `path`s through the chat's file cache (`useFileUrl`: `asset://` locally, RPC bytes otherwise, private owners respected), and `{session, call_id}` artifacts through the artifact list, then their path.
5. **Snapshots capture the card body only,** the area below the 30 px title bar. `wb_crop` of a card therefore returns body pixels.
6. **The light green and amber ink colours differ from the contract.** The contract's `#2f9e44` (2.54 : 1) and `#b7791f` (2.69 : 1) sat below 3 : 1 on bone `#e8dcc8`, which design A1 forbids.
   - They are now `#2a8e3d` (3.08 : 1) and `#aa701d` (3.07 : 1): the same hue, with lightness moved just far enough. `readable()` of the contract values gives exactly these.
   - Frame colours and the pinned label use the same light/dark pairs (`--wb-*` in `canvas.css`).
   - Frame borders are now drawn at full strength so they keep ≥ 3 : 1; before, they were mixed at 55 %.
   - Every named colour is now ≥ 3 : 1 on its canvas in both themes (`colour.test.ts`).
7. **The Pencil does not ink on controls.** On buttons, inputs, toolbars and the scrubber (`CONTROL` in `Canvas.tsx`) it taps them. On an iPad the Pencil is the main pointer, so toolbars must work with it.
8. **The Pencil over an entered live page now inks (pointer path), through a relay inside the page** (`canvas/penRelay.ts`).
   - The frame is sandboxed with an opaque origin, so the host cannot filter its events. Instead, whiteboard page cards (not chat pages) carry a small script, added by `withPenRelay` after `injectBootstrap`.
   - While the canvas switches capture on — card entered, pen route `ink`/`erase`, native ink not drawing — the script takes `pointerType === 'pen'` events only: `preventDefault`, then post the points to the host. It also suppresses pen clicks and stylus touch scrolling.
   - Fingers and mouse still scroll and tap the page. In link mode, or with capture off, the Pencil taps the page again.
   - The host accepts messages only from that frame's window, refuses points outside the viewport (+64 px) or more than 256 points per message, then maps frame px → card → world and commits through the normal stroke path (anchoring, captions, undo).
   - Trust cost: a hostile page can post fake pen points, but only while its card is entered and the pen tool is active, and only inside its own card.
   - Native ink (P3) draws above the web view and never needed this.
9. **Frames are grabbed by their title.** The interior passes through to the canvas, so pan, lasso, ink and frame drawing all work inside frames. Otherwise a big frame made lassoing or drawing inside it drag the frame.
10. **Keyboard shortcuts are split between the shell and the canvas.**
    - B4's `WhiteboardView` already owns V P E L F N O, ⌘0 and ⌘↵, and Esc for an entered card, the composer and the cluster.
    - The canvas handles only ⌘Z / ⇧⌘Z / ⌘Y, Enter (enter the selected card), Delete/Backspace (archive cards, remove links and frames, erase lassoed ink), and Esc for its own modes and selection.
    - Handling the same keys in both places would toggle a mode twice.
11. **Lassoed fresh strokes are held in canvas state (`inkSel`), not in `store.selection`.** Fresh ink has no mark id, so it has nothing to put in `selection`. `store.selection` does carry the marks of lassoed ink.
    - The canvas shows its own multi-selection bar at the top centre: Dim with a reason, Pin/Unpin marks, Archive/Erase, Clear.
12. **Additions beyond the build list:**
    - card resize
    - per-card error boundary
    - off-screen chevrons for placeholders (design §4.5)
    - links without ports attach on the sides the cards face each other
    - a new frame adopts every card whose centre it encloses
    - one fit when the first cards arrive after mount
13. **The accent is the theme's `--fg-accent` (ochre), not the mock's blue.** In this codebase `--accent` is shadcn's `bg-layer-2`.
14. **`PageCard` uses its own copy of `CardSheet`'s artifact lookup.** It follows the same pattern as B4's `CardSheet.PageBody`: `useSessions` cache, then `list_artifacts`, then `useText`. The few lines live in `cards/PageCard.tsx` (§8 rule: copy, don't import).
15. **Snapshot "again before demotion":** a card about to leave `live` with a stale snapshot is held live until one capture attempt finishes, then demoted.

## Checks run (all in `crates/desktop/ui`)

- `bun install` exited 0. `perfect-freehand@1.2.3` was added with `bun add --exact`.
- `bun run check` exited 0. Zero `error TS` lines in the whole project at the last run; earlier runs showed only B4's files.
- `bun x vitest run src/components/whiteboard/canvas src/components/whiteboard/ink` exited 0: **15 files, 82 tests, all pass.**

| File | Tests | What they cover |
|---|---|---|
| `ink/pack.test.ts` | 6 | golden vector, round trip, round half away from zero, long step interpolated, 4 000-point split, RDP |
| `ink/anchor.test.ts` | 6 | 60 % rule, z tie-break and archived cards, card-local storage, ride-with-one-card, stay-between-two |
| `ink/marks.test.ts` | 7 | 48 px link, chaining, 4 s, anchors never mix, off-screen excluded, other version not offered, mark box |
| `ink/life.test.ts` | 6 | — |
| `ink/colour.test.ts` | 4 | theme flip, contrast |
| `canvas/model.test.ts` | 3 | §2.10 through a `Y.Doc` and back, `example-v1.update.b64`, tolerance |
| `canvas/ops.test.ts` | 11 | origins, undo vs `engine`, `chunked` ≤ 60 writes and ≈ 30 kB, ≤ 48 KiB base64 per transaction for a 9 000-point stroke, bulk fixture, frame move, pin |
| `canvas/rows.test.ts` | 2 | — |
| `canvas/binding.test.ts` | 8 | frames as parents, absolute ↔ relative, write on drag end only, riders, links, facing ports |
| `canvas/lod.test.ts` | 6 | thresholds, cheap kinds, reach, budget 4 and 8, 30 s demotion, pin |
| `canvas/input.test.ts` | 9 | — |
| `canvas/send.test.ts` | 6 | — |
| `canvas/lasso.test.ts` | 2 | extra |
| `canvas/turns.test.ts` | 2 | extra (session card rows) |
| `canvas/penRelay.test.ts` | 3 | extra: validation against a hostile page, and the relay script parses |

**Follow-up round: pen relay checked in headless Chrome.** The relay script ran in a sandboxed `srcdoc` iframe, with Pencil events sent through CDP `pointerType: 'pen'`.
- With capture off, a pen tap clicked the page's button.
- With capture on, a pen stroke arrived at the host as `down`, 5 × `move`, `up`, with pressure 0.6. The pen's click was suppressed.
- A mouse click still reached the page.

**Visual check in headless Chrome.** I used a throwaway harness, since deleted. It ran Vite with no engine and loaded the fixture whiteboard, in light and dark.

Things that worked:
- stickies, excerpts, frames, links with labels
- dimmed card with its tag, `v2 of 2` scrubber, placeholder shimmer, off-screen chevron
- mouse ink, free and anchored
- lasso → "1 stroke" bar → erase → ⌘Z restores it
- drawing a frame, and its toolbar
- handle drag onto empty canvas → "New sticky linked here", which opens the new sticky ready to type
- "React Flow" attribution shown while `clusterOpen`

Bugs this found and fixed:
- The wet `<canvas>` grew with every redraw (a replaced element ignores `inset: 0`), covering the screen.
- The `--accent` token was wrong.
- Frames swallowed drags.
- Stickies rendered as chips.
- A mermaid card's `useApi` throw took down the whole canvas. A real engine connection avoids this; the boundary now contains it.

**Performance on the 2 000-stroke fixture:**
- `commitCaptured` of one stroke took 114 ms, including the two frames it waits for. That was measured before ink was indexed by card, and not re-measured.
- A pan took 57 ms.

## Not verified / not built

- **Not verified:**
  - Anything that needs an iPad: real touch, the Pencil, the palm rule, stylus `touchType` filtering, multi-finger taps.
  - Native capture snapshots (`host.capture`), describe-point captions against real pages, `composeImage` output, Follow/spotlight, and pointer ink on WebKit.
  - Page, session and artifact-image cards: they need an engine connection and were not loaded in the browser.
  - The pen relay inside a real page card: Tauri `theuth-page://` and WebKit/iPadOS. Its page-side script was checked in Chrome on its own (follow-up section).
- **Not built:**
  - live terminals on the canvas (deviation 2)
  - "Ask about selection" (it belongs to the shell; the selection is in the store)

## For the other builders and the orchestrator

- **B1:** `canvas/fixtures/example-v1.update.b64` is the §2.10 document as a Yjs v1 update (clientID 1, one transaction), for `reads_the_yjs_fixture`.
- **B4 — things the canvas already handles:**
  - Pointer ink sets `penDown`; with native ink, a swallowed pen sets it too.
  - `onCanvasTap` fires only for a finger or mouse tap on empty canvas, never right after a stroke, lasso or frame drag.
  - The canvas never writes `mode` except to leave link/select/frame mode (Esc, empty-canvas tap in link mode, after a frame is drawn).
- **B4 — things the shell must know:**
  - `host.capture` returning `null` once latches "no capture" for that mount.
  - `freeSpot` avoids cards (16 px padding) and ink.
- **B4 — palette copies:** `native-ink.ts` (lines 16–17) still has the contract's light green `#2f9e44` and amber `#b7791f`, and `glass.css:50` uses `#2f9e44`.
  - Import `INK_COLOURS` / `inkCss` from `ink/colour.ts` so the native picker gets the ≥ 3 : 1 values.
  - `nearestInk` still maps the old hexes to `green` / `amber` (RGB distance under 40).
- **Orchestrator:** B3's tree has no `218-TEMP(wire)` cast and does not import `wire.ts`.

## Fix round (review M1)

The problem: `Y.UndoManager` merged the chunks of a bulk write into one stack item and replayed it as one transaction. Undoing a lasso erase of heavy ink therefore produced one update over the 48 KiB cap, and the provider closed.

The fix:
- **`undo.ts`:**
  - `captureTimeout: 0`, so stack items never merge.
  - Each chunk carries a `wb-group` id in its transaction meta.
  - `createUndo` now returns a wrapper (`undo`/`redo`/`canUndo`/`canRedo`/`stopCapturing`/`destroy`/`manager`). Its `undo()`/`redo()` replay every stack item of the top group, one transaction each: one press for the owner, every update under the cap.
- **`ops.ts`:**
  - `chunked` tags all its chunks with one group.
  - It takes an optional `inverse` byte estimate per op, because a delete's own update is tiny but its undo re-inserts the content. `eraseStrokes` passes `pts.length + 200` per stroke, so chunks are sized for the undo too.
  - `addStrokes` groups the parts of one split stroke.
  - `removeLink` joins a held group.
  - New `holdGroup()` makes several writes one undo step: one eraser drag (pointer path and page relay), and the lasso Delete (archive + remove + erase).
  - Before, those steps merged only through the 500 ms capture window.

Tests (`canvas/ops.test.ts`):
- **`review M1: undo and redo of a bulk erase …`:** 60 strokes × 4 000 points (≈ 1.4 MB) are erased, undone and redone, then undone again. Each undo/redo is one press, the replay spans more than one update, and every update is ≤ 48 KiB base64.
- **`an eraser drag (a held group) is one undo step`.**

Checks:
- `bun x vitest run src/components/whiteboard/canvas src/components/whiteboard/ink` exited 0: 15 files, 84 tests, all pass.
- `bun run check` shows no B3 lines.

Not re-verified in a browser.
