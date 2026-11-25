# 218 — as built: Hand, the session composer on the whiteboard, Pencil hover

Branch `feat/218-composer`, off `main` at `734b8b7de` (v2.30.3). Three owner requests, one commit each.

## Hand

- `canvas/store.ts` `Mode` gains `hand`; `canvas/input.ts` `penPoints(mode)`: in `hand` (and `link`) the Pencil is a pointer — `routePointer('pen')` passes, the stylus touch filter in `Canvas.tsx` lets d3-zoom have it, and a Pencil tap on the pane is a tap (deselect, cluster). Finger and mouse pass in `hand` as in `idle`.
- The ink panel (`Cluster.tsx` `InkPanel`: Hand · Pen · Fine liner · Marker · Eraser, colours, Thin/Medium/Thick) is shared by the cluster's ink button and the composer's tools button; both show the active tool (`toolFace`), Hand included. `H` picks Hand.
- Esc from an ink tool goes to `hand` (it went to `idle`, where the Pencil still inks). The latch rule is unchanged otherwise.

## The session composer on the whiteboard

- `Pill.tsx` and `Sheet.tsx` are deleted, with `composerOpen` (store). `WhiteboardComposer.tsx` docks the chat `Composer` bottom right (`.wb-dock`: ≤ 390 px, safe areas; the frame is the visible band, so it sits above the software keyboard). `--wb-dock` (its top edge, measured) places the cluster, the reply bubble and the approval card above it.
- `Composer` gains optional props, unused by every other host: `lead` (toolbar start: the tools button), `deliver` (the host sends a plain message and answers the session; no navigation), `draftScope` (draft key prefix: `wb:<doc>:`), `carries` (an empty box can send: fresh marks). Commands, `/skill:` and parked skills keep the composer's own paths (parked skills therefore go without the whiteboard's images and context).
- Above the box: the session picker (`SessionChip`: New session, this whiteboard's sessions, recent others, Open in chat, Detach) and the agent's status line; then the tray (thumbnails, each removable; the off-screen opt-in; "N images · M marks" or "captions only"). The plan is re-planned 200 ms after a board or view change; removals are kept by `imageKey` until the send.
- `deliver` is `sendWhiteboard` (unchanged order: images → handwriting → mark blobs → attachments → create session → bind → context → send/steer → commit marks); `SendInput.files` carries the composer's own attachments first. "New session": the first send creates the session with the composer's model/effort pick only after the images and blobs succeeded, binds it, sends, and makes it the target.
- Menus over the canvas (tools, sessions) close on a canvas press (`useCloseOnCanvas`): d3-zoom stops the canvas mousedown, which Radix reads as handled. The composer's own popovers (model, `+`) still need a tap elsewhere or Esc there.
- Dev fixture: `src/dev/whiteboard-composer-fixture.html` (the real shell on a local Y.Doc, stub engine).

## Pencil hover preview

- `canvas/hover.ts` `hoverLook`: the tool's colour at its drawn width × zoom (marker translucent), the eraser's 12 px circle (`ERASER_PX`, also `eraseAt`'s reach), nothing where the pointer would not ink (Hand, link, a finger, the mouse without an ink tool). WebKit gives no hover height, so the dot does not fade.
- `Canvas.tsx`: pen moves with no button down (and mouse moves) set one transform per frame on `.wb-hover` (no pointer events, never ink); hidden on leave, pen-down, a tool change to nothing, and 200 ms without a Pencil hover move. The mouse keeps its crosshair; its eraser shows only the existing eraser cursor.

## Tests

Added: `pen-latch-canvas.test.ts` Hand round trip for the Pencil and the mouse (pen draws → Hand: drags reach React Flow and draw nothing → pen draws), `canvas/input.test.ts` Hand routing, `composer-dock.test.ts` (the chat Composer mounts with the tools button and the picker and no pill/sheet; the tools menu has Hand and the cluster actions; a picked session takes the send with the marker and the context before it; New session creates with the pick, binds, sends, targets), `hover-preview.test.ts` (position under pan and zoom, size by tool, pen-down hides and the stroke holds no hover point, leave and idle hide, Hand none, eraser ring, mouse). No pill/sheet tests existed.

## Only a real iPad can prove

1. Hand: a Pencil drag pans and drags cards; a Pencil tap selects; no stroke.
2. Hover: the dot tracks the Pencil tip (Apple Pencil 2 / Pro, M2+ iPad) and vanishes on lift-away and pen-down.
3. Dictation and the voice-note hold in the whiteboard composer (the same component as chat).
4. The composer above the software keyboard with a real keyboard and in Stage Manager windows.
