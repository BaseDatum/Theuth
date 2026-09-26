# 71 — Boards: Whiteboard's visual language for explaining complex topics and codebases

Status: design, 2026-09-26, for Sky's rulings before code. Extends 30 (surfaces: catalog, one Rust definition per component, inline fences), 59 (documents: BlockNote notes over Yjs, `doc_*` tools, NoteChat), 60 (codeindex: `code_map`/`code_symbol`/`code_impact`), 65 (`Graph`, mermaid 12, render queue), 66 (attachment markers), 70-POPOUT/POPOUT-2 (popout targets), 70-MOBILE-FIXES (`canPopout`, coarse pointer). Amends 30 §0.7/§10 (catalog count), 59 §9 (a document kind and a reading mode), 65 §0.6 (one in-board anchor on a node). Raw recon with file:line evidence for every claim about Whiteboard: `71-whiteboard-recon.md` (cited below as R §A/B/C/D).

Sky's ask, verbatim: *"we need to write a design doc to HEAVILY borrow from this app as well to visualize things in the same style, especially complex topics or codebases."* — the app being dev.fast's Whiteboard (`~/Develop/whiteboard`, MIT).

Scope: a **board** — a long-form, sectioned explanation of a complex topic or a codebase that the model draws progressively while the reader watches: prose, code excerpts pinned to real `path:line` ranges, diagrams, callouts, and links that tie the three together; select anything on it and ask the assistant in the same session; opened from a chat turn, the notes list, or a popout window; readable on a phone. Nothing here is a second wire format, a second renderer stack or a second document store: a board is a document (59) whose blocks are the catalog (30/65) plus one new component.

Not in v1 (seams left): live editor peeks — Whiteboard mounts a Code OSS editor per excerpt; we have no editor (00 §1.8) and an excerpt is embedded text (the seam is an "Open in editor" action through the opener plugin later); Whiteboard's `sequence` step → source binding (mermaid owns sequence; a `code:` link in the caption is the v1 answer); `call_stack_diff` and `database_lens` as components (a `Graph` with `state` and a `Table` cover them until a board shows they do not); sharing boards by link (66/68 replication carries notes already; a public share is its own doc); trace quotes (no trace store); TUI reading mode (text channels get the receipt and `doc_markdown`).

## 0. Questions for Sky (each with the default the rest of this doc assumes)

1. **Prose typeface on boards.** Whiteboard sets prose in a serif (Newsreader 15 px / 1.72) against mono chrome (R §B.2) and it is a large part of why it reads like a document, not an app. Default: **keep `--font-sans`, adopt the two-register rule** (prose at 15 px/1.7, all chrome — paths, captions, gutters, TOC — in mono 11 px). Override: ship Newsreader woff2 (OFL) and set `.board-prose { font-family: var(--font-serif) }`; one CSS rule, ~120 KB of fonts.
2. **Where boards live.** Default: **boards are notes** (`DocKind::Board`, same store, same list, a board glyph and kind badge in the notes list; no `/boards` route). Override: a separate `/boards` list and sidebar entry.
3. **Excerpt size cap.** Default **≤ 120 lines and ≤ 16 KiB per `Excerpt`** (error, not truncation — the model gets the correction round, 30 §5). Whiteboard shows 18 lines then scrolls (R §B.4); `code_symbol.body` cuts at 200 (60 §21 dev. 13).
4. **Which session a board question goes to.** Default: **the session that drew the board**; a board opened with no live session (from the list, after restart) falls back to NoteChat's behaviour (first message creates a session titled like the board, 59 §9.3). Override: always a fresh board-bound session.
5. **A `ref` on `Graph` nodes.** 65 §0.6 says a `Graph` is a sink with "no file/symbol bindings". Default: **add `ref?: "path#L10-L24"`, an in-board anchor only** — the renderer never fetches or navigates; a click scrolls to the `Excerpt` with that anchor or opens select-to-ask with it. Override: no `ref`; diagram → code linking stays in captions.
6. **Open in panel.** Default: **not in v1** — a board opens on its route or in a popout; the ArtifactPanel sheet stays for artifacts and apps. Override: `ArtifactPanel` gains a `board` content kind (one `Sheet` body, ~60 lines).

## 1. Decisions in one screen

1. **A board is a document of kind `board`** (`crates/document/src/meta.rs:83` `DocKind` gains `Board`; `doc_create` gains `kind?: note|board`). It is written with the `doc_*` tools the agent already has (`doc_insert`/`doc_replace`/`doc_move`/`doc_read`, `document/src/tools.rs:167-440`), streams to every window through the Yjs document actor as each block lands, is user-editable, versioned by snapshots, replicated with the rest of the documents, and already has a popout target (`lib/popout.ts:10-19` `{kind:'note'}`) and a mobile editor (59 §10). This is Whiteboard's own model — an ordered block tree with server-assigned ids, one block per edit, whole-snapshot streaming (R §A.2–3) — already built. **Rejected: a new artifact kind** (`Kind::Board`, a JSON block document): `show` is whole-content-per-version and invisible mid-call (`store/reduce.ts:86-101`; R §D.1), so progressive drawing would re-send the growing board every version, and a second block schema plus a second renderer would exist beside BlockNote's — 30 §0.2 "no new wire types". **Rejected: an OpenUI app**: positional-argument programs are the wrong medium for two pages of prose, `CodeBlock(language, code)` has no path/line/highlight (R §D.4), every board would churn the library hash, and text channels would get nothing.
2. **One new catalog component, `Excerpt`, the `code_peek` of this design.** `Excerpt(path, text, start?, lang?, caption?, mark?)` joins the 36 in `crates/surface/src/spec.rs` with the same derives, located `SpecError`s and one correction round. It renders on every surface (TUI arm: header line + numbered lines), in chat fences, in markdown artifacts (the viewer already runs the chat `plugins`, `Viewer.tsx:54`) and in notes (`editor/CodeBlock.tsx:161` renders `json-render` code blocks live). Whiteboard references code (`source:{file,start,end,pins}`) and lets the canvas fetch it (R §A.1); we **embed the text** because a spec is a sink (30 §0.10, 65 §0.6) — the renderer never reads a file — and because the text is what makes a board readable in a text channel and truthful after the tree moves. The rule for the model is Whiteboard's, inverted: *never type an excerpt from memory; `read_file` the range and paste it.*
3. **Anchors are block ids and `path#Lstart-Lend`.** Every block already has a stable id (BlockNote; `doc_read format=markdown` emits `<!-- id -->` before each block, `tools.rs:167`). An `Excerpt` is additionally addressable by its range. Prose links code as `[label](code:crates/x/src/y.rs#L10-L24)` — Whiteboard's `review-source:head/path#L10-L24` (R §A.1, `blocks/markdown.ts:8`) without the side (a board pins one tree; before/after is the `Diff` component). A `code:` link is handled by the link renderer everywhere markdown renders: on the same page it scrolls to and flashes the `Excerpt` whose anchor matches; otherwise it opens select-to-ask with the range. It never opens a file.
4. **Layout is the renderer's.** Like Whiteboard (R §A.6), the data carries no positions: sections are `##` heading blocks and what follows them, collapse state is UI state, diagrams take the wide column because they are diagrams. Nothing new in the wire beyond `Excerpt`, `GraphNode.ref` and `doc_create.kind`.
5. **A board page is the note page in reading mode.** `/notes/$id` renders `BoardPage` when `kind == board`: BlockNote `editable=false` (an **Edit** toggle returns the full editor — it is still a note), a TOC from the heading blocks (pill on narrow shells, rail at ≥ 1360 px, R §B.3), collapsible `##` sections with an anchored heading, prose at 720 px and diagrams up to 1120 px in an 860 px column (R §B.3 values), mono chrome, a "theuth is drawing…" badge while the bound session runs, and a draw-in for blocks that arrive while the page is open. The popped form is the same component (`Popped` shell, `routes/__root.tsx:16-24`), and the popout target is the existing `note` (zero change in `popout.ts`).
6. **Select-to-ask is a text header, not a protocol change.** Whiteboard's loop is clipboard-only: ⌘⇧C copies a selection with `Session ID`/`Version` header lines the agent maps back (R §C.1). Ours goes one step further and lands the quote in the composer of the right session: a selection (prose, a block, an `Excerpt` line range, a `Graph` node) becomes `[board <doc> · block <id> · path:10-24]` + `> quote` + the user's question, prefilled with `prefillDraft` (`Composer.tsx:62`) and the board pinned as `[attached note …]` (66 §3.3). Precedents: `[app name vN · call]` (`apps/lifecycle.ts:344`) and the attachment marker (`lib/attach.ts:48`). `UserInput` stays `{id, text, files, ticket, scope, defer}` (`agent/src/loop_.rs:126`). The transcript renders the header back as a quote card the way `parseAppInput` does (`lifecycle.ts:377`).
7. **A board opened from a chat turn is a card.** `MessageTool.tsx:129` routes `doc_create` with `kind: "board"` to `BoardCard` (title, "drawing… N blocks" while the turn runs, **Open board**, **Open in new window** when `canPopout()`), and binds the board to that session in the `note-chat` store (`lib/note-chat.ts:11-40`) so questions from the board page return to the chat that drew it (§0 Q4). A board is also reachable from the notes list, by `theuth://notes/<id>` in any reply, and as an artifact through `show` of `doc_markdown` (the markdown renders identically; fences included).
8. **Progressive reveal is per block, and the turn is the lease.** Whiteboard: "one paragraph per edit, so the document draws itself"; a 3-minute lease ends the review (R §C.1). Ours: `doc_insert` per block from one Python cell (08: tools are runtime coroutines, so six inserts in one cell land in under a second each), the page draws each as it arrives, and the badge clears when the bound session stops running. No lease, no activity RPC: the session state already exists in every window (`sessions[session].running`).
9. **The prompt teaches Whiteboard's method, not its tools.** A `## Boards` block registered by the document plugin (`RegisterPrompt{plugin:"document"}`, `tools.rs:44`; static, cache-safe) says when a board beats an inline reply and the six rules; a `board.md` skill (written by `first_run` like `visualize.md`, `main.rs:716-733`) carries the section order, the diagram-choice rule, the excerpt rule, the read-back rule and the three codebase recipes of §9 — borrowed from `instructions/authoring.md` and `scratchpad.md` (MIT, attributed; R §C.5–6).
10. **Codebases are explained through codeindex.** `code_map(focus)` → the map section (a `Graph` of the top-ranked files grouped by package, a `Tree` of the paths); `code_symbol` → the definition's `path:line_start-line_end` → `read_file(offset, limit)` → `Excerpt` (never `body`, which is lossy past 200 lines, 60 §21 dev. 13); `code_impact(git_range)` → a **diff board**: sections per change cluster (Whiteboard's file lenses, R §C.4), a `Graph` changed→direct→transitive with `danger`/`active`/`muted`, `Diff` per changed symbol, tests as a `Checklist`. Every claim about the tree has an anchor.
11. **Style is borrowed as tokens and rules, not as CSS.** Whiteboard's values that matter (R §B): 860/720/1120 px columns; h2 22 px with 40 px above / 12 px below; 15 px/1.72 body; 11 px mono chrome with .04 em tracking; accent for links and the active element; added/removed/modified as green/red/amber strokes; `--marker-tint` (accent at 14 %) for the selected row; grid dots on the canvas. Mapped onto Tailwind tokens theuth already has (`accent`, `success`, `destructive`, `warning`, `muted-foreground`); no new palette, no font files unless §0 Q1 says so.
12. **Mobile is the same page, one column.** Prose at 100 % width, diagrams pan/zoom inside `DiagramFrame` (65 §3.4), `Excerpt` gutter shrinks to 3 ch and wraps off, TOC is a pill that opens a sheet, selection uses the platform's selection with a floating **Ask** button anchored inside the document (Whiteboard anchors "Copy for Agent" in `.review-document`, R §C.1), popout hidden on iOS (`canPopout()`, 70-MOBILE-FIXES), 44 px targets on coarse pointers (59 §10). Nothing board-specific in Rust.

## 2. Why

- The chat already draws (30/65/70-SURFACES: 36 components, `Graph`, mermaid 12, inline openui) and the prompt already says "show first". What it cannot do is hold a **long** explanation together: a "how does X work" reply is a scroll of fences and paragraphs with no sections, no table of contents, no code pinned to lines, and no way to point at a node and ask about it. Every serious codebase question ends in a paste of `read_file` output.
- Whiteboard solved exactly this for code reviews and did it with ideas, not machinery: a document with sections, code quoted by `file:line`, one diagram chosen for the shape of the change, every claim linked to code, drawn incrementally while the reader watches, and a copy-selection loop back to the agent (R §A, §C). Its renderer is 8 000 lines of CSS on a Code OSS fork; its *method* fits in 37 lines of `authoring.md`.
- theuth already has the seams the method needs: notes stream block by block over Yjs and the agent edits them with `doc_*` (59); the catalog renders in notes (`CodeBlock.tsx`), chat and artifacts from one Rust definition (30); codeindex answers "where, who, what changed" structurally (60); popouts and mobile rules exist (70). What is missing is one component (`Excerpt`), one link scheme (`code:`), one page mode, one header grammar, and a skill that teaches the method.

## 3. What we borrow, mapped

| Whiteboard (R ref) | Borrowed as | Where in theuth |
|---|---|---|
| `section {title, defaultCollapsed, children}` (§A.1) | `##` heading block = section; collapse is UI state, TOC derived | `BoardPage` (§6.2) |
| `markdown` with `review-source:` links (§A.1) | paragraph blocks; `code:path#L10-L24` links (§5.3) | Streamdown link renderer, BlockNote link click |
| `code_peek {source:{file,start,end}}` (§A.1, §B.4) | `Excerpt` with embedded text, `start`, `mark` ranges (§5.2) | `crates/surface` + `surface/Excerpt.tsx` |
| `callout {tone}` (§A.1) | existing `Callout`/`Alert` | catalog |
| `flow_diagram {nodes, edges, direction}` + node attachments (§A.1) | `Graph` + `GraphNode.ref` (§5.4) | 65 |
| `sequence` with step `source` (§A.1) | mermaid `sequenceDiagram`; a `code:` link in the caption | 30 |
| `call_stack_diff` base/head frames (§A.1) | `Graph` TB with `state` per node + `Diff`; revisit if boards want more | catalog |
| `database_lens` (§A.1) | `Table` of stores/fields + `Graph` of actors→stores | catalog |
| whole-snapshot NDJSON stream, `lastEdit` animation (§A.3) | Yjs updates per block; draw-in on new block ids | document actor, `BoardPage` |
| `session_activity` lease + focus badge (§C.1) | bound session `running` → "theuth is drawing…" | store |
| Copy for Agent (⌘⇧C, `Session ID`/`Version` header) (§C.1) | select-to-ask → `[board …]` header into the bound session's composer (§7) | Composer, Transcript |
| `session_create` auto-open (§C.2) | `BoardCard` in the turn; **Open board** / popout | `MessageTool` |
| TOC pill/rail, 860/720/1120 columns, h2 40/12, serif/mono registers (§B.2–3) | `BoardPage` layout tokens (§6.1) | CSS |
| `authoring.md` rules (§C.5) | `## Boards` prompt + `board.md` skill (§8) | document crate |
| file lenses (§C.4) | diff-board sections per change cluster (§9.3) | skill recipe |

Not borrowed: the Code OSS inline editor and `reveal` verb (no editor), software map, trace quotes, the share manifest, the MCP server (theuth's tools are native), server-assigned diagram unit ids (a diagram is one block; the model replaces it whole — `doc_replace` — which is also what Whiteboard tells the agent to do when "the whole picture was wrong", R §C.5 rule 26+).

## 4. Shape

```mermaid
flowchart LR
  subgraph agent["agent turn (Python cell)"]
    CM[code_map / code_symbol / code_impact] --> RF[read_file ranges]
    RF --> DI["doc_create(kind=board) · doc_insert × N"]
  end
  DI --> DA[(document actor · Yjs)]
  DA -->|DocumentChanged| BUS[bus → rpc bridge]
  DA -->|doc_subscribe| BP[BoardPage · reading mode]
  BUS --> CARD[BoardCard in the chat turn]
  CARD -->|Open board / popout| BP
  BP -->|select → [board …] header| CMP[Composer of the bound session]
  CMP -->|send_input / steer| agent
  classDef new fill:none,stroke:#2563eb,stroke-width:2.5px;
  class BP,CARD,CMP new;
```
Blue outline = new UI; everything else exists. No new crate, no new dependency, no new RPC method, no new bus event.

## 5. Data model

### 5.1 The document
`DocKind::Board` (`meta.rs:83`, serde `board`), `Binding::Note` unchanged (a board is free-standing like a note). `CreateNote` (`document/src/lib.rs:36`) gains `kind: DocKind` (default `Note`); `doc_create(title, markdown?, kind?)`; `doc_list(kind)` already filters by kind. Blocks are BlockNote's schema from 59 §9.2 (`createDocumentSchema(frontmatterBlock, codeBlock)`, `Editor.tsx:33`): headings 1–3, paragraphs, lists, quote, table, divider, code blocks. Diagrams and catalog components are code blocks with `language: json-render | mermaid` (`codeLanguages.ts:21-28`); openui stays out of notes (not built there, 70-SURFACES "not built"). A section is a `##` block and every block until the next `##` or `#`; `###` nests inside. Nothing is stored for collapse or TOC.

### 5.2 `Excerpt` (`crates/surface/src/spec.rs`)
```
Excerpt(path: string, text: string, start?: integer ≥ 1 (default 1), lang?: string,
        caption?: string, mark?: [[from, to]] (1-based, absolute, inclusive))
```
Compact signature for the catalog line: `Excerpt(path, text, start?, lang?, caption?, mark?[[from,to]])`. `#[serde(deny_unknown_fields)]`. Validation (`validate.rs`, located errors): `path` root-relative with `/`, ≤ 256 chars, no `..` segment, no scheme; `text` ≤ `MAX_EXCERPT_LINES` 120 and ≤ 16 KiB (§0 Q3); each `mark` within `[start, start + lines)` and `from ≤ to`; `lang` from the code-language table or omitted (the renderer guesses from the extension). Anchor = `path#L<start>-L<start+lines-1>`. A `Diff` with a `path` header is not added: `Diff.from/to` already label the sides ("`main`", "`HEAD`").

TUI (`render.rs` arm): `▍crates/surface/src/lib.rs:71-84 · rust` then `  71 │ pub fn parse(...)` per line, marked lines with `▶` in the gutter, caption dimmed under; width < 60 drops the gutter. Slack/Signal (`flatten.rs`): header line + fenced text.

### 5.3 `code:` links
Grammar: `code:<path>#L<a>-L<b>` or `code:<path>#L<a>` or `code:<path>`; path rules as in 5.2. Recognised wherever markdown renders through the shared link renderer (chat `MessageAssistant.tsx:222`, artifacts `Viewer.tsx:48-54`, BlockNote links on the board/note page). Behaviour: the same document/message holds an `Excerpt` whose range contains `[a,b]` → scroll to it, flash `--marker-tint` for 1.2 s; else → select-to-ask with `path:a-b` and an empty quote. In the TUI and text channels the link renders as `label (path:a-b)`.

### 5.4 `GraphNode.ref` (65 §3.1 amendment; §0 Q5)
`GraphNode { id, label, kind?, group?, state?, ref?: string }` — `ref` is a 5.3 target without the `code:` prefix. Validation: same path rules. Desktop: node click with `ref` → the 5.3 behaviour; `onSelect` in apps still fires. TUI: ignored. The lowering `to_mermaid()` ignores it. Sink argument: nothing is fetched or navigated; the renderer resolves the anchor against the DOM it already has, or hands the text to the composer.

### 5.5 Select-to-ask header (client-composed text; §7)
```
[board <docId> · block <blockId> · <path>:<a>-<b>]
> <quote, ≤ 4 KiB, each line prefixed>
<question typed by the user>
```
Fields after `board <docId>` are optional and dot-separated: a prose selection has `block`; an `Excerpt` selection has `block` and the range; a `Graph` node has `block` and `node <id>`; a `code:` link with no excerpt has only the range. Regex on the client mirrors `APP_HEAD` (`lifecycle.ts:371`): `^\[board (\S+)((?: · [^\]]+)*)\]$`. The agent reads it as text; the prompt block (§8.1) tells it what the header means and to `doc_read(doc, blocks=[blockId])` before answering or `doc_replace`-ing.

## 6. Rendering

### 6.1 Tokens and rules (`BoardPage.css`, all values from R §B unless noted)
| Rule | Value | Whiteboard source |
|---|---|---|
| Page column | `max-width: 860px; margin: 0 auto 96px`; inline padding `clamp(20px, calc((100cqi - 720px) * .122 + 20px), 64px)` | `styles.css:4449-4462` |
| Prose width | `width: min(100%, 720px); margin-inline: auto` on every non-diagram block | `:4763-4788` |
| Diagram width | up to `1120px`, centred; `1000px` while the ask popover is open | `:4450, :4477` |
| Body | 15 px / 1.72, `--font-sans` (or serif, §0 Q1); `li + li` 8 px | `:4471-4473, :4751` |
| h1 / h2 / h3 | 34/40 w500, 28 above 18 below · 22/28 w500, 40 above 12 below · 17/23 w600, 30 above 10 below; `scroll-margin-top: 24px` | `:4491-4499, :4724-4740, :3044-3047` |
| Chrome (paths, captions, gutters, TOC, badges) | mono 11 px, weight 400/600, tracking .04 em; micro-labels 10 px w700 tracking .08 em | `:658-679, :6629-6632` |
| Inline code | mono .9 em on `muted` background | `:3088-3090` |
| Excerpt frame | 1 px `border`, radius 6, code 12 px / 19–20 px line, gutter `42px 14px 1fr`, gutter text `muted-foreground` right-aligned, header 40 px with the path; 18 visible lines then scroll | `:3309-3317, :3357-3370`; `DocumentCodeView.tsx:18-22` |
| Marked lines / selected row | `--marker-tint` = accent at 14 % (light 8 %) + 2 px accent bar | `:9, :7781-7796` |
| Links | accent; open/active link `--link-open-wash` accent at 15 % | `:54, :90, :3080-3083` |
| Change colours | added `success`, removed `destructive`, modified `warning` — strokes and names, never fills (30 §6.3 "outline, never fill") | `:69-77` |
| Section header | flex baseline, gap 10, `margin-block: 40px 12px`; chevron 18×18 at `left:-24px` at rest opacity .45, full on hover; collapsed h2 dims to `muted-foreground` | `:4806-4853, :39` |
| TOC | pill 32×32 at `top: 32px; left: 24px` opening to a 248 px card; always-open rail at ≥ 1360 px; active entry accent | `:4238-4341, :4443-4447`; `review-toc.tsx:20-26` |
| Canvas | dotted grid `--canvas-grid` behind the column on desktop only | `:11, :62` |
| Motion | new block: opacity 0→1 and 6 px rise over 240 ms; `prefers-reduced-motion` → none | `whiteboard.css:2110-2128` |

### 6.2 `BoardPage` (`components/notes/BoardPage.tsx`, chosen by `routes/notes_.$id.tsx` when `meta.kind === 'board'`)
- BlockNote `Editor` with `editable={!editing}`; **Edit** toggle in the header (mono chrome). Reading mode hides side/formatting menus, keeps link clicks and code-block diagram rendering (`CodeBlock.tsx:72-74` already renders when not selected). Selection in reading mode is native text selection (§7).
- Sections: derived from the block list (`editor.document`), memoised by ids; each `##` gets an `id` anchor, a chevron, and `hidden` on its body when collapsed; state in `useReviewUiState`-style local storage keyed `board:<doc>:<blockId>` (Whiteboard scopes collapse per session, `review-components.tsx:160-167`). The TOC expands a collapsed section before scrolling (their `review-section-expand` event, `:186-195`).
- TOC entries = `#`/`##`/`###` blocks with text; active = the last heading above the viewport middle (IntersectionObserver).
- Drawing badge: `note-chat` store's `session` for this doc → `sessions[session].running` → "theuth is drawing…" in the header; block ids not present at the last render get the draw-in class. First paint of an already-complete board draws nothing.
- Popped (`isPopout()`): no NoteChat panel, `document.title = title`, otherwise identical (as `NotePage.tsx:118` does today).
- Header actions: Edit · Ask about board (opens NoteChat or focuses the bound chat) · Copy link (`theuth://notes/<id>`) · Open in new window (`canPopout()`) · ⋯ (Rename, Export markdown → `show`, Delete — the note menu).

### 6.3 `Excerpt.tsx` (`components/chat/surface/Excerpt.tsx`, registered in `registry.tsx`; zod mirror in `catalog.ts`; parity rule in `parse.ts`)
Header: path in mono with the range (`crates/surface/src/lib.rs:71-84`) and a copy button for `path:a-b`; body: `@streamdown/code` highlighting with a gutter starting at `start`, marked ranges tinted; caption under in mono `muted-foreground`; `data-anchor="path#L71-L84"` and `data-lines` on rows so 5.3 and §7 can target it. Long excerpts scroll inside 18 lines with a "show all" affordance. In chat the same component renders inside the fence with the fence's margins; nothing board-specific.

### 6.4 Progressive reveal
The board page subscribes like the note page (`doc_subscribe`); each `doc_insert` is one Yjs update and one React commit. Diagram code blocks render through the 65 §4.3 queue (80 ms, one in flight), so six inserts in a second cost ≤ 6 layouts. A `json-render` block that fails validation shows the note editor's existing fallback (`FenceFallback`, dim one-liner) — the agent got its `<surface_error>` round when it inserted (the document tools run the block through `surface::check` on insert; if they do not today, step 3 adds it: same function, same message).

### 6.5 Mobile (`useIsMobile()`, `usePointerCoarse()`)
One column; prose 100 %; the diagram column becomes horizontally scrollable with `DiagramFrame` pan/zoom; `Excerpt` gutter 3 ch, header wraps the path with `overflow-wrap: anywhere`; TOC pill → bottom `Sheet` (the `ArtifactPanel` sheet primitive); section chevron inline before the title (no negative offset at 20 px padding); floating **Ask** button appears above a non-empty selection, anchored inside the document so it scrolls with the text; NoteChat panel keeps its `--kb-inset` behaviour (59 §10). No popout button on iOS.

## 7. Interaction

### 7.1 Select-to-ask (Whiteboard's copy loop, landed in the composer)
Sources, in the order Whiteboard has them (R §C.1) plus one:
1. **Prose**: `selectionchange` inside `.board-prose`; a selection that crosses a diagram or an `Excerpt` is rejected (their eligibility rule). Header `[board <doc> · block <first block id>]`, quote = selected text.
2. **Excerpt lines**: selection inside an `Excerpt` body → the covered `data-lines` → `[board <doc> · block <id> · path:a-b]`, quote = those lines.
3. **Graph node**: click a node with or without `ref` while holding ⌥, or "Ask about this" in the node's context menu → `[board <doc> · block <id> · node <nodeId>]`, quote = the label.
4. **`code:` link with no local excerpt** → `[board <doc> · path:a-b]`, no quote.
Affordance: floating **Ask** (⌘⇧A; `aria-keyshortcuts`) anchored in the document; Escape clears. Sending: `prefillDraft(session, text, [noteRef], panel)` with `pinned` = the board's `AttachRef` so `[attached note …]` leads the message (66 §3.3) — the agent can `doc_read` without being told the id twice. Session = the bound session (§0 Q4); if it is running, the composer's existing steer path applies (`AppHost.tsx:430-452` pattern). The transcript shows the message as `BoardQuoteCard` (header parsed, quote in a `blockquote`, question below), and the reply usually edits the board (`doc_replace`) and says one line in chat — the skill says so (§8.2).

### 7.2 Opening a board
- **From a chat turn**: `BoardCard` (§1.7) in place of the `doc_create` tool row. While the turn runs it shows "drawing… N blocks" (N from `doc_subscribe` is too heavy for a card; use `DocumentChanged.updated_ms` ticks as a pulse and drop the count if it costs a query). Auto-open: none — the card is the receipt (an inline reply is still where the takeaway sentence goes).
- **From the notes list**: boards carry a board glyph and `board` kind badge; the row opens `/notes/$id` (the sibling worker's list filter can filter by kind; no change here).
- **From a reply**: `theuth://notes/<id>` already resolves; the skill tells the model to end a board turn with one sentence and that link.
- **As an artifact**: `show(content=doc_markdown(id), name="<slug>.md")` when the user wants a frozen copy or a text channel needs it; the viewer renders the same blocks.
- **Popout**: existing `note` target; label `popout-note-<id>`.

## 8. Prompt and skill

### 8.1 `## Boards` prompt block (`crates/document/src/tools.rs` PROMPT, appended; static)
> A **board** is a long-form explanation the reader can scroll, search and point at: sections, prose, code excerpts pinned to `path:line`, diagrams, callouts. Draw one instead of an inline reply when the answer has three or more parts, spans several files, or the reader will come back to it — "how does X work", "walk me through this change", "explain this codebase". Create it with `doc_create(title, kind="board")`, then `doc_insert` one block at a time in reading order; the reader watches it draw. Sections are `##` headings in this order, omitting any that do not apply: **What and why → Map (one diagram that shows the shape) → How it works (walk the code in the order a reader should follow it, entry point first) → Decisions and trade-offs → Open questions**. Pin every claim about code to an `Excerpt` (`read_file` the range; never type code from memory) or a `[label](code:path#L10-L24)` link. Keep it short; a small topic is three blocks. Before you finish, `doc_read` the whole board and fix contradictions and unverified claims. End the turn with one sentence and `theuth://notes/<id>`. A message that starts with `[board <id> · block <bid> …]` quotes a board: `doc_read(doc, blocks=[bid])`, answer or `doc_replace` the block, and say one line in chat. Full method: the `board` skill.

### 8.2 `board.md` skill (`crates/document/board.md`, `{{catalog}}`-free, written by `first_run`)
Sections, with the Whiteboard rule each carries (attributed to `instructions/authoring.md` / `scratchpad.md`, MIT © 2026 dev.fast):
1. **When a board, when a reply** — "Answer in chat when one sentence or one code line does it"; a board when the explanation "spans more than one file or more than one repository, or would be an ASCII diagram in chat".
2. **Flow** — orient (`code_map(focus)`), create, first section immediately ("immediately after reading …, without any other tool calls, put down a first pass at the what/why section"), then the rest; "Write incrementally. The user sees you write … Show them visual progress every few seconds."
3. **Section order** — 8.1's five, each with a two-line description and a size hint; "keep whiteboards short and sweet … feel free to omit sections".
4. **Pick one diagram** — `Graph` when components, dependencies or data flow are the point; `sequenceDiagram` "if participants interact over time"; `stateDiagram-v2` "if the interesting part is branches, retries or state transitions"; `Table` + `Graph` for a schema and who reads/writes it; `Diff` for a contract change. One per section at most.
5. **Excerpts** — "`code_peek` for the few spots that carry the mechanism or an invariant; link everything else inline" → `Excerpt` for ≤ 3 spots per section, `code:` links for the rest; always `read_file(path, offset=line_start, limit=n)`; `mark` the lines the prose talks about; caption = the claim the excerpt proves.
6. **Codebase recipes** — §9 verbatim (Python cells).
7. **Read back** — `doc_read(format="markdown")`, check every `Excerpt.path:start` still matches the tree (`read_file` again if the tree moved), remove sections that say nothing.
8. **Answering from a board** — the header grammar, `doc_read(blocks=[…])`, edit in place, one line in chat.
9. **Block shapes** — the seven blocks a board uses with one JSON example each (`Excerpt`, `Graph` with `ref`, `Callout`, `Steps`, `Diff`, `Table`, mermaid sequence), and the json-render rules that avoid errors (from `visualize.md`).

### 8.3 `visualize.md` / "Show first, then tell"
Catalog line gains `Excerpt(...)`; the pick-by-shape table gains `| a passage of real code the reader should see, with its location | json-render Excerpt (path, start, mark) |` and `| an explanation with three or more parts, or one the reader will return to | a board (the board skill) |`.

## 9. Explaining a codebase, specifically

### 9.1 `code_map` → the Map section
```python
m = await code_map(focus=["crates/surface"], budget=1500)
top = [r for r in m["rows"] if r["rank"] > 0][:12]            # Tag {path, line, kind, name, signature, rank}
by_pkg = group(top, key=lambda r: r["path"].split("/")[1])    # crate name
nodes = [{"id": slug(r["path"]), "label": base(r["path"]), "group": pkg(r), "ref": f'{r["path"]}#L{r["line"]}'} for r in top]
edges = imports_between(top)                                   # from code_outline(path).imports[].target
await doc_insert(b, "## Map\n\nOne sentence on the shape.\n\n```json-render\n" + graph(nodes, edges, groups(by_pkg), "LR") + "\n```")
await doc_insert(b, "```json-render\n" + tree([r["path"] for r in top]) + "\n```")
```
Rules: ≤ 12 nodes (30 §6.2), groups = packages, `state: "active"` on the file the question is about, `ref` on every node so a click lands on its excerpt or asks about it.

### 9.2 `code_symbol` → pinned excerpts
```python
s = await code_symbol("crates/surface/src/lib.rs:71")          # parse()
d = s["definition"]                                            # {symbol_id, name, kind, path, line_start, line_end, signature}
n = min(d["line_end"] - d["line_start"] + 1, 60)
text = await read_file(d["path"], offset=d["line_start"], limit=n)
await doc_insert(b, excerpt(d["path"], text, start=d["line_start"], lang="rust",
                            caption=f'{d["name"]}: strict parse, then jsonrepair, then validate', mark=[[d["line_start"]+3, d["line_start"]+5]]))
callers = s["callers"]                                         # [{path, count, refs:[{line, from_symbol, provenance, confidence}]}]
await doc_insert(b, "Called from " + ", ".join(f'[{c["path"].split("/")[-1]}](code:{c["path"]}#L{c["refs"][0]["line"]})' for c in callers[:4] if c["refs"][0]["confidence"] >= 0.5) + ".")
```
Rules: `body` is never pasted (cut at 200 lines and by budget); callers with `confidence < 0.5` are not cited as fact (60 §21 dev. 14); a symbol longer than 60 lines gets the head and a `code:` link to the rest.

### 9.3 `code_impact` → a diff board
```python
i = await code_impact(git_range="main..HEAD", include_tests=True)
# changed: [{path, status, symbols_changed:[{name, kind, line}]}], direct: [{path, via, is_test}], transitive: [{path, depth}], tests: [path]
await doc_create(title=f"What {branch} changes", kind="board")
# What and why: from the commit messages / PR text, in the author's words
# Map: Graph — changed files state danger, direct active, transitive muted, groups by crate, edges = via
# How it works: one ## per change cluster (Whiteboard's file lenses: tests/docs/generated first, then by the part of the design each file serves), each with an Excerpt of the new code and a Diff(before=git show base:path range, after=read_file range, from="main", to="HEAD") for the symbol that carries the mechanism
# Checks: Checklist of i["tests"] with done = ran green
```
Rules: root every flow at the user/agent entry point including unchanged nodes (their `call_stack_diff` rule); `ambiguous_skipped` is reported in Open questions, not hidden; a cluster is one section a reader finishes in one sitting.

## 10. Build steps (omp workers; one branch each; `cargo check --workspace --tests` and the UI trio green at every step)

| # | Step | Done when |
|---|---|---|
| 1 | **`surface`: `Excerpt`** — `ExcerptProps`, caps, validation (path rules, `mark` bounds), TUI arm, `flatten.rs`, fixtures `valid-excerpt.json`, `broken-excerpt-{path,mark,long}.json`, goldens at 60/100; `GraphNode.ref` + validation; `catalog.rs` pins 36→37; `prompt.rs` and `visualize.md` rows of §8.3 | `cargo test -p surface` green; `catalog_compact()` contains `Excerpt(`; TUI golden shows the numbered gutter and `▶` marks at 100 cols and no gutter at 40; every valid `Graph` fixture with `ref` still lowers to mermaid that merman parses |
| 2 | **Desktop `Excerpt.tsx` + `code:` links** — renderer in `registry.tsx`, zod mirror, `parse.ts` rules with the Rust paths/messages, `data-anchor`/`data-lines`; link handling in the shared Streamdown link renderer and BlockNote link click: scroll+flash or select-to-ask handoff (a `theuth:ask-code` CustomEvent for step 5 to consume); Graph node `ref` click; bindings regenerated (`just ts-desktop`) | `registry.test.ts` renders the fixture with gutter starting at `start` and two marked rows; `catalog.test.ts` pins 37; a chat reply with an `Excerpt` and a `[x](code:…)` link to it scrolls and flashes in the browser smoke; `bunx tsc --noEmit`, vitest, `bunx vite build` green |
| 3 | **`document`: `DocKind::Board`** — enum variant, `CreateNote.kind`, `doc_create(kind?)`, `doc_list(kind="board")`, `surface::check` on inserted `json-render` blocks with the located error as the tool error; `PROMPT` `## Boards` (§8.1); `crates/document/board.md` (§8.2) + `first_run` line; `DocumentChanged` unchanged | `cargo test -p document` (create with kind, list by kind, insert of a broken block returns `/props/mark/0: …`); a headless engine on a scratch data dir (68-two-engine-smoke recipe) shows `~/.theuth/skills/board.md` written and `doc_create(title, kind="board")` returning `theuth://notes/<id>` |
| 4 | **`BoardPage`** — route switch on kind, reading mode + Edit toggle, sections/collapse/TOC, §6.1 tokens, drawing badge, draw-in, popped form, mobile rules of §6.5 | Browser smoke (`bunx vite --port 5188` against the scratch engine): a board with 5 sections, a `Graph`, two `Excerpt`s and a mermaid block renders at 1440 (rail TOC), 1024 (pill TOC), 390 (one column, sheet TOC); inserting a block from the agent while the page is open draws it in; popout shows the same page without the chat panel; `prefers-reduced-motion` disables the draw-in |
| 5 | **Select-to-ask** — §7.1 sources, header composer, floating Ask + ⌘⇧A, `BoardQuoteCard` parser/renderer in the transcript, `prefillDraft` into the bound session with the board pinned, mobile long-press path | Selecting prose → composer holds `[board … · block …]` + quote; selecting two `Excerpt` lines → header carries `path:a-b`; a sent question reaches the agent and `doc_read(blocks=[id])` returns the block; the transcript shows the quote card; `lifecycle`-style parser test for the header grammar |
| 6 | **`BoardCard` + binding** — `MessageTool` route for `doc_create` kind board, card states (drawing/ready/error), Open board / Open in new window, `note-chat` session binding on create and on Open; notes-list glyph and kind badge only (the filter is the sibling's) | A chat turn that creates a board shows the card with the title; Open lands on `/notes/$id` with the badge "theuth is drawing…" while the turn runs and clear after; a question from that page arrives in the same chat; the card hides popout on iOS |
| 7 | **Reference boards + excerpt check** — three boards drawn on this repository by the real binary from the skill alone: "how the surface crate renders a fence" (§9.1–9.2), "what the step-1 commit changed" (§9.3), "how a popout window boots" (cross-crate); `scripts/board-check.py` (`doc_markdown` → every `Excerpt` text equals `read_file` of its range) | Each board ≤ 3 turns, ≥ 4 sections, ≥ 3 `Excerpt`s; `board-check` reports 100 % of excerpts verbatim and every `code:` link resolvable; numbers (blocks, turns, tokens) recorded in the as-built |
| 8 | **Steering and as-built** — 10 prompts before/after (5 "how does X work here", 3 "walk me through this change", 2 comparisons that should stay inline); tune §8.1 once if needed; `70-as-built-BOARD.md` | ≥ 8/10 of the first eight produce a board, 2/2 comparisons stay inline; as-built records deviations, numbers, screenshots at the three widths |

Effort: 1 and 2 are the catalog pattern of 65/70-SURFACES (a day each); 3 is small; 4 and 5 carry the judgement (a page mode and a selection loop, two–three days); 6 is a card; 7 and 8 are measurement.

## 11. Verification

- **Rust**: `cargo test -p surface` (fixtures, goldens, catalog pins, mermaid lowering), `cargo test -p document` (kind, list, insert-time check). No new crate, so `cargo check --workspace --tests` is the whole gate.
- **Desktop**: `registry.test.ts`, `catalog.test.ts`, `parse.test.ts` rule parity, `board-header.test.ts` (grammar round trip: compose → parse for all four sources; malformed headers fall through as plain text), `popout.test.ts` unchanged (the `note` target is reused — assert a board URL round-trips), `header.test.ts` if the board header adds an item.
- **Browser smoke** (the 70-POPOUT procedure: `bunx vite --port 5188` against a scratch engine, never the user's): the step-4 and step-6 rows; screenshots at 1440/1024/390 into the as-built.
- **TUI**: `t -p 'show me parse() in the surface crate as an Excerpt'` prints the numbered gutter at 100 cols; a board turn prints the receipt and the `theuth://notes/<id>` line.
- **iOS**: one board at 390 pt in the simulator — sheet TOC, pan/zoom on the `Graph`, native selection → Ask, no popout button.
- **Measured** (recorded in the as-built): time from `doc_create` to first block on screen; draw-in cost for a 6-insert cell (≤ 6 diagram layouts, 65 §4.3 queue); board page first paint for a 40-block board; `board-check` results for the three reference boards.

## 12. Deviations, risks, what we knowingly give up

- **Embedded excerpts go stale.** Whiteboard pins commits and fetches; ours pins text. Mitigation: the anchor keeps `path:start`, the read-back step re-verifies, and `board-check` exists. A board is an explanation at a moment, like a design doc — which is also what "as built" sections are for.
- **A board is a note, so it is editable.** A reader can break a `json-render` block by typing in it; the editor already shows the source when a block does not parse. Reading mode is the default; Edit is a toggle.
- **Two prompts compete**: "show first" pushes toward inline blocks, "boards" toward a document. §8.1 draws the line at three parts / several files / will-return; step 8 measures it and tunes once.
- **The header is text**, so a user can type `[board …]` by hand — harmless (the agent will `doc_read` and find nothing or something). Same status as `[app …]` today.
- **No live editor, no `reveal`.** Deliberate (00 §1.8); the seam is a header action on `Excerpt` later.
- **Serif or not** is a taste call (§0 Q1); the layout rules do the heavy lifting either way.
- **Notes list changes** are minimal to keep clear of the sibling branch (kind filter); if both touch `NotesList.tsx`, the badge is two lines and rebases cleanly.
- Given up: sequence steps bound to code (mermaid owns the syntax); server-assigned unit ids inside diagrams (replace the block); Whiteboard's tour overlay (a fullscreen diagram with code stops — `DiagramFrame` fullscreen plus `code:` links is the v1; revisit when a board asks for it); sharing by capability link; the software map.

## 13. Amendments (same commit as the step that lands them)

- **30** §0.7/§10: catalog 36 → 37 (`Excerpt`); §6.2: the two rows of §8.3; new §12 "`code:` links": grammar and behaviour of §5.3. Step 1/2.
- **59** §9: `DocKind::Board`; §9.3 gains "a board page is the note page in reading mode (71 §6.2)"; §9.3 NoteChat binding rule of §1.7. Step 3/4/6.
- **60** §8: one sentence after the prompt paragraph — "`code_symbol.definition` and `code_impact` feed board excerpts and diff boards (71 §9)". Step 7.
- **65** §0.6: append "Exception, 71: `GraphNode.ref` is an in-board anchor (`path#La-Lb`); the renderer scrolls to a matching `Excerpt` or hands the text to the composer, and never fetches or navigates." §3.1 signature gains `ref?`. Step 1.
- **66** §3.3: `[board …]` listed beside `[app …]` and `[attached …]` as a client-composed header. Step 5.
- **00**: no crate, no dependency line change, no §9 storage change (boards are documents).

## 14. Licence and attribution

Everything borrowed from Whiteboard is MIT, Copyright (c) 2026 dev.fast (R §C.6). What is copied verbatim — sentences from `instructions/authoring.md` and `scratchpad.md` inside `board.md` and §8.1 — carries a one-line attribution at the top of `board.md` ("Method adapted from dev.fast Whiteboard, MIT © 2026 dev.fast"). Type shapes and CSS values are re-implemented from the recon, not copied (no notice needed, none copied). Nothing under `apps/review-desktop/code-oss/` (Microsoft notices) is used. theuth's crates stay `MIT OR Apache-2.0`; no `NOTICE` file.

## 15. Status

Design drafted 2026-09-26 by the orchestrator from the read-only Whiteboard recon (`71-whiteboard-recon.md`) and the theuth seams it names. Awaiting Sky's answers to §0 and rulings on §1; step 1 can start the moment they land. Not on main; branch `design/71-whiteboard-visuals`.
