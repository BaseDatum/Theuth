# 218 — Whiteboard: a full-screen thinking table for iPad, Pencil and the agent

Status: design, 2026-11-24, for Sky's rulings before code. Mocks: `218-mock.html` (four states of one iPad screen). Review: `218-review-fable.md` (Claude Fable 5.2, native theuth subagent).
Extends 59 (documents: one store, Yjs, `doc_*` tools), 66 (attachments: a typed reference on a message), 71 (boards — a *different* thing, see §0.1), 120 (inline pages: sandboxed frames), 198 (Liquid Glass: native Swift chrome over the WKWebView via `crates/tauri-plugin-glass`), 205 (web view tile), 211 (`show_note`/`show_pty` tiles), 212 (`show_file`/`show_diff`).

## What and why

Linear chat is the right shape for coding, debugging and running things. It is the wrong shape for **design work that branches**: comparing variants, iterating a layout or an architecture, keeping requirements in view while a mock changes. In chat, every new picture scrolls the last one away, and the owner ends up saying "take the header from two messages ago and the list from five messages ago" in words.

A whiteboard is an independent, infinite, full-screen canvas where the things the harness already renders — mocks (pages), diagrams (json-render, mermaid), notes, code excerpts, diffs, terminals, web views, images — live as **cards that stay where they are put**, next to each other, linked by explicit arrows, marked up with Apple Pencil, while the owner talks to an agent through one translucent composer in the bottom-right corner. Nothing scrolls away. Rejected options stay visible, dimmed, with the reason. At the end the board *is* the spec.

Non-goals: a general drawing app, a Miro clone, real-time multi-human collaboration (the Yjs substrate allows it later; v1 is one human plus agents), replacing chat for linear work.

**The productivity test** this design has to pass (and §9 measures): a design session of ~20 minutes with ≥3 variants should need **zero scroll-back and zero "the one from before" descriptions**, and should end with an artefact a worker can be briefed from without re-explaining.

## 0. Decisions in one screen (each a default the rest of the doc assumes; Sky overrides)

1. **Name and kind.** The feature is **Whiteboard**; the document kind is `whiteboard`. It is *not* design 71's "board" (a scrolling, document-flow code explanation) and not the tasks board. UI copy never shortens it to "board".
2. **Independent, attachable.** A whiteboard is a first-class document like a note: it lives in the document store, appears in the sidebar under **Whiteboards**, and is linked to any number of sessions the same way a note is attached (66): `[attached whiteboard <id> "<title>" · N cards]`. It is never owned by a session.
3. **Full screen, chrome on demand.** Opening a whiteboard takes the whole window (iPad: whole screen, status bar hidden). The **only** always-visible control is the **translucent glass composer pill, bottom-right**. A **finger tap on empty canvas** toggles the control cluster above it; it hides again after 4 s without finger input, on any Pencil stroke, or on a second tap.
4. **Pencil draws, fingers navigate.** Apple's own convention (PencilKit `drawingPolicy = .pencilOnly` when a Pencil has been seen): Pencil = ink, always, and never summons chrome; one finger = pan / select / tap, two fingers = zoom, two-finger tap = undo, three-finger tap = redo. A "draw with finger" toggle exists for no-Pencil devices and the Mac.
5. **Links are explicit, ink is never interpreted.** A link (an arrow the agent and the data model understand) is made only by an explicit gesture: dragging from a card's edge handle to another card, the link tool, or the agent. A hand-drawn line is ink: it stays a picture, and its meaning comes from what the owner *says* when sending. No stroke classifier — a guessed arrow that guesses wrong is worse than none.
6. **The agent reads structure, sees pixels only for fresh ink.** The board reaches the agent as a structured outline plus a change log since its last read. Pictures are sent only as tight crops of *fresh ink* in the message being sent (§6) — never a screenshot per change.
7. **Stack: React Flow + Perfect Freehand + native PencilKit + Yjs.** `@xyflow/react` 13.0.3 (MIT) for the infinite viewport, cards, handles and edges; `perfect-freehand` 1.2.3 (MIT) for pressure-shaped ink rendering everywhere and ink capture off-iPad; **PencilKit** (`PKCanvasView` + `PKToolPicker`) in a new Swift Tauri plugin for ink capture on iPad; `yjs` 13.6.33 (MIT, already the notes substrate). tldraw 5.6.2 rejected: proprietary licence, production licence key, watermark. Excalidraw 0.18.1 (MIT) rejected: a drawing-first scene model that would fight live cards. (Versions and licences read from registry.npmjs.org on 2026-11-24.)
8. **Agent replies land on the board, not in a transcript.** A text reply shows as a transient glass bubble above the composer (fades after reading time; tap to pin, swipe up for the full session). Anything the agent *makes* — a mock, a diagram, a diff — is placed as a card next to what it answers.

## 1. The experience (the reason this exists)

### 1.1 Couch, iPad, a new screen to design

1. Sky opens **Whiteboards → New** on the iPad. Full-screen paper-coloured canvas, the glass pill bottom-right reading *"Ask… · no session"*. He taps the pill, picks the session chip → *"New session"* (or an existing one), says: *"Let's design the attention inbox for the phone. Put up what we have today and two directions."*
2. The agent attaches the whiteboard to its session, places a **screenshot card** of today's screen, two **page cards** (mock A: grouped by source; mock B: one flat list with swipe actions) side by side, and a **requirements sticky** with three bullets it took from the conversation. Each card fades in where it lands; the viewport eases to fit them (only because Sky tapped *Follow* — §4.4).
3. Sky pans with one finger, pinches into mock B. With the Pencil he circles mock A's header, scribbles a box low on mock B where a filter row should go, and crosses out mock B's third tab. No chrome appeared; nothing was sent.
4. He taps the pill and says *"Keep A's header. Filters go where I drew the box. Drop that tab."* The pill shows **3 marks attached** (thumbnails of the three ink clusters). Send.
5. The agent receives one message: his words, the attachment marker, a change packet (*3 ink clusters: on card mockA region "header" · on card mockB region below list · on card mockB tab bar*), and three tight crops. It places **mock C** to the right of B, links B → C with a labelled arrow *"v3: A header + filters"*, and dims A with a tag *"header kept, grouping rejected — too many sections on phone"*. Its text reply floats above the pill for 6 s.
6. Twenty minutes later there are five variants in a row, the rejected ones dimmed with reasons, a state diagram the agent added under the winner, and Sky's ink everywhere. He says *"Write the build brief from this."* The agent writes a note from the board (links back to every card) and places it as a card in the corner.

### 1.2 Later, at the Mac

The same whiteboard opens on the Mac (sidebar → Whiteboards). Ink renders through Perfect Freehand from the same stroke points. Sky drags the winning mock next to a **diff card** a worker produced, hovers, sees the diff's anchors light the mock regions. The board is the context he hands the next session: one attachment, not a recap.

### 1.3 What it is not good for (and so has no affordance for)

Running a build, reading a long log, chatting about a bug. The composer's chevron hands the session back to the normal chat view in one tap.

## 2. Map

```
iPad                                    every client (Mac, web, iPad, phone read-only)
┌──────────────────────────────┐        ┌────────────────────────────────────────┐
│ PencilKit overlay (Swift)    │ strokes│ Whiteboard view (React)                │
│ tauri-plugin-ink             ├───────►│  React Flow viewport · cards · edges   │
│ PKCanvasView + PKToolPicker  │◄───────┤  Perfect Freehand ink layer (SVG)      │
└──────────────────────────────┘viewport│  glass composer (198 on iOS, CSS else) │
                                        └───────────────┬────────────────────────┘
                                                        │ Yjs updates (59 sync path)
                                        ┌───────────────▼────────────────────────┐
                                        │ document store (crates/document)       │
                                        │  kind = whiteboard · Yjs doc · journal │
                                        └───────────────┬────────────────────────┘
                                                        │ wb_* runtime functions
                                        ┌───────────────▼────────────────────────┐
                                        │ agent sessions attached to it (66)     │
                                        └────────────────────────────────────────┘
```

## 3. The data model

One Yjs document per whiteboard, in the existing document store (59), kind `whiteboard`. Top-level maps:

| Map | Entry | Fields |
|---|---|---|
| `meta` | — | `title`, `created`, `bg` (paper/dots/grid/none), `sessions: [session_id]` (attached, informational — the authority is the attachment record) |
| `cards` | `id → Card` | `kind`, `x, y, w, h` (world units = CSS px at zoom 1), `z`, `title`, `source` (§3.1), `state` (`live`/`dimmed`/`archived`), `tag?` (short reason text), `frame?`, `added_by` (`owner`/`session:<id>`), `added_at` |
| `links` | `id → Link` | `from`, `to` (card ids, optional `from_port`/`to_port`: top/right/bottom/left), `label?`, `style` (solid/dashed), `arrow` (end/both/none), `added_by` |
| `frames` | `id → Frame` | `title`, `x, y, w, h`, `colour` — a named region; cards inside move with it |
| `ink` | `id → Stroke` | `points: [x, y, pressure, t]` in world units, `tool` (pen/marker/pencil/highlighter), `colour`, `width`, `anchor?` (card id when the stroke's bbox centre is on a card — the stroke then moves with the card, stored relative to it), `batch` (send generation, §6), `pk?` (opaque PKStroke blob for lossless round-trip on iPad) |
| `journal` | append-only | `{seq, at, actor, op, ids}` — the change log the agent reads (§5.2) |

### 3.1 Card kinds — every one is a renderer theuth already has

| `kind` | `source` | Renders with | Live? |
|---|---|---|---|
| `page` | artifact call_id (an `.html` from `show`) | the 120 inline-page sandboxed frame | live in viewport, snapshot otherwise |
| `diagram` | json-render / openui / mermaid spec (inline) | the surface renderers | yes |
| `note` | doc id | BlockNote read view; tap → `show_note` tile | yes |
| `excerpt` | `path#La-Lb` + text (as 71's `Excerpt`) | Excerpt | yes |
| `diff` | commit / range / files (212) | the split diff view | yes |
| `terminal` | job id (211) | xterm read-only mirror | live only in viewport |
| `web` | URL (205) | snapshot image; tap → web view tile | snapshot |
| `image` | artifact or file | `<img>` | — |
| `sticky` | inline markdown, ≤ 280 chars | text card, editable by finger tap or Scribble | — |
| `session` | session id | last N turns, read-only | yes |

A card is a **reference plus a box**, never a copy: deleting or editing the source updates the card (66's rule). The exception is `diagram` and `sticky`, whose content lives in the card.

### 3.2 Level of detail

Live cards cost (iframes, xterm, BlockNote). Each card has three LODs chosen per frame by on-screen size: **live** (≥ 240 px wide and in the viewport ± one screen), **snapshot** (a cached PNG rendered when it last left live), **chip** (< 60 px: kind icon + title). At most 8 live `page`/`terminal`/`web` cards at once; the rest snapshot. This keeps a 100-card board at 60 fps on an M-series iPad.

## 4. The screen

### 4.1 Idle (most of the time)

Canvas edge to edge, no status bar, no title. Bottom-right: the **glass composer pill** — 44 pt high, ~320 pt wide, `UIGlassEffect` on iOS 26 (198's material), `backdrop-filter: blur(20px) saturate(1.4)` at 55 % opacity elsewhere, so the board shows through. Pill content: session chip (dot + short session title, or *"no session"*), placeholder *"Ask…"*, mic. When the agent is working, the dot pulses and a one-line status replaces the placeholder (*"placing mock C…"*).

### 4.2 Controls (after a finger tap on empty canvas)

A vertical glass cluster rises above the pill, right-aligned, thumb-reachable:

| Control | Does |
|---|---|
| Pencil tools | opens the native `PKToolPicker` (iPad) / the PF tool strip (elsewhere) |
| **+ Add** | menu: sticky · note · artifact · file excerpt · diff · terminal · web · image · from clipboard |
| **Link** | link mode: tap card A, tap card B (or drag between handles) |
| **Frame** | drag a rectangle to make a named region |
| **Select** | lasso (finger) to multi-select cards and ink; then move, dim, delete, *Ask about selection* |
| Fit | zoom to fit all / selection |
| Follow | toggle: let the agent move your viewport (§4.4) |
| Undo / Redo | also two/three-finger tap |
| ⋯ | title, background, attached sessions, export, close |

It never covers the top-left two thirds of the screen. It hides after 4 s idle, on any Pencil touch, or on a tap on empty canvas.

### 4.3 The composer, expanded

Tapping the pill grows it upward into a glass sheet ~360 × 220 pt anchored bottom-right: a text field (Scribble works), the **attachment tray** (thumbnails of fresh ink clusters and the current selection, each removable), the session chip (tap → pick session / new session / detach), send, and a chevron that opens the full session as a slide-over from the right edge. Voice: holding the mic dictates (existing STT path); in a live talk session the agent's speech plays and its text bubble shows.

### 4.4 Agent presence

- **Placement.** A new card from the agent appears with a 300 ms fade-and-settle at the position it chose, beside the card it relates to (§5.3 rules), never on top of the owner's ink.
- **Follow (off by default).** When on, the agent may pan/zoom the owner's viewport (`wb_focus`) and spotlight a card (dim the rest 40 % for 3 s). Any finger touch cancels a focus animation and turns Follow off for 60 s. This is the seam for voice-synced walkthroughs later.
- **Off-screen work.** An agent edit outside the viewport shows as an edge chevron pointing to it; tap → pan there.

### 4.5 Mac and web

Same layout. Controls also appear when the pointer rests in the bottom-right quadrant; keyboard: `V` select, `P` pen, `L` link, `F` frame, `N` sticky, `Space`-drag pan, `⌘0` fit, `⌘↵` focus composer, `Esc` hides chrome. Ink with mouse/trackpad uses Perfect Freehand capture (pressure simulated from velocity). Opening a whiteboard on the desktop is a full-window mode with an exit button that appears with the chrome; it can also be shown as a tile beside the chat (`show_whiteboard`).

### 4.6 Phone

Read-and-talk: pan, zoom, tap cards, use the composer. No ink tools in v1 (screen too small to mark up usefully); cards and links can be moved.

## 5. Linking and the agent's view of the board

### 5.1 Making links (owner)

- Every card shows four small handles on hover (Mac) or when selected (touch). **Drag from a handle** to another card → link, arrow at the target. Release on empty canvas → menu: *new sticky linked here* / *cancel*.
- **Link mode** (cluster): tap A, tap B; stays in mode until Esc / tap empty.
- Pencil can drag from a handle too (a Pencil touch that *starts on a handle* is a link gesture, not ink — the only place a Pencil touch is not ink, and the handle is only visible when the card is selected, so it cannot happen by accident).
- Tap a link → label field, direction flip, style, delete.

There is deliberately **no** "turn this ink line into a link" inference in v1. Open question Q4 asks whether a manual *"make link from this stroke"* action (select a stroke → button) is worth adding.

### 5.2 What the agent reads

Three runtime functions, mirroring `doc_read`:

- `wb_read(id, region?, depth?)` → an **outline**, one line per item, grouped by frame, positions as coarse regions not raw numbers:
  `c7 page "Mock B — flat list" (artifact 01a…) frame:"Variants" right-of:c6 · state live · 2 ink marks`
  `l3 c6 → c7 "v2 from v1"`
  plus a 12×8 **spatial summary** (which cards sit where) for layout reasoning.
- `wb_changes(id, since?)` → the journal since the session's last read (cursor kept per session): *owner moved c7 next to c4 · owner linked c4 → c9 · owner dimmed c2 · 3 new ink strokes on c7 (batch 12)*. This is what arrives automatically with each message (§5.4), so the agent rarely needs `wb_read`.
- `wb_crop(id, region | card | ink_batch, scale?)` → a PNG of that area rendered client-side (or server-side from the stored model for headless reads), for when the agent *does* need eyes.

### 5.3 What the agent writes

`wb_create`, `wb_place(card, near?, side?, frame?)`, `wb_update`, `wb_link`, `wb_frame`, `wb_dim(ids, tag)`, `wb_remove`, `wb_focus(region|card, spotlight?)`, `wb_export(format: note | pdf | png)`. Placement rules (`near` + `side`): never overlap an existing card or ink cluster; prefer right-of then below the reference; keep variants of one thing in one row inside a frame; new rows go below. Agent edits are journalled with `actor = session:<id>`, and undo on the owner's device can undo them.

### 5.4 The message packet

When the owner sends from a whiteboard's composer, the message to the chosen session carries:

1. The text (or transcript).
2. `[attached whiteboard <id> "<title>" · N cards · viewport: frame "Variants"]` — the 66 marker; first send in a session also attaches it.
3. `[whiteboard changes since your last read]` — the `wb_changes` render, capped at 40 lines (then a count and "call wb_changes for the rest").
4. `[selection c7, c9]` when cards are selected.
5. **Ink crops**: one image per fresh ink cluster (§6), each with a caption line `ink batch 12 · cluster 2 · on c7 "Mock B" lower third`.

Sending on a board without a linked session starts a new one with the whiteboard attached.

## 6. Ink — capture, storage, and how it reaches the agent

### 6.1 iPad: native PencilKit overlay

A new Swift Tauri plugin, `crates/tauri-plugin-ink`, built exactly like `tauri-plugin-glass` (198): a transparent `PKCanvasView` added above the WKWebView, below the glass composer. It owns Pencil touches only (`drawingPolicy = .pencilOnly`; finger touches fall through to the web view via `hitTest` returning nil for non-Pencil touches). `PKToolPicker` is the native tool palette (pen, marker, pencil, eraser, lasso, ruler, colour), shown from the cluster's Pencil-tools button or a Pencil double-tap / Pencil Pro squeeze.

**Viewport sync is the hard part.** The web canvas owns pan/zoom (React Flow). On every viewport change the web side posts `{tx, ty, k}` over the plugin channel; Swift applies it as the canvas's `CGAffineTransform` (drawing in world units). During a pinch the overlay may lag a frame; to hide that, at gesture start the plugin rasterises the current strokes into the web layer's PF rendering and hides the overlay until the gesture ends (strokes are already mirrored in Yjs, §6.2). Drawing never happens while panning (Pencil + finger simultaneously: Pencil wins, pan pauses).

**Per stroke, on `canvasViewDrawingDidChange`:** the new `PKStroke` is converted to the portable form (points in world units, force → pressure, timestamps), anchored (§3), written to Yjs with the current `batch`, and its `pk` blob kept for lossless editing. Eraser and lasso edits map to stroke deletes/updates.

### 6.2 Everywhere else: Perfect Freehand

All clients render ink from the portable points with `getStroke(points, {size, thinning, smoothing, streamline, simulatePressure: !hasPressure})` into SVG paths, one layer above cards (or inside a card's coordinate space when anchored). On the iPad this layer shows only while the native overlay is hidden (pan/zoom) or for remote strokes. Mac/web capture uses the same library on pointer events.

### 6.3 From ink to the agent

- Ink is batched by **send generation**: every stroke drawn since the last send has the current `batch`.
- On send, the client clusters the fresh strokes (single-link, gap ≤ 48 world px, time gap ≤ 4 s), takes each cluster's bbox padded by 24 px (at least 160 × 160), and renders that region **with the cards underneath** at 2× to a PNG ≤ 1024 px on the long edge. Max 6 crops per message; beyond that, one crop of the union bbox.
- Each crop's caption names the cards under it and, where the card exposes regions (pages: DOM element ids/aria labels at that point via the frame's `elementsFromPoint`; excerpts: line numbers; diffs: hunk and line), the region — *"on c7 'Mock B' over `#tabbar > li:nth-child(3)`"*. This is the cheap structural hint; the picture carries the meaning, the owner's words carry the intent.
- The agent never receives the whole board as a picture unless it calls `wb_crop` itself.

### 6.4 Handwriting

Scribble (system) handles handwriting into the composer and stickies for free. Recognising handwriting *on the canvas* into text is **out of v1**: the crop goes to a vision model that reads it fine. Q5 asks if on-device `VNRecognizeTextRequest` text should ride along with crops to save tokens.

## 7. Decisions and trade-offs

| Choice | Picked | Over | Why |
|---|---|---|---|
| Canvas engine | React Flow | tldraw, Excalidraw, custom | cards are React components we already have; MIT; handles/edges built in; tldraw's licence key + watermark; Excalidraw's scene model is shapes, not live components |
| iPad ink | native PencilKit | PF in the web view | Apple's prediction and latency (~9 ms), palm rejection, `PKToolPicker`, Pencil Pro squeeze/hover — a web view cannot match them, and Sky named native Pencil as essential |
| Ink storage | portable points + PK blob | PKDrawing only | Mac/web/headless must render and crop; PK blob keeps iPad edits lossless |
| Links | explicit gestures | stroke inference | ambiguity (arrow vs divider vs underline) makes inference wrong often enough to cost trust |
| Agent view | outline + journal + ink crops | periodic screenshots | tokens, latency, and the agent needs ids to act on |
| Substrate | Yjs doc in the document store | new store | sync, offline, undo, attachment, search and sharing come with it |
| Chrome | composer pill only + tap-to-reveal | permanent toolbar | full screen is the point; thumb reach bottom-right |

## 8. Phases

| Phase | Ships | Done when |
|---|---|---|
| P0 model + tools | `whiteboard` kind, Yjs schema, `wb_*` functions, outline/changes render, attachment marker kind, headless crop renderer | an agent can create a board, place 5 cards of 3 kinds, link them, and `wb_read`/`wb_changes` render correctly in tests |
| P1 web view | full-screen view on Mac/web, cards with LOD, handles + link mode, frames, select/dim, glass composer, controls cluster, `show_whiteboard` tile, PF ink on Mac | the §1.1 flow works end to end on the Mac with mouse ink |
| P2 iPad ink | `tauri-plugin-ink`, PKToolPicker, viewport sync, Pencil-only policy, gestures, crops with region captions | the §1.1 flow works on the iPad simulator + a real iPad with Pencil; pan/zoom at 60 fps with 100 cards, 2 000 strokes |
| P3 presence | Follow, `wb_focus`, spotlight, edge chevrons, live-talk bubbles | an agent walkthrough of 5 cards plays in sync with speech |
| P4 export | board → note brief, PDF, PNG | a worker briefed only with the exported note builds the right screen |

## 9. Checks

- Unit: outline/journal renders (golden), placement never overlaps (property test), clustering/crop bboxes, anchored ink moves with cards.
- Simulator (`glass-198-ipad`, 333): pencil-only policy (idb finger taps never ink), chrome toggling rules, viewport sync drift ≤ 1 px after 50 pinch/pan cycles, 60 fps trace with the 100-card fixture.
- Token budget: a 20-turn design session costs ≤ 25 % of the image tokens of "screenshot every turn" (measured on the §1.1 script).
- The productivity test from *What and why*, run by Sky once per phase from P1.

## 10. Open questions for Sky

1. Composer target when no session is linked: **new session (default)** vs the last session used anywhere.
2. Should finger drawing be allowed on iPad when a Pencil is paired? Default **no** (Apple's default; toggle in ⋯).
3. Agent text replies: **transient bubble (default)** vs a persistent mini-transcript strip.
4. Add a manual "make link from this stroke" action? Default **no** in v1.
5. On-device handwriting text with crops? Default **no** in v1.
6. Multi-human editing (sharing a whiteboard with a contact, 191/213)? Default **later**; the substrate allows it.
7. Should whiteboards join notebooks (144) as an item kind? Default **yes, P1**.
