# 218 as built: whiteboard chat cards, free resize, bare diagrams, the whiteboard dash

Four owner requests on branch `fix/218-cards`, one commit each.

## 1. Creating or showing a whiteboard leaves a chat card

- `wb_create` (`crates/document/src/wb/tools.rs`, `created_receipt`) and `show_whiteboard`
  (`crates/artifact/src/tileview.rs`, `whiteboard_receipt`) return receipts the transcript keeps across a reload:
  `created whiteboard "<title>" <doc> — …` / `opened whiteboard "<title>"`, then `theuth://whiteboards/<doc>`, then
  `N cards` (`show_whiteboard` reads it with `WbInfo`; a count it cannot read leaves the line out). The structured
  values carry `cards` too.
- `chat/WhiteboardViewCard.tsx` reads them (`whiteboardCall`): title, `New whiteboard · empty` / `Whiteboard · 3
  cards`, **Open** (the whiteboard tile beside the chat; with no tile host, `/whiteboards/<id>?session=`), pop out.
  `MessageTool` mounts it for both tools; `ToolRunCard`'s `DELIVERABLE_TOOLS` keeps it out of folded runs.

## 2. Cards resize freely

- Cause: `CardNode`/`FrameNode` passed `NodeResizer` an inline `onResizeEnd`. React Flow's resize control re-binds its
  d3 drag whenever that callback changes, and every resize step re-renders the node. A touch (finger or Pencil) drag
  listens on the handle element itself, so it died after its first step; the mouse survived because d3 listens on
  `window` for it. `canvas/resize.ts` `useResizeEnd` gives both a stable callback.
- No upper size anywhere: the resizer had no max; `wb_place` clamped `w`/`h` to 40–4000 and now keeps any size ≥ 40.
- Handles: 11 px with a 31 px hit area; on coarse pointers 16 px with a 44 px hit area (`canvas.css`).
- Content follows the size: stickies and notes reflow, images `object-contain`, diagrams fill (below), page cards scale
  their device viewport to the card (unchanged: a preset device keeps its aspect).

## 3. Diagram cards are just the diagram

- `chat/surface/bare.ts` `BareSurface`: the whiteboard's diagram body renders the chat renderers bare. json-render
  unwraps a root `Card`/`Stack` that only wraps (a `Card` of several children becomes a plain `Stack`); `DiagramFrame`
  drops its border, `graph`/`mermaid` label and inline height and fills; openui drops its margins. The transcript never
  sets it.
- `CardNode` draws a diagram card without its title bar (`send.ts` `isBare`/`headerOf`); its title is a caption chip
  on hover or selection. Snapshot captures and the composed export use `headerOf`.

## 4. The whiteboard dash (W1)

- `components/whiteboard/useSessionWhiteboards.ts`: the chat's whiteboards — those its transcript created or showed,
  then those `session_whiteboards` has it bound to — behind the whiteboard requirements.
- `useTray` carries them; `BackgroundDashes` draws one W1 dash each (nile, a dot grid in `--background`, 16 px) after
  the browsers; `TrayTiles` has a **Whiteboard** tile (titles, count): one opens directly, several list.

Dev fixture: `crates/desktop/ui/src/dev/whiteboard-cards-fixture.html` (`?big=1`, `?chat=1`, `?tray=1`, `?theme=dark`).
