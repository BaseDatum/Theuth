# 71 — Boards (v2): the reader's mechanisms first, the machinery second

Status: design v2, 2026-09-26, for Sky's rulings before code. Replaces v1 (commit a51e38f) in full; the raw evidence stays in `71-whiteboard-recon.md` (cited as R §x.y; §B.7 and §D.9 were added for this revision). Extends 30 (surfaces, catalog, one Rust definition per component), 59 (documents: BlockNote over Yjs, `doc_*` tools, NoteChat), 60 (codeindex), 65 (`Graph`, mermaid 12, render queue), 66 (attachment markers), 70-POPOUT-2 / 70-MOBILE-FIXES. Amends 30 §0.7/§10, 59 §9, 60 §8, 65 §0.6/§3.1, 66 §3.3 — listed in §6.4.

Sky's ruling that drives this revision, verbatim: *"what informs our decisions is not how easy is it to do something like this in our code base but rather, how can we take inspiration from whiteboard and produce something that truly reduces the cognitive load of absorbing ideas."*

What v1 got wrong: it led with the substrate ("a board is a `DocKind::Board` note drawn with `doc_*` tools + one `Excerpt` component") and dropped the two mechanisms that do the most work when a reader absorbs a codebase — a single active anchor that lights the sentence, the diagram unit and the code window together, and the split pane / stepped tour — because both are awkward inside a one-column BlockNote editor. It also left diagram units as pictures, deferred typography to a taste question, and gave sequence diagrams no addressable units. v2 states the reader mechanisms as requirements (§1), then designs the smallest architecture that satisfies them (§2). Where the substrate of v1 survives the requirements, §2 says so explicitly.

A note on the evidence: the second recon pass (R §B.7) shows Whiteboard itself has **two independent, click-only** anchor systems — a prose link opens a 560 px side peek and washes itself; a diagram unit opens a fullscreen tour and recolours itself; prose and diagrams never light each other up, and there is no hover coupling anywhere. §1 borrows what each mechanism does for the reader and, where the reader lens says so, requires more than Whiteboard built (M4, M6, M11–M14).

## 0. Questions for Sky (each with the default the rest of this doc assumes)

1. **Ship a serif for prose.** Whiteboard forces `h1,h2,h3,p,li,blockquote` into Newsreader 15 px/1.72 and everything else into Geist Mono 11 px (R §B.2); the two registers let a reader tell *claim* (author's voice) from *evidence* (paths, gutters, captions, TOC) by letterform before reading a word. Default: **ship Newsreader 400 / 400 italic / 600 (OFL 1.1, R §B.7) and set `--font-serif: "Newsreader", Georgia, serif` on board prose only**; a `boards.serif` setting falls back to `--font-sans`. Override: two registers with `--font-sans`/`--font-mono` only (the layout rules still apply).
2. **Hover preview.** Whiteboard couples only on click (R §B.7). Default: **on a fine pointer, hovering an anchored span/unit/excerpt for 120 ms lights every element that shares its anchor — no pane opens, no layout moves; click pins.** Override: click-only, like Whiteboard.
3. **What a unit click opens.** Whiteboard: any diagram unit click opens the fullscreen tour, which hides the prose (R §B.7). Default: **click pins the anchor and opens the side pane beside the prose; the tour opens from the diagram header's Tour button, `T`, or ⌘↵ on a unit** — the sentence stays in view. Override: Whiteboard's behaviour (unit click → tour).
4. **Live excerpts.** Whiteboard pins commits and flags `staleSources` (R §A.2). Default: **an `Excerpt` embeds its text (so it renders everywhere) and the board reader verifies every anchor against the working tree on open through one new read-only RPC (`code_locate`, §2.5), marking `changed` / `moved → L88–L101` / `missing` and offering "show current".** Override: embedded text only, no RPC, staleness found only by the agent's read-back.
5. **Which session answers a question from the board.** Default: **the session that drew it** (bound in the `note-chat` store, R §D.2); a board opened with no live session falls back to NoteChat's first-message-creates-a-session (59 §9.3). Override: always a fresh board-bound session.
6. **Sequence diagrams.** mermaid 12's sequence renderer exposes only ordinal `data-id="i<n>"` on the arrow line, shared with notes/loops, and no id on the label (R §D.9); the grammar cannot carry a code reference. Default: **a `Sequence` catalog component with addressable steps and `ref` (§2.4); mermaid `sequenceDiagram` stays for un-anchored sketches.** Override: keep mermaid and address steps by position (fragile: one added `note` shifts every anchor).

Dropped from v1 §0 because they were about convenience, not the reader: where boards live (they are documents, §2.1), the excerpt cap (a decision, §2.5), open-in-panel (§2.11), `ref` as "in-board anchor only" (superseded by §2.3: `ref` is a real binding the renderer acts on).

## 1. Reader mechanisms — the requirements

Each mechanism: **what it does for the reader** (Whiteboard evidence), **the theuth requirement** (testable), **acceptance check**. Requirements are numbered M1–M14 and are what §2 must satisfy and §5 must ship.

### M1 · One shape per idea
- *Reader:* one diagram chosen for the shape of the thing — "`sequence` if participants interact over time… `flow_diagram` if the interesting part is branches, retries or state transitions… `database_lens` if… a schema change" (R §C.5 rule 4); "pick one diagram that best shows the shape of the change" (R summary 8). A second diagram of the same thing makes the reader reconcile two pictures.
- *Requirement:* the skill (§3) makes the model choose exactly one diagram kind per section from a routing table; the board reader draws diagrams at full column width (up to 1120 px, R §B.3) so one picture can carry the section.
- *Check:* in the ten steering prompts of step 9, no section of a produced board holds two diagrams of the same subject; the skill's routing table is the visualize.md table extended, not a second one.

### M2 · Fixed section order
- *Reader:* "each of these should be written as a top-level `section`, in this order: what/why … requirements … design … implementation" (R §C.5 rule 3); "keep whiteboards short and sweet… feel free to omit sections" (rule 10). A known order means the reader knows where they are and what is coming without a map.
- *Requirement:* boards use the order **What and why → Map → How it works → Decisions and trade-offs → Checks → Open questions**, any section omissible, never reordered; the TOC (M9) reflects it.
- *Check:* `board-check` (step 9) fails a reference board whose `##` headings are out of this order; the skill text carries the order verbatim.

### M3 · Code only where the mechanism or invariant lives; links elsewhere
- *Reader:* "`code_peek` for the few spots that carry the mechanism or an invariant; link everything else inline" (R §C.5 rule 7); "when something… describes actual code in the codebase, always default to attaching/hyperlink code" (rule 11). Quoted code is the most expensive thing on the page to read; a link is a promise the reader can cash only when they need it.
- *Requirement:* an `Excerpt` block for ≤ 3 spots per section; every other claim about code is a `code:` link or a diagram unit `ref` (§2.3); **a link is never a dead end** — it opens the code window in the side pane (M5) even when no `Excerpt` on the board covers it.
- *Check:* reference boards: ≤ 3 `Excerpt`s per section, ≥ 1 anchored claim per paragraph in "How it works"; every `code:` link resolves in the side pane in the browser smoke.

### M4 · One active anchor across prose ↔ diagram ↔ code
- *Reader:* Whiteboard washes the clicked prose link (`a[data-review-anchor-open]`, `--link-open-wash`) and recolours the active sequence message / selected flow node (`--accent`, `--marker-tint`) (R §B.7, §B.5) — but each side only for itself: prose ids are `${blockId}:${href}`, `code_peek` ids are the block id, "same source ≠ same anchor" (R §B.7). The reader still has to find the node the sentence is about and the lines the node is about by eye. The mechanism's whole value is removing that search: the sentence, the unit and the lines are *one thing* and look like one thing.
- *Requirement:* a board has **one anchor model** (§2.3): prose spans (`code:` links), diagram units (`ref`), `Excerpt`s and excerpt line ranges all resolve to anchors; **exactly one anchor is active** at a time (hover < pinned < tour step, §2.3); every element whose anchor overlaps the active one carries `data-anchor-active` and the same `--marker-tint`/accent treatment; activating from any of the three sides lights the other two within one frame; keyboard focus on an anchored element activates it (hover on fine pointers, §0 Q2).
- *Check:* browser smoke: hover a `[parse()](code:crates/surface/src/lib.rs#L71-L84)` link → the `Graph` node with `ref: crates/surface/src/lib.rs#L71` and the `Excerpt` covering L71–L84 light up in the same paint; click the node → the link washes and the excerpt scrolls into view; unit test on the anchor resolver (overlap rules of §2.3) with ten fixtures.

### M5 · Split pane: the code beside the sentence
- *Reader:* a prose link opens its source in a 560 px right column (`grid-template-columns: minmax(0,1fr) 10px var(--side-peek-width, 560px)`, resizable 360–920) while the document stays put (R §B.3, §B.7). The reader never leaves the sentence to see the code it talks about.
- *Requirement:* pinning an anchor opens a side pane with the **code window** for it: the board's `Excerpt` if one covers the range, else the range fetched live (§2.5); the pane shows the anchor's path, the claim (the sentence or unit label that pinned it) and the marked lines; the column is 560 px default, drag 360–920; below 1080 px container width the pane becomes a bottom sheet; below 720 px the column collapses (R §B.6). One pane, one content — pinning replaces (R §B.7 "no stack").
- *Check:* browser smoke at 1440 / 1024 / 390 px: pin from a link, a node and an excerpt line; the pane shows the right range each time; the prose column does not reflow more than the width change; `Escape` closes and returns focus to the pinning element.

### M6 · Stepped tour
- *Reader:* the tour is a fullscreen grid — the same diagram re-rendered left with the active unit recoloured, one numbered "stop" per unit right (`minmax(0,1fr) 10px 594px`), stops in step order, scroll in the stop feed moves the active unit, Prev/Next buttons, Escape closes (R §B.7). It turns a diagram from a picture into an ordered reading: *this arrow is these lines, next arrow, next lines.*
- *Requirement:* every diagram with ≥ 1 anchored unit offers **Tour**; stops = the diagram's units with a `ref` in authored order (Graph: `nodes` order; Sequence: `steps` order — R §B.7 "one stop per step / per source"); each stop shows the unit label, the code window (M5 rules) and, if the unit has none, the unit's description ("explanation" stop, R §B.7); scroll-sync uses Whiteboard's rule (first stop at or below the scroller top, active if in the top half, else the previous — R §B.7 `activeTargetForScroll`); keyboard `↑/↓`, `j/k`, `Home/End`, `Escape`; focus returns to the unit that opened the tour (Whiteboard has neither arrows nor focus return, R §B.7 — both are accessibility basics, not extras); the TOC offers **Tour this board** = all anchors in reading order through the same component.
- *Check:* browser smoke: a `Sequence` with 6 steps opens a tour with 6 stops; `↓` ×3 recolours step 4 and scrolls its stop; scrolling the feed to stop 2 recolours step 2; `Escape` returns focus to the step that opened it; `prefers-reduced-motion` disables the stage scroll animation.

### M7 · Progressive reveal
- *Reader:* "write small and often: one paragraph per edit, so the document draws itself as you go. Insert a new diagram whole… Link each added flow_node to a node already drawn, so it arrives attached" (R §C.1); units trace in draw order (`lastEdit.units`, R §A.3); the "Agent working…" badge and marker-ink sweep say *someone is writing* (R §C.1). Watching the argument assemble is how the reader learns its order.
- *Requirement:* each block appears on the reader as soon as its `doc_insert` lands (one Yjs update, R §D.2); a new block rises in (opacity 0→1, 6 px, 240 ms; none under `prefers-reduced-motion`); a new diagram draws its units staggered in authored order (40 ms apart, renderer-owned — the model inserts diagrams whole); a **drawing** badge shows while the bound session runs and names its current section; first paint of a finished board draws nothing.
- *Check:* a Python cell inserting 6 blocks lands all six on an open board page in < 1.5 s (measured, step 6); the badge appears while the turn runs and clears when it stops; reopening the board plays no animation.

### M8 · Reading measure and two type registers
- *Reader:* one 860 px column, prose capped at 720 px, diagrams at 1120 px (R §B.3); prose serif 15 px/1.72 with headings 34/22/17 (R §B.2); *all* chrome mono 11 px with .04 em tracking (R §B.1 chrome ramp); code 12 px on 19–20 px lines (R §B.2). The measure keeps the eye's return sweep short; the two registers separate "what the author says" from "what the machine says" pre-attentively.
- *Requirement:* the board reader applies the §2.7 tokens; prose uses the prose face only, chrome (paths, captions, gutters, TOC, badges, stop headers) the mono face only; no third register.
- *Check:* a CSS test walks the rendered board and asserts every element in `.board-prose` computes to the prose family and every `.board-chrome` element to the mono family; screenshots at 1440/1024/390 in the as-built.

### M9 · Collapse and the TOC rail / pill
- *Reader:* each `section` has an anchored `h2`, a chevron at `left: -24px` at rest opacity .45, collapse persisted per session, a collapsed heading dims and shows a summary; the TOC is a 32×32 pill on narrow shells and an always-open rail at ≥ 1360 px; the TOC expands a collapsed section before scrolling to it (R §B.3, §C.4). The reader can shrink what they already know and keep the shape of the whole in view.
- *Requirement:* sections derive from `##` blocks; collapse state is UI state keyed `board:<doc>:<blockId>` (never stored in the document, R §A.6 "layout is renderer-owned"); collapsed heading dims and shows "N blocks · M anchors"; TOC entries = `#/##/###` with the active entry chosen by the M6 scroll rule; rail ≥ 1360 px, pill otherwise; the TOC expands before scrolling.
- *Check:* browser smoke: collapse §2, reload, still collapsed; click its TOC entry → expands and scrolls; at 1440 the rail is visible, at 1024 the pill.

### M10 · Highlight → feedback
- *Reader:* ⌘⇧C "Copy for Agent" turns a prose or code selection into markdown with `Session ID`/`Version` header lines the agent maps back to the document (R §C.1). The reader points at the thing that is wrong instead of describing it.
- *Requirement:* a selection (prose, excerpt lines, a diagram unit, a stale chip) plus **Ask** (⌘⇧A, floating button anchored in the document — R §C.1 anchors theirs in `.review-document`) prefills the bound session's composer with a `[board …]` header (§2.9), the quote, and the board pinned as `[attached note …]` (66 §3.3); the reply edits the board in place and says one line in chat (§3).
- *Check:* select two excerpt lines → composer holds `[board <doc> · block <id> · path:a-b]` + the two lines; send → the agent's `doc_read(blocks=[id])` returns that block; the transcript renders the quote card; header grammar round-trip test.

### M11 · Live excerpts that mark themselves stale *(theuth-only; Whiteboard has `staleSources` against pinned commits, R §A.2)*
- *Reader:* an explanation of a codebase decays; a reader who cannot tell whether the quoted lines are today's lines must re-check every one. Whiteboard pins commits and fetches (R §A.1 `pins`); we own the engine and the working tree.
- *Requirement:* on open (and on **Verify**), the board reader calls `code_locate` (§2.5) for every anchor; each `Excerpt` header shows a chip — nothing when identical, `changed` when the range's text differs, `moved → L88–L101` when `symbol` is given and its `Definition` now sits elsewhere, `missing` when the file or symbol is gone; "show current" swaps the embedded text for the live range; `code:` links and unit `ref`s show the same states on hover.
- *Check:* edit a referenced file in a scratch repo, reopen the board → the chip reads `changed`; rename the symbol's position (insert lines above) → `moved` with the new range; delete the file → `missing`; the RPC is read-only (no `fs` write path in its handler) and rejects paths outside the board's `root`.

### M12 · A board that re-verifies its claims *(theuth-only)*
- *Reader:* "before finishing, read the whole whiteboard back and fix any contradictions/unverified claims" (R §C.5 rule 8) is a rule for the writer; the reader benefits only if it was obeyed.
- *Requirement:* the read-back is machine-checked: `board-check` (agent-side, in the skill) re-reads every `Excerpt` range and every `code:`/`ref` target and refuses to end the turn with a mismatch; the reader's **Verify** (M11) shows the count — `14 anchors · 0 stale` in the header chrome — and **Ask to refresh** sends the stale list as an M10 message.
- *Check:* `scripts/board-check.py` reports 100 % verbatim excerpts on the three reference boards (step 9); after a deliberate edit, the header shows `1 stale` and "Ask to refresh" prefills the stale anchor list.

### M13 · Select-to-ask with the engine's context *(theuth-only)*
- *Reader:* Whiteboard's loop is clipboard → paste → the agent re-fetches by `Session ID`/`Version` (R §C.1). Ours lands in the right composer with the board attached and the anchor named; the agent has `doc_read`, `code_symbol` and the working tree, so the answer can be an in-place edit.
- *Requirement:* the M10 header carries the anchor (`path:a-b`, `node <id>`, `step <n>`) and the block id; the prompt block (§3.1) tells the agent to `doc_read(blocks=[…])` and `code_symbol(path:line)` before answering and to prefer `doc_replace` over a chat paragraph.
- *Check:* the step-8 smoke: a question from a unit produces a `doc_replace` of the block that owns it and one line in chat, in the bound session.

### M14 · Diagram units are objects, not pictures *(precondition of M4/M6)*
- *Reader:* Whiteboard's sequence messages have an 18 px hit-area stroke, a dot, a label, hover ring, `clickable` class, Enter/Space (R §B.5, §B.7); flow nodes are `role=button`-like with `is-selected` (R §B.7). The reader can *touch* the argument.
- *Requirement:* every `Graph` node and `Sequence` step is focusable (`tabIndex=0`, `role="button"`), has a ≥ 18 px hit area, carries `data-node`/`data-step` (Graph already does, R §D.9), reacts to hover/focus/active per M4, and exposes its `ref` as `aria-description`; mermaid blocks stay pictures (documented limit, §0 Q6).
- *Check:* keyboard-only smoke: Tab reaches every unit of a `Graph` and a `Sequence`; Enter pins; ⌘↵ opens the tour; axe reports no missing names on units.

## 2. Architecture that satisfies §1

### 2.1 Storage and streaming — the block document survives
**Kept, explicitly.** A board is a `document` of `DocKind::Board` (`meta.rs:83`, R §D.2): an ordered block tree with stable ids, one `doc_insert` per block, Yjs updates to every subscriber, snapshots, replication, a popout target and a mobile editor already exist (R §D.2, §D.6). Every §1 requirement about *content* is satisfied by blocks the model already writes: sections are `##` blocks (M2, M9), prose anchors are `code:` links inside paragraph blocks (M3, M4), diagrams and excerpts are `json-render` code blocks (M1, M14), progressive reveal is per-block Yjs delivery (M7). What does **not** survive is BlockNote *as the reading surface*: it owns one column, cannot host a side pane or a fullscreen tour, and its code blocks are opaque to a cross-block active state. So: **storage = document; reader = a renderer of our own (§2.2); editor = BlockNote, unchanged.** Rejected again: an artifact kind (no streaming path for files, R §D.1 gaps) and a reply-fence-only board (no persistence, no ids).

Additions to the document crate: `DocKind::Board`; `doc_create(title, markdown?, kind?, root?)` — `root` (absolute repo root the anchors resolve against) is written into the frontmatter block (`frontmatterBlock`, R §D.2) as `root:`; `doc_insert`/`doc_replace` run `surface::check` on `json-render` blocks and return the located error as the tool error (the model gets its correction round, 30 §5).

### 2.2 The board reader — its own read mode
`components/board/BoardReader.tsx`, chosen by `routes/notes_.$id.tsx` when `meta.kind === 'board'`; **Edit** toggles to the existing `NotePage`/BlockNote editor (a board is still a note). Popped form is the same component under `Popped` (R §D.6; the `note` popout target is reused, zero change in `popout.ts`).

- **Input:** `doc_open` + `doc_subscribe` (R §D.2) into a headless `BlockNoteEditor.create({collaboration})` used **only to read `editor.document`** (typed blocks with ids; no ProseMirror view mounted). Each Yjs update yields a new block array; blocks are rendered by id, so an insert is one new element and the M7 draw-in.
- **Block mapping** (renderer-owned, R §A.6 "the data carries no layout"): heading 1–3 → section headers / TOC; paragraph, lists, quote → prose (`.board-prose`, 720 px); table → mono 13 px table (R §B.2); divider → rule; code block `language: json-render` → the catalog renderer (`registry.tsx`) inside a diagram frame at up to 1120 px when the root component is `Graph`/`Sequence`, prose width otherwise (`Excerpt`, `Callout`, `Table`…); `language: mermaid` → `Mermaid.tsx` (un-anchored); other code → `Code`. Inline link with `code:` href → an **anchor span** (§2.3). Unknown/invalid blocks → the existing `FenceFallback` one-liner (R §D.3).
- **Regions:** `[TOC rail 248 px | column 860 px | side pane 560 px]` as a CSS grid on the page; the rail exists at ≥ 1360 px, the pane column is `0` until an anchor is pinned (R §B.3 `minmax(0,1fr) 0 0` → `minmax(0,1fr) 10px 560px`). Tour is a portal over everything (§2.6).
- **Layout responsibilities:** columns and caps (M8); pane open/close and width (M5); tour stage/feed (M6); section collapse and TOC (M9); draw-in and badge (M7); hover/pin/tour precedence (M4). Nothing of this is stored.
- **Header chrome (mono):** title · kind badge · `N anchors · M stale` (M12) · **Verify** · **Tour this board** · **Edit** · Ask about board · Copy link (`theuth://notes/<id>`) · Open in new window (`canPopout()`) · ⋯ (Rename, Export markdown → `show`, Delete).

### 2.3 The anchor model
```
Anchor   := CodeAnchor | BlockAnchor
CodeAnchor := <path>#L<a>-L<b> | <path>#L<a> | <path>        # root-relative, 1-based, inclusive
BlockAnchor := block:<blockId>                                  # for topic boards: a unit that explains a paragraph
```
Where anchors come from (all content the model already writes, none renderer-invented):
| Source | Carries | Anchor |
|---|---|---|
| prose link `[label](code:crates/x/y.rs#L10-L24)` | href | `crates/x/y.rs#L10-L24` |
| `GraphNode.ref`, `Sequence.steps[i].ref` | `ref` string (§2.4) | as written |
| `Excerpt(path, start, text)` | derived | `path#L<start>-L<start+lines-1>`; each gutter row is `path#L<n>` |
| `code:` link with no range | href | `path` (whole file — used by Map nodes) |

**Resolution (the rule that makes three things one):** two `CodeAnchor`s match when their paths are equal and their ranges overlap (a bare `path` overlaps everything in that file); `BlockAnchor`s match by id. An `Excerpt` is the **code window** for every anchor its range contains; a `code:` link or `ref` with no containing `Excerpt` gets a live window (§2.5). Whiteboard's ids are per block and unshared (R §B.7); ours are the reference itself, which is what the sentence, the unit and the lines have in common — and what `board-check` can verify against the tree.

**State** (`components/board/anchors.ts`, one zustand store per open board):
```ts
{ hover: Anchor|null, pinned: {anchor, by: ElementId}|null, tour: {units: Anchor[], index: number, by: ElementId}|null }
active = tour ? tour.units[tour.index] : pinned?.anchor ?? hover           // M4: exactly one
registry: Map<ElementId, {anchor, kind: 'span'|'unit'|'excerpt'|'row', blockId}>  // every anchored element registers on mount
```
Consumers compute `isActive(el) = active && overlaps(registry[el].anchor, active)` — O(registered elements) per change, a few hundred at most on a long board; `data-anchor-active` on the element, `highlight` prop on `GraphSvg` (exists, R §D.9) and on `Sequence`. Setters: `mouseenter` after 120 ms / `focus` → `hover` (fine pointer only; §0 Q2); click / Enter → `pinned` (opens the pane); Tour button / `T` / ⌘↵ → `tour`; `Escape` pops the top of the precedence stack and returns focus to `by`.

**`ref` is a binding, not a decoration** (amends 65 §0.6): the renderer acts on it — highlight, pin, tour stop, live window, stale chip. The sink rule's purpose (no navigation side channel, no fetch a spec can aim at an arbitrary target) is kept: a `ref` can only resolve inside the board's `root` through a read-only RPC, opens nothing outside the board, and fires no app action. In chat fences and apps `ref` is inert (no board, no root).

### 2.4 Diagram primitives with addressable units
- **`Graph`** (exists; own SVG + ELK, `data-node`, `highlight`, `onSelect`, R §D.9): gains `GraphNode.ref?: string` (validated as a `CodeAnchor`/`BlockAnchor` string). Covers Whiteboard's `flow_diagram` (nodes, edges, direction, `state` for added/removed/modified — R §A.1, §B.5) and the map. Units = nodes with `ref`; edges are not anchors (Whiteboard's edges carry no source either, R §A.1).
- **`Sequence`** (new, `crates/surface/src/spec.rs`; §0 Q6):
  ```
  Sequence(actors: [{id, label}] (2..12), steps: [{from, to, label, style?: call|return|async, ref?}] (1..40), title?)
  ```
  Validation (located errors): `from`/`to` ∈ actors (Whiteboard's check, R §A.1 `sequence`), `label` ≤ 80, `ref` grammar. Desktop renderer `components/chat/surface/Sequence.tsx`: lanes ≥ 176 px, first message at 112 px, 76 px per step (R §B.5 values), return dashed `6 4`, async open arrowhead (R §B.5); each step is `<g data-step={i} role="button" tabIndex=0>` with an 18 px hit stroke, a dot that becomes a numbered badge on the tour stage (R §B.5). TUI arm: `1. parser → validator  check(spec)   crates/surface/src/validate.rs:12-40`. `to_mermaid()` lowering (as `Graph` has) emits `sequenceDiagram` for Slack/Signal/markdown export and drops `ref`.
- **mermaid** stays for `stateDiagram-v2`, `classDiagram`, ER and un-anchored sketches: a picture inside `DiagramFrame` with pan/zoom (65 §3.4). The skill says when (§3.2 rule 4). Not addressed from outside: ordinal ids and an un-idd label (R §D.9) would make anchors break on the first added `note`.
- Whiteboard's `call_stack_diff` → `Graph` TB with `state` per node and a `Diff` per changed symbol; `database_lens` → `Table` + `Graph` — unchanged from v1 until a real board shows the gap.

### 2.5 `Excerpt` — in flow, in the side pane, and live
```
Excerpt(path, text, start?: int ≥ 1 = 1, lang?, caption?, mark?: [[from, to]], symbol?: string)
```
Caps: ≤ 120 lines and ≤ 16 KiB (error, not truncation); `path` root-relative, no `..`, no scheme; `mark` within the range. `symbol` (optional) is the definition name the range quotes; it enables `moved` detection (M11). Anchor as in §2.3. TUI arm and `flatten.rs` as v1 (header line + numbered gutter, `▶` on marked rows).

**In flow** (`components/chat/surface/Excerpt.tsx`, registered in `registry.tsx` so it also renders in chat and artifacts): header (mono) `crates/surface/src/lib.rs:71-84 · parse · rust` + live chip + copy; body highlighted by `@streamdown/code` with a gutter from `start`; marked rows tinted `--marker-tint` + 2 px accent bar (R §B.1, §B.5 `aria-current` row); 18 visible lines then scroll with "show all" (R §B.4 `heightMode: capped` = 18 lines); caption under in mono muted. Rows carry `data-line` and register as row anchors, so a `code:` link to L73 lights exactly that row.

**In the side pane** (M5): the same component with `heightMode: content` (R §B.7 tour stops use `content`), plus the claim line (the sentence or unit label that pinned it) above it.

**Live** (M11; §0 Q4) — one new read-only RPC and one actor message:
```rust
// crates/codeindex/src/lib.rs — `ask` → Result<Vec<Located>, String>
pub struct Locate { pub root: PathBuf, pub anchors: Vec<LocateAnchor> }          // batch: one call per board open
pub struct LocateAnchor { pub path: String, pub start: Option<u32>, pub end: Option<u32>, pub symbol: Option<String>, pub text_sha: Option<String> }
pub struct Located { pub status: LocateStatus, pub start: u32, pub end: u32, pub text: Option<String> } // Same | Changed | Moved | Missing
```
`api.rs`: `code_locate(root, anchors)` → `send(&codeindex()?, Locate{..})`. Semantics: read the range from the working tree (never the index's cached body — R §D.7 "`body` is not a faithful excerpt"); `Same` when `sha256(text) == text_sha`; else if `symbol` is given, look up the `Definition` by name in `path` (`store.rs:740-756`) — a different `line_start` is `Moved` (with the new range and text), the same range with different text is `Changed`; no file / no symbol → `Missing`. Paths outside `root` are rejected; `root` must be a git top-level the codeindex knows (60 §10). Results are cached per `(path, mtime)` in the reader. The renderer never writes.

### 2.6 Split pane and tour — geometry and behaviour
- **Side pane:** page grid `minmax(0,1fr) 10px var(--board-pane-width, 560px)` when pinned (R §B.3 :1930-1932), resizer 10 px, width persisted, min 360 / max 920 (R §B.7). Content: claim line · code window · anchor meta (path:a-b, symbol, live chip) · **Ask about this** (M10). ≤ 1080 px container: bottom sheet at 40 % height (the `ArtifactPanel` sheet primitive, R §D.1). ≤ 720 px: column collapses to one (R §B.6).
- **Tour:** portal `position: fixed; inset: 0`, `role="dialog" aria-modal`, grid `minmax(0,1fr) 10px var(--board-tour-pane, 594px)` (R §B.7), pane 360–760, stage ≥ 480 (R §B.7). Stage = the same diagram component re-rendered with `fullscreen` and `highlight=[active]`, auto-scrolled to the active unit (R §B.7 stage scroll-follow). Feed = one `<section class="tour-stop" data-anchor>` per unit: numbered badge, unit label, `from → to` for sequence steps (R §B.7), code window, or the unit's description when it has no `ref`. Scroll-sync by `activeTargetForScroll` (R §B.7); Prev/Next pill; `↑/↓`, `j/k`, `Home/End`; `Escape` closes and focuses `by`. ≤ 1080 px container: stacked `minmax(180px, 40%) 1fr` (R §B.6 flow tour). Tour state `{blockId, index}` persists per board session (R §B.7 `useTourPersist`) so a reload lands on the same stop.
- **Tour this board:** units = every anchored element in document order (spans, units, excerpts); stage = the section's diagram if any, else the excerpt.

### 2.7 Typography and tokens (`components/board/board.css`; every value R §B unless noted)
| Rule | Value | R |
|---|---|---|
| Faces | `--font-serif: "Newsreader", Georgia, serif` (§0 Q1; files `newsreader-{400,400i,600}.woff2`, `font-display: swap`); chrome `var(--font-mono)` | §B.2, §B.7 |
| Column | `max-width: 860px; margin: 0 auto 96px`; inline padding `clamp(20px, calc((100cqi - 720px) * .122 + 20px), 64px)` | §B.3 |
| Prose | `width: min(100%, 720px)`; 15 px / 1.72; `li + li` 8 px; `h1,h2,h3,p,li,blockquote` in the prose face | §B.2, §B.3 |
| Diagrams | up to 1120 px (1000 px while the pane is open) | §B.3 |
| h1 / h2 / h3 | 34/40 w500, 28 above 18 below · 22/28 w500, 40 above 12 below · 17/23 w600, 30 above 10 below; `scroll-margin-top: 24px` | §B.2 |
| Chrome | mono 11 px, w400/600, tracking .04 em; micro-labels 10 px w700 .08 em; control 22 px, radius 5 | §B.1 |
| Code | mono 12 px on 19–20 px rows; gutter `42px 14px 1fr`, `muted-foreground` right-aligned; frame 1 px `border`, radius 6 | §B.2, §B.4 |
| Active / selected | `--marker-tint` = accent at 14 % (light 8 %) + 2 px accent bar; open link wash accent at 15 % | §B.1 |
| Diagram active unit | stroke accent, sequence `stroke-width 2.4` vs 1.25 rest, label w700 accent; muted edges `--edge-muted` | §B.5 |
| Change colours | added `success`, removed `destructive`, modified `warning` — strokes and names only (30 §6.3) | §B.1 |
| Section header | flex baseline gap 10, `margin-block: 40px 12px`; chevron 18×18 at `left: -24px`, rest opacity .45; collapsed h2 → `muted-foreground` | §B.3 |
| TOC | pill 32×32 at `top: 32px; left: 24px` → 248 px card; rail ≥ 1360 px; active entry accent | §B.3, §C.4 |
| Motion | block rise 240 ms; unit stagger 40 ms; `prefers-reduced-motion` → none | §B.5 draw-in, §B.6 |
Mapped onto theuth's Tailwind tokens (`accent`, `success`, `destructive`, `warning`, `muted-foreground`, `border`); the only new tokens are `--font-serif`, `--marker-tint`, `--edge-muted`.

### 2.8 Progressive reveal and the drawing badge
Per block via `doc_subscribe` (R §D.2); block ids unseen at the previous render get the rise; a new `Graph`/`Sequence` block staggers its units (renderer-owned — the model inserts diagrams whole, R §C.1 "Insert a new diagram whole"; Whiteboard's per-unit edits and `drawOrder` are not needed because the renderer owns the order). Diagram layouts go through the 65 §4.3 render queue (80 ms, one in flight). Badge: `note-chat` binding → `sessions[session].running` → "theuth is drawing · How it works" (the section of the last inserted block); no lease, no activity RPC — the session state is already in every window (R §D.1 store shape).

### 2.9 Select-to-ask header (client-composed text; M10, M13)
```
[board <docId> · block <blockId> · <path>:<a>-<b> | node <id> | step <n> | stale <k>]
> <quote, ≤ 4 KiB>
<question>
```
Fields after `board <docId>` are optional, `·`-separated; regex mirrors `APP_HEAD` (`lifecycle.ts:371`, R §D.3): `^\[board (\S+)((?: · [^\]]+)*)\]$`. Sent with `prefillDraft(session, text, [noteRef], panel)` and `pinned` = the board's `AttachRef` so `[attached note …]` leads (66 §3.3); running session → the steer path (`AppHost.tsx:430-452` pattern, R §D.3). Transcript renders a `BoardQuoteCard`. `UserInput` stays `{id, text, files, ticket, scope, defer}`.

### 2.10 Mobile — one column, the same anchors
`useIsMobile()` / `usePointerCoarse()` (R §D.5): prose 100 %; diagrams pan/zoom in `DiagramFrame`; no hover (tap = pin); the side pane is the bottom sheet; the tour stacks (§2.6); TOC pill → sheet; excerpt gutter 3 ch, path wraps `anywhere`; section chevron inline; native selection → floating **Ask** anchored in the document; NoteChat keeps `--kb-inset`; no popout on iOS (`canPopout()`); 44 px targets. Anchors, registry and precedence are identical — only the pane's container changes.

### 2.11 Opening a board
`BoardCard` in the chat turn for `doc_create(kind="board")` (title, "drawing… · section" while the turn runs, **Open board**, **Open in new window**) and the `note-chat` binding on create/open (§0 Q5); the notes list shows a board glyph and kind badge; `theuth://notes/<id>` in a reply; `show(content=doc_markdown(id))` for a frozen copy or a text channel (renders the same fences without anchors' liveness). Not in v2: a board inside `ArtifactPanel` (a board needs the page's width for M5/M6).

### 2.12 What changes where
| Crate / dir | Change |
|---|---|
| `crates/surface` | `Excerpt`, `Sequence`, `GraphNode.ref`; shared `anchor::parse` validator; TUI arms; `flatten.rs`; `to_mermaid` for `Sequence`; catalog 36 → 38; `visualize.md` rows |
| `crates/document` | `DocKind::Board`; `doc_create(kind?, root?)` → frontmatter `root`; insert/replace-time `surface::check`; `## Boards` prompt block; `board.md` skill; `first_run` line |
| `crates/codeindex` | `Locate` message (§2.5), reads the working tree, resolves `symbol` via `Definition` |
| `crates/rpc` | `code_locate(root, anchors)` |
| `crates/desktop/ui` | `components/board/{BoardReader,anchors,SidePane,Tour,Toc,Section,board.css}`, `components/chat/surface/{Excerpt,Sequence}.tsx`, `Graph.tsx` ref plumbing, `registry.tsx`/`catalog.ts`/`parse.ts` mirrors, `BoardCard`, `BoardQuoteCard`, `fonts/newsreader-*.woff2` + OFL notice, route switch in `routes/notes_.$id.tsx` |
No new crate; one new RPC; one new dependency-free font asset.

## 3. The model-side instructions — half the product

Whiteboard's renderer is ~8 000 lines of CSS on a Code OSS fork; its *method* is 37 lines of `authoring.md` (R §C.5). Ours: a static `## Boards` prompt block registered by the document plugin (`RegisterPrompt{plugin:"document"}`, R §D.8) and a `board.md` skill written by `first_run` (R §D.8). Attribution: "Method adapted from dev.fast Whiteboard, MIT © 2026 dev.fast" at the top of `board.md` (R §C.6).

### 3.1 `## Boards` prompt block (static, cache-safe)
> A **board** is a long-form explanation the reader scrolls, points at and tours: sections, prose, code excerpts pinned to `path:line`, one diagram per idea, every claim about code anchored so the sentence, the diagram unit and the lines light up together. Draw one instead of an inline reply when the answer has three or more parts, spans several files, or the reader will come back to it — "how does X work", "walk me through this change", "explain this codebase". `doc_create(title, kind="board", root=<repo root>)`, then `doc_insert` one block at a time in reading order; the reader watches it draw. Sections, in this order, omitting any that do not apply: **What and why → Map → How it works → Decisions and trade-offs → Checks → Open questions**. Anchor everything: `[label](code:path#L10-L24)` in prose, `ref` on every `Graph` node and `Sequence` step that describes code, `Excerpt(path, text, start, symbol)` for ≤ 3 spots per section that carry the mechanism or an invariant. Before you finish, run the read-back (`board.md`). A message beginning `[board <doc> · …]` quotes a place on a board: `doc_read(doc, blocks=[…])` and `code_symbol(path:line)` first, then edit the board in place (`doc_replace`) and say one line in chat.

### 3.2 `board.md` — rules adopted, rules changed
Adopted verbatim (R §C.5 rule numbers):
1. *Audience* — "you are writing an interactive rfc-style whiteboard, for consumption by a staff engineer." (rule 1; "whiteboard" → "board").
2. *Order* — the section order (rule 3) with our six names; "keep whiteboards short and sweet when possible (esp. for small changes.) feel free to omit sections." (rule 10).
3. *One diagram* — "pick one diagram that best shows the shape of the change" (rule 4) — the routing: `Sequence` "if participants interact over time"; `Graph` "if the interesting part is branches, retries or state transitions" or components/dependencies/data flow; `Table` + `Graph` "if the interesting part is a schema change"; mermaid `stateDiagram-v2`/`classDiagram` only when no unit needs a `ref`.
4. *Contracts* — "if the change is mostly a new/changed contract, show the key types / interfaces as `code_peek`(s)" (rule 5) → `Excerpt`s with `symbol`.
5. *Entry point* — "always root the flows in the user/agent entry point (eg a button click, CLI command, etc.), including unchanged nodes along the way." (rule 6).
6. *Excerpt economy* — "`code_peek` for the few spots that carry the mechanism or an invariant; link everything else inline" (rule 7).
7. *Incremental* — "Write incrementally. The user sees you write on the canvas in real-time. Show them visual progress every few seconds." (rule 9); "Insert a new diagram whole" (rule 33).
8. *Attach code* — "when something (a phrase in the prose, diagram node, etc.) describes actual code in the codebase, always default to attaching/hyperlink code." (rule 11).
9. *Verified ranges* — "Source paths are repository-relative; line numbers are 1-based and inclusive. Replace example paths and lines with verified source ranges." (rule 31).
10. *Chat vs board* — "Answer in chat when one sentence or one code line does it." (rule 23).
11. *Clusters* — "categorize all the changes into buckets… first, categorize away non-implementation code: tests, docs, generated files and lockfiles… then… split it by the part of the design each file serves… keep each lens small enough to read in one sitting" (rule 22) → one `##` per cluster in a diff board.

Changed, and why:
- *First six steps* (rule 2: register → create → lens subagent → lease → diff → what/why) → **orient (`code_map(focus)`), `doc_create(kind="board", root)`, then "immediately… without any other tool calls, put down a first pass at the what/why section."** No registry, no lease, no subagent: the engine owns the tree and the session is the lease (§2.8).
- *Link grammar* `review-source:head/path#L10-L24` (rule 11) → `code:path#L10-L24`; no side — a board pins one working tree; before/after is a `Diff` block.
- *`ref` is mandatory on units that describe code* (new; Whiteboard's `attachments`/step `source` were optional per node, R §A.1). Reason: M4/M6 exist only for anchored units; an un-anchored node is a picture.
- *Read-back* (rule 8) becomes a procedure: `doc_read(format="markdown")`; for every `Excerpt` → `read_file(path, offset=start, limit=lines)` must equal `text`; for every `code:`/`ref` → `code_symbol(path:line)` or `read_file` must return something; fix or remove; end with one sentence and `theuth://notes/<id>`. (M12.)
- *Unit-by-unit edits* (rule 33 "Change a diagram already on the board one unit at a time") → **replace the block** (`doc_replace`); the renderer staggers units itself (§2.8).
- *Scratchpad rules* (rules 24–25) — not adopted; NoteChat and chat are the scratchpad.
- *`body` never pasted* (theuth-specific, R §D.7): excerpt text comes from `read_file(path, offset, limit)`; callers with `confidence < 0.5` are not stated as fact (60 §21 dev. 14).
- *Answering from a board* (new): the §2.9 header, `doc_read(blocks)`, `code_symbol`, `doc_replace`, one line in chat; a `stale <k>` header means re-read those anchors and re-issue the `Excerpt`s.
- *Block shapes*: one JSON example each for `Excerpt`, `Sequence`, `Graph` with `ref`, `Callout`, `Diff`, `Table`, `Steps`; the json-render error-avoidance rules from `visualize.md`.

### 3.3 `visualize.md` rows
Catalog line gains `Excerpt(…)` and `Sequence(…)`; the pick-by-shape table gains `| a passage of real code with its location | Excerpt |`, `| participants exchanging messages over time, each step tied to code | Sequence |`, `| an explanation with three or more parts, or one the reader will return to | a board (board.md) |`.

## 4. Codebase recipes through the anchor / tour model

### 4.1 `code_map` → the Map section
```python
m = await code_map(focus=["crates/surface"], budget=1500)                 # rows: Tag{path, line, kind, name, signature, rank}
top = [r for r in m["rows"] if r["rank"] > 0][:12]
nodes = [{"id": slug(r["path"]), "label": base(r["path"]), "group": crate(r["path"]),
          "ref": f'{r["path"]}#L{r["line"]}', "state": "active" if r["path"] == focus_file else None} for r in top]
await doc_insert(b, "## Map\n\nOne sentence on the shape.\n\n```json-render\n" + graph(nodes, imports_between(top), groups, "LR") + "\n```")
```
What the reader gets: hover a node → the file's paragraph links in "How it works" wash (they share the path); click → the side pane shows the file's top symbol live; **Tour** walks the twelve files in rank order. ≤ 12 nodes (30 §6.2); `ref` on every node (§3.2).

### 4.2 `code_symbol` → pinned excerpts
```python
s = await code_symbol("crates/surface/src/lib.rs:71"); d = s["definition"]   # {path, line_start, line_end, name, …}
n = min(d["line_end"] - d["line_start"] + 1, 60)
text = await read_file(d["path"], offset=d["line_start"], limit=n)
await doc_insert(b, excerpt(d["path"], text, start=d["line_start"], lang="rust", symbol=d["name"],
                            caption="strict parse, then jsonrepair, then validate", mark=[[d["line_start"]+3, d["line_start"]+5]]))
await doc_insert(b, "Called from " + ", ".join(f'[{base(c["path"])}](code:{c["path"]}#L{c["refs"][0]["line"]})'
                                                for c in s["callers"][:4] if c["refs"][0]["confidence"] >= 0.5) + ".")
```
What the reader gets: the sentence "strict parse, then jrepair" is the excerpt's caption; the marked rows are the claim; each caller link opens its call site in the pane without leaving the paragraph; `symbol` lets the chip say `moved → L88–L101` next month.

### 4.3 `code_impact` → a diff board, and the tour of this change
```python
i = await code_impact(git_range="main..HEAD", include_tests=True)
# changed[{path, status, symbols_changed[{name, kind, line}]}], direct[{path, via, is_test}], transitive[{path, depth}], tests[path]
await doc_create(title=f"What {branch} changes", kind="board", root=root)
# What and why — from the commit messages, in the author's words
# Map — Graph: changed nodes state=danger, direct=active, transitive=muted, groups by crate, edges = via, ref on every node
# How it works — Sequence rooted at the user/agent entry point (unchanged actors included), one step per hop, ref = the NEW code of that hop;
#                one ## per cluster (tests/docs/generated first, then by design part); Excerpt (symbol) + Diff(before=git show base:path, after=read_file) for the symbol that carries the mechanism
# Checks — Checklist of i["tests"], done = ran green;  Open questions — ambiguous_skipped, unresolved_imports
```
**The tour of this change, as the reader lives it:** 1. the card in chat says "drawing · Map"; Open lands on the board while "How it works" is still arriving (M7). 2. The Map shows red (changed), accent (direct) and dim (transitive) nodes; hovering a red node washes the two sentences that mention it (M4). 3. Tour on the `Sequence`: stop 1 is the CLI entry point (unchanged, so its stop is an explanation); `↓` → stop 2 recolours the second arrow and shows the new `dispatch()` lines with the changed rows marked; `↓` → stop 3 is the hop into the changed crate with its `Diff` (M6). 4. `Escape` returns to the arrow; the reader clicks the `Excerpt` header chip — `14 anchors · 0 stale` — because they are reviewing a week later, and one chip now reads `moved → L120–L134`; "show current" swaps the text (M11). 5. They select two lines that look wrong, ⌘⇧A, type "why is the lock taken here?"; the answer is a `doc_replace` of that paragraph and one line in chat (M10, M13).

## 5. Implementation steps (omp-sized; `cargo check --workspace --tests` and the UI trio green at every step; one branch each)

| # | Step | Done when |
|---|---|---|
| 1 | **`surface`: anchored primitives** — `Excerpt` (+`symbol`), `Sequence`, `GraphNode.ref`, shared `anchor::parse` validator with located errors, TUI arms, `flatten.rs`, `to_mermaid` for `Sequence`, fixtures (`valid-excerpt`, `valid-sequence`, `broken-{excerpt-mark,sequence-actor,ref-scheme}`), goldens at 60/100 cols, catalog 36 → 38, `visualize.md` rows (§3.3) | `cargo test -p surface` green; `catalog_compact()` lists `Excerpt(` and `Sequence(`; TUI golden shows the numbered gutter with `▶` marks and the numbered step list; every `Graph`/`Sequence` fixture lowers to mermaid that merman parses; a `ref` of `../x` or `http:` is a located error |
| 2 | **`document`: board kind + instructions** — `DocKind::Board`, `doc_create(kind?, root?)` writing frontmatter `root`, insert/replace-time `surface::check`, `## Boards` prompt block (§3.1), `crates/document/board.md` (§3.2) + `first_run`; from here the agent can draw boards that already render in the BlockNote note page (no anchors yet) | `cargo test -p document` (create with kind+root, list by kind, broken block → `/props/mark/0: …`); a headless engine on a scratch data dir writes `skills/board.md`; `t -p "explain how the surface crate parses a fence, as a board"` produces a board with ≥ 4 sections and ≥ 3 `Excerpt`s by the skill alone |
| 3 | **Desktop primitives in chat** — `Excerpt.tsx`, `Sequence.tsx`, `Graph.tsx` `ref`→`aria-description`, zod mirrors, `parse.ts` parity, `registry.tsx`, `data-line`/`data-step` markup, Newsreader assets + `--font-serif` token (unused until step 6), `just ts-desktop` | `registry.test.ts` renders both fixtures (gutter from `start`, two marked rows; six focusable steps); `catalog.test.ts` pins 38; keyboard reaches every step; `tsc`, vitest, `vite build` green |
| 4 | **First shippable slice: `BoardReader` with the anchor + split pane** — route switch on kind, headless BlockNote read, block mapping, 860/720/1120 columns, `anchors.ts` store + registry + overlap resolver, prose `code:` spans, in-flow `Excerpt`s, hover (120 ms) / focus / click precedence, side pane (560 px, resizable, sheet ≤ 1080) with the board's excerpt or a placeholder "open the file" when none covers the range (live window arrives in step 7), `Escape` + focus return, Edit toggle to BlockNote, popped form | Browser smoke (`bunx vite --port 5192` against the live engine, read-only; or a scratch engine when inserting): the step-2 board — hover `[parse()](code:…#L71-L84)` → the covering excerpt lights the same frame; click → the pane opens with it and the link washes; click an excerpt row → the links to that line wash; Edit → BlockNote with the same content; `anchors.test.ts` covers the ten overlap fixtures |
| 5 | **Diagram units and the tour** — `Graph`/`Sequence` units join the registry (`highlight` from `active`), unit click pins, Tour button / `T` / ⌘↵, `Tour.tsx` portal (stage + feed, scroll-sync rule, `↑/↓ j/k Home/End Escape`, focus return, stacked ≤ 1080), tour persistence, **Tour this board** from the TOC | Smoke: a 6-step `Sequence` → 6 stops; `↓`×3 recolours step 4; feed scroll to stop 2 recolours step 2; `Escape` refocuses the step; keyboard-only run reaches every unit; axe clean on units |
| 6 | **Sections, TOC, typography, reveal** — `##` sections with chevron/collapse (persisted `board:<doc>:<block>`), collapsed summary, TOC rail ≥ 1360 / pill, expand-before-scroll, §2.7 tokens with `--font-serif` on `.board-prose`, `boards.serif` setting, block rise + unit stagger, drawing badge with section name, `prefers-reduced-motion` | Smoke at 1440/1024/390: rail / pill / sheet; collapse survives reload; the CSS register test passes; a 6-insert cell lands all blocks in < 1.5 s with rises; reopening plays nothing; screenshots into the as-built |
| 7 | **Live anchors** — `codeindex::Locate`, `code_locate` RPC (root check, read-only), reader batch on open + **Verify**, chips (`changed`/`moved → …`/`missing`), "show current", header `N anchors · M stale`, live window in the side pane for anchors without an excerpt, **Ask to refresh** | Scratch repo: edit → `changed`; insert lines above the symbol → `moved` with the new range; delete → `missing`; a path outside `root` → error; a `code:` link with no excerpt opens live lines in the pane; `cargo test -p codeindex` covers the four states |
| 8 | **Select-to-ask, card, binding, mobile** — floating **Ask** (⌘⇧A) for prose / excerpt rows / units / stale chips, header composer (§2.9) + `BoardQuoteCard`, `prefillDraft` with the board pinned, `BoardCard` for `doc_create(kind=board)` + `note-chat` binding, notes-list glyph/badge, mobile rules of §2.10 | Header grammar round-trip test; a question from two excerpt rows reaches the bound session and `doc_read(blocks=[id])` returns the block; the card shows "drawing · section" then Open; iOS simulator at 390 pt: sheet pane, stacked tour, tap-to-pin, no popout button |
| 9 | **Reference boards, checks, steering, as-built** — three boards drawn on this repo by the real binary from the skill alone ("how the surface crate renders a fence", "what the step-1 commit changed", "how a popout window boots"); `scripts/board-check.py`; 10 steering prompts (5 "how does X work", 3 "walk me through this change", 2 comparisons that should stay inline); `70-as-built-BOARDS.md` | Each board ≥ 4 sections, ≥ 3 `Excerpt`s, every unit of every diagram has a `ref`, `board-check` 100 % verbatim and every anchor resolvable; ≥ 8/10 of the first eight prompts produce a board, 2/2 comparisons stay inline; as-built records times (create→first block, 6-insert cell, 40-block first paint), deviations and the three widths' screenshots |

Steps 1–3 are prerequisites; **step 4 is the first slice a reader can use** and it already delivers M3, M4, M5, M8's measure and M14 for excerpts on a real question drawn by the agent. Typography and TOC (M8 registers, M9) land in step 6, after the mechanisms.

## 6. Risks, deviations from Whiteboard, licence

### 6.1 Risks
- **Anchor overlap is by range, so a sloppy range lights too much.** A `code:` link to a whole file washes every excerpt of that file. Mitigation: the skill demands line ranges except for Map nodes; the resolver prefers the tightest containing excerpt for the pane.
- **Live verification costs a batch RPC per open.** Bounded by anchors per board (hundreds) and cached per `(path, mtime)`; the working tree is local. If it measures > 200 ms on a 40-block board (step 7), verify lazily per visible section.
- **`ref` weakens 65's sink rule.** Bounded: resolves only under the board's `root`, read-only, fires no action, inert outside boards (§2.3). Amendment in §6.4.
- **Two prompts compete** ("show first" vs "boards"). §3.1 draws the line at three parts / several files / will-return; step 9 measures and tunes once.
- **Headless BlockNote as a parser.** If `BlockNoteEditor.create` without a mounted view proves heavy or unsupported for collaboration, the fallback is the Yjs `XmlFragment` → block JSON walk (BlockNote's own `nodeToBlock`), same output.
- **A board is editable.** Reading mode is the default; Edit is a toggle; a broken block shows its source, as notes do today.
- **Serif shipping** adds font assets to the bundle and a first-paint swap (`font-display: swap`, R §B.7). The setting falls back to sans.

### 6.2 Deviations from Whiteboard, and why (reader lens)
| Whiteboard | theuth | Why |
|---|---|---|
| Two unshared anchor systems; click only (R §B.7) | One anchor model; hover, focus, pin, tour; three sides light together (M4) | Removes the eye search between sentence, unit and lines — the load the mechanism exists to remove |
| Unit click → fullscreen tour (R §B.7) | Unit click → side pane beside the prose; tour on demand (§0 Q3) | The sentence stays in view; the tour is for ordered reading, not for every click |
| No arrows, no focus return (R §B.7) | `↑/↓ j/k Home/End`, focus return, `role=button` units | Accessibility basics |
| Code fetched through a Code OSS editor per peek (R §B.4) | Embedded excerpt text + live verification and live windows via one read-only RPC | Renders everywhere (TUI, channels); still truthful against the tree |
| Server-assigned unit ids, unit-level edits, `drawOrder` (R §A) | Units are the model's ids; blocks replaced whole; renderer staggers | Fewer moving parts; the same reader effect |
| mermaid-free (ReactFlow) | Own `Graph` + `Sequence`; mermaid for un-anchored kinds | Addressable units where anchors matter; pictures where they do not |
| Clipboard copy loop (R §C.1) | Composer prefill with header + pinned board | Lands in the right session with the engine's context |
| Commit pins + share manifest (R §C.3) | Working tree + `root`; sharing by note replication | A board explains a tree the reader has |
Not borrowed: the Code OSS editor and `reveal` verb, software map, trace quotes, MCP server, lease/activity protocol, share links.

### 6.3 Licence
Everything borrowed is MIT, Copyright (c) 2026 dev.fast (R §C.6): the quoted rules in `board.md` and §3.1 carry the one-line attribution; type shapes and CSS values are re-implemented from the recon, not copied. Fonts: Newsreader is SIL OFL 1.1 (Copyright 2020 The Newsreader Project Authors, R §B.7) — ship the three woff2 files with `newsreader-OFL.txt` beside them; the OFL forbids selling the fonts alone and requires the notice, both satisfied. Nothing under `apps/review-desktop/code-oss/` (Microsoft notices) is used. theuth's crates stay `MIT OR Apache-2.0`.

### 6.4 Amendments (same commit as the step that lands them)
- **30** §0.7/§10: catalog 36 → 38 (`Excerpt`, `Sequence`); §6.2: the §3.3 rows; new §12 "`code:` links and anchors" (§2.3). Steps 1/3.
- **59** §9: `DocKind::Board`, `doc_create(kind, root)`; §9.3: "a board's reading mode is `BoardReader` (71 §2.2); Edit is the note editor"; NoteChat binding rule. Steps 2/4/8.
- **60** §8: "`code_symbol.definition` and `code_impact` feed board excerpts and diff boards (71 §4)"; §10: `code_locate` RPC and `Locate` message (71 §2.5). Steps 7/9.
- **65** §0.6: "Exception, 71 §2.3: `GraphNode.ref` is a binding the board reader acts on — highlight, pin, tour stop, live window — resolved only under the board's `root`, read-only, never an action; inert in chat and apps." §3.1: `ref?`; §3.5: `highlight` driven by the board anchor. Steps 1/5.
- **66** §3.3: `[board …]` beside `[app …]` and `[attached …]`. Step 8.
- **00**: one RPC (`code_locate`); no crate, no dependency, no §9 storage change.

## 7. Status
v2 drafted 2026-09-26 on branch `design/71-whiteboard-visuals-v2` from the second recon pass (R §B.7, §D.9) and Sky's ruling. Awaiting answers to §0; step 1 can start the moment they land.
