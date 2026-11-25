# 218 as-built — B4 (shell)

Builder B4, night of 2026-11-25. `UI` = `crates/desktop/ui/src`. Two sub-builders took self-contained slices (sidebar/routes; attach/describe-point/tiles); their work is in the commits below and was reviewed and adjusted here.

## Commits

| sha | subject |
|---|---|
| 3109b34f3 | seam — wire.ts, ink-native.ts, wb-capture.ts |
| d4a570772 | whiteboards list, routes, nav tab and sidebar row |
| 836b2d2ef | fix whiteboards popover test |
| 34714c70d | send budget and pipeline, cluster/bubble timers, native ink bridge, canvas host, glass, outline view |
| 8c96b762b | whiteboard attach kind, describe-point, tile + arrival |
| 93fe28265 | WhiteboardView shell — pill, session chip, sheet with model picker, cluster, bubble, card sheet, add, outline; tests |
| 7027e5d96 | hide whiteboard context blocks in chat, no arrival from a hidden chat, stable tray thumbnails, theme re-push to native ink |
| 5c02bf2f0 | Reduce Motion cuts the outline's focus animation |

## What was built (files)

- **Seam (§4.4, §6, §7):** `components/whiteboard/wire.ts` (verbatim), `lib/ink-native.ts` (full implementation, not a stub: dynamic `@tauri-apps/api/core`, `Channel`s, never throws, `loader` seam for tests), `lib/wb-capture.ts` (+ `base64Blob`).
- **Shell:** `WhiteboardView.tsx` (full window `fixed inset-x-0 top-0 h-band z-50` over sidebar and status bar, or `relative` in a tile; `useDocument({doc})`, store, `CanvasHost`, lazy `canvas/Canvas` in `<Suspense>`, the R1/R12 session target, notices as the pill's status line, cluster timer mirrored into `store.clusterOpen`, native ink, composer, reply bubble, card sheet, outline, Add, ⋯ menu, keyboard `V P E L F N O ⌘0 ⌘↵ Esc`, bottom-right-quadrant hover raises the cluster on desktop). `Pill.tsx` (grabber, `SessionChip` = dot + short name + link glyph / "no session"; picker: bound sessions, 8 recent others, *New session with this whiteboard*, *Detach*), `Sheet.tsx` (field, tray with removable thumbnails via `handle.composeImage(…,160)`, off-screen opt-in, no-vision line, `SessionChip`, `<ComposerModel session own />` (A2), Send/Steer/"Send to a new session", Open session), `Cluster.tsx` (mock order; ink tools popover with pen/fine liner/marker/eraser and the six theme colours; Add popover; Link/Frame/Select modes; Fit; Outline; Undo/Redo; ⋯), `Bubble.tsx`, `CardSheet.tsx` (note, page via artifact `HtmlPage`, diagram fences, excerpt, diff, terminal, web, image, sticky, session), `OutlineView.tsx` (+ `outlineText` for *Copy outline*), `add.ts`, `host.ts`, `glass.css` (A1 light/dark, Reduce Motion cuts).
- **Logic (pure, tested):** `send/budget.ts`, `send/send.ts` (§8.8 order), `cluster-timer.ts`, `reply.ts` (bubble anchor/collapse/last reply), `native-ink.ts` (bridge + `connectNativeInk`).
- **Routes/nav/sidebar (sub-builder):** `routes/whiteboards.tsx`, `routes/whiteboards_.$id.tsx` (`?session=`), `routeTree.gen.ts` (router-generator), `WhiteboardsList.tsx`, `useWhiteboards.ts`, `requirements.ts`, `lib/nav.ts` (Library tab), `sidebar/Whiteboards.tsx` (+ test) and the Whiteboards row + rail icon in `AppSidebar.tsx`.
- **Attach/bridge/tiles (sub-builder):** `lib/attach.ts` (`whiteboard` kind, `whiteboardRef`), `attach.test.ts`, `lib/page-bootstrap.ts` (`theuth/describe-point` page + host side, `sanitizeDescription`, 300 ms timeout), `page-bootstrap.test.ts`, `tiles/{open.ts,registry.tsx,kinds/whiteboard.tsx}`, `lib/tile-arrival.ts` (+ test), the `App.tsx` `on(…)` line. Exhaustiveness arms: `components/chat/AttachChip.tsx` (icon, open → `/whiteboards/$id`), `components/tasks/{NewTaskSheet,TaskDetail}.tsx` (one `218-TEMP(wire)` cast each).
- **`lib/injected.ts`:** `HIDDEN.whiteboard` / `HIDDEN.whiteboard_changes` (B2's new blocks made `injected-wrappers.test.ts` fail; they are volatile context like `<notebook>`).

## Named tests (vitest) — all pass

`lib/attach.test.ts` ✓ · `lib/page-bootstrap.test.ts` ✓ · `lib/ink-native.test.ts` ✓ · `lib/tile-arrival.test.ts` ✓ · `components/whiteboard/native-ink.test.ts` ✓ · `send/budget.test.ts` ✓ · `send/send.test.ts` ✓ · `cluster.test.ts` ✓ · `bubble.test.ts` ✓ · `outline-view.test.ts` ✓ · plus `sidebar/Whiteboards.test.ts` ✓.

## Checks run

- `bun run check` → exit 0 (whole UI, after B3's and B2's commits through fac0bab26).
- `bunx vitest run src/components/whiteboard src/lib --exclude canvas/** --exclude ink/**` → 134/135 files, 1018/1019 tests; the one failure, `lib/doc/provider.test.ts › a long offline queue replays…`, is a timing flake under load (nobody touched `lib/doc`): 3 isolated reruns 28/28 pass.
- `bunx vitest run src/components/sidebar` → green (sub-builder).

## Deviations and decisions

1. **File names:** the pure cluster/bubble modules are `cluster-timer.ts` and `reply.ts`, not `cluster.ts`/`bubble.ts`: on the case-insensitive macOS FS they collide with `Cluster.tsx`/`Bubble.tsx` (tsc TS1149). Test file names are the contract's.
2. **`whiteboardViewOpened(e, visible, go)`** takes a third `go(href)` argument (the router is private to `App.tsx`). A chat **not on screen moves nobody** (no tile, no navigation) — navigating the user away from whatever they are doing because a background agent opened a whiteboard is hostile; on screen: tile when tileable, else navigate to `/whiteboards/<id>?session=`.
3. **Handwriting (R6)** is recognised only for an image of exactly one free mark: a card's image contains the page's text, which OCR would return as "handwriting". Confidence < 0.5 is dropped; text is sanitised (≤ 200, no newlines/brackets).
4. **Send:** mark blobs are stored even on the no-vision path (they are the pixel record `wb_crop` reads later); `omitted` = all images then. No optimistic `markSent` (the transcript is not on screen; the engine echo fills it).
5. **Budget:** "scale together" has a legibility floor (long side ≥ 384 px); merge-per-frame yields a `card: null` image of the frame's union; a last resort keeps the first six and scales without the floor.
6. **Add:** sticky, note (`doc_create`), web (URL dialog), image (file → `img/<ulid>` blob), clipboard (image → image card, link → web, text → sticky), artifact (session's HTML/image artifacts), diff (`{ mode: 'worktree' }`, `as_of` now; no `frozen` — a client cannot freeze), terminal (session's terminals). **File excerpt is not built** (needs a file/range picker and a file-read RPC).
7. **Phone:** outline view is the default and the canvas is `readOnly` (contract); the composer and card sheet work there.
8. Picking a session in the chip **binds immediately** (`whiteboard_bind`), so the agent sees the whiteboard on its next turn even before a send. *Detach* unbinds.
9. React Flow credit: a link in ⋯ (B3 renders the attribution panel while `clusterOpen`, R3).

## Not built / unverified

- Not built: file excerpt in Add; hold-to-talk mic (stretch; mic opens the sheet focused for system dictation); Follow toggle and edge chevrons (stretch P4); *Add to whiteboard…* in chat (stretch); export beyond *Copy outline* (P5).
- Unverified (no runtime tonight): nothing was rendered in a browser or simulator — layout of pill/cluster/sheet/bubble in light and dark, the keyboard-riding sheet on iPad, native ink end to end (needs B5's plugin and a Pencil), `wb_capture` sizing (the `width` argument is computed as points so the PNG's long side ≈ `maxSide` px).

## For the orchestrator and other builders

- Every new RPC goes through `wbRpc(client)` in `wire.ts`; casts are marked `// 218-TEMP(wire)` (`requirements.ts`, `useWhiteboards.ts`, `App.tsx`, `tasks/*`, `AttachChip`?). `grep -rn "218-TEMP" crates/desktop/ui/src` lists them.
- B3: the shell relies on `handle.freeSpot`, `planSend`, `composeImage`, `commitSend`, `commitCaptured` (must resolve after paint, and **reject** when it cannot commit so nothing is acked), `frameRect`, `focus`, `fit`, `undo/redo/canUndo/canRedo`, and `onViewChange(…,'end')` once after mount (the native viewport). `canUndo()` is read at render time; the cluster re-renders on board changes.
- B5: JS calls `plugin:ink|attach|set_enabled|set_viewport|set_tool|set_picker_visible|ack|recognize_text|detach` with exactly the §6 argument names; `set_viewport` gets web-view coordinates (`view + frameRect.left/top`).
