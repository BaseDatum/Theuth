# 218 — Whiteboard: a full-screen thinking table for iPad, Pencil and the agent

Status: design rev 2, 2026-11-25, rulings in §10, approved to build — answers `218-review-fable.md` (4 blockers, 17 should-fix, 12 nits; every id is dispositioned in *Changes from rev 1*). Mock: `218-mock.html` (seven states of one iPad screen).
Extends 59 (documents: one store, Yjs/yrs), 66 (attachment markers), 120 (inline pages: sandboxed frames), 144/157 (the notebook binding and its `<notebook>` / `<notebook_changes>` injection — the pattern copied here), 205 (web view tile), 211, 212. Amends 65, 73, 147 and 208 (§0.10, §7.1). Does **not** depend on 198 (unmerged, §7.2). It is a different thing from design 71's "board" (§0.1).

Wording rule used in this document and in everything the agent reads: the feature is always **the whiteboard**, never "the board".

## What and why

Linear chat is the right shape for coding, debugging and running things. It is the wrong shape for **design work that branches**: comparing variants, iterating a layout or an architecture, keeping requirements in view while a mock changes. In chat every new picture scrolls the last one away, and the owner ends up saying "take the header from two messages ago and the list from five messages ago" in words.

A whiteboard is an independent, infinite, full-screen canvas where the things the harness already renders — mocks (pages), diagrams, notes, code excerpts, diffs, terminals, web views, images — live as **cards that stay where they are put**, linked by explicit arrows, marked up with Apple Pencil, while the owner talks to an agent through one translucent composer in the bottom-right corner. A refinement replaces a card's content in place (a version stack); a new direction gets a new card; rejected directions stay, dimmed, with the reason; ink that has been answered gets out of the way. At the end the whiteboard *is* the spec.

Non-goals: a general drawing app, a Miro clone, real-time multi-human collaboration (v1 is one human plus agents), replacing chat for linear work.

**The productivity test** (it decides whether this ships; protocol and thresholds in §9.1): the same kind of design task is run once in chat + tiles and once on a whiteboard, about 20 minutes each, at least 3 directions and 6 refinements. The whiteboard has to win on things counted from the transcript and the client, not on how it feels: **zero back-references** ("the one from before"), **at most half the owner's words per refinement**, **no litter at the end** (cards ≤ directions + 3, no stale full-strength ink), and **a worker briefed from the whiteboard alone builds the right thing**. If it does not win twice out of three, the feature stops before any Swift is written.

## 0. Decisions in one screen (each a default the rest of the doc assumes; Sky overrides)

1. **Name and kind.** The feature is **Whiteboard**; the document kind is `whiteboard` (`DocKind::Whiteboard`). It is not design 71's board (`DocKind::Board` serialises as `"board"`, `crates/document/src/meta.rs#L101-L104`, `#L124`; the agent prompt already has a `## Boards` block, `crates/document/src/tools.rs#L39-L44`) and not the tasks board. UI copy, tool descriptions and packet text never shorten it to "board"; both prompt blocks get one disambiguating sentence (§5.5).
2. **Independent, attachable, bound.** A whiteboard is a document nobody owns, listed in the sidebar under **Whiteboards**. Any session is pointed at one the way it is pointed at a note: a 66 marker, `[attached whiteboard <id> "<title>" · N cards]`. Because a 66 reference is per message and keeps no record ("a reference belongs to the message it came with", `crates/agent/src/attach.rs#L39-L41`), the first marker — or the first send from the whiteboard's composer — also creates a **binding** (session ↔ whiteboard, many to many), which is what keeps changes flowing to that session until it is detached (§5.4). Deleting every session leaves the whiteboard intact.
3. **Full screen, chrome on demand.** Opening a whiteboard takes the whole window. The **only** always-visible control is the **translucent glass composer pill, bottom-right**. Every other control appears only after a **finger tap** — on empty canvas, or on the grabber that is part of the pill for when a card fills the screen — as one cluster in the bottom-right, above the composer (§4.2).
4. **Pencil draws, fingers navigate.** Pencil = ink, always, and never summons chrome. One finger pans, selects and taps; two fingers zoom. Two-finger tap = undo, three-finger tap = redo — Procreate's convention, chosen deliberately; it is not Apple's. A "draw with finger" toggle exists for devices without a Pencil and for the Mac.
5. **Links are explicit; ink is never interpreted.** A link is made only by an explicit act: a finger drag from a card's handle to another card, link mode, or the agent. A hand-drawn line stays a picture; its meaning is what the owner *says* when sending. No stroke classifier, no "this looks like an arrow". Outside link mode a Pencil touch is ink without exception.
6. **The agent reads structure and sees pixels only for fresh ink.** The whiteboard reaches the agent as an engine-written outline once per context epoch plus a change note per turn. Pictures are sent only for *fresh ink*: one image per inked card (the card with its marks), inside a pixel budget (§6.4) — never a screenshot per change.
7. **Stack.** `@xyflow/react` (MIT; installed 13.0.2, bump to 13.0.3) for viewport, cards, handles and links; `perfect-freehand` 1.2.3 (MIT, no dependencies; new) as the one renderer of committed ink on every client; `yjs` (MIT; installed 13.6.32, bump to 13.6.33) in the clients and `yrs` in the engine; **PencilKit** as the native Pencil capture layer on iPad (P3, committed — §6.2). tldraw 5.6.2 rejected (proprietary licence, production key, watermark); Excalidraw 0.18.1 (MIT) rejected (a drawing-first scene model that fights live cards). Versions, licences and peers read from registry.npmjs.org on 2026-11-25; installed pins from `crates/desktop/ui/package.json#L45`, `#L81`.
8. **Agent replies land on the whiteboard.** A text reply is a glass bubble that stays until the next send or a dismiss, pinned to the card it is about. Anything the agent *makes* is placed as a card, or pushed as a new version of the card it refines.
9. **Refinements stack, directions spread, ink has a life.** A card holds a version stack; `wb_update` pushes a version, `wb_place` is for a new direction. Ink belongs to (card, version): fresh ink is the only full-strength ink on screen; sent ink drops to 30 %; answered ink is hidden with the version it marked; a mark can be pinned as a lasting annotation (§3.3).
10. **Renderer rulings are amended, scoped.** 208 admitted `@xyflow/react` "for the Attention graph only" and 65/73/147 ruled out a second renderer and an infinite canvas as a primary surface. 218 widens that to exactly one more place, `components/whiteboard/**`, and nothing else (§7.1).

## 1. The experience (the reason this exists)

### 1.1 Couch, iPad, a new screen to design

1. Sky opens **Whiteboards → New** on the iPad. Paper-coloured canvas edge to edge; the glass pill bottom-right reads *"Ask… · no session"*. He taps it and says: *"Let's design the attention inbox for the phone. Put up what we have today and two directions."* Sending without a bound session starts one, bound to this whiteboard.
2. Within a second three **placeholders** appear in a row inside a frame *Variants* — labelled boxes at phone size (390 × 844), shimmering. They fill as the agent finishes: a screenshot of today's screen, **Mock A** (grouped by source), **Mock B** (flat list with swipe actions), all at the same device size, plus a **requirements sticky** with three bullets it took from the conversation. Sky's viewport does not move (Follow is off).
3. Sky pinches into Mock B and **double-taps it to enter**: the border turns accent, the mock is live under his finger; he flicks the list and taps its tabs. A tap outside leaves. With the Pencil he circles Mock A's header, scribbles a box low on Mock B where a filter row should go, and crosses out Mock B's third tab. No chrome appeared; nothing was sent. His ink is the only full-strength ink on screen.
4. He taps the pill and says *"Keep A's header. Filters go where I drew the box. Drop that tab."* The tray shows **2 cards with marks** (Mock A, Mock B). Send. The marks drop to 30 %; a *"v2 · building…"* shimmer covers Mock B.
5. The agent receives his words, the marker, an engine-written change note (*mark m1 on c2 v1 over "header" · marks m2, m3 on c3 v1 below the list and over tab 3*) and two images. This is a refinement of B, so it **pushes version 2 onto Mock B** — same card, same place, a "v2 of 2" scrubber — and dims A with the tag *"header kept; grouping rejected — too many sections on a phone"*. The marks on B's v1 are answered and disappear with v1; the circle on A is resolved by the agent and disappears too. The reply sits in a bubble pinned to Mock B, with **Undo this turn**.
6. *"Now try a completely different one: a timeline."* A new placeholder appears to the right at once — a new direction is a new card, linked B → C *"timeline direction"*.
7. Twenty minutes later the frame holds four cards for four directions, two of them dimmed with reasons, each with its own stack of refinements, a state diagram under the winner, and no stale ink: only two marks Sky pinned as annotations. He says *"Write the build brief from this."* The agent writes a note from the outline (each card linked as `theuth://whiteboards/<id>#<card>`) and places it as a card.

### 1.2 Later, at the Mac

The same whiteboard opens on the Mac (sidebar → Whiteboards). Ink renders through the same renderer from the same points. Sky drags the winning mock next to a **diff card** a worker produced and binds the worker's session: the whiteboard is the context he hands over — one attachment, not a recap. From any normal chat, an artifact card's menu has **Add to whiteboard…**, so today's work reaches a whiteboard without asking the agent.

### 1.3 What it is not good for (and so has no affordance for)

Running a build, reading a long log, chatting about a bug. The composer's chevron hands the session back to the normal chat view in one tap.

## 2. Map

```
iPad only, P3                              every client (iPad, Mac, web; phone = outline view)
┌───────────────────────────────┐          ┌──────────────────────────────────────────────┐
│ tauri-plugin-ink (Swift)      │ stroke   │ Whiteboard view (React, components/whiteboard) │
│ PKCanvasView: the stroke in   ├─────────►│  React Flow viewport · cards · handles · links │
│ flight only; PKToolPicker     │◄─────────┤  Perfect Freehand ink layer = all committed ink│
└───────────────────────────────┘ painted  │  pointer-event ink capture (P2; Mac/web always)│
                                           │  CSS glass composer · cluster · reply bubble   │
 native takeSnapshot(rect) of the main     └───────────┬───────────────────┬──────────────┘
 WKWebView (Tauri shells) ── card snapshots, mark images│ Yjs updates (59)  │ blobs (snap/, mark/)
                                           ┌───────────▼───────────────────▼──────────────┐
                                           │ engine: document store, kind = whiteboard     │
                                           │  yrs doc (maps, not blocks) · outline export  │
                                           │  whiteboard reader → outline, baseline diff   │
                                           │  session ↔ whiteboard bindings · alias table  │
                                           └───────────┬──────────────────────────────────┘
                                                       │ <whiteboard> once per epoch,
                                                       │ <whiteboard_changes> per turn; wb_* tools
                                           ┌───────────▼──────────────────────────────────┐
                                           │ every bound session (any number)              │
                                           └──────────────────────────────────────────────┘
```

## 3. The data model

### 3.1 What the document store gives, and what P0 builds

The store is BlockNote-shaped end to end: one `Y.XmlFragment` named `document-store` holds a document's blocks (`crates/document/src/lib.rs#L90-L91`); export is unconditionally `helper.markdown` (`crates/document/src/actor.rs#L756-L766`) through a BlockNote + linkedom helper (`crates/document/src/helper.rs#L1-L6`); the index walks block kinds (`crates/document/src/index/mod.rs#L31-L38`); `DocKind` and its predicates enumerate kinds (`crates/document/src/meta.rs#L87-L147`). A `whiteboard` kind therefore **inherits** the yrs document, the update log, snapshots, fan-out and relay, the 8 MiB document cap and 48 KiB update cap (`crates/document/src/lib.rs#L130`, `crates/document/src/actor.rs#L1061`), and on link devices the offline store (below). **P0 builds:**

- `DocKind::Whiteboard` and `Binding::Whiteboard` (`documents/<id>/whiteboard.md`), every `match` on `DocKind`, and the predicates: `free_standing` yes, `homed` yes, `movable` no in v1. `doc_*` tools refuse the kind and name `wb_*`.
- A **yrs-side reader** in Rust for the maps below (no JS helper). It renders the outline (§5.2); the derived file is that outline, so export, history and search exist without BlockNote. `INDEXABLE` gains the kind with an adapter that indexes outline lines (titles, sticky text, tags, link labels).
- Blob storage beside the document for snapshots and mark images (§3.4), with two RPCs (`wb_blob_put`, `wb_blob_get`).
- The binding table, the alias table and the injection (§5.2–§5.4).

**Offline.** Rev 1 said offline "comes with it"; the review said there is none. Both overshoot. On a link device (`engine !== LOCAL` — the iPad) the whole Y.Doc and the FIFO of unacknowledged updates are in IndexedDB and survive a killed app (`crates/desktop/ui/src/lib/doc/queue.ts#L1-L5`, `crates/desktop/ui/src/lib/doc/provider.ts#L113-L115`); in-process on the Mac the queue is memory only. The limit that matters here: the state blob is dropped from IndexedDB above 4 MiB (`queue.ts#L10-L11`, `#L101-L110`; pending updates never are). So ink is stored compactly (§3.3) and the whiteboard keeps its Y.Doc under 4 MiB (§9, check 3).

**Undo** does not come with it either: the only `Y.UndoManager` in the UI is y-prosemirror's and needs a revive hack (`crates/desktop/ui/src/components/editor/reveal.ts#L252-L264`). The whiteboard has its own (§4.6).

### 3.2 The maps

One Yjs document per whiteboard; top-level `Y.Map`s (the `document-store` fragment is unused):

| Map | Entry | Fields |
|---|---|---|
| `meta` | — | `title`, `created`, `bg` (paper/dots/grid/none) |
| `cards` | `id → Card` | `kind`, `x, y, w, h` (world units = CSS px at zoom 1), `z`, `title`, `versions: [{source, at}]` (current = last; §3.5), `viewport?` (`{w, h, preset}`, §3.6), `state` (`live`/`dimmed`/`archived`), `tag?`, `frame?`, `placeholder?` (label while the agent is still making it), `by` |
| `links` | `id → Link` | `from`, `to` (card ids; optional ports top/right/bottom/left), `label?`, `style` (solid/dashed), `arrow` (end/both/none), `by` |
| `frames` | `id → Frame` | `title`, `x, y, w, h`, `colour`, `preset?` (device preset its page cards share) — a React Flow parent node; its cards are children with relative positions |
| `ink` | `id → Stroke` | `pts` (packed bytes, §3.3), `tool` (monoline/pen/marker), `colour`, `width`, `card?`, `ver?` (when anchored: points are relative to the card), `mark?`, `at` |
| `marks` | `id → Mark` | `card?`, `ver?`, `strokes: [id]`, `state` (`sent`/`answered`/`pinned`), `region?` (§6.5), `sent_at` |

There is **no journal map and no per-session cursor** (rev 1 had both; §5.2 replaces them). Ids are client-minted ULIDs, so two offline clients cannot mint the same one; the short aliases the agent sees (`c7`, `l3`, `m4`) are assigned by the engine in a per-whiteboard alias table and are the same for every session. `by` (`owner` / `session:<id>`) is display only: the engine stamps it on its own `wb_*` writes, a client can write anything, and no trust decision reads it.

### 3.3 Ink: storage and life

A stroke is simplified at commit (Ramer–Douglas–Peucker, ε = 0.35 px at capture zoom) and packed at 6 bytes a point (`int16` Δx, Δy in ⅛ world px, `uint8` pressure, `uint8` Δt in 4 ms). A typical stroke is 40–150 points, 0.25–0.9 kB; a stroke is capped at 4 000 points (24 kB, under the 48 KiB update cap) and split beyond that. 2 000 strokes are 1–2 MB.

**Anchoring.** A stroke belongs to the card that contains at least 60 % of its length, and to that card's current version; otherwise it is free. A free stroke that touches exactly one card rides with it when the owner moves the card; a free stroke touching two or more cards (a hand-drawn line between them) stays put. The agent's tools refuse to move a card that unanswered ink touches.

**Life.** Sky sees one life per stroke, whichever session it was sent to:

| State | Looks | Becomes it when |
|---|---|---|
| fresh | 100 % | drawn, not yet sent to anyone |
| sent | 30 % | a send carried it (it now has a `mark`) |
| answered | hidden | its card gets a new version, or the agent resolves the mark (`wb_resolve`), or the owner's next send goes out |
| pinned | 100 %, annotation style | the owner long-presses a mark → *Pin*; never re-sent, never hidden |

Scrubbing a card back to an old version shows the ink that marked that version at 30 %; ⋯ → *Show answered marks* does the same everywhere. When the document passes 2 MiB, strokes of answered marks older than a day are baked into their version's snapshot blob and deleted from the Y.Doc (Q8).

### 3.4 Snapshots and mark images (the only pixels there are)

A `page` card is the 120 frame: `sandbox="allow-scripts allow-forms"`, opaque origin (`crates/desktop/ui/src/components/chat/HtmlPage.tsx#L1-L5`, `#L169-L173`; `crates/desktop/src/pages.rs#L1-L8`, `#L134`). The host cannot read its pixels or its DOM from JavaScript, and the engine has no renderer for pages, mermaid or json-render. So there is exactly one rasteriser, and it is native:

- **Tauri shells (macOS, iOS):** `takeSnapshot` of a rectangle of the main `WKWebView` (`WKSnapshotConfiguration.rect`, iOS 11 / macOS 10.13). The selector is already bound for web view tiles, with the configuration passed as `None` (`crates/desktop/src/ios_webview.rs#L162-L170`; `crates/desktop/src/apple_tile.rs#L12-L14`, `#L270-L307`); P0 adds a `wb_capture(rect)` command for the main webview with a typed configuration. Because committed ink is in the webview (§6.1), one capture has cards and ink.
- **Card snapshot** `snap/<card>/<version>`: taken by whichever Tauri client has the card live and fully on screen, 500 ms after it settles and again before it is demoted; ≤ 1 024 px on the long side. Before any snapshot exists a card that is not live is a **title chip** (kind icon + title).
- **Mark image** `mark/<mark>`: the image sent with a message (§6.4), stored when it is sent.
- **Relay-served web client:** cannot capture. It composes from stored snapshots plus vector ink in a `<canvas>`; a card nobody has snapshotted is an outlined, labelled box, and the caption says so.
- **Headless (`wb_crop` with no client open):** the engine returns stored blobs — a card's snapshot, a mark's image — and nothing else. An arbitrary region needs a connected Tauri client and fails fast without one, the way `page_preview` does. There is no engine-side renderer and none is promised.
- PNG/PDF export is built on the same capture and waits for P5.

### 3.5 Card kinds — every one is a renderer theuth already has

| `kind` | A version's `source` | Renders with | When the source is gone |
|---|---|---|---|
| `page` | `{session, call_id}` of an `.html` artifact | the 120 sandboxed frame at the card's `viewport`, scaled | snapshot + "source gone" |
| `diagram` | json-render / openui / mermaid spec (inline) | the surface renderers (65's one renderer, unchanged) | — |
| `note` | doc id | BlockNote read view | tombstone |
| `excerpt` | `path#La-Lb` + text (as 71's `Excerpt`) | Excerpt | the stored text |
| `diff` | `{session, spec}` for a commit or range; a **patch artifact captured at placement** for a working-tree diff | the 212 split view | snapshot; working-tree cards say "as of 14:02" |
| `terminal` | `{session, job}` (`crates/desktop/ui/src/components/tiles/open.ts#L71-L79`) | xterm read-only mirror | last screen as snapshot + "job ended" |
| `web` | URL | snapshot image | snapshot |
| `image` | artifact or file | `<img>` | tombstone |
| `sticky` | inline markdown, ≤ 280 chars | text card, editable by finger tap or Scribble | — |
| `session` | session id | last N turns, read-only | tombstone |

A card is a **reference plus a box**. A reference to something versioned is to one version (an artifact `call_id` is one version, `crates/agent/src/attach.rs#L54-L55`), which is why refinements are a **version stack** on the card rather than an edit of the source: `wb_update(card, source)` appends to `versions`; the card shows the last one, a small "v3 of 4" scrubber steps back (view state, per client), and a stacked edge shows there is history. `note`, `sticky` and `diagram` cards edit in place and have one version.

Artifacts, diffs and terminals belong to a session (`crates/artifact/src/rpc_artifacts.rs#L141-L142`; `open.ts#L108-L111`). A bound session reads a card's content through `wb_card(card)`, which the engine authorises by *bound to this whiteboard* ∧ *the card references it* — a whiteboard does not grant blanket access to another session's artifacts.

### 3.6 Device-true pages, and level of detail

A page card has a `viewport {w, h}` with presets — phone 390 × 844, tablet 820 × 1180, desktop 1280 × 800, or `auto` (the frame sizes itself from the page's reported height, today's behaviour, `HtmlPage.tsx#L148-L151`). The frame is laid out at the viewport size and scaled to the card's box. A frame can carry a preset its page cards share; `wb_place(…, device)` sets it. Variants at different widths are not a comparison.

Three LODs chosen by on-screen size: **live** (≥ 240 px wide and inside the viewport ± one screen), **snapshot**, **chip** (< 60 px, or no snapshot yet). React Flow's `onlyRenderVisibleElements` is **not** used (the two existing canvases use it, e.g. `crates/desktop/ui/src/components/attention/graph/Canvas.tsx#L309`): it unmounts off-screen nodes, which reloads a page and loses its state on every pan. Budget (S17): at most 4 live `page`/`terminal`/`web` cards on iPad, 8 on the Mac; a live page nobody has entered for 30 s is demoted to its snapshot unless pinned live; demotion loses the page's in-memory state, and that is stated on the pin.

## 4. The screen

### 4.1 Idle (most of the time)

Canvas edge to edge. Bottom-right: the **glass composer pill** — 44 pt high, ~330 pt wide, inset 24 pt from both edges (clear of the window-resize corner; the Pencil corner swipe for Quick Note starts in the same corner and has to be tried on a device, §9). The glass is CSS in every phase: `backdrop-filter: blur(20px) saturate(1.4)` over a surface tint strong enough that pill text keeps ≥ 4.5 : 1 over paper and over a dark card. Pill content: a **grabber** at its leading edge (§4.2), the session chip (dot + short title, or *"no session"*), placeholder *"Ask…"*, mic. While the agent works, the dot pulses and a one-line status replaces the placeholder. "Whole screen" is best effort under iPadOS 26 windowing: the whiteboard fills its window and asks for the status bar to hide; it cannot promise more.

### 4.2 Controls (after a finger tap)

A finger tap on **empty canvas**, or on the pill's **grabber** (so the controls are reachable when one card fills the screen), raises a vertical glass cluster above the pill, right-aligned, 44 pt targets, icons with accessibility labels and no visible text:

| Control | Does |
|---|---|
| Ink tools | pen · marker · eraser · colour (the web strip in P2; the native `PKToolPicker` in P3) |
| **+ Add** | sticky · note · artifact · file excerpt · diff · terminal · web · image · from clipboard |
| **Link** | link mode: tap card A, tap card B, or drag between handles — with a finger or, in this mode only, the Pencil |
| **Frame** | drag a rectangle to make a named region |
| **Select** | finger lasso over cards and ink; then move, dim, delete, pin mark, *Ask about selection* |
| Fit | zoom to fit all / the selection |
| Outline | the outline view (§4.7) |
| Follow | let the agent move this viewport (§4.5) |
| Undo · Redo | also two- and three-finger tap |
| ⋯ | title, background, bound sessions, show answered marks, keep controls open, ink engine, export, close |

It never covers the top-left two thirds of the screen. It hides after 4 s without finger input, on any Pencil stroke, or on a second tap; ⋯ → *Keep controls open* turns the timer off (WCAG 2.2.1). A palm cannot summon it (§6.1, palm rule).

### 4.3 The composer, expanded

Tapping the pill grows it upward into a glass sheet ~380 × 230 pt anchored bottom-right, riding above the software keyboard (visual-viewport inset): a text field (Scribble works), the **tray** — one thumbnail per inked card in view plus the current selection, each removable, and *"+ N marks off screen"* as an opt-in — the session chip (pick / new / detach), send, and a chevron that opens the session as a slide-over. Holding the mic dictates. If the session's model cannot take images and no vision fallback is set (`agent.vision_model`, `crates/agent/src/rpc_settings.rs#L74-L76`), the tray says *"this model cannot see marks — captions only"* before the send, and the packet says how many images were left out.

### 4.4 Using a card versus moving the canvas

Every live frame sits under a transparent **shield**, so one finger on a card pans and a pinch zooms — an iframe would otherwise swallow both. **Double-tap** (or Enter on a selected card) *enters* the card: accent border, touches go to the frame, the canvas does not pan. A tap outside or Esc leaves. Ink never enters: a Pencil stroke over an entered card is still ink on the whiteboard. A single tap selects (handles appear); the card's *Open* button raises the **sheet** — the card's full view (note editor, web view, terminal, diff) sliding in from the right over 60 % of the screen, full-screen on a phone. Tiles are not used inside a full-screen whiteboard: `openTile` returns `null` without a chat pane and there are none in the mobile layout (`crates/desktop/ui/src/lib/tiles.ts#L58-L69`, `#L137-L141`). When the whiteboard is itself a tile beside a chat (`show_whiteboard`, a new kind in `crates/desktop/ui/src/components/tiles/registry.tsx#L43-L61`), *Open* makes a sibling tile as today.

### 4.5 Agent presence

- **Placeholders.** `wb_place` reserves a labelled placeholder at once, at phone size if that is the frame's preset, and the artifact fills it when it exists; a version push shows *"v3 · building…"* on the card. A placeholder still empty when the turn ends is removed.
- **The reply bubble** stays until the next send or a dismiss, collapses to one line after 6 s, and is pinned to the card it is about when the turn touched exactly one (else above the pill). It carries **Undo this turn**.
- **Follow (off by default).** When on, the agent may pan/zoom this viewport (`wb_focus`) and spotlight a card. Any finger touch cancels the animation and turns Follow off for 60 s. Reduce Motion replaces settle, spotlight and Follow animations with cuts.
- **Off-screen work** shows as an edge chevron pointing to it; tap → pan there.
- The agent does not move, dim or archive a card the owner placed unless the message asks for it; `wb_remove` archives and never deletes.

### 4.6 Undo

One stack per client: a `Y.UndoManager` over the whiteboard's maps that tracks the local owner's origin only. Engine-applied updates arrive under the provider's `engine` origin (`crates/desktop/ui/src/lib/doc/provider.ts#L15-L16`) and never enter it, so two-finger-tap undo removes the stroke just drawn, not something the agent did off screen. An agent turn is one unit, offered on its reply as **Undo this turn**: the engine keeps the inverse of the `wb_*` operations of the last 10 turns and applies it as a new transaction, skipping and reporting anything the owner has changed since. Edits inside a referenced source (a note's text, a file) are that source's undo, not the whiteboard's. Capture-only ink (§6.1) is what keeps this at one stack — PencilKit's own undo is never used.

### 4.7 Outline view, Mac, phone, accessibility

The canvas is opaque to VoiceOver and the keyboard, so the outline the agent reads is also a **view**: frames → rows → cards → links, each openable, Tab order by frame and row, every action of the cluster reachable from it. It is the accessible representation, the **phone's default** (the phone gets the outline, the composer and card sheets; the canvas is one tap away, read-only for ink), and what a brief is exported from.

Mac and web: same layout. Controls also appear when the pointer rests in the bottom-right quadrant; `V` select, `P` pen, `E` eraser, `L` link, `F` frame, `N` sticky, `O` outline, `Space`-drag pan, `⌘0` fit, `⌘↵` composer, `Esc` leaves a card / hides chrome. A whiteboard opens full-window (an exit button appears with the chrome) or as a tile beside a chat.

## 5. Linking and the agent's view

### 5.1 Making links (owner)

- A selected card shows four handles (hover shows them on the Mac). **Drag from a handle with a finger** to another card → a link, arrow at the target. Release on empty canvas → *new sticky linked here* / *cancel*.
- **Link mode** (cluster): tap A, tap B; stays on until Esc or a tap on empty canvas. While it is on, the Pencil is a pointer, not a pen (in P3 the native drawing recogniser is disabled for the duration of the mode — a mode switch, not a race).
- Tap a link → label, direction flip, style, delete.
- Rev 1 let a Pencil touch that starts on a handle make a link. That is gone: outside link mode the Pencil is ink, always.

There is no "turn this ink line into a link" inference and no manual stroke-to-link action in v1 (R4).

### 5.2 What the agent reads — copied from the notebook mechanism

Rev 1 kept a journal inside the Yjs document and a per-session cursor. Both are wrong in kind: `seq` collides across offline clients, the array grows forever and syncs to everyone, a line is an unauthenticated claim about its actor, and after a compaction the cursor says the agent has an outline it no longer has. The notebook binding already solved this: the engine derives the baseline **from the transcript itself** — the last `<notebook …>` block resets it, each `<notebook_changes>` applies, a stale notice is ignored (`crates/agent/src/notebook_context.rs#L595-L612`) — and writes its block as its own message after the user's (`crates/agent/src/loop_.rs#L7503-L7513`; `notebook_context.rs#L852-L861`). The whiteboard does the same:

- **`<whiteboard id title>`** — the outline, once per context epoch (again after a compaction). One line per item; positions are **frames and rows in reading order**, not coordinates:
  ```
  <whiteboard id="01JD…" title="Attention inbox">
  frame f1 "Variants" · phone 390×844
    row 1: c1 c2 c3
  c1 image "Today" · live
  c2 page "Mock A — grouped" v1/1 · dimmed "header kept; grouping rejected"
  c3 page "Mock B — flat list" v2/2 · live · mark m4 sent
  loose
    row 1: c4 c5
  c4 sticky "Requirements" · 3 lines
  l1 c2 → c3 "v2"
  </whiteboard>
  ```
- **`<whiteboard_changes whiteboard>`** — every later turn in which something differs: the engine parses its own earlier lines back into a baseline and diffs the yrs document against it, field by field (`~c3 v2 → v3`, `~c2 dimmed "…"`, `~c5 moved: loose row 1 → Variants row 2`, `+l2 c4 → c3`, `+m5 on c3 v3 "page says: tab bar, item 3" (image 1)`, `-c6 archived`), capped at 40 lines, then a count. It also carries what only this send knows — `in view: frame "Variants"`, `selected: c3` — which the client passes as structured send parameters, not as body text.
- No journal, no cursor state, correct after compaction, forks and restarts, and the provenance is the engine's. Each bound session has its own baseline, so a mark sent to session X is still new to session Y, exactly once (S9).
- On demand: `wb_read(id, frame?)` (the outline again, or one frame in detail), `wb_card(card, version?)` (a card's content through its kind's reader), `wb_crop(card | mark | region)` (§3.4).

### 5.3 What the agent writes

`wb_create`, `wb_place(kind, title, near?, side?, frame?, device?)` → a placeholder id at once, `wb_update(card, source)` → fills a placeholder or pushes a version, `wb_link`, `wb_frame`, `wb_dim(ids, tag)`, `wb_resolve(marks)`, `wb_remove` (archives), `wb_focus(card | frame, spotlight?)`, `wb_export`. Placement is solved by the engine against the live document at write time: never over a card or over ink; right of the reference, then below; variants of one thing in one row of a frame; nothing the owner placed moves and no auto-layout ever runs. All of it is engine-applied, so the turn's inverse is known (§4.6).

### 5.4 Attachment, binding and the message

- **Marker.** `Kind::Whiteboard` joins `crates/agent/src/attach.rs#L49-L76`, with its row in the prompt table (`#L17-L41`), the TS twin and the shared fixture (`#L10-L11`), and the tasks tool's kind list (`crates/tasks/src/tools.rs#L168-L170`). First look for the agent: the `<whiteboard>` block it already has, else `wb_read`.
- **Binding.** A `session_whiteboards(session_root, whiteboard)` table — the notebook binding's shape (`crates/agent/src/store.rs#L3665-L3672`), but many to many. Created by the first marker a session receives for that whiteboard or by a send from its composer; removed by *detach*. A whiteboard lists its bound sessions in ⋯; a session bound to more than three whiteboards gets full outlines for the three most recently touched and one line for the rest.
- **The message** a send produces is the owner's words, preceded by the marker line and followed by ordinary image parts. Nothing else is typed into the body: marker parsing stops at the first non-marker line (`attach.rs#L3-L5`, `#L163`), so rev 1's bracketed "changes" and "selection" lines would have shown in the user's bubble and titled a wordless first message (`attach.rs#L243-L256`). They are in the engine's `<whiteboard_changes>` message instead; a wordless first send is titled with the whiteboard's title.

### 5.5 Naming in prompts

A new `## Whiteboards` block says what a whiteboard is, that it is not a board, and when to prefer version pushes over new cards. The existing `## Boards` block (`crates/document/src/tools.rs#L39-L44`) gains one sentence: *"A whiteboard is something else — a canvas the user arranges; see Whiteboards."* `wb_*` descriptions and all packet text say "whiteboard". Design 71's two files are named `71-whiteboard-*.md` and its skill credits "dev.fast Whiteboard" (`tools.rs#L1313`); They are left untouched (R9).

## 6. Ink — capture, rendering, and how it reaches the agent

### 6.1 One renderer, one capture path

**Perfect Freehand in the web layer is the single renderer of committed ink on every client**: `getStroke(points, {size, thinning, smoothing, streamline, simulatePressure: !hasPressure})` into SVG paths, in a layer above the cards, or in the card's own coordinate space when anchored. Whatever captured a stroke hands over the same thing — points, pressure, time, tool, colour — through one interface, so the capture path can change without touching storage, crops or undo.

- **Pointer capture — the only path, on every platform (iPad, Mac, browser; R13).** Pointer events with `pointerType === "pen"` and `pressure`; `getCoalescedEvents()` and `getPredictedEvents()` where WebKit has them (Safari 18.2+, per MDN browser-compat-data `api/PointerEvent.json`), each sample taken once and in time order. The stroke in flight is drawn on a `<canvas>` once per animation frame (the predicted tail drawn, never committed); on pointer-up it is simplified and written to Yjs. A capture-phase filter on the React Flow wrapper swallows pen pointers and stylus touches before the pan/zoom handler sees them, so a pen never pans, selects or drags. Mouse and finger ink (the toggle) use the same path with pressure simulated from velocity. The cluster's tool picker is the only tool UI; the tool latches until another tool, Select or Esc.
- **Native capture** (§6.2) was built, shipped in 2.30.2 and removed (R13).

**Palm rule:** while a pen is down and for 500 ms after, finger touches are ignored — no pan, no chrome toggle, no selection, no multi-finger undo. A pen-down also cancels a palm that landed first: its long-press (mark menu) and its multi-finger tap.

### 6.2 Native Pencil — retired (R13)

> **Retired 2026-11-25 by R13.** The native capture layer below was built (P3) and shipped in 2.30.2; Sky turned "Native Pencil ink" off on a real iPad, felt no lag, and chose web-only. Ink is §6.1's pointer path everywhere. `tauri-plugin-ink` keeps two stateless services: Vision handwriting (`recognize_text`, §6.6) and a `UIPencilInteraction` forwarder (`pencil_listen` / `pencil_unlisten`) whose double-tap / squeeze events the web maps to its own tool switch. The hybrid is preserved at the git tag `ink-hybrid-final`. Decision and risks: `218-recon/ink-decision.md`; as built: `218-as-built-ink-web.md`. The text below is the record of what was built.

**Why it is not optional.** Sky named native Pencil quality as essential, and the pointer path cannot give it: every point crosses the web content process and the JS main thread before any ink shows; there is no `PKToolPicker`; double-tap and squeeze cannot follow the user's Settings; palm rejection is ours, not the system's. P2 ships pointer ink first for one reason only — so the productivity test runs on the couch before any Swift exists. **P3's entry gate is P2's exit (the test passes); there is no second decision.** Its exit gate is the acceptance list at the end of this section.

**Why rev 1's overlay could not work:** `hitTest(_:with:)` is asked before the event carries its touches, so "nil for non-Pencil touches" is not available (reported behaviour, Stack Overflow 67478111 — not an Apple statement; P3's first spike confirms it); `PKCanvasView` is a `UIScrollView` that "automatically scales its underlying content" — a `CGAffineTransform` on it scales raster tiles and its content rect is bounded; `PKStroke.id` is iOS 27.0+ while the app targets 17.4 (`crates/desktop/tauri.ios.conf.json#L10`); a stroke has no data representation, only `PKDrawing.dataRepresentation()` does; and strokes from other clients would have to be rebuilt natively. (The last four are from Apple's developer documentation, read 2026-11-25: `pencilkit/pkcanvasview`, `pkstroke/id`, `pkdrawing/datarepresentation()`.)

**Rev 2: the native layer is capture-only.**

- `crates/tauri-plugin-ink`, a Swift Tauri plugin of its own (§7.2). A transparent `PKCanvasView` over the main `WKWebView`, same bounds, `zoomScale` 1, scrolling off, **never transformed**, `drawingPolicy = .pencilOnly` (which is unconditional; rev 1 described `.default`). It holds only strokes that are not yet handed over.
- **Touch routing.** The canvas's `hitTest` returns nil always; its `drawingGestureRecognizer` is attached to the web view with `allowedTouchTypes = [.pencil]`. This is the one arrangement reported to work over a `WKWebView` (Stack Overflow 66251895; its author calls it hacky) and it is the first thing P3 proves. The web view stays the hit view, so the page still gets pen pointer events — which the §6.1 filter already drops. If the arrangement does not hold on iPadOS 26/27, route B: our own pencil-only recogniser on the web view (coalesced and predicted `UITouch`es) drawing the wet stroke in a `CAShapeLayer`, with the same hand-over. Route B keeps the picker, squeeze and system palm rejection and loses PencilKit's wet-ink renderer; it is a fallback, not a plan.
- **Hand-over.** While the pen is moving, PencilKit draws. At pen-idle (no Pencil touch for 120 ms) the plugin samples each new stroke with `PKStrokePath.interpolatedPoints(in:by:)` — the stored points are spline control points, not samples — converts screen → world with the viewport captured at stroke begin (it cannot change mid-stroke: palm rule), and sends them in order. The web layer writes Yjs, paints, and acknowledges after the paint; only then are those strokes removed from the canvas. Strokes are identified by order (the canvas is append-only: erasing is web-side), so `PKStroke.id` is not needed. No acknowledgement within 300 ms → the native stroke stays visible and the hand-over retries; a stroke is never dropped.
- **What this deletes from rev 1:** viewport sync during gestures (the canvas is empty whenever fingers move), the hide-and-rasterise swap on every pinch, the `pk` blob, stroke-identity diffing, native undo, native rebuild of remote and anchored strokes.
- **What it costs, stated:** a stroke's shape changes slightly when it is handed over. So the picker is built with `PKToolPicker(toolItems:)` (iOS 18.0+) from inks Perfect Freehand can match — monoline, pen, marker — plus an eraser item and colours; no pencil, crayon, watercolor, fountain pen or reed, no ruler, no lasso (selection is the cluster's). With the eraser selected, drawing is disabled (`isDrawingEnabled`, iOS 18.0+) and pen events erase whole strokes web-side; the pixel eraser is out of v1.
- **First responder.** The web view is always first responder; the canvas never is. The picker is attached through the web view's `pencilKitResponderState.activeToolPicker` (`PKResponderState`, iPadOS 26.0+) and hidden while the composer sheet is open, so the keyboard and the picker never contend. This is why native capture requires **iPadOS 26+** at run time (`#available`; the app's minimum stays 17.4 and older iPads keep pointer ink) — Q5.
- **Double-tap and squeeze** follow `UIPencilInteraction.preferredTapAction` / `preferredSqueezeAction` (12.1 / 17.5+) through a `UIPencilInteraction` on the web view. Pencil hover is out of v1.
- **Exit gate (all on a real iPad with a Pencil):** (1) 100 strokes of fast handwriting, 0 lost, 0 doubled, order kept; (2) finger pan/pinch between strokes never shows two renderings of one stroke; (3) hand-over shape change invisible at 1× for monoline and marker, and accepted by Sky for pen; (4) blind A/B against pointer ink (⋯ → *Ink engine*): Sky prefers native, and 240 fps video shows the wet stroke trailing the tip by no more than the pointer path does; (5) composer keyboard, picker, double-tap and squeeze all work in one session without a restart. A miss is fixed (route B, or tuning the renderer to the ink), not waived.

### 6.3 Marks

On send, the client clusters the fresh strokes that are in view (single-link, gap ≤ 48 world px, time gap ≤ 4 s) into **marks**, writes them to `marks`, and the strokes turn 30 %. Fresh ink off screen stays fresh and is offered in the tray, not sent silently — a doodle from ten minutes ago elsewhere does not ride the next message.

### 6.4 What is sent, and what it costs

- **One image per inked card: the whole card with its fresh marks**, long side ≤ 768 px — the agent sees *where* on the card a mark is, which a tight crop loses. A second, zoomed image of a mark only when its box is under 15 % of the card. Free marks: the mark's box padded 24 px (at least 160 × 160) with whatever is under it.
- **Budget: 1.2 MP per message in total, at most 6 images**; beyond it all images are scaled down together, then merged per frame. At roughly w·h/750 tokens an image, a phone mock at 355 × 768 is ~360 tokens and the cap is ~1 600 — under one full-screen screenshot (~2 500). Rev 1's "6 crops up to 1 024 px" was ~8 000.
- Source of the pixels: a live capture when the card is on screen, else its stored snapshot with the ink drawn over it (§3.4). The image is stored as the mark's blob.
- The agent never receives the whole whiteboard as a picture unless it calls `wb_crop` itself.

### 6.5 Captions: the structural hint

Each mark's line names its card, version and — where the card can say — the region. Excerpts and diffs are host-rendered, so the host knows the lines and hunks. For a page, rev 1's `elementsFromPoint` through the frame is impossible (opaque origin); the only channel is the `postMessage` bridge, which today carries size, open-link and console (`HtmlPage.tsx#L142-L158`; `crates/desktop/ui/src/lib/page-bootstrap.ts#L226-L249`). `injectBootstrap` (`page-bootstrap.ts#L199-L218`) gains one RPC, `theuth/describe-point {x, y}` → `{selector, role, label, rect}`, asked **at stroke end while the card is live** and stored on the mark. The answer is text written by a page — agent- or web-authored HTML talking into the next turn — so it is treated as untrusted: ≤ 120 characters, newlines and brackets stripped, always quoted and prefixed *"page says:"*, never an instruction.

### 6.6 Handwriting

Scribble handles handwriting into the composer and stickies. In the iOS shell (every iPad, whatever the ink path — R13), each image of one free mark is also run through on-device Vision text recognition (`VNRecognizeTextRequest`, `.accurate`; `tauri-plugin-ink` `recognize_text`) and the recognised text rides in the mark's caption line as `handwriting: "…"` (untrusted, capped 200 chars). The image is still sent; the text is a hint (R6). Clients without the plugin (Mac, browser) send the image only.

## 7. Decisions and trade-offs

| Choice | Picked | Over | Why |
|---|---|---|---|
| Canvas engine | React Flow | tldraw, Excalidraw, custom | cards are React components we already have; MIT; handles and links built in; already in the bundle for 208 |
| Committed ink | one web renderer (Perfect Freehand) | PencilKit as renderer on iPad | one snapshot has cards and ink; Mac, web and remote strokes need no native twin; one undo stack |
| iPad capture | pointer events first (P2), native PencilKit capture second (P3, committed) | native first | the productivity test must run before Swift is written; native is still required (§6.2) |
| Pixels | native `takeSnapshot` + stored blobs | an engine renderer, DOM-to-image | the 120 sandbox makes pages unreadable from JS; the engine cannot render React |
| Agent view | engine-written outline + baseline diff + per-card ink images | in-document journal + cursor; periodic screenshots | correct after compaction; provenance is the engine's; ids to act on; tokens |
| Refinement | version stack on the card | a new card per change | fifteen cards for five ideas is scroll-back sideways |
| Ink after it is answered | fades, then hides with its version | stays | the next image would carry old marks the agent must not re-read |
| Links | explicit gestures | stroke inference | a guessed arrow that guesses wrong costs more trust than it saves taps |
| Substrate | a `whiteboard` kind in the document store | a new store | sync, relay, link-device offline and history are real; BlockNote export, search and undo are not inherited and are built (§3.1) |
| Chrome | composer pill only + tap-to-reveal | permanent toolbar | full screen is the point; thumb reach bottom-right |

### 7.1 Amendments to earlier rulings (scoped)

| Ruling | Where | 218 says |
|---|---|---|
| "`@xyflow/react` is admitted for the Attention graph only (`components/attention/graph/**`)" | 208 §9.1, `docs/design/208-attention-graph.md#L183` | admitted for the Attention graph **and `components/whiteboard/**`**; nothing else |
| "One React renderer" for diagrams | 65 §0.4, `docs/design/65-graph-diagrams.md#L14` | kept: a `diagram` card renders through the same surface `Graph`; React Flow draws only the viewport, card boxes, handles and links |
| No xyflow beside `GraphSvg`; no "infinite/pannable canvas as the primary surface" | 73 §0.4, §6.5, `docs/design/73-codemap.md#L14`, `#L391`, `#L431` | kept for CodeMap. Amended for the whiteboard only: here position is the owner's data, not a layout result, so the canvas *is* the surface. It is never the only one — the outline view exists and is the phone default |
| No second renderer (xyflow, dagre, cytoscape) | 147, `docs/design/147-codemap-perspectives.md#L985` | same scope as above |
| 73's "an existing node never moves" | 208 §9.1 keeps it | kept and stronger: no auto-layout ever runs; the agent does not move what the owner placed |
| React Flow attribution "kept visible this round" | 208 §9.1, `208-attention-graph.md#L197` | it renders bottom-right, under the composer, and is a second always-visible control. Default: hidden on the whiteboard (`proOptions.hideAttribution`), credited in ⋯, and theuth subscribes to React Flow Pro as the project asks of those who hide it — Q3 |

React Flow specifics: frames are parent nodes (`parentId`); positions are written to Yjs on drag end, not per frame; the Yjs ↔ React Flow binding is ours and small (the collaborative example on reactflow.dev is a Pro example and is not copied); `onlyRenderVisibleElements` is off (§3.6).

### 7.2 Dependency on 198 (Liquid Glass): none

Rev 1 said the ink plugin is "built exactly like `tauri-plugin-glass`" and the pill uses "198's material". On `main` there is no `crates/tauri-plugin-glass` and no `docs/design/198-*.md` (glob); `docs/design/199-notes.md#L431` records the crate as gone after a merge; six `origin/feat/198-*` refs are unmerged (`.git/packed-refs#L42-L47`). So: the composer is **CSS glass in every phase** and switches to a native material only if 198 lands, as a follow-up outside this design. `tauri-plugin-ink` is its own Swift Tauri plugin and takes nothing from the 198 branch; if 198 is still unmerged at P3, the plugin's build wiring is done fresh (the non-Swift alternative is objc2 beside Tauri's webview in the style of `crates/desktop/src/ios_webview.rs#L1-L5`, at the price of PencilKit delegates written through objc2).

## 8. Phases

| Phase | Ships | Done when |
|---|---|---|
| **P0** model, binding, tools | `DocKind::Whiteboard`, the maps, the yrs reader and outline export, `Kind::Whiteboard` marker, binding and alias tables, `<whiteboard>` / `<whiteboard_changes>` injection, `wb_*` tools, blob RPCs, the two prompt sentences. Spikes: `takeSnapshot(rect)` of the main webview includes sandboxed frames on macOS and iOS; the offline kill test | an agent creates a whiteboard, reserves and fills 5 cards of 3 kinds, links them, pushes a version; outline and changes goldens pass, including after a compaction and for two bound sessions |
| **P1** the table | full-screen view on iPad, Mac and web: cards, LOD, shield / enter, device presets, version stacks, handles and link mode, frames, select / dim, CSS glass composer with grabber, cluster, reply bubble, placeholders, Undo this turn, outline view, sheet drill-in, `show_whiteboard` tile, *Add to whiteboard…*, snapshots | the §1.1 flow works without ink on the iPad simulator and the Mac; 60 fps pan/zoom with the 100-card fixture |
| **P2** ink over pointer events | pointer capture with Pencil pressure on iPad, the renderer, palm rule, anchoring, marks and ink life, per-card images, captions with `describe-point`, the no-vision path | the §1.1 flow on a real iPad with a Pencil; 2 000 strokes at 60 fps; **Sky runs the productivity test (§9.1) and it passes** |
| **P3** native Pencil — committed, then retired (R13) | `tauri-plugin-ink`: capture-only `PKCanvasView`, routing, hand-over, `PKToolPicker`, tap / squeeze, on-device handwriting text (Vision) for mark captions. After R13 only Vision handwriting and the tap / squeeze forwarder remain | entry: P1 shipped (built regardless of the §9.1 result — Sky's ruling R11, superseded by R13). Exit: the §6.2 gate — not met (gate 4), the layer was removed |
| **P4** presence | Follow, `wb_focus`, spotlight, edge chevrons, live-talk bubbles | an agent walkthrough of 5 cards plays in sync with speech |
| **P5** export | whiteboard → note brief, PNG, PDF | a worker briefed only with the exported note builds the right screen |

## 9. Checks

1. **Unit:** outline and changes renders (golden, including baseline re-derivation after compaction and a stale notice); placement never overlaps a card or ink (property test); anchoring by stroke length; mark clustering; ink life transitions; packed-stroke round trip; alias stability across sessions.
2. **Concurrency:** two offline clients add cards and strokes and merge with no lost item (ULIDs); undo on one never removes the other's or the agent's work.
3. **Offline (iPad):** 50 strokes drawn with the engine unreachable, app killed, relaunched, engine back → all 50 present. Y.Doc of a 60-minute fixture session stays under 2 MiB.
4. **Simulator (`glass-198-ipad`, 333) and device:** chrome rules (a palm or a Pencil never summons the cluster; the grabber always does); pen never pans; enter / leave a card; the pill clears the keyboard and the corner; Pencil corner swipe for Quick Note versus the pill, on a real iPad.
5. **Pixels:** fixture screenshots are taken with Chrome headless or the simulator, never the session browser (it has drawn xyflow blank before). A page card's snapshot contains the page.
6. **Token budget:** on the §1.1 script, image tokens are ≤ 25 % of "screenshot every turn", and no message exceeds 1.2 MP.
7. **Security:** a page whose `describe-point` answer is an instruction reaches the agent quoted, capped and prefixed; live-card budget holds with 20 page cards.
8. **Accessibility:** the whole §1.1 flow except drawing can be done from the outline view with VoiceOver and with a keyboard; Reduce Motion removes every animation named in §4.5.

### 9.1 The productivity test (run by Sky after P2; it tunes the experience, it does not gate P3)

Two comparable design tasks from the real backlog, one in chat + tiles, one on a whiteboard, about 20 minutes each, ≥ 3 directions and ≥ 6 refinements; three such pairs.

| Measure | Counted from | Whiteboard must be |
|---|---|---|
| Back-references — owner messages that name an earlier artefact by description ("the one from before", "two messages ago") | transcripts | 0 (chat's count is the baseline) |
| Owner words per refinement message (median) | transcripts | ≤ 50 % of chat's |
| Finding things — uses of Fit or Outline to locate a card | client counters | ≤ 2 per session (chat: scroll-backs of more than a screen, counted for contrast) |
| Litter at the end — cards beyond directions + 3; strokes at full strength that are neither fresh nor pinned | the document | 0 and 0 |
| Hand-off — a fresh session given only the whiteboard builds the chosen variant | that session | no question about *which* variant; corrections ≤ chat's |
| Wait — send to first placeholder | client timing | ≤ 3 s |

Pass: the first, second, fourth and fifth rows hold in two of the three pairs. Fail: the review of why goes in this file and drives a rev; native Pencil (P3) was built regardless (R11), then retired (R13).

## 10. Sky's rulings (2026-11-25) and additions

| # | Question | Ruling |
|---|---|---|
| R1 | Composer target with no bound session | a **new session** with the whiteboard attached |
| R2 | Finger ink on iPad with a Pencil paired | **no — Pencil only**; toggle in ⋯ |
| R3 | React Flow attribution | **shown, top-left, only while the control cluster is up** (free licence; no Pro) |
| R4 | Manual stroke-to-link | **no** in v1 |
| R5 | Native capture OS | ~~**iPadOS 26+**; older iPads keep pointer ink~~ — **superseded by R13**: every iPad uses pointer ink |
| R6 | Handwriting text with marks | **yes** (§6.6) |
| R7 | Sharing with a contact | **later** |
| R8 | Baking answered ink above 2 MiB | **yes, automatic** |
| R9 | Design 71's files | **do nothing** |
| R10 | Notebook item kind | **yes, after P2** |
| R11 | Native Pencil gated on the productivity test | ~~**no — build P3 regardless**~~ — **superseded by R13**: built, shipped in 2.30.2, lost the A/B, removed |
| R12 | Session indicator on the composer | **a small dot + short session name** with a link glyph; tap → session picker |
| R13 | Ink engine (2026-11-25, after a real-iPad test with "Native Pencil ink" off: "no lag") | **web-only on every platform** (Pointer Events + perfect-freehand on iPad, Mac and browser). Supersedes R5 and R11. `tauri-plugin-ink` keeps Vision handwriting (R6) and the Pencil double-tap / squeeze forwarder; the hybrid is at tag `ink-hybrid-final`. Tripwire: a 240 fps latency baseline on the real iPad (`218-recon/ink-decision.md` §4) |

Additions from Sky the same night:

- **A1 Dark mode.** Every surface of the whiteboard follows the app theme: canvas background (paper / dark slate with dots), cards, links, frames, the glass (light: white 52 % + blur; dark: near-black 48 % + blur, light hairline), ink colours (the default pen colour flips black ↔ white; coloured inks keep hue, adjust lightness for contrast ≥ 3:1 on the canvas), snapshots taken in the current theme and re-taken on a theme switch when live. Page cards get the theme variables like every 120 page.
- **A2 Model picker on the composer.** The expanded composer sheet carries the same model picker as the chat composer (design 215: a quiet model label inside the composer; tap → picker), applying to the bound session. The collapsed pill shows only the session dot + name.
- **A3 Built-in skill `whiteboard`.** Driving a whiteboard is a built-in skill shipped with theuth (like `board`): when to use a whiteboard vs a board vs chat; reading `<whiteboard>` / `<whiteboard_changes>`; placement and version-push rules (refine = push a version, new direction = new card); linking; dimming with reasons; never interpreting ink without the owner's words; reading mark images and handwriting captions; using `wb_*` correctly and economically; worked examples. The system-prompt sentences in §5.5 point at it.

## Changes from rev 1

Every finding was checked against the working tree on 2026-11-25 before it was accepted; the citations in the sections named are the ones re-read. Accepted 28, adapted 5, rejected 0 as a whole (one sub-claim of B3 rejected).

| Id | Verdict | What changed, where |
|---|---|---|
| **B1** PencilKit overlay | accepted | Native layer is capture-only; Perfect Freehand renders all committed ink; no transform, no `pk` blob, no stroke-id diffing, no native undo (§6.1, §6.2). Pencil-on-handle exception dropped; the Pencil links only inside link mode (§5.1). First responder, palm rule, hand-over at pen-idle, picker restricted to matchable inks, iPadOS 26+ at run time, route B, and an exit gate are specified (§6.2). Staged after pointer ink but committed (§8) |
| **B2** no rasteriser | adapted | `takeSnapshot(rect)` of the main webview is the one rasteriser; snapshots and mark images are stored blobs; title chip before a snapshot; web client composes from blobs; `describe-point` RPC with untrusted-text handling; export moved to P5 (§3.4, §6.5). Not taken: the Rust compositor for headless reads — the engine serves stored blobs and renders nothing |
| **B3** substrate, journal, cursor, attachment | adapted | What is inherited vs built is listed (§3.1); journal and cursor deleted; outline + baseline diff copied from the notebook mechanism (§5.2); ULIDs + engine aliases (§3.2); marker *and* an explicit binding (§0.2, §5.4); packet lines moved into the engine's message (§5.4); own undo (§4.6). **Rejected sub-claim:** "no IndexedDB persistence anywhere" — link devices persist state and pending updates (`lib/doc/queue.ts#L1-L5`, `provider.ts#L113-L115`); what is true is the 4 MiB state bound and the Mac's in-memory queue, both now stated (§3.1, §9 check 3) |
| **B4** accumulation | accepted | Version stacks; ink belongs to (card, version); fresh / sent / answered / pinned (§0.9, §3.3, §3.5); §1.1 rewritten around it; "litter" is a pass/fail row of the productivity test (§9.1) |
| **S1** 198 not on `main` | accepted | Dependency removed: CSS glass in every phase, own plugin (§7.2). The 198 branch contents were not readable from this session (no git access); only its absence on `main` and the unmerged refs were verified |
| **S2** prior rulings | accepted | §0.10 and the amendment table §7.1 (65, 73, 147, 208) |
| **S3** undo | accepted | §4.6 |
| **S4** using a live card | accepted | Shield, double-tap to enter, ink never enters (§4.4) |
| **S5** page viewport | accepted | `viewport` + presets, frame preset, `wb_place(…, device)` (§3.2, §3.6) |
| **S6** session-scoped sources | accepted | `{session, …}` sources, gone-source behaviour, working-tree diffs frozen as a patch artifact, read authorised by binding ∧ reference (§3.5) |
| **S7** drill-in | accepted | In-whiteboard sheet; sibling tile only when the whiteboard is itself a tile; `show_whiteboard` kind (§4.4) |
| **S8** crop budget | accepted | One image per inked card, 1.2 MP per message, numbers restated; no-vision path (§4.3, §6.4) |
| **S9** batches vs sessions | adapted | Batch counter deleted. Delivery is per session, derived from each session's baseline; the *visible* life of ink is per whiteboard because one person looks at it (§3.3, §5.2). Only in-view fresh ink is sent by default (§6.3) |
| **S10** anchor rule | adapted | ≥ 60 % of stroke length; free ink touching exactly one card rides with it, ink between cards stays; agent moves are refused under unanswered ink (§3.3) |
| **S11** presence while working | accepted | Placeholders, archive-not-delete, hands off owner-placed cards (§4.5, §5.3) |
| **S12** reply bubble | accepted | Stays until next send or dismiss, pinned to its card (§0.8, §4.5) |
| **S13** chrome summon | accepted | Grabber inside the pill; auto-hide switch (§0.3, §4.1, §4.2) |
| **S14** React Flow specifics | accepted | Attribution decision (§7.1, Q3); no `onlyRenderVisibleElements` (§3.6); own Yjs binding, write on drag end, frames as parent nodes (§7.1) |
| **S15** naming | adapted | "Whiteboard" everywhere, prompt sentences in both blocks (§0.1, §5.5). Design 71's files are annotated, not renamed (Q9) |
| **S16** accessibility and phone | accepted | Outline view; phone default; Reduce Motion (§4.5, §4.7, §9 check 8) |
| **S17** security of live cards | accepted | Live budget and demotion, `by` is display-only, sharing consequence stated (§3.2, §3.6, Q7) |
| **Nits 1–5** versions; `.pencilOnly`; tap-undo is Procreate's; tap / squeeze preferences; "~9 ms" | accepted | §0.7 cites installed pins and says bump, `yrs` named; §6.2; §0.4; §6.2; the latency number is gone and the web-view claim is now "cannot give picker, Settings-following gestures or system palm rejection" |
| **Nits 6–8** diff anchors lighting a mock; 12 × 8 grid; Quick Note corner and windowing | accepted | cut from §1.2; rows per frame in reading order (§5.2); pill inset, "whole screen" hedged, device check (§4.1, §9 check 4) |
| **Nits 9–11** keyboard inset; card href; fixture screenshots | accepted | §4.3; `theuth://whiteboards/<id>#<card>` (§1.1); §9 check 5 |
| **Nit 12** the mock | accepted | Rebuilt as seven states: new cards never land on ink; composing, sent and answered are separate frames; 44 pt cluster buttons with Redo and no permanent labels; pill text contrast raised; pill inset 24 pt in a true 4 : 3 screen; states added for the version stack, answered ink and a placeholder |
| Review's "Suggested changes" 1–12 | all taken | 1–2 → §3.3, §3.5; 3 → §4.4; 4 → §3.6; 5 → §4.5; 6 → §4.5; 7 → §6.4; 8 → §4.6; 9 → §4.7; 10 → §4.2; 11 → §1.2; 12 → §8 (with P3 committed rather than conditional on latency alone) |

Not verified in this revision: the contents of the `origin/feat/198-*` branches; that `takeSnapshot` of the main webview includes sandboxed `theuth-page` frames on iOS (P0 spike); the drawing-recogniser routing over `WKWebView` on iPadOS 26/27 and the `hitTest` behaviour (P3's first spike); that stylus touches can be filtered ahead of React Flow's pan handler on iPad (P2's first task); how artifact reads are authorised across sessions beyond the comments cited in §3.5; the w·h/750 token estimate for providers other than Anthropic; iPadOS's own three-finger undo swipe (not used here). Nothing was built or run on a device.