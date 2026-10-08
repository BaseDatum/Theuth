# R2 — OURS: theuth codeindex + CodeMap + diagram stack recon

Tree: ~/Develop/theuth (main `5cddad8e`, 7 untracked/dirty entries of other sessions, read-only). Date 2026-10-08 17:40–18:30.
Author: omm-R2-ours (research subagent). Every claim is marked **VERIFIED** (I read the lines) or **INFERRED**; every
piece is marked **BUILT** / **DOC-NOT-BUILT** / **OPEN**. Citations are `path#La-Lb` relative to the theuth root.
Companion measurements (raw dir→dir edge lists) are under `~/.theuth/briefs/2026-10-08/codemap-omm/o4-omm-R2/`.

Reading order: §A1–A17 answer the shared axes for theuth as it stands; O1–O6 are the port-specific extras; the two
closing sections describe the CodeMap panel as it ships today and the storage options for a generated map.

## A1. Promise and audience

- **Promise (design 73, verbatim requirement):** *"a CodeMap, which is a visual representation of the code that's being
  worked on … pops up as available, just like the browser and PTY … updated live … lightning fast … signal is more
  important than noise … reduce cognitive load … understand what's in the codebase, how it works together and what is
  being changed."* `docs/design/73-codemap.md#L5-L5` VERIFIED.
- **Audience:** the person watching an agent work on one root. The map is agent-driven ("the lens shown follows the
  agent's last `code_*` call unless the user pinned one") `docs/design/73-codemap.md#L13-L13` VERIFIED. It is not a
  stand-alone "explain this repo to me" product — that role is explicitly given to **boards** (design 71): "Boards (71)
  remain the stored explanation; a lens can be snapshotted into a board" `docs/design/73-codemap.md#L11-L11` VERIFIED.
- **"Understanding a codebase" operationally today** = three questions with hard caps: *What's here* (ranked tree ≤ 2 000
  rows), *How it fits* (butterfly ≤ 20 nodes), *What's changing* (impact ripple) `docs/design/73-codemap.md#L13-L13`
  VERIFIED. Nothing answers "how does this system work as a whole" without an LLM: the index knows symbols, refs, imports,
  ranks and co-change; it has no notion of a *perspective* or a prose description (A7). **OPEN** for the omm-style map.
- Sky's rulings of 2026-09-28 (knowledge `decisions/theuth-codeindex-wave2-and-codemap-2026-09-28`): CodeMap is a live
  view not a document; availability like browser/PTY; real side pane; phone = lists; pop-out; signal over noise.
  INFERRED from knowledge (not re-read in the tree).

## A2. Vocabulary and unit

| theuth term | meaning | identity | where | status |
|---|---|---|---|---|
| **root** | one canonical repository/directory; one SQLite per root | canonical path; db file `<slug>-<hash8>.sqlite` | `crates/codeindex/src/root.rs#L11-L55` | BUILT VERIFIED |
| **file** | a walked file (`language` may be `other`) | `files.path` (root-relative, UNIQUE) | `crates/codeindex/src/store.rs#L29-L40` | BUILT VERIFIED |
| **symbol / definition** | a tree-sitter tag | `symbols.id` + **moniker** `<lang>:<path>#<Qualified.Name>(<kind>)` | `crates/codeindex/src/store.rs#L41-L57`, grammar `docs/design/73-codemap.md#L231-L231` | BUILT VERIFIED |
| **ref / import** | resolved reference (to_symbol, candidates, provenance, confidence) / import edge (target_file) | row ids | `crates/codeindex/src/store.rs#L66-L92` | BUILT VERIFIED |
| **component** | depth-2 directory (`crates/codeindex`, `src/store`) — the only "group" unit the engine has | path prefix | `crates/codeindex/src/rank.rs#L777-L786` | BUILT VERIFIED |
| **lens** | Here / Fit / Change — a view over one root for one session | `(root, lens, params)` per window, not persisted | `docs/design/73-codemap.md#L145-L153` | BUILT VERIFIED |
| **row / node** | `MapRow` (Here), `FitNode`/`FitEdge` (Fit), `ChangeFile`/`ChangeSym` (Change) | moniker (symbols) or path (files) | `crates/codeindex/src/lens.rs#L113-L307` | BUILT VERIFIED |
| **entry point** | manifest-declared bin/lib/main files | root-relative path list in `meta.entry_points` | `crates/codeindex/src/rank.rs#L301-L330`, `#L768-L773` | BUILT VERIFIED |

- There is **no element / perspective / group / leaf** unit, no stable id for a *diagram*, and no notion of a node that
  is "a directory + a description". The closest stable ids a port can reuse are the **moniker** (symbol level) and
  the **root-relative path** (file/dir level) — both already used as React keys and as `ref` anchors
  (`crates/desktop/ui/src/lib/codemap/fence.ts#L13-L13`) VERIFIED.
- Monikers are *not* stable across renames or signature changes (they embed path and kind); boards solved the same
  problem with `LocateAnchor.text_sha` + `LocateStatus = Same|Changed|Moved|Missing`
  (`crates/codeindex/src/locate.rs#L32-L59`) — the reusable staleness primitive (A10). VERIFIED (struct/enum lines).

## A3. Generation pipeline

Today there are **two producers and neither produces a map document**:

1. **Deterministic index (BUILT).** Every `code_*` call: `open(ix, root)` → `root::discover` → `open_root` →
   `reconcile` (walk with `ignore`, `(mtime_ns,size)` diff, ≤ 64 files inline else `stale:true` + background) → query
   → envelope → `CodeToolRan` event. `crates/codeindex/src/index.rs#L163-L197` (open_root + LRU of 8 pools),
   `#L199-L201` (reconcile signature), `crates/codeindex/src/tools.rs#L188-L201` (`ran!` + `tool_ran`) VERIFIED.
   Rank: `Graph::load` reads files/symbols/edges(confidence ≥ 0.5, cross-file)/imports(not `mod:`)/cochanges/entry
   points `crates/codeindex/src/rank.rs#L247-L340`; `persist` writes `files.rank`, `symbols.rank`, `meta.entry_points`
   `#L757-L775` VERIFIED. Constants: damping 0.85, 50 iterations, barrel/type damping, seeds for mentioned/edited/
   co-change/entry `#L19-L62` VERIFIED.
2. **The LLM, by hand, into a board (BUILT, not automated).** The `board` skill tells the model to write a six-section
   explanation with `Graph` fences carrying `ref` anchors; `check_excerpts` validates every Excerpt/`code:` link against
   the index at insert time `crates/document/src/tools.rs#L201-L234` VERIFIED; the skill text in
   `docs/design/71-whiteboard-visuals.md#L222-L222` VERIFIED.

- **Recursion/termination rules:** none for a map (there is no map). The Here lens does have a *coverage* rule that
  resembles omm's "every big directory gets a line": every depth-2 component with ≥ 5 % of parsed files and no row in
  the budgeted tree gets one line (`COVERAGE_SHARE`, `crates/codeindex/src/rank.rs#L62-L62`, `#L788-L795`) VERIFIED.
- **Prompts/skills used:** `codeindex` skill (`~/.theuth/skills/codeindex.md`), `board` skill (`crates/document/board.md`
  → skills), `visualize` skill (diagram routing). None asks the model to generate a repository map. INFERRED from the
  skill index in this session's prompt + `crates/document/src/tools.rs#L50-L50` VERIFIED.
- **Snapshot path (BUILT):** the panel's ⊡ button turns the current lens into one `json-render` `Graph` fence (≤ 12
  nodes, `ref` on every node) for the clipboard / a board `crates/desktop/ui/src/lib/codemap/fence.ts#L1-L47` VERIFIED.
  This is the only existing "generated diagram" path and it is UI-side and manual.

**Verdict:** a generation pipeline that writes *perspectives + per-element prose + nested diagrams* is **OPEN**; the
deterministic half (ranked symbols, import/ref graph, entry points, components, impact) is **BUILT** and queryable in
tens of ms (A13).

## A4. Perspectives / lens catalog

| lens | question | source RPC | caps | chosen how | status |
|---|---|---|---|---|---|
| **Here** | what is in this root, ranked | `codemap_here(root, focus?, edited?, limit)` → `HereLens` | `HERE_LIMIT = 2000` rows, dirs = depth-2 components | after `code_map` / `code_find` / `code_outline` | BUILT |
| **Fit** | who calls X / what X calls | `codemap_fit(root, symbol, limit)` → `FitLens` | `FIT_LIMIT = 20` (40 popped out), counted remainders | after `code_refs` / `code_symbol`, or "How it fits" on a row | BUILT |
| **Change** | what did this change touch | `codemap_change(root, git_range?, files?, include_tests?)` → `ChangeLens` | `CHANGE_CAPS = [20,30,30,30]`, ≤ 12 symbols per file | after `code_impact`; default = WORKTREE | BUILT |

Cites: `crates/codeindex/src/lens.rs#L21-L30` (caps), `crates/rpc/src/api.rs#L1075-L1112` (the four methods and their
doc comments), `crates/desktop/ui/src/components/codemap/CodeMapPanel.tsx#L117-L128` (one render queue, `limitFit =
popped ? 40 : 20`) — all VERIFIED. The lens state machine (auto/pinned, follow, replay) is pure TS:
`crates/desktop/ui/src/lib/codemap/lens.ts#L23-L86` VERIFIED (export lines).

- There is **no architectural perspective** (layers, data flow, request lifecycle, deployment, domain model), no
  per-directory "what this part is for", and no user-chosen catalog. The three lenses are *agent-activity* views. For
  the omm port this is the main gap: **OPEN**.
- The engine *does* already hold the ingredients for a deterministic "structure" perspective: components, import edges
  per file (`store::import_edges`, `crates/codeindex/src/store.rs#L1184-L1191`), entry points, rank share, test
  linkage (`test_subjects`, `#L1302-L1331`) VERIFIED.

## A5. Hierarchy and drill-down

- **Here** is a two-level tree: directory rows (`DirRow`, depth-2 component, with `ranked_rows` = dropped tags under
  it) → file rows → symbol rows; directories collapse by share of rank; ≤ 12 open dirs then `+N directories`
  (`docs/design/73-as-built-CODEMAP-UI.md#L34-L35` INFERRED from the as-built text; `crates/codeindex/src/lens.rs#L513-L530` component grouping VERIFIED).
- **Fit** is three fixed layers (callers | focus | callees) with *bundle nodes* for the remainder (`more_callers`,
  `more_callees`) — direction by position, no arrowheads (`docs/design/73-codemap.md#L13-L13` VERIFIED;
  `crates/desktop/ui/src/lib/codemap/layout.ts#L48-L48` `columnsLayout` VERIFIED).
- **Change** is four bands with per-band caps and `more: [u32;4]` (`crates/codeindex/src/lens.rs#L282-L307`) VERIFIED.
- **Drill-down between diagrams:** none. Click-through exists only as *actions on a node*: Ask about this (anchor into
  the composer), How it fits (pin Fit on it), Snapshot to board (`docs/design/73-codemap.md#L20-L20`;
  `CodeMapPanel.tsx#L223-L233` appendDraft/toGraphFence) VERIFIED. There is no "open the diagram of this node" and no
  breadcrumb (A11). **OPEN**.
- **Node caps as policy:** ≤ 20 in panel, ≤ 40 popped out, ≤ 12 on phones, from Yoghourdjian 2020 via the survey
  (`docs/design/73-codemap.md#L13-L13`) VERIFIED; the `Graph` component's hard validation caps are 120 nodes / 240
  edges / 12 groups (`crates/surface/src/lib.rs#L50-L52`) VERIFIED. A nested omm-style map would therefore land in a
  **~12–20 node per diagram** regime by existing rule (O4 measures how many directories exceed it).

## A6. Diagram language

Two languages coexist; **neither is the storage format of anything today**:

1. **`json-render` `Graph`** (design 65) — the typed in-house grammar: `Graph(nodes[{id,label,kind?,group?,state?,ref?,
   sub?,tag?,icon?}], edges[{from,to,label?,style?}], groups?[{id,label}], direction?)`
   `crates/surface/src/spec.rs#L544-L562` (GraphNode incl. `ref`) VERIFIED; `docs/design/65-graph-diagrams.md#L45-L62`
   (spec/validation/lowering) VERIFIED. Validated engine-side with located errors; caps above. Desktop renders it with
   `GraphSvg` over ELK in a worker (`crates/desktop/ui/src/lib/graph-layout.ts#L2-L3`, `#L12-L18`) VERIFIED. Text
   surfaces lower it **to** mermaid: `graph::to_mermaid` `crates/surface/src/graph.rs#L89-L89`, `crates/surface/src/lib.rs#L275-L281` VERIFIED.
2. **` ```mermaid ` fences** — rendered on the desktop by mermaid 12 (`securityLevel: 'strict'`, `htmlLabels: false`,
   ELK default layout, 80 ms render queue, theme from CSS tokens) `crates/desktop/ui/src/components/chat/Mermaid.tsx#L1-L45` VERIFIED;
   on the TUI/Slack by **merman** (Rust) → ASCII, with unsupported diagrams falling back to the source block
   `crates/surface/src/mermaid.rs#L1-L40` VERIFIED.

- **Does anything parse mermaid INTO a `Graph` spec?** **No.** Grep of `crates/desktop/ui/src` for mermaid parsing finds
  only the renderer and the editor's starter text; no Rust `from_mermaid`. VERIFIED (grep, 2026-10-08). However the
  dependency **merman-core 0.7** (already in the tree through `crates/surface/Cargo.toml#L17-L17` VERIFIED) exposes a
  parser (`parse_diagram_sync`, flowchart `FlowNode`/`FlowEdge` model in `~/.cargo/registry/src/*/merman-core-0.7.0/src/diagrams/flowchart/model.rs#L43-L77`) — a Rust seam for *mermaid → typed graph* exists but is unused. INFERRED (I read the grep output and the struct lines, not the parser body).
- **Subset in use:** `to_mermaid` emits `flowchart LR|TB`, `subgraph id["label"] … end`, shapes from `kind`, `-->` /
  `-.->`, quoted/escaped labels; merman 0.7 draws every shape except `[/ /]` and rejects valid ER `o|`
  (`crates/surface/src/mermaid.rs#L28-L37`) VERIFIED. No `classDef`, no `click`, no `%%` directives are written.
- **Ids:** mermaid ids sanitised to `[A-Za-z0-9_]` with keyword avoidance (`crates/surface/src/graph.rs#L28-L30`) VERIFIED.

**Implication for the port:** theuth's native diagram unit is the *typed `Graph` spec*, not a `.mmd` text; a map stored
as `Graph` specs renders on every channel (desktop SVG, TUI ASCII, Slack via mermaid lowering) and validates. A map
stored as mermaid text renders only where mermaid 12 or merman runs and cannot carry `ref`s. **Recommendation-grade,
INFERRED.**

## A7. Per-element prose

- **None in the index.** `symbols.docs` holds the doc comment captured by tree-sitter (`crates/codeindex/src/store.rs#L50-L50`)
  and is FTS-indexed (`#L58-L61`), but no row describes a file, directory, or component. VERIFIED.
- The lens replies carry only facts: signature (≤ 200 chars, `SIGNATURE_CHARS`), kind, rank, importers/callers/recent,
  `entry`, `is_test`, provenance/confidence (`crates/codeindex/src/lens.rs#L30-L30`, `#L113-L144`) VERIFIED.
- Prose about code lives in **boards** (markdown blocks + Excerpt/Graph fences) and **knowledge** concepts
  (`DocKind::Knowledge | Artifact | MeetingNote | Note` `crates/document/src/meta.rs#L83-L91` VERIFIED; a board is a
  Note with a `root:` frontmatter, `crates/document/src/tools.rs#L449-L452` VERIFIED).
- **OPEN** for the port: a per-element `{description, context, constraints, concerns}` record with an owner (LLM vs
  deterministic) and a verified-at stamp.

## A8. Grounding

- **Anchors:** the grammar is `path#L<a>-L<b>` everywhere (71 §2.3); `Graph` nodes carry `ref` (`spec.rs#L556-L556`);
  boards verify anchors at insert and on read (`code_locate` RPC, `crates/rpc/src/api.rs#L1064-L1074` VERIFIED;
  `LocateStatus` `crates/codeindex/src/locate.rs#L50-L50` VERIFIED).
- **Edges are real by construction:** refs are resolved import-first, "never guesses" (`docs/design/60-codeindex.md#L17-L17`
  decision 5) VERIFIED; the rank graph keeps only `confidence ≥ 0.5` cross-file refs (`rank.rs#L257-L259`) VERIFIED;
  ambiguous refs are a count, never an edge (`docs/design/73-codemap.md#L21-L21`) VERIFIED.
- **Cross-references between elements:** the snapshot fence gives every node a `ref`; nothing links one diagram to
  another. **OPEN**.
- **Verification of LLM-made diagram claims against code:** exists for Excerpts/anchors (text sha), not for edges.
  A port could verify an LLM-drawn edge `A → B` by asking the index whether any import/ref crosses from dir A to dir
  B — the data is there (`store::import_edges`, `refs_to`), the check is **OPEN**. INFERRED.

## A9. Storage

- **Index:** one SQLite per root under `<data_dir>/codeindex/<slug>-<hash8>.sqlite`, WAL, `CREATE … IF NOT EXISTS`,
  `SCHEMA_VERSION = "4"` (a bump = rebuild) `crates/codeindex/src/store.rs#L21-L21`, `crates/codeindex/src/index.rs#L132-L142`,
  `docs/design/60-codeindex.md#L21-L21` VERIFIED. Nothing is written inside the repository (60 decision 7; "putting the
  index in the repo pollutes the user's tree", `docs/design/60-codeindex.md#L303-L315` VERIFIED).
- Tables: `meta`, `files`, `symbols` (+ `symbols_fts`, `symbols_tri`), `imports`, `refs`, `cochanges`
  `crates/codeindex/src/store.rs#L27-L99` VERIFIED. A key-value **`map_cache:*` namespace in `meta`** already stores
  rendered map bodies keyed on index state (`store.rs#L1249-L1280`, used by `lens.rs#L415-L424`, `#L543-L543`) VERIFIED
  — i.e. the store already persists *derived renderings* with eviction by state.
- Observed on disk 2026-10-08: 36 index files under `~/.theuth/codeindex/` (theuth 49 MB + 7 MB WAL; oh-my-pi 133 MB;
  spike-moli 173 MB + 162 MB WAL; portal/2gP 15 MB). VERIFIED (`ls`).
- **Map/panel state:** nothing on disk; per-session slice in the UI store (`roots`, `last`, `trail`, `edited`;
  `crates/desktop/ui/src/lib/codemap/slice.ts#L1-L61`) and per-window lens state; only `codemap-panel-width` in
  localStorage (`CodeMapHost.tsx#L20-L23`) VERIFIED. **No stored CodeMap by ruling** (`docs/design/73-codemap.md#L11-L11`).
- **Boards/notes:** the document crate (Yjs-backed blocks, `kind="board"` + `root:` frontmatter), exported via
  `doc_export` to PDF/markdown. **Artifacts:** `<data_dir>/artifacts/<id>.<ext>` immutable per tool call
  (`crates/artifact/src/lib.rs#L161-L165`) VERIFIED.
- Sharing/cloud: none for the index; boards share through document membership (out of scope here).

## A10. Freshness

- **Reconcile-on-call, no watcher** (60 decision 8, `docs/design/60-codeindex.md#L23-L23`; 73 keeps "no watcher",
  `docs/design/73-codemap.md#L7-L7`) VERIFIED. Walk → `(mtime_ns,size)` diff → ≤ 64 files inline else `stale: true`
  + background task; `suspect_root` guard when > 80 % of files vanish (`crates/codeindex/src/index.rs#L26-L31`) VERIFIED.
- **Targeted `touch(root, paths)`** re-parses only named files and publishes `CodeReconciled` (gate ≥ 250 ms per root,
  `index.rs#L34-L34`, `#L371-L371`, `#L430-L430`) VERIFIED. The UI calls it 500 ms after an `edit`/`write_file` while a
  panel is mounted (`CodeMapPanel.tsx#L173-L180`) VERIFIED.
- **Measured cost of one touched file: 709 ms** (parse 148 + resolve 218 + rank 324) because every write re-persists
  the whole rank (`docs/design/73-as-built-CODEMAP-ENGINE.md#L53-L53`) VERIFIED — the known hot spot ("ponytail: defer
  `rank::persist`").
- **Per-element staleness:** only for board anchors (`LocateStatus`, text sha). Rank/lens bodies are cached per
  *index state* `(files, symbols, head)` — a whole-root key (`store::map_state`, `store.rs#L1240-L1246`) VERIFIED. A
  per-directory or per-diagram staleness key does **not** exist: **OPEN**. The cheapest deterministic key a port can
  derive is "max `files.mtime_ns` and file count under this dir" — one indexed query. INFERRED.
- **Diffs between versions:** `code_impact(git_range)` / Change lens (`Range::Worktree | Commits`, gix read-only,
  `crates/codeindex/src/git.rs`), not a diff of maps. VERIFIED via as-built; git.rs not re-read line by line.

## A11. Viewer

What exists is the **CodeMap side pane** (described in full in the closing section) plus the generic diagram stack:

- **Renderer:** `GraphSvg`/`GraphBody` (pure SVG from a `Layout`; `markerId: null` = no arrowheads; `badges`, `hatch`,
  `nodeClass`) `crates/desktop/ui/src/components/chat/surface/Graph.tsx#L176-L177`, `#L310-L323` VERIFIED. Layout for
  the generic `Graph` fence = ELK in a Web Worker, LRU by structural hash, 2 s time-box
  (`crates/desktop/ui/src/lib/graph-layout.ts#L2-L3`) VERIFIED. Lens layouts are hand-placed columns/bands with
  never-move slots (`lib/codemap/layout.ts#L26-L48`, `#L165-L165`) VERIFIED.
- **Frame:** `DiagramFrame` — pan/zoom, Copy PNG · Save PNG… · Save SVG… · Fullscreen, shared by mermaid, Graph and
  the 19 diagram grammars of 123 (`crates/desktop/ui/src/components/chat/DiagramFrame.tsx#L18-L19`, `#L173-L189`) VERIFIED.
- **Nested groups:** `Graph.groups` → ELK compound nodes (`graph-layout.ts#L2-L2`) VERIFIED; the lenses draw no groups.
- **Navigation:** lens tabs (`1/2/3`), `p` follow, Escape; root picker when > 1 root; trail replay; detail line; no
  tree-of-diagrams, no breadcrumbs, no back stack, no search box (search = the agent's `code_find`)
  (`CodeMapPanel.tsx#L292-L345`) VERIFIED. **OPEN** for omm-style navigation.
- **Empty/error states:** "run a code_* tool first" when the root is not indexed; "no symbol yet — run code_refs…" for
  Fit without a focus (`CodeMapPanel.tsx#L269-L290`) VERIFIED.
- **Boards reader** (`components/board/*`, 2 797 lines: Sections, Toc, Tour, Reveal, SidePane, anchors, live) is the
  other viewer that already does section navigation + anchor highlighting over `Graph` fences. VERIFIED (`wc`,
  file list); internals not re-read.

## A12. Live activity

BUILT and the strongest asset for the port:
- `CodeToolRan { session, root, call_id, tool, args, summary, seq }` on every `code_*` call (errors included) and
  `CodeReconciled { root, reparsed, removed, more, … }` on every write (`crates/codeindex/src/event.rs#L23-L60`) VERIFIED.
- UI: reducer sets `codemap.roots ∪= root`, `last`, `trail` (≤ 20) on `CodeToolRan`; `edit`/`write_file` starts append
  to `edited[root]` (≤ 200) (`crates/desktop/ui/src/lib/codemap/slice.ts#L24-L52`; wired at
  `crates/desktop/ui/src/store/reduce.ts#L771-L772`, `#L991-L991`) VERIFIED.
- Panel: the **marker** `►` sits on the trail target (definition anchor / outlined path / first focus / impact range,
  `slice.ts#L10-L21`), `▎` focus bar, `~` edited, `→N ↑N new ◆entry test` badges (as-built UI `#L35-L35`) VERIFIED.
- Agents' *reads* (`read_file`, `grep`) are **not** on the map — only `code_*` calls and edit tools. INFERRED from the
  reducer (no `ToolCallStarted` handling beyond edit/write_file in `codeEdited`). For an omm-style map the overlay
  seam is: `trail[].target` and `edited[root]` → highlight the diagram node whose path-prefix contains them (O6).

## A13. Scale and performance

Measured numbers from the engine as-built (debug build, this repo at 1 760 files / 2 184 ranked definitions,
`docs/design/73-as-built-CODEMAP-ENGINE.md#L42-L57` VERIFIED):

| path | contract | measured |
|---|---|---|
| `codemap_here` warm (2 000 rows) | < 30 ms | 13 ms engine, 44 ms wall (0.5 MB JSON) |
| `codemap_here` miss | < 80 ms | 176 ms |
| `codemap_here` personalised miss (PageRank) | — | 462 ms debug |
| `codemap_fit` (283 callers → 19 shown) | < 30 ms | 7 ms |
| `codemap_change` WORKTREE cold / cached | < 300 ms | 206 ms / 2–4 ms |
| `codemap_touch` unchanged / changed file | < 50 ms | 25 ms / **709 ms** |
| rank persist (whole repo) | — | 323–329 ms |

- Today's theuth index: 3 255 files, 1 613 parsed, 31 161 symbols, 244 947 refs, 11 511 resolved imports (sqlite3,
  2026-10-08) VERIFIED. Walk alone 73–89 ms debug (60 §22.1, cited by `docs/design/73-codemap.md#L17-L17`) VERIFIED.
- UI contract: first paint of 2 000 rows < 100 ms (virtualised), delta < 16 ms, idle = no animation
  (`docs/design/73-codemap.md#L16-L16`) VERIFIED. ELK 40-node layout < 100 ms warm (65 §9 step 3,
  `docs/design/65-graph-diagrams.md#L131-L131`) VERIFIED.
- Known ceilings: the event bus is one 1 024-slot broadcast ring shared by every subscriber (73 rejected rows on events
  for that reason, `docs/design/73-codemap.md#L15-L15`) VERIFIED; the codeindex walk is serial (knowledge
  `projects/theuth-wave-glob-parallel-2026-10-07`, INFERRED).

## A14. Cost

- **Index and lenses: zero model tokens.** Every `code_*` tool is `Risk::ReadOnly`, deterministic (60 decision 9,
  `docs/design/60-codeindex.md#L19-L19`) VERIFIED; "no LLM in the render path" (`docs/design/73-codemap.md#L7-L7`) VERIFIED.
  Wall time per full index of this repo: parse+resolve+rank on the order of seconds (60 §22.1 as cited; not re-measured
  here — building is forbidden). Per touch: 25–709 ms (A13).
- **Agent-side cost of using the index:** `DEFAULT_BUDGET = 4000` tokens for `code_map` text, `FETCH_LIMIT = 1000` rows
  (`docs/design/73-codemap.md#L36-L36` cites `tools.rs`) VERIFIED in the design; constants not re-read in tools.rs
  (line numbers have moved since).
- **Boards (LLM-authored explanations):** paid by the session that writes them — one turn of the session model; no cap
  specific to boards.
- **Budget controls that exist for off-turn work (O5):** `Budget { nanos, currency, since }` per session, `SetBudget`
  from the schedule plugin on every fire, `Reserved::open` refuses any model call over the cap
  (`crates/agent/src/lib.rs#L462-L489`) VERIFIED; routine files carry `budget:` and `cap:` keys
  (`crates/schedule/src/file.rs#L423-L429`) VERIFIED; spend is attributed per owner in the `model_calls` ledger
  (`lib.rs#L455-L455` comment) VERIFIED. **Who pays for a generated map today: nobody — it does not exist.**

## A15. Integration surface

| surface | what | cite | status |
|---|---|---|---|
| runtime functions (python) | `code_map/find/outline/refs/symbol/impact`, `project_profile` | `docs/design/60-codeindex.md#L19-L19`; `crates/codeindex/src/profile.rs#L13-L60` | BUILT |
| events | `CodeindexEvent::{CodeToolRan, CodeReconciled}` on the bus, bridged to `subscribe_events` | `crates/codeindex/src/event.rs#L23-L60` | BUILT |
| RPC | `codeindex_stats`, `code_locate`, `codemap_here/fit/change/touch` | `crates/rpc/src/api.rs#L1062-L1112` | BUILT |
| TS bindings | ts-rs exports of lens types/events; `Api` + `METHODS` | `crates/desktop/ui/src/lib/codemap/types.ts` (60 lines) | BUILT (file listed, not re-read) |
| skills | `codeindex`, `board`, `visualize` | prompt index of this session | BUILT |
| CLI | none for the index (the `t` binary has no `codeindex` subcommand that I found) | — | OPEN (INFERRED, not exhaustively grepped) |
| agent tool to *write* a map element | none | — | OPEN |
| document tools | `doc_create(kind="board", root=…)`, `doc_insert/replace`, `check_excerpts` against the index | `crates/document/src/tools.rs#L50-L50`, `#L201-L234` | BUILT |

## A16. Channels and export

- **Desktop:** side pane (360–920 px, persisted width, ≤ 50 vw) beside the chat; phone (< 768 px) = full-height bottom
  Sheet with list forms only; graph forms need ≥ 600 px of panel width (`CodeMapHost.tsx#L1-L5`, `#L20-L23`;
  `CodeMapPanel.tsx#L35-L36`) VERIFIED.
- **Pop-out:** `PopoutTarget {kind:'codemap', session, root}` → route `/codemap/$session/$root` → `PoppedCodeMap`
  (`crates/desktop/ui/src/lib/popout.ts#L21-L22`, `crates/desktop/ui/src/routes/codemap.$session.$root.tsx#L1-L10`) VERIFIED;
  hidden on iOS by `canPopout()`; popped graphs get the 40-node cap (`CodeMapPanel.tsx#L120-L120`) VERIFIED.
- **PNG/SVG:** every diagram inside `DiagramFrame` has Copy PNG / Save PNG / Save SVG (`DiagramFrame.tsx#L173-L189`)
  VERIFIED; the lens SVGs are drawn by `GraphSvg` and are exportable the same way (INFERRED — the lens components pass
  through `GraphSvg`; I did not confirm they are wrapped in `DiagramFrame`).
- **Text fallbacks:** `Graph` → mermaid → merman ASCII on the TUI; Slack gets Block Kit for 26 components and flattens
  diagrams at 60 cols (knowledge `facts/theuth-agent-visual-surfaces-inventory-2026-10-06`, INFERRED); mermaid fences
  on the TUI via `mermaid_lines` (`crates/surface/src/mermaid.rs#L1-L16`) VERIFIED.
- **Snapshot to board** → `json-render` Graph fence on the clipboard (`fence.ts#L44-L47`) VERIFIED. **Share links:** none.
- **HTML pages (design 120):** `show x.html` renders a sandboxed page inline — a possible host for an omm-like
  self-contained viewer, but it has **no channel back to the agent** and no access to the engine RPC (knowledge
  `facts/theuth-borrow-list-from-t3code-visual-replies-2026-10-06`, INFERRED).

## A17. Licence, dependencies, tests, code quality

- Crate `codeindex`: `MIT OR Apache-2.0`; deps tree-sitter 0.27 + tags + 5 grammars, sqlx, gix (read-only), ignore,
  kameo actor, cargo_toml/globset/oxc_resolver/toml for import resolution, ts-rs optional
  (`crates/codeindex/Cargo.toml#L1-L35`) VERIFIED. Workspace: 49 members under `crates/*` (`Cargo.toml#L3-L3`) VERIFIED.
- Size: 9 536 lines in `crates/codeindex/src` (tools.rs 2 070, store.rs 1 507, rank.rs 1 401, lens.rs 879) VERIFIED (`wc`).
- Tests: `tests/{codemap,index,locate}.rs` + unit tests (rank.rs 11, index tests 11 `#[tokio::test]` attributes counted
  by grep; the as-built reports 40 unit + 7 codemap + 23 index + 1 locate green at build time,
  `docs/design/73-as-built-CODEMAP-ENGINE.md#L58-L65` INFERRED for line range) VERIFIED counts by grep. UI:
  `lib/codemap/{fence,here,labels,layout,lens}.test.ts` (5 files) VERIFIED (`wc`).
- Quality signals: every public fn documented with the design § it implements; `ponytail:` markers name known debts
  (rank persist on touch, one write lock across walk+diff+write, `index.rs#L58-L60`) VERIFIED.
- UI diagram deps: `mermaid ^12.0.0` (`crates/desktop/ui/package.json#L50-L50`), `elkjs` via mermaid, `@tanstack/react-virtual`
  for Here; **no dagre** (73 §10 rejected it, `docs/design/73-codemap.md#L477-L477`) VERIFIED.
- Known import-resolution gap seen while indexing omm (O4): **107 of 188 imports unresolved** — TypeScript ESM
  specifiers ending in `.js` (`../lib/cloud.js`) are not mapped to `.ts` (oxc_resolver needs an `extensionAlias`).
  VERIFIED by sqlite query. Any TS project using NodeNext resolution gets *no import edges*; refs still resolve.

## O1. Live design plans and constraints

Rules currently in force, with the design that owns each (all VERIFIED unless marked):

| rule | owner | cite | status |
|---|---|---|---|
| CodeMap is a live view, **not a document**; no `DocKind::CodeMap`, no artifact per map; snapshot into a board instead | 73 §0.1 | `docs/design/73-codemap.md#L11-L11` | ruling (Sky accepted) |
| **No LLM in the render path**; no general graph explorer / "show me the whole repo"; no force layout; no editing from the map; no watcher | 73 scope line | `docs/design/73-codemap.md#L7-L7` | ruling |
| Availability = one `CodeToolRan` flips a per-session boolean; **never auto-opens** | 73 §0.2 | `#L12-L12` | BUILT |
| Node caps ≤ 20 panel / ≤ 40 popped / phones = lists; hidden things shown as counted bundles; colour never alone | 73 §0.3, §0.11 | `#L13-L13`, `#L21-L21` | BUILT |
| Rendering = existing `GraphSvg` + hand layouts; **no second layout library** (dagre rejected) | 73 §0.4, §10 | `#L14-L14`, `#L477-L477` | BUILT |
| No rows on the event channel; lenses read the store, **never walk** | 73 §0.5 | `#L15-L15` | BUILT (`lens_rpcs_never_walk` test) |
| Performance contract: lens RPC < 30 ms warm, idle = no animation, ≤ 1 MiB per root slice | 73 §0.6 | `#L16-L16` | BUILT (mostly met; see A13) |
| Index lives under `<data_dir>/codeindex/`, never in the repo; schema change = `rm` | 60 §0.7 | `docs/design/60-codeindex.md#L21-L21`, `#L303-L315` | BUILT |
| Reconcile on call, no daemon; ≤ 64 files inline | 60 §0.8 | `#L23-L23` | BUILT |
| Resolution never guesses; type-aware edges knowingly given up | 60 §0.5, §12 | `#L17-L17`, `#L303-L305` | BUILT |
| One React renderer for graphs; ELK off-thread shared with mermaid 12 | 65 §0.3/0.4 | `docs/design/65-graph-diagrams.md#L13-L13` | BUILT |
| `Graph` caps 120 nodes / 240 edges / 12 groups / label ≤ 80 | 65 §3.2 | `crates/surface/src/lib.rs#L50-L52` | BUILT |
| Boards: six fixed sections, anchor everything `path#La-Lb`, `ref` on every Graph node | 71 §3 | `docs/design/71-whiteboard-visuals.md#L222-L222` | BUILT |
| Inline HTML pages: self-contained, sandboxed, no agent channel | 120 | knowledge (INFERRED) | BUILT |
| 19 diagram grammars share `DiagramFrame` + `lib/diagram` kernel | 123 | `DiagramFrame.tsx#L18-L19` | BUILT |

**In flight (≥ 140) touching these seams:** none touches codemap/codeindex UI. 140 (autoresearch) only *uses*
`code_map(focus, budget)` in its experiment brief and gix read-only (`docs/design/140-autoresearch.md#L265-L265`, `#L56-L56`)
VERIFIED; 144 (notebooks) reuses `codeindex::LocateStatus` for `cite_resolve` (`docs/design/144-notebooks.md#L426-L426`,
`#L708-L708`) VERIFIED; 145 (talk) touched every `ToolCx` literal incl. codeindex (`git log`, commit `d61b1d80`) VERIFIED.
Branches `feat/140-*`, `feat/141-*`, `feat/144-*`, `feat/145-*`, `fix/146-*` exist; none named codemap. VERIFIED (`git branch -a`).
Last codemap commits: `806056d6` (138 DIVIDERS shared ResizeHandle), `2ab95414` (123 C kernel), then the 73 wave. VERIFIED.

**Constraint the port must negotiate with Sky:** 73 §0.1 ("not a document, nothing stored") and §9 ("LLM summaries in
the render path" given up, `docs/design/73-codemap.md#L472-L472`) are the two rulings an omm-style *generated, stored,
prose-bearing* map contradicts. Everything else (caps, no walk in RPC, no watcher, store under data_dir) is compatible.

## O2. Gaps for this port

Ordered by how much of the omm promise each blocks (status of each: OPEN unless noted).

1. **No map document / element record** — nothing stores "perspective → diagram → elements → prose" (A2, A7, A9).
2. **No generator** — no job that produces diagrams + prose from the index, with or without an LLM (A3).
3. **No drill-down / navigation model** — no diagram-of-this-node, no breadcrumb/back, no tree of diagrams (A5, A11).
4. **No per-element staleness** — only whole-root `map_state` and board-anchor `LocateStatus` (A10).
5. **No perspective catalog** — three activity lenses only (A4).
6. **No verification of LLM-drawn edges** against the index (A8) — the data exists, the check does not.
7. **No mermaid → typed graph path**; a map authored as mermaid text would have no `ref`s and no validation (A6).
   merman-core's parser is an unused seam.
8. **Import resolution gap for TS NodeNext (`.js` specifiers)** — omm itself indexes with 107/188 imports unresolved (A17).
9. **Touch cost 709 ms** per edited file (rank re-persist) — fine for the agent, painful for a staleness-driven
   regenerate loop (A13).
10. **No agent tool to write/refresh an element**; `doc_insert` on a board is the nearest (A15).
11. **No per-directory rank share or entry-point RPC** — computable from `files.rank`/`meta.entry_points` (O4 shows it
    is a one-query aggregate) but not exposed.
12. **Phones** — the panel is lists on < 768 px; an omm-style nested diagram needs a list form by rule (73 §0.9).

## O3. Assets to reuse

| asset | where | why it matters for the port | status |
|---|---|---|---|
| Ranked file/symbol graph with PageRank, entry points, components, test linkage | `rank.rs#L247-L340`, `#L757-L786` | deterministic skeleton: nodes = dirs/components, weights = rank share, edges = aggregated imports/refs (O4) | BUILT |
| `map_cache:*` keyed on index state | `store.rs#L1249-L1280` | storage pattern for derived bodies with eviction; a `map:` namespace could sit beside it | BUILT |
| `CodeToolRan`/`CodeReconciled` + `seq` + 250 ms gate | `event.rs#L23-L60`, `index.rs#L34-L34` | progress + staleness events, already bridged to the UI | BUILT |
| `touch(root, paths)` | `index.rs#L371-L371` | targeted refresh after edits | BUILT |
| `LocateAnchor`/`LocateStatus` | `locate.rs#L32-L59` | per-element "is this anchor still true" | BUILT |
| `Graph` spec with `ref`, validation, mermaid lowering | `spec.rs#L544-L562`, `graph.rs#L89-L89` | the storage/render unit for every diagram, every channel | BUILT |
| `GraphSvg` + ELK worker + `DiagramFrame` | `Graph.tsx#L310-L323`, `graph-layout.ts#L2-L18`, `DiagramFrame.tsx#L173-L189` | render nested groups, pan/zoom, PNG/SVG | BUILT |
| Never-move `slots`, badges, hatch, marker, trail | `lib/codemap/layout.ts#L26-L48`, `slice.ts#L10-L21` | live overlay primitives | BUILT |
| Side pane host, Sheet, pop-out route, header button | `CodeMapHost.tsx#L1-L98`, `popout.ts#L21-L22`, `header.ts#L73-L89` | the panel chrome stays | BUILT |
| Boards reader (sections, TOC, tour, anchors, live) | `components/board/*` | a tree-of-sections viewer that already lights anchors | BUILT |
| `doc_*` tools + `check_excerpts` | `document/src/tools.rs#L201-L234` | agent-side writing of validated, anchored prose + diagrams | BUILT |
| Off-turn budgeted routine machinery | `dreaming/src/routine.md#L1-L16`, `agent/src/lib.rs#L462-L489` | generate the map in the background under a budget (O5) | BUILT |
| `project_profile` | `profile.rs#L13-L60` | kind/package manager/commands/manifests for the root — the "what is this repo" header | BUILT |
| merman-core parser | cargo registry (via `surface/Cargo.toml#L17-L17`) | mermaid → typed graph if the LLM is allowed to write mermaid | UNUSED seam |

## O4. Deterministic-skeleton feasibility measured on real repos

Method (2026-10-08, read-only `sqlite3` over `~/.theuth/codeindex/*.sqlite`; omm indexed fresh by one `code_map` call,
73 files in < 2 s; raw edge lists kept under `o4-omm-R2/`): file→file edges = resolved `imports` (target_file set,
not `mod:`) ∪ cross-file `refs` with `confidence ≥ 0.5` (the same edge set `Graph::load` uses, `rank.rs#L257-L278`),
aggregated to dir→dir at depth 1, 2, 3 (self-loops dropped). "Containers > cap" counts every directory (any depth) whose
direct children (files + subdirs among *parsed* files) exceed the cap — i.e. how many drill-down diagrams would need
bundling at that cap. Rank share = Σ `files.rank` under the directory ÷ Σ all.

### Roots

| root | files walked | parsed (lang ≠ other) | languages | symbols | refs | resolved imports | file→file edges | entry points (`meta.entry_points`) | index size |
|---|---|---|---|---|---|---|---|---|---|
| ~/Develop/theuth (Cargo workspace, 49 crates + 2 bun packages) | 3 255 | 1 613 | rust 662 · ts 585 · tsx 345 · py 15 · js 3 · go 2 | 31 161 | 244 947 | 11 511 | **7 185** | **60** (every crate's lib.rs/main.rs + packages/*/src entry files) | 49 MB + 7 MB WAL |
| ~/Develop/oh-my-mermaid (npm, vitest) | 73 | 55 | ts 52 · js 3 | (small) | (small) | **3** (107 `.js`-suffixed relative imports unresolved) | 83 | **0** (package.json `bin` not treated as an entry) | < 1 MB |
| ~/Develop/portal/2gP (Grails + webpack JS monorepo, already indexed) | 9 185 | 1 038 | js 889 · tsx 104 · ts 35 · py 10 | 8 888 | 58 287 | 1 147 | 1 328 | **0** (no Cargo/pyproject; package.json without main) | 15 MB |

### Directory graphs

| root | depth | dir nodes (connected / all) | dir edges | Σ weight | max fan-in | max fan-out |
|---|---|---|---|---|---|---|
| theuth | 1 | 5 / 5 | 8 | 170 | crates 3 | crates 3 |
| theuth | 2 (= crates/packages) | **60 / 61** | **342** | 5 846 | crates/runtime 37 | crates/rpc 38 |
| theuth | 3 | 97 / 105 | 419 | 7 122 | crates/runtime/src 49 | crates/rpc/src 36 |
| omm | 1 | 3 / 3 | 2 | 11 | scripts 1 | src 2 |
| omm | 2 | 7 / 7 | 9 | 171 | src/lib 3 | src/commands 3 |
| omm | 3 | 8 / 8 | 10 | 171 | src/lib 3 | src/commands 4 |
| portal/2gP | 1 | 4 / 4 | 6 | 596 | icd-common 2 | 2gP 3 |
| portal/2gP | 2 | 10 / 13 | 13 | 661 | icd-common/grails-app 4 | 2gP/src 4 |
| portal/2gP | 3 | 11 / 21 | 14 | 661 | icd-common/grails-app/assets 5 | 2gP/src/main 4 |

### Caps

| root | directories (containers) | > 12 children | > 20 children | worst |
|---|---|---|---|---|
| theuth | 249 | **35** | **16** | `crates/desktop/ui/src/lib` 163 · `…/components/chat` 98 · `crates/rpc/bindings` 59 · `crates/agent/bindings` 52 · `crates` 49 |
| omm | 8 | 1 | 1 | `src/commands` 22 |
| portal/2gP | 231 | 23 | 11 | `admin/…/static/docs/admin/Data` 152 · `2gP/…/components/compliance` 38 · `…/tradeledger` 37 |

### Rank share per directory (top, depth 2) — available from `files.rank` with one GROUP BY

| root | shares |
|---|---|
| theuth | crates/desktop **53.3 %** (758 files) · crates/agent 5.7 · crates/tools 4.7 · crates/web 2.7 · crates/codeindex 1.8 · packages/apps 1.8 |
| omm | src/lib 46.4 % · src/commands 20.0 · src 17.6 · src/__tests__ 7.0 · scripts 4.1 · src/server 4.1 |
| portal/2gP | 2gP/src 47.8 % · admin/src 27.4 · icd-common/grails-app 12.0 · icd-common/src 11.0 · infrastructure/pulumi 1.0 |

### Hubs (theuth, depth 2)

Fan-in: runtime 37, tools 32, agent 23, platform 18, replica 16, mesh 12. Fan-out: rpc 38, tui 31, theuth 19, agent 18,
document 14, slack 14. Removing the three largest fan-in hubs as explicit nodes drops the crate graph from 342 to 230
edges — still far above a 20-node/40-edge diagram. VERIFIED (computed).

### What the numbers say (INFERRED from the measurements above)

1. **Depth 1 is useless on a workspace** (theuth: 5 nodes, 95 % of rank in `crates`); **depth 2 is the natural top
   diagram for a Cargo workspace but blows every cap** (60 nodes, 342 edges). A deterministic skeleton therefore needs a
   *grouping step above directories* — by layer (fan-in hubs = foundation: runtime/tools/platform; fan-out = shells:
   rpc/tui/theuth/desktop; the rest = plugins) or by rank-share bucketing — to reach ≤ 20 nodes. The hub statistics
   above are exactly the signal needed, and they are one query away.
2. **Mid-size app repos (portal/2gP) fit the caps at depth 2–3 without help** (10–11 connected nodes, 13–14 edges) but
   their *leaf* directories do not (23 containers > 12): drill-down into a components folder needs bundling or a list.
3. **Small repos (omm) are trivially within caps** at every depth (≤ 8 nodes, ≤ 10 edges) — one top diagram + per-dir
   diagrams would be 8 diagrams in total.
4. **Rank share per directory is available and discriminating** (53 % / 48 % / 46 % in one directory for all three
   roots): it tells the generator where to spend LLM prose and where to descend first.
5. **Entry points are only identified when a manifest declares them** (60 for theuth, 0 for omm and 2gP). A skeleton
   for JS/TS repos needs `package.json` `bin`/`main`/`exports` and framework conventions (`src/index.ts`, `cli.ts`,
   `server/*`) — a small addition to `walk::entry_points` (`crates/codeindex/src/walk.rs#L227-L267` VERIFIED: it
   reads only `Cargo.toml`/`package.json`/`pyproject.toml` and, per the rank loader, Go `package main`).
6. **Edge weights are heavy-tailed** (theuth depth 2: 5 846 ref+import counts across 342 edges) — a dir→dir edge with
   weight 1–2 is noise; thresholding by weight share per source node would prune most of the 342 before bundling.
7. **Import resolution for TS NodeNext must be fixed first** for TS-first repos (omm 3/188 resolved); refs alone
   still produced 83 file edges for omm, so the skeleton is usable but under-connected.
8. **Cost:** all of the above ran in well under a second per root from the existing SQLite — there is no indexing
   work to add for a deterministic skeleton, only aggregation queries (dir→dir, rank share, hub stats, container sizes).

### Cross-check against the sibling spike (knowledge `facts/theuth-codemap-generation-spike-on-theuth-2026-10-08`, INFERRED)

The R4 spike that ran in parallel (17:41–17:51) reached the same shape independently: its deterministic top level of
theuth had 68 nodes / 366 edges (49 crates + 12 UI folders + 7 dirs, Cargo path deps as ground truth) — "correct data
but fails as a diagram"; in-degree tiering gave three true-but-useless tiers; a model made an 11-node top level with
92 % index-backed edges, and every drill-down below it was 100 % index-derived in structure. My depth-2 numbers
(60 nodes / 342 edges from refs+imports; hubs runtime/tools/agent) agree with that. Consequence for the design: the
index decides *drill-down structure, weights, staleness and edge truth*; a model decides *the top-level partition and
the names* — exactly the split O6 assumes.

## O5. Off-turn LLM machinery

How the engine runs model work outside a user turn today, and what a "generate the map in the background under a
budget" job would reuse.

| mechanism | what it does | cite | reusable for |
|---|---|---|---|
| **Detached side call** `Side::call(model, system, user, budget: Duration, kind: CallKind)` | one non-streaming, no-tools, no-history request on a *role* model, outside the turn, fenced on the session's `closed` token, accounted under its own `CallKind`, carrying the session's `Budget` | `crates/agent/src/loop_/side.rs#L1-L27`, `#L83-L100` | **per-element prose** (one call per directory with the deterministic facts as `user`); no tools, so the facts must be pre-assembled by the engine |
| **Title generation** (`maybe_title`) | `title::due(...)` gate → `role_model(Title)` → spawn a tokio task over a digest of the session; aborts the previous task | `crates/agent/src/loop_.rs#L5121-L5175` | the *pattern* of a cheap side model on a gate + abort-on-newer |
| **Harvest** (`spawn_harvest`) | one in-flight knowledge-harvest task per session after turns | `crates/agent/src/loop_.rs#L6465-L6475` | post-turn "refresh the elements this turn touched" hook |
| **Compaction** (`Compact`, `compact_now`) | summariser model call under `Reserved::open` (budget-checked) | `crates/agent/src/lib.rs#L425-L439`, `crates/agent/src/loop_.rs#L6319-L6319` | shows every off-turn call goes through the same budget gate |
| **Schedules / routines** | markdown files with `cron/every/at/when:<sensor>`, `fire`, `cooldown`, `mode`, `memory/runtime: clear`, `quiet`, `approvals`, **`cap:` risk ceiling**, **`budget:` money cap**, `run_timeout`, `max_runs`, `until` | `crates/schedule/src/file.rs#L414-L429` (spec fields), `crates/schedule/src/lib.rs#L498-L507` (`ScheduleEvent::Fired{name, session, at}`) | **the generator job itself**: a routine per root (or one routine that iterates stale roots) |
| **Dreaming (136)** — the worked example | binary-owned routine template: `every: 30m`, `when: dreaming.py window=… idle=…` sensor gate, `fire: while_true`, `cooldown: 3h`, `quiet: true`, `approvals: false`, `cap: mutating`, `run_timeout`, `budget`, `max_runs: 100000`, `cwd: <data_dir>`, `model: <tidy tier>`; body = "load the skill, run phases, spend in budget order, the budget guard ends the turn" | `crates/dreaming/src/routine.md#L1-L16`, `crates/dreaming/src/lib.rs#L100-L150` (`routine_text`, `install_files`) | **template to copy**: a `codemap.md` routine installed by the codeindex plugin, gated by a sensor ("index newer than map && user idle"), with its own skill and a bounded toolset |
| **Budget** `Budget{nanos,currency,since}` + `SetBudget` + `Reserved::open` | the schedule plugin sends the cap on every fire; any round/harvest/compaction/side call over the cap is refused; persisted on the session row | `crates/agent/src/lib.rs#L455-L489` | per-fire money cap for the generator |
| **Toolset / cap** | `Toolset{tools, prompts, runtime, hint_limit}` + `tools::SetCap` confine a routine session | knowledge `facts/theuth-autoresearch-substrate-and-gaps-2026-10-08` (INFERRED; `SetToolset` at `crates/agent/src/lib.rs` ~463–490 not re-read) | the generator's tool list: `code_*`, `doc_*`/`map_*`, read-only fs |
| **Subagent spawn from engine code** | `SpawnSubagentTool` (peers.rs) is a *tool*; supervisor has `SpawnChild`; max_depth 1, 16 native / 32 external permits | `crates/agent/src/peers.rs#L35-L67` (struct lines) VERIFIED; limits INFERRED from knowledge | fan-out per perspective is possible but heavy; one routine session with side calls is cheaper |
| **Tasks** | `task_create(owner, state: todo)` wakes a desk; durable board | this session's prompt (not re-read in tree) | a user-visible "generate map for X" task if the map should be reviewable |
| **Sensors** | `when: <file>.py key=value` polled; `list_sensors()` | `crates/dreaming/src/lib.rs#L141-L142` (`sensors/dreaming.py` bundled) VERIFIED | `codemap.py`: fires when `meta.last_reconcile_at` > map's `generated_at` for any open root |

**Shape a port would reuse (INFERRED):** the dreaming shape verbatim — a plugin-installed routine file with `budget:`,
`cap: read_only` (the generator only writes its own store through plugin tools), `quiet: true`, a sensor gate, and a
skill that says "for each stale element: read the deterministic facts the engine hands you, write ≤ N words, validate
the diagram"; prose per element through `Side::call`-style no-tool calls on a cheap tier (`title_model` precedent) to
keep the cost at cents per root; progress published as a new `CodeindexEvent` variant so the panel can show
"generating 12/60" like `CodeReconciled` does. Caveat from 136's review: the first cut of its budget was a lifetime cap
on the reused session — the fix made it per fire (`Budget::since`), and that is what `SetBudget` does today (`lib.rs#L468-L471`).

## O6. Seams table

Where each piece of an omm-style port would live, and which design owns that seam today. Status: every row is **OPEN**
(to build) unless the "exists today" column says BUILT.

| piece | crate / file / function it would live in | owner design today | exists today |
|---|---|---|---|
| Generator job (deterministic skeleton) | `crates/codeindex/src/map.rs` (new) next to `rank.rs`; aggregation queries in `store.rs` beside `map_facts` (`store.rs#L1200-L1237`) and `import_edges` (`#L1184-L1191`); run from the codeindex actor like `lens_handler!` (`lib.rs:208-224` per as-built) | 60 (store/rank), 73 (lens actor) | pieces BUILT (rank, components, import edges, entry points); the aggregation + skeleton builder OPEN |
| Generator job (LLM prose/perspectives) | a plugin-installed routine `schedules/codemap.md` + `skills/codemap-gen.md` + sensor `sensors/codemap.py`, modelled on `crates/dreaming/src/routine.md#L1-L16` / `lib.rs#L100-L150`; prose via `Side::call` (`crates/agent/src/loop_/side.rs#L83-L100`) or the routine's own turn with a bounded toolset | 136 (routine shape, budget, cap), 33/34 (schedules), 57 (SideCall) | machinery BUILT; the routine, skill and sensor OPEN |
| Storage of generated diagrams + prose | **option A** new tables in the root's index SQLite (`map_elements`, `map_diagrams`) beside `map_cache:*` (`store.rs#L1249-L1280`) — derived data, rebuild = `rm`, bump `SCHEMA_VERSION` (`store.rs#L21-L21`); **option B** a board per root in the document store (`DocKind::Note` + `root:` frontmatter, `document/src/meta.rs#L83-L91`, `tools.rs#L449-L452`); **option C** `<data_dir>/codemap/<slug-hash8>.json`; **option D** `.theuth-map/` in the repo (refused by 60 §12) | 60 §0.7 (data dir), 73 §0.1 (no stored map), 71 (boards) | A/B/C storage substrates BUILT; schema and ruling OPEN (see closing section) |
| Staleness per element | key `(dir, max(files.mtime_ns), count(files), git_head)` computed in `store.rs`; compare at `reconcile` end (`index.rs#L199-L201`) and at `touch` (`#L371`); anchors via `locate.rs#L32-L59` | 60 (reconcile), 73 (touch/gate), 71 (Locate) | inputs BUILT; the per-element key OPEN |
| RPC to read the map | `codemap_map(root, element_id?)` → `MapLens`/`MapElement` in `crates/codeindex/src/lens.rs` + `crates/rpc/src/api.rs` trait (`#L1075-L1112` pattern) + impl (`:4934-4989` region) + `export.rs` bindings | 73 §5.2 | pattern BUILT; method OPEN |
| Events for progress / staleness | new `CodeindexEvent::{MapProgress{root, done, total, seq}, MapStale{root, elements}}` in `event.rs#L23-L60`, through the same 250 ms gate (`index.rs#L34-L34`, `#L430`) and `bridge.rs forward!` | 73 §5.1 | pattern BUILT; variants OPEN |
| Agent tool / skill to write or refresh elements | `MapPutTool`/`MapRefreshTool` in `crates/codeindex/src/tools.rs` (`ran!` shape `#L188-L201`), `Risk::Mutating` on the index only; or reuse `doc_insert` if storage = board (`document/src/tools.rs#L201-L234` validates anchors) | 60 §0.9 (tools), 71 (doc tools) | OPEN (tool); doc path BUILT |
| Viewer component + navigation | `crates/desktop/ui/src/components/codemap/Map.tsx` as a **fourth lens** in `CodeMapPanel.tsx` (`LENSES`, tabs `#L308-L318`), rendering `Graph` specs through `GraphView`/`GraphSvg` + ELK (`graph-layout.ts#L2-L18`) with groups; breadcrumb + back stack in `lib/codemap/lens.ts` (`#L23-L86`); list form below 600 px (`CodeMapPanel.tsx#L35-L36`) | 73 §6, 65 §3.4, 123 (DiagramFrame/kernel) | renderer BUILT; lens, nav OPEN |
| Overlay of live activity onto the map | map node ↔ path prefix; `trail[].target` / `edited[root]` from `lib/codemap/slice.ts#L10-L52` → `highlight`/`badges`/`hatch` props of `GraphBody` (`Graph.tsx#L310-L323`); Here/Fit/Change become drill-downs from a map node ("How it fits" already pins Fit, `CodeMapPanel.tsx#L223-L233`) | 73 §6.5 (encodings), §12 | primitives BUILT; mapping OPEN |
| Pop-out | nothing new: `PopoutTarget {kind:'codemap'}` + route already carry any lens (`popout.ts#L21-L22`, `routes/codemap.$session.$root.tsx#L1-L10`) | 73 §0.8, 70 | BUILT |
| Phone | list form of the map (perspective → elements as rows) in the bottom Sheet (`CodeMapHost.tsx#L84-L98`) | 73 §0.9 | host BUILT; list form OPEN |
| Export | per diagram: `DiagramFrame` PNG/SVG (`DiagramFrame.tsx#L173-L189`); whole map: snapshot into a board → `doc_export` PDF/markdown (72); mermaid text via `surface::to_mermaid` (`surface/src/lib.rs#L275-L281`) for Slack/TUI | 65, 72, 30 | BUILT per diagram; "export the whole map" OPEN |
| Entry-point detection for JS/TS | `crates/codeindex/src/walk.rs#L227-L267` (`entry_points`): add `bin`/`main`/`exports` + conventions | 60 §17 item 9 | partial BUILT |
| TS NodeNext import resolution | `oxc_resolver` options in the resolver (not re-read; A17 evidence) | 60 §6.1 | gap OPEN |

## What the CodeMap panel shows today

The "minimal view" Sky dislikes, precisely (all VERIFIED in `crates/desktop/ui/src/components/codemap/`):

- **How it appears.** Nothing until an agent runs any `code_*` tool in the session; the engine's `CodeToolRan` event
  flips `codemap.roots` (`slice.ts#L24-L33`, `reduce.ts#L771-L772`) and a **map icon** appears in the chat header
  (`CodeMapButton`, `CodeMapHost.tsx#L31-L50`; slot order in `lib/header.ts#L73-L89`) with a dot when a call arrived
  that the pane has not shown. Clicking docks a **side pane** (360–920 px, default 480, persisted) to the right of the
  chat (`CodeMapPane`, `#L53-L81`); under 768 px a bottom Sheet (`#L84-L98`). It never opens by itself
  (`docs/design/73-codemap.md#L12-L12`). On history load a `code_*` call before the UI listened yields the root `"?"`,
  resolved through `codeindex_stats(null)` (`CodeMapPanel.tsx#L80-L83`).
- **Header row (32 px, monospace 11 px):** "CodeMap", a root picker when > 1 root, `N files · N symbols` from
  `codeindex_stats` (≥ 520 px), freshness chip (`fresh` / `stale · N pending` / `walk capped` / `computing…`), `N calls`,
  three lens tabs **here · fit · change** with an "agent is here" dot when pinned elsewhere, `follow`, ⊡ Snapshot to
  board, ⤢ pop-out, ✕ (`CodeMapPanel.tsx#L292-L345`). A `suspect_root` banner with a **Reconcile** button when the
  index refuses a walk that would delete > 80 % of files (`#L340-L345`, `index.rs#L26-L31`).
- **Body = one lens:**
  - *Here*: a virtualised indented tree (22 px rows) of directories (depth-2 components) → files → ranked symbols,
    with badges `→N` importers, `↑N` callers, `new`, `◆entry`, `test`, a `►` marker on the agent's last target,
    `▎` focus bar, `~` on edited files; keyboard ↑/↓/←/→/Enter (as-built UI `#L35-L35`; `Here.tsx` 169 lines).
  - *Fit*: at ≥ 600 px panel width a 3-column butterfly (callers | focus | callees, ≤ 20 nodes, dashed edges for
    confidence < 1, `×N` labels, bundle nodes `+N callers`), else two lists (`Fit.tsx` 124 lines; `fence.ts#L19-L29`
    shows the same encoding for the snapshot).
  - *Change*: four bands changed/direct/transitive/tests with hatched stale nodes and edges only for the selection,
    else four list sections (`Change.tsx` 171 lines).
  - Empty states: "run a code_* tool first"; "no symbol yet — run code_refs or code_symbol, or pick 'How it fits' on a
    row" (`CodeMapPanel.tsx#L269-L290`).
- **Below the body:** a collapsible **trail** of the session's `code_*` calls (newest first, ≤ 20; click = replay as
  pinned; expanded by default ≥ 1080 px) and a one-line **detail** `path:a-b · kind · name · #rank · →N ↑N ·
  provenance confidence` (`#L190-L191`, `#L370-L390` region).
- **Node actions** (right-click / ⋯ / Enter): *Ask about this* appends `` `path#La-Lb` `` to the composer draft with a
  `[codemap <root> · <lens> · <focus>]` header; *How it fits* pins Fit; *Snapshot to board* copies a ≤ 12-node
  `json-render` Graph fence (`#L223-L233`, `fence.ts#L1-L47`).
- **Why it reads as minimal (INFERRED):** every view is a *neighbourhood of the agent's last call*; there is no
  picture of the whole system, no names for parts ("what is crates/runtime for?"), no prose, no way to start from the
  top and descend. The panel is a very good *instrument* and not a *map*.

## Where a generated map could be stored

Trade-offs visible in the code (all INFERRED judgements over VERIFIED facts):

| option | mechanics | pros | cons |
|---|---|---|---|
| **A. Index store** (new tables in `<slug>-<hash8>.sqlite`) | beside `map_cache:*` (`store.rs#L1249-L1280`); keyed by root; read by `codemap_*` RPC in < 30 ms; invalidated by `map_state`/per-dir keys; schema change = `rm` + rebuild (`store.rs#L21-L21`, 60 §0.7) | one place, one lifecycle, one event bus, zero new crate deps; staleness computed where the facts live; `touch` already lands here; survives the "derived data" rule | LLM prose becomes *derived* data that a schema bump deletes (cost to regenerate); no human editing surface (no `doc_*`); not shareable/exportable without a lowering step; 73 §0.1 must be amended ("nothing stored") |
| **B. Board / note per root** (document store) | `doc_create(kind="board", root)`; elements = sections; diagrams = `Graph` fences with `ref`; prose = blocks; `check_excerpts` validates anchors on insert (`document/src/tools.rs#L201-L234`); export via `doc_export`; agents already know how to write it (board skill) | human-editable, shareable, exportable, tourable today; anchors verified; agent tooling exists; consistent with 73's "snapshot into a board" ruling | boards are *stored explanations* not live views — staleness is per anchor (`LocateStatus`) not per element; no engine-side regeneration hook; a Yjs document is heavy for a 60-diagram map; drill-down between fences needs a new block/link kind; the panel would read a document, not the index |
| **C. Data-dir JSON/markdown beside the index** (`<data_dir>/codemap/<slug-hash8>/…`) | plain files; the omm `.omm/` shape; read by the engine, served by RPC | trivially inspectable/diffable; cheap to version (copy on regenerate → diff between versions); independent of the SQLite schema | second store to keep in sync with the index; no queries; no WAL/transactions; another thing `rm -rf` must know about |
| **D. In the repo** (`.theuth/map/`, omm's `.omm/`) | files committed with the code | shareable with the team; version-controlled with the code | **refused** by 60 §12 ("pollutes the user's tree") and 60 §0.7 ("nothing is written inside the user's repository") — `docs/design/60-codeindex.md#L21-L21`, `#L313-L313`; would also need gitignore handling in the walk |
| **E. Artifacts** (`<data_dir>/artifacts/<id>`) | immutable per tool call (`artifact/src/lib.rs#L161-L165`) | nothing | wrong lifecycle: 73 §0.1 already rejected "an artifact per map (immutable outputs of a tool call; this is a window)" |

**Recommendation-grade reading (INFERRED):** A for the skeleton (diagrams, element ids, staleness keys — derived,
rebuildable, fast), with prose stored in A as well but *stamped* (`generated_at`, `facts_hash`, model) so a schema bump
can keep it when the facts hash still matches; B as the **export/snapshot** target (whole map → board) rather than the
primary store; C only if Sky wants `.omm`-style inspectability; D refused by standing rule unless Sky overturns it.
