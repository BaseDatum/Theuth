# 73 — CodeMap: a live panel over the code index (three lenses, one root, no hairball)

Status: design, 2026-09-28, branch `feature/73-codemap-design` (base `22b39455`). Extends 60 (codeindex: tools, envelope, store), 65 (`Graph` renderer, `GraphSvg`, ELK worker, render queue), 71 (cognitive-load rules, anchors `path#L10-L24`, change colours), 66 (composer intake), 72 (export), 70-as-built SURFACES / POPOUT / POPOUT-2 / PTY-POPOUT / MOBILE-FIXES. Amends 60 §10 (the "no events in v1" clause) and 65 §0.6 (a graph node bound to code, outside the catalog). Designed to consume the fields four sibling workers are adding on `feature/codeindex-{render,rank,search,impact}` (§2.C); where a field does not exist yet §5.4 says what the panel needs.

Sky's requirement, verbatim: *"a CodeMap, which is a visual representation of the code that's being worked on, and if the agent uses any of the codeindex tools it pops up as available, just like the browser and PTY, opens in its own panel, but can be popped out, is updated live, it doesn't compromise on performance either, it must be lightning fast. … signal is more important than noise. The goal is to reduce cognitive load of the user, to make it easier for them to understand what's in the codebase, how it works together and what is being changed."*

Out of scope, stated once: no general graph explorer (no "show me the whole repo"), no editing from the map, no LLM in the render path, no new `DocKind`/artifact `Kind`, no force-directed layout anywhere, no type-aware edges (60 §12), no watcher (reconcile stays on-call plus the targeted `touch` of §5.2).

## 0. Decisions in one screen

1. **A CodeMap is a live view, not a document.** It is UI state over one root's index for one session — nothing is stored, nothing is authored. Boards (71) remain the stored explanation; a lens can be *snapshotted* into a board as one `json-render` `Graph` fence with a `ref` on every node (§6.8), and exported through `DiagramFrame`'s Copy PNG / Save SVG (65 as-built step 1) or, once in a board, through `doc_export` (72 §2). Rejected: `DocKind::CodeMap` (a stored copy of a derived index is stale on write) and an artifact per map (35's artifacts are immutable outputs of a tool call; this is a window).
2. **Availability like the browser and PTY: one event, one boolean per session.** The engine publishes `CodeToolRan { session, root, … }` from every `code_*` tool call (§5.1); the reducer sets `SessionSlice.codemap.roots ∪= root` exactly where `BrowserShowLive` sets `browser` and `JobStarted.pty` sets `terminal` (`store/reduce.ts:588-589`, `:556-563`); the header shows a **CodeMap** button through `headerOrder` (`lib/header.ts:65`) — never auto-opens (the browser auto-opens only on the explicit `browser_show`; a map opening itself on every `code_find` is noise). Keyed by root: the panel lists the roots touched this session, ≤ 8 (the engine's `POOL_LRU`, `index.rs:23`).
3. **Three lenses, one panel, agent-driven with a user pin** (§6.1–6.4): **Here** (ranked map as an indented, virtualised tree with badges; ≤ 2 000 rows; directories collapsed by share of rank), **Fit** (focus + context: ≤ 15 nodes, callers left / callees right, provenance as edge style, ambiguity as a number), **Change** (impact as four bands changed → direct → transitive → tests; edges only on hover). The lens shown follows the agent's last `code_*` call unless the user pinned one (§6.1 state machine). Rejected: tabs the user must drive (the agent already knows what it just looked at) and a single "smart" canvas that mixes the three (the hairball).
4. **Rendering stack: what the UI already has, plus ~100 lines of layout code and zero dependencies.** `GraphSvg` (`components/chat/surface/Graph.tsx:84`) takes a `Layout` (`lib/graph-layout.ts:12-38`) — we build that `Layout` by hand: fixed columns for Fit, fixed bands for Change (§6.6). Positions are a pure function of `(band, sort key)`, so a delta moves only what changed; nodes are keyed by `moniker` and glide with a 200 ms CSS transform transition. Here is a `@tanstack/react-virtual` list, no SVG. ELK is not used: it is whole-graph, hash-keyed and non-incremental (`graph-layout.ts:64-78`, `:259`), and ≤ 15 nodes in three columns need no crossing minimisation. Rejected: d3-hierarchy (a tree list needs none), react-flow/xyflow (a second renderer beside `GraphSvg`), a treemap (rank is an order, not an area), ELK `INTERACTIVE` seeding (available at zero bytes if a lens ever exceeds bands; §10).
5. **Two events, four RPC methods, no rows on the event channel** (§5). `CodeToolRan` (args + a compact summary) and `CodeReconciled` (which paths were re-parsed) ride `subscribe_events`; the rows come from `codemap_here / codemap_fit / codemap_change / codemap_touch`, which **read the store and never walk the tree** — freshness is whatever the agent's tool call (or `touch`) just produced. The engine coalesces `CodeReconciled` to ≥ 250 ms per root (the `ToolCallProgress` cadence, `agent/src/lib.rs:1605-1614`); the UI already rAF-batches events (`App.tsx:124-143`) and refetches through `createRenderQueue` (`lib/render-queue.ts:18`) at 200 ms. Rejected: streaming rows in events (the bridge channel is `CAPACITY = 1024` `Arc<Value>`s shared by every subscriber, `bridge.rs:42`) and polling `codeindex_stats` every 500 ms as `BrowserLive` polls screenshots (`BrowserLive.tsx:8-14`).
6. **Performance contract in numbers** (§7): lens RPC warm < 30 ms (store lookups only, render cache hit for Here), first paint of a 2 000-row Here < 100 ms after data (virtualised: ~40 DOM rows), a delta applied < 16 ms (path-keyed patch, no relayout), Change (WORKTREE) < 300 ms on this repo and shown as "computing" over the previous rings, transitions 200 ms, memory ≤ 1 MiB per root slice. Nothing runs in a Web Worker in v1: the only heavy computation (impact diff, rank) is engine-side; a 2 000-row group-by is O(n) and measured in single-digit ms. If Fit ever outgrows bands, the ELK worker already ships.
7. **Live edits without a watcher.** The UI sees `ToolCallStarted { name: "edit" | "write_file", args.path }` (`edit.rs:596`, `fs.rs:444`) for its session; while a CodeMap panel for that root is mounted it calls `codemap_touch(root, paths)` (coalesced 500 ms), which stats and re-parses only the named files — no walk (the walk alone is 73–89 ms in a debug build, 60 §22.1) — and publishes `CodeReconciled`. Only a mounted panel pays; two windows both touching is idempotent (mtime check). Rejected: codeindex subscribing to `AgentEvent` (a `codeindex → agent` dependency for one field) and `notify` (60 §0.8).
8. **Pop-out reuses `lib/popout.ts` unchanged in mechanism**: `PopoutTarget` gains `{kind:'codemap', session, root}`, a file route `routes/codemap.$session.$root.tsx` renders `PoppedCodeMap` under the `Popped` root (`routes/__root.tsx:16-28`) exactly as `routes/pty.$session.$job.tsx:5-10` does for terminals; no host (Rust) change, `popout-*` already covers the label (`capabilities/default.json:5`). Each window subscribes to the engine itself (70-POPOUT §Concurrency); a CodeMap is read-only, so there is no two-viewer conflict to arbitrate. Hidden on iOS by `canPopout()` (`popout.ts:86`).
9. **Mobile is lists.** Below 768 px (`hooks/use-mobile.ts:3`) the panel is a full-height bottom `Sheet` (the `TerminalViewer` pattern, `TerminalViewer.tsx:325-348`): Here as the same virtualised list, Fit as two lists (callers / callees) around a header row, Change as four sections — no SVG, no hover, tap = pin (§6.9).
10. **Interaction feeds the composer, never the file system.** A node offers **Ask about this** → `path#L<a>-L<b>` appended to the session's draft (`prefillDraft`, `Composer.tsx:76-78`; the anchor grammar of 71 §2.3 / `surface/anchor.ts:5-7`), **How it fits** → pins Fit on that node, **Snapshot to board** → a `Graph` fence on the clipboard and an optional prefilled ask (§6.8). No `AttachRef` kind is added: `lib/attach.ts:15` has five kinds and 66 §3.1 refuses locators; the text anchor is what the board skill already teaches the model (`document/src/tools.rs:52`).
11. **Honesty chips, same words as the index.** `stale · N pending`, `walk capped`, and a `suspect_root` banner with the engine's `hint` verbatim (impact sibling; §2.C) — lenses freeze under `suspect_root` until **Reconcile** (`codemap_touch(root, null)`). Ambiguous references are a count, never an edge; confidence is an edge style, never a number at rest.
12. **Two workers, one contract.** §5 is written so an engine worker (events, store queries, RPC, Rust) and a UI worker (panel, lenses, pop-out, mobile) build in parallel from the same JSON; §11 splits the commits E1–E2 / U1–U3 with a runnable check each.

## 1. Why

- The agent already answers structural questions through six `code_*` tools (60 §8); the user sees a ≤ 2 000-char text blob per call (`ToolCallFinished.output`, `loop_.rs:91` `DISPLAY_OUTPUT_CHARS`). The structured rows — ranks, callers, provenance, impact rings — never reach the UI (`ToolOutput.structured` is not on any event, §2.A). The information exists; the view does not.
- Boards (71) explain a codebase *after* the agent has understood it; they are authored, stored, and cost the model a turn per block. A user watching an agent work needs the cheaper thing: what the agent is looking at *now*, what it just changed, and what that touches — updated as the tools run, gone when the session ends.
- Empryo's public code (recon 2026-09-28, `~/.theuth/artifacts/dbc0092fb984e94e.md` §1) ships `(→N)` importer badges, `[NEW]`, caller badges and a `<soul_map_update>` delta; its desktop "live code genome" is private and BSL-licensed — ideas only, no code. Four sibling workers are adding the data side of those ideas to codeindex now (§2.C). This doc is the view side, sized so it consumes exactly those fields.
- Every code-graph visualiser in the wild fails the same way: it draws everything. 71's ruling — reduce cognitive load, signal over noise — is the constraint here: three small pictures the reader can hold, each answering one question, each with a hard node cap and a rule for what is never drawn (§6.5).

## 2. Facts relied on (recon 2026-09-28, this tree @ `22b39455`)

### A. Engine: codeindex and rpc

- Tools: six `pub struct Code…Tool(pub Arc<Codeindex>)` (`codeindex/src/tools.rs:157, 232, 391, 602, 729, 1080`); `register` `:43`; `Envelope { root, indexed_files, stale, pending, dropped, truncated_walk }` `:77-87`; `DEFAULT_BUDGET = 4000` `:38`, `FETCH_LIMIT = 1000` `:40`, `TRANSITIVE_DEPTH = 3` `:1083`. Every call goes through `open(ix, root_arg)` → `root::discover` → `open_root` → `reconcile(ri, false)`. `code_map` without focus is `store::ranked_tags` (`store.rs:960`, a lookup on stored ranks); with focus it is `Graph::load` + `rank` (whole graph, `rank.rs:131`, `:211`).
- Store: `SCHEMA_VERSION = "3"`, DDL `store.rs:25-96` (`files`, `symbols`, `symbols_fts`, `imports`, `refs(qualifier, to_symbol, candidates, provenance, confidence)`, `cochanges`, `meta`); indexes `symbols_file`, `symbols_name`, `imports_file`, `imports_target`, `refs_file`, `refs_to`, `refs_name`. Queries the lenses need already exist: `definitions_named` `:788`, `definition_by_id` `:800`, `definition_at` `:810`, `refs_to` `:858`, `refs_from` `:877`, `ranked_tags` `:960`, `cochange_partners` `:1042`, `referencers` `:1059`, `import_edges` `:1117`, `counts` `:211`. No hand-written statement cache; sqlx's pool cache only.
- Freshness is in-memory per process: `RootIndex { root, pool, write: Mutex<()>, background: AtomicBool, pending: AtomicUsize }` `index.rs:40-50`; `Reconciled` `:54`; `reconcile` `:116` (walk on `spawn_blocking` → `(mtime_ns, size)` diff `:226` → ≤ `sync_limit` (64) inline else `stale: true` + background task); `POOL_LRU = 8` `:23`; `Codeindex::new(cfg, data_dir)` `:73` holds no `Runtime`. **No bus event**: `lib.rs:13` "no events in v1 (60 §10)"; the background task's completion is `tracing::info!` only.
- Measured (60 §22.1, debug build, load ≈ 28): second call with nothing changed **73–89 ms** (walk + diff); `code_impact(files=[…])` **94 ms**; rank persist 173 ms cold / 182 ms warm; graph load 101 ms; personalised rank 43 ms; co-change walk 422–429 ms once per HEAD move. WORKTREE impact hashes every HEAD blob against disk (~1 000 reads on this repo, 60 §22.3 dev. 1).
- Actor: `CODEINDEX_ACTOR = "codeindex"`; messages `Handle`, `Stats { root }`, `Reconcile { root }`, `Locate { root, anchors }` (`lib.rs:130-207`); `IndexStats` `lib.rs:80` (`ts_rs` under the `ts` feature). Plugin start `lib.rs:210-236`: config → `Codeindex::new` → `CodeindexActor::spawn` → `outline_hook::register` → `tools::register` via `lookup_when::<ToolRegistry>`.
- RPC: flat snake_case `#[method(name = "…")]` on `trait Api` (`rpc/src/api.rs:143-147`); `codeindex_stats(root: Option<String>) -> IndexStats` `:887-888` / impl `:3874`; `code_locate(root, anchors) -> Vec<Located>` `:894-899` / `:3884`; `fn codeindex() -> RpcResult<ActorRef<CodeindexActor>>` `:1953` (maps `NOT_RUNNING` → "codeindex is disabled"); `ts_rs` exports `export.rs:116-118`; UI mirror `rpc/api.ts:230-232`, `METHODS` `:744-745` (checked against `bindings/methods.json`).
- Events: bridge `rpc/src/bridge.rs` — `forward!(…)` list `:90-119` and `rt.subscribe::<Bridge, T>` list `:137-…`; every wire event is `#[serde(tag = "type")]`; `subscribe_events(session?, descendants?)` filters by the JSON `session` field in `wanted` (`api.rs:5088`): **an event without `session` reaches every subscriber**; `admitted` `:5059`. `AgentEvent::ToolCallStarted { session, turn, call_id, name, args: Value }` / `ToolCallFinished { session, turn, call_id, output, is_error, files }` (`agent/src/lib.rs:1616-1643`), published `loop_.rs:3799`, `:3892`; `files` are files the tool *produced* (screenshots), not edited paths (`lib.rs:1640`). Plugins publish with `rt.publish(event)` (`runtime/src/lib.rs:48`; browser example `browser/src/tools.rs:683` `publish(BrowserEvent::ShowLive { session: cx.session })`).
- Edit tools the UI can recognise: `edit` (`tools/src/edit.rs:596`, `EditArgs.path` `:514`), `write_file` (`tools/src/fs.rs:444`, `WriteFileArgs.path` `:433`).
- `SessionSummary.cwd: Option<String>` exists (`agent/src/lib.rs:1313`) but the UI never reads it; codeindex resolves `root` per call (`root::discover`, `root.rs:11`). The panel therefore takes `root` from the event, never from the session.

### B. UI: panels, pop-out, store, graph renderer, composer

- Panels are per-session booleans on `SessionSlice` (`store/sessions.ts:205-228`: `browser`, `browserLive`, `terminal`, `ptys`, `jobs`, `artifactPanel`), defaults `reduce.ts:38-42`, flipped in `apply` (`BrowserShowLive` `:588-589`; `JobStarted.pty` `:556-563`; `browser_*` name prefix on `ToolCallPending/Started` `:751`, `:788`; history load `sessions.ts:344-348`). `route()` drops events without `session` (`reduce.ts:1089-1092`); those are observable through `on(type, cb)` (`sessions.ts:578`). Events enter once: `App.tsx:124` `attach` → rAF `drain` → `dispatch` (`sessions.ts:593`).
- Header: `HeaderId` union `lib/header.ts:28`; `headerOrder(panel, popout)` `:65` with `shed: true` items for `terminals`/`browser`/`artifacts` (`:86-88`); panel priority `Header.tsx:194` `artifacts?.size ? 'artifacts' : browser ? 'browser' : terminals ? 'terminals' : null`; panels mounted at the header's tail `Header.tsx:310-316`. Tiers are pane-width measured (`header.ts:6-25`).
- Panel hosts: `BrowserLive` centred `Dialog` (`BrowserLive.tsx:17`); `TerminalViewer` `Dialog h-[85vh]` / mobile bottom `Sheet` (`TerminalViewer.tsx:316-348`); `ArtifactPanel` the one resizable right `Sheet` — `MIN_W = 360`, `DEFAULT_W = 640` (`ArtifactPanel.tsx:37-38`), 6 px drag strip clamped to `innerWidth/2` (`:238-261`, `// ponytail: … react-resizable-panels if a second panel ever needs it`). Width not persisted; boards persist `board-pane-width` 360–920 (`BoardReader.tsx:35-38`).
- Pop-out: `PopoutTarget` union `lib/popout.ts:10-20`; `popoutLabel` `:47` (`popout-<kind>-<safe id>`, `safe` = `[^A-Za-z0-9_-]` → `_`); `popoutUrl` `:49`; `popoutTarget(href)` `:52-66`; `POPOUT` read once `:69`; `canPopout = container() !== 'ios'` `:86`; `openPopout` `:89` (`invoke('open_window')` | `window.open`). Host `desktop/src/commands.rs:15` `POPOUT_PREFIX`, `open_window` `:21` (focus-if-open, 960×720, `on_document_title_changed`). Root route `routes/__root.tsx:16` → `Popped` `:20-28`. Precedent for a new kind: `routes/pty.$session.$job.tsx:5`, `PoppedTerminal`, `popoutSession` for `SessionDeleted` close (`App.tsx:299-302`).
- Store: zustand vanilla `createStore` + `subscribeWithSelector` (`sessions.ts:1-2`), per-engine `Stores` bag in `store/engine-scope.tsx:24-58`, `scoped()` hook factory `:95`; nothing persisted in `store/`.
- Mobile: `MOBILE_BREAKPOINT = 768` (`hooks/use-mobile.ts:3`); `usePointerCoarse` (`lib/mobile.ts:11-17`); board CSS container queries at 1080 / 720 (`board.css:261, 514`); `canPopout()` deliberately ignores width (70-MOBILE-FIXES).
- Graph renderer: `GraphSvg({ graph, layout, tokens, markerId, highlight, onSelect })` `Graph.tsx:84`; `paint(state, tokens)` `:30` (`active` accent 2 px + 10 % fill; `done` success; `danger` destructive; `muted` 50 % opacity); `effectiveState` `:23`; nodes `<g data-node role="button" tabIndex=0 aria-description={ref}>` `:132-139`; `adjacencyText` `:170`; `GraphView` `:205` (the card: hash → `layout()` → queue). `Layout { nodes: {id,x,y,w,h}[], edges: {index, sections: {start,bends,end}[], label?}[], groups, width, height }` `graph-layout.ts:12-38`; `structuralHash` `:64`; `LAYOUT_TIMEOUT_MS = 2000` `:57`; the one `new Worker` in the UI `:239`; `WRAP = 24`, `FONT_PX = 13`, `LINE_PX = 17`, `MIN = {w: 64, h: 32}` `:54-58`. Caps enforced as parse errors for fences: `MAX_GRAPH = { nodes: 120, edges: 240, groups: 12, label: 80 }` (`parse.ts:16`) — a directly mounted `GraphSvg` bypasses `parse.ts` (boards do this, `board/blocks.ts:83` `classifyCode`). Tokens `lib/diagram-theme.ts:5-35`. `DiagramFrame` `:16-17` (pan/zoom, Copy PNG · Save PNG… · Save SVG… · Fullscreen); `standalone()` / `rasterize()` `lib/diagram-export.ts:19, 33`. Measured (65 as-built step 3): ELK 6 nodes median 12.7 ms in the worker; cache hit 0–0.6 ms; React re-render to next frame 15.3 ms.
- Sequence renderer is hand-laid SVG with fixed constants (`Sequence.tsx:17-25`) — precedent for a hand `Layout`.
- Composer: `prefillDraft(session, text, refs = [], panel = false)` `Composer.tsx:76-78` writes `useDrafts` (`lib/drafts.ts:21` `Draft = { text, pastes, unsent? }`, `:73`); `attachTo(session, ref)` → `theuth:attach` (`lib/attach.ts:201`); `AttachRef` kinds `note | knowledge | artifact | session | meeting` (`attach.ts:15-19`), no locator (66 §3.1 `:227-232`). Boards prefill with a header line `[board <doc> · block <id> · path:a-b …]` (`lib/board-ask.ts:3, 11, 84`, `QUOTE_MAX = 4096` `:18`). Anchor grammar `surface/anchor.ts:5-10`, `parseAnchor` `:41`.
- Boards are agent-created only: client `doc_create(title, markdown)` has no `kind`/`root` (`rpc/api.ts:273`, `api.rs:819-825`); `doc_create(kind="board", root)` is the tool (`document/src/tools.rs:792-866`); the reader mounts a `json-render` fence whose root is `Graph` as a diagram (`blocks.ts:83-95`). Export: `doc_export(doc, format, dest, assets)` (72 §2), assets = client-rendered SVGs of mounted Graph/Sequence fences (72 §5).
- 71's rules this panel inherits: ≤ 12 nodes per picture (§4.1), one picture per idea (M1), chrome register mono 11 px (§2.7), change colours added `success` / removed `destructive` / modified `warning` "strokes and names only" (§2.7), `ref` on every unit (§3.2), Map recipe `changed → danger, direct → active, transitive → muted` (§4.3), motion 240 ms / none under `prefers-reduced-motion` (M7).

### C. Sibling work in flight (`feature/codeindex-{render,rank,search,impact}`, all at base `22b39455` on 2026-09-28; only `impact` has uncommitted edits)

Observed in `~/Develop/theuth-ci-impact` (`index.rs`, `tools.rs`, uncommitted): `SUSPECT_GONE_PERCENT = 80`, `SUSPECT_ROOT_HINT: &str = "most indexed files are missing from the walk; kept the old index — check the root is fully checked out and mounted, or force a reconcile"`, `Reconciled.suspect_root: bool`, `hint: Option<&str>`, and `"suspect_root"`/`"hint"` keys in every tool's JSON. The rest is from the recon brief (`~/.theuth/artifacts/dbc0092fb984e94e.md` §3) and is **expected, not observed**:

| Branch | Field / behaviour the panel consumes | Used by |
|---|---|---|
| render | row fields `importers` (distinct importer files), `recent` (mtime < 48 h), `callers` (distinct caller files) | Here badges `→N`, `NEW`, `↑N` |
| render | rendered-map cache keyed `(schema, file count, max mtime, HEAD, budget, focus)` | `codemap_here` cache hit (§7) |
| render | `code_map(mode="changed")` delta; directory-coverage fallback rows | Here `dirs` coverage rows; Change "since" chip |
| rank | barrel/type damping; `edited=[paths]` personalisation; `entry: true` rows | Here `entry` chip; `codemap_here(edited)` |
| search | trigram infix `code_find` with `match:`; stable `moniker` `<lang>:<path>#<Qualified.Name>(<kind>)` on every row | node identity across reindex (transitions); `CodeToolRan.summary.definition.moniker` |
| impact | `tests_by_subject`; `project_profile(root)`; `suspect_root` + `hint` | Change tests band (`ref` vs `subject` marker); banner |

## 3. Shape

```
crates/codeindex/src/
  event.rs        CodeindexEvent { CodeToolRan, CodeReconciled }  (#[serde(tag="type")], ts_rs)
  lens.rs         LensMeta, MapRow, HereLens, FitLens, ChangeLens + the four lens functions (store reads only)
  index.rs        Codeindex gains `rt: Option<Runtime>` and `seq: AtomicU64` per RootIndex; `touch(root, paths)`;
                  publish CodeReconciled after every write (inline or background), coalesced ≥ 250 ms per root
  tools.rs        every Tool::call publishes CodeToolRan { session: cx.session, … } after building its envelope
  lib.rs          actor messages Here / Fit / Change / Touch → lens.rs
crates/rpc/src/
  api.rs          codemap_here / codemap_fit / codemap_change / codemap_touch (flat names, `send(&codeindex()?, …)`)
  bridge.rs       CodeindexEvent in forward! and rt.subscribe
  export.rs       the lens types + CodeindexEvent
crates/desktop/ui/src/
  lib/codemap.ts                        pure: lens state machine, badge/label rules, layouts (columns, bands), toGraphFence
  store/sessions.ts, store/reduce.ts    SessionSlice.codemap { roots, last, edited }
  lib/header.ts, components/chat/Header.tsx   HeaderId 'codemap', button, panel mount
  components/chat/codemap/{CodeMapPanel,Here,Fit,Change,Detail}.tsx   the panel (Sheet), the three lenses, the detail line
  routes/codemap.$session.$root.tsx     PoppedCodeMap
  lib/popout.ts                         {kind:'codemap', session, root}
  rpc/api.ts, rpc/events.ts             method mirrors, WireEvent |= CodeindexEvent
```

Dependency direction unchanged: `runtime ← tools ← codeindex ← rpc`; codeindex gains a `Runtime` handle (it already receives `rt` in `start`, `lib.rs:210`) and no new crate dependency. UI: zero new packages.

Data flow for one `code_refs`:

```
agent: code_refs(symbol="RegisterTool")
  → tools.rs: open → reconcile → query → envelope
  → rt.publish(CodeToolRan { session, root, tool:"code_refs", args, summary{definition{moniker,…}, rows:12, stale:false,…}, seq })
  → bridge → subscribe_events → App.attach → dispatch → reduce: codemap.roots ∪= root; codemap.last = …
  → CodeMapPanel (if mounted, mode=auto): lens := fit, focus := summary.definition.moniker
  → api.codemap_fit(root, moniker, 15) → FitLens (≤ 30 ms) → columnsLayout() → GraphSvg (200 ms glide)
agent: edit(path="crates/tools/src/registry.rs", …)
  → ToolCallStarted → reduce: codemap.edited[root] ∪= path
  → panel (mounted): codemap_touch(root, [path]) after 500 ms → engine stat+parse 1 file → CodeReconciled{reparsed:[path]}
  → panel: reparsed ∩ visible rows ≠ ∅ → refetch current lens through the 200 ms queue → patch by moniker/path
```

## 4. Data

Nothing new on disk. The engine keeps, per process:

- `RootIndex.seq: AtomicU64` — monotonic per root, stamped on every `CodeToolRan`/`CodeReconciled` and every lens reply so a window can drop out-of-order replies (a popout and main may race).
- `RootIndex.last_reconciled_at: AtomicU64` (ms) — the ≥ 250 ms coalescing gate for `CodeReconciled`; a write inside the gate defers one trailing event (same shape as `createRenderQueue`).
- The render sibling's map cache is the only cache the lenses read; `codemap_here` reports `cache_hit` so the as-built can prove the warm path.

The UI keeps, per engine (`Stores`), on `SessionSlice`:

```ts
codemap: {
  roots: string[]                                  // touched this session, insertion order, ≤ 8 (oldest dropped)
  last: { root: string; tool: string; callId: string; args: unknown; summary: CodeToolSummary; seq: number } | null
  edited: Record<string /*root*/, string[]>        // root-relative paths from edit/write_file this session (≤ 200, oldest dropped)
}
```
Defaults `{ roots: [], last: null, edited: {} }` in `reduce.ts` `empty()`. On history load (`sessions.ts:344`) a `tool` entry with `name.startsWith("code_")` sets `roots = ["?"]` — root unknown until the next `CodeToolRan`; the panel then resolves `"?"` through `codeindex_stats(null)` (the engine's default root) and labels it *default root* (§12).

Per window, not persisted, in the panel component (or one zustand store per mounted panel, like `board/anchors.ts`):

```ts
{ root, mode: 'auto' | 'pinned', lens: 'here' | 'fit' | 'change', focus: string | null /* moniker */,
  params: { here: { focus?: string[]; edited?: string[] }, fit: { symbol: string }, change: { git_range?: string; files?: string[]; include_tests: boolean } },
  expanded: Set<string /*dir*/>, pinnedNodes: Set<string /*moniker*/>, hover: string | null, seq: number }
```
Persisted per device: `codemap-panel-width` (localStorage, 360–920, default 640 — `ArtifactPanel`'s numbers, boards' range).

## 5. Events and RPC — the contract (copy verbatim, both workers)

### 5.1 Events (`crates/codeindex/src/event.rs`, `#[serde(tag = "type")]`, `ts_rs` export `CodeindexEvent.ts`)

```json
{ "type": "CodeToolRan",
  "session": "<SessionId>",                   // ToolCx.session — scoped by `wanted`
  "root": "/Users/…/theuth",                  // canonical, as Envelope.root
  "call_id": "toolu_01…",                     // ToolCx.call_id
  "tool": "code_refs",                        // code_map | code_find | code_outline | code_refs | code_symbol | code_impact
  "args": { "symbol": "RegisterTool", "direction": "in" },     // the tool's args as received, minus `budget` and `root`
  "summary": {
    "rows": 12, "dropped": 0, "is_error": false,
    "stale": false, "pending": 0, "truncated_walk": false, "suspect_root": false, "hint": null,
    "focus": ["crates/surface"],                                          // code_map only: focus_files ∪ focus_idents as given
    "path": "crates/tools/src/fs.rs",                                     // code_outline only
    "definition": { "moniker": "rust:crates/tools/src/registry.rs#ToolRegistry::start(method)",
                    "path": "crates/tools/src/registry.rs", "line_start": 1366, "line_end": 1439, "name": "start", "kind": "method" },
                                                                          // code_refs / code_symbol when exactly one definition matched; null when `candidates`
    "impact": { "mode": "range", "range": { "old": "9532e7ff", "new": "176bf52" }, "include_tests": true,
                "changed": 13, "direct": 12, "transitive": 4, "tests": 8, "ambiguous_skipped": 464, "unresolved_imports": 2 }
                                                                          // code_impact only
  },
  "seq": 41 }

{ "type": "CodeReconciled",
  "root": "/Users/…/theuth",                  // no `session`: global; the UI filters by root
  "reparsed": ["crates/tools/src/registry.rs"],
  "removed": [],
  "stale": false, "pending": 0, "suspect_root": false, "hint": null,
  "git_head": "22b39455d849…",
  "seq": 42 }
```
Rules: `CodeToolRan` once per `Tool::call`, after the envelope exists (errors too, `is_error: true`, `summary.rows: 0`); `CodeReconciled` after every `write` (inline, background completion, `touch`) with `reparsed`/`removed` capped at 200 paths each plus `"more": n`; ≥ 250 ms apart per root (trailing event kept). Both join `forward!` and `rt.subscribe` in `bridge.rs`, and `WireEvent` in `rpc/events.ts`.

### 5.2 RPC methods (`crates/rpc/src/api.rs`, flat names, positional params; UI `rpc/api.ts` + `METHODS`)

```
codemap_here  (root: String, focus: Option<Vec<String>>, edited: Option<Vec<String>>, limit: Option<u32>)          -> HereLens
codemap_fit   (root: String, symbol: String, limit: Option<u32>)                                                    -> FitLens
codemap_change(root: String, git_range: Option<String>, files: Option<Vec<String>>, include_tests: Option<bool>)    -> ChangeLens
codemap_touch (root: String, paths: Option<Vec<String>>)                                                            -> LensMeta
```
Semantics: all four `send(&codeindex()?, …)` like `codeindex_stats` (`api.rs:3874`). `root` must already have a database under `<data_dir>/codeindex/` (the agent indexed it) — otherwise `ASK_FAILED` "not indexed; run a code_* tool first" — so a panel can never trigger a first index. `codemap_here/fit/change` **never reconcile**; `codemap_touch(root, Some(paths))` stats and re-parses only those root-relative paths (rejecting any that escape the root, 60 §11) and returns the new meta; `codemap_touch(root, None)` is a forced full reconcile (`Reconcile { root }`, `lib.rs:148`) — the **Reconcile** button. `limit` defaults: here 2 000 rows, fit 15 nodes (focus + up to 7 callers + 7 callees, best confidence then count first), change 60 files across bands. `codemap_change` with neither `git_range` nor `files` means `WORKTREE`.

### 5.3 Types (`crates/codeindex/src/lens.rs`, `ts_rs`; every reply carries `meta`)

```rust
pub struct LensMeta { pub root: PathBuf, pub seq: u64, pub indexed_files: i64, pub stale: bool, pub pending: usize,
                      pub suspect_root: bool, pub hint: Option<String>, pub truncated_walk: bool,
                      pub git_head: Option<String>, pub ms: u32 /* engine time for this reply */ }

pub struct MapRow  { pub moniker: String, pub path: String, pub line: i64, pub line_end: i64, pub kind: String, pub name: String,
                     pub signature: String /* ≤ 200 */, pub rank: f64, pub exported: bool,
                     pub importers: u32, pub callers: u32, pub recent: bool, pub entry: bool, pub is_test: bool }
pub struct DirRow  { pub path: String, pub files: u32, pub ranked_rows: u32, pub importers: u32 }   // coverage fallback: dirs with no row in `rows`
pub struct HereLens { pub meta: LensMeta, pub rows: Vec<MapRow> /* file order by best rank, symbols by line */, pub dropped: u32,
                      pub dirs: Vec<DirRow>, pub personalised: bool, pub cache_hit: bool }

pub struct FitNode { pub moniker: String, pub path: String, pub line_start: i64, pub line_end: i64, pub kind: String, pub name: String,
                     pub signature: String, pub is_test: bool }
pub struct FitEdge { pub from: String, pub to: String /* monikers */, pub count: u32, pub kind: String /* call|type|… */,
                     pub provenance: String /* same_file|import|package|unique */, pub confidence: f64 }
pub struct FitLens { pub meta: LensMeta, pub focus: FitNode, pub callers: Vec<FitNode>, pub callees: Vec<FitNode>, pub edges: Vec<FitEdge>,
                     pub ambiguous: u32 /* refs named like focus with to_symbol NULL */, pub more_callers: u32, pub more_callees: u32,
                     pub candidates: Vec<FitNode> /* non-empty when `symbol` matched several; focus = the best-ranked */ }

pub struct ChangeSym  { pub moniker: String, pub name: String, pub kind: String, pub line: i64 }
pub struct ChangeFile { pub path: String, pub status: Option<String> /* added|modified|deleted; changed band only */,
                        pub symbols: Vec<ChangeSym> /* ≤ 12 */, pub via: Vec<String> /* direct band: names or "<import>" */,
                        pub is_test: bool, pub test_link: Option<String> /* "ref" | "subject" (impact sibling) */, pub depth: u8 }
pub struct ChangeLens { pub meta: LensMeta, pub mode: String /* worktree|range|files */, pub range: Option<(String, String)>,
                        pub changed: Vec<ChangeFile>, pub direct: Vec<ChangeFile>, pub transitive: Vec<ChangeFile>, pub tests: Vec<ChangeFile>,
                        pub more: [u32; 4] /* dropped per band */, pub ambiguous_skipped: u32, pub unresolved_imports: u32,
                        pub unindexed: Vec<String>, pub cache_hit: bool }
```
Monikers: the search sibling's `<lang>:<path>#<Qualified.Name>(<kind>)`; until it lands, `lens.rs` synthesises `<language>:<path>#<name>(<kind>)` from `files.language`, `symbols.name`, `symbols.kind` (same grammar, unqualified) so the UI contract does not move.

### 5.4 Contract with codeindex — what the panel needs that does not exist today

| Need | Where it comes from | If absent when U-steps land |
|---|---|---|
| `importers`, `callers`, `recent` per row | render sibling row fields; `lens.rs` computes them with three indexed `GROUP BY`s (`imports_target`, `refs_to`, `files.mtime_ns`) if the sibling stores nothing | badges hidden, not zero |
| `entry: bool` | rank sibling | chip hidden |
| `moniker` | search sibling | synthesised (§5.3) |
| `suspect_root`, `hint` | impact sibling (observed in its worktree) | `false`/`null` |
| `test_link: ref\|subject` | impact `tests_by_subject` | `null`; tests band unmarked |
| map cache hit for `codemap_here` | render sibling's cache keyed on `(…, budget, focus)`; `lens.rs` adds `edited` to the key | `cache_hit: false`, `ranked_tags LIMIT n` path (§7) |
| `CodeindexEvent`, `seq`, `touch`, lens queries | **this doc**, E1–E2 | — |

## 6. UI

### 6.1 Lens state machine

```
                 CodeToolRan code_map|code_find|code_outline          CodeToolRan code_refs|code_symbol (definition ≠ null)
   ┌───────────────────────────────────────────┐        ┌────────────────────────────────────────────────┐
   │                                           ▼        ▼                                                │
   │   auto(here) ◀────────────────────── auto(fit) ◀──┘   ◀── auto(change) ◀── CodeToolRan code_impact  │
   │      │  user clicks a lens tab / a node action ("How it fits", "What changes")                       │
   │      ▼                                                                                               │
   │   pinned(lens) ── CodeToolRan: lens unchanged; the auto lens's tab gets a dot ── "Follow agent" ──▶ auto(last)
   │      │
   │      └── ToolCallStarted edit|write_file (path under root): edited ∪= path; touch (500 ms);
   │          if lens = change ∧ mode = worktree → refetch after CodeReconciled; else badge `~` only
   └── CodeReconciled(root): reparsed ∩ rows(visible lens) ≠ ∅ → refetch through the 200 ms queue; else no-op
```
`auto` is the default; `pinned` is a per-window choice (a popout can pin Change while main follows the agent). Errors (`is_error: true`) never switch lens; `candidates` (ambiguous symbol) switch to Fit with the best-ranked candidate as focus and a chip `1 of N candidates`. Focus for Here: `args.focus` (paths/idents) highlighted with the accent bar; for `code_find`, rows whose `name` matches; for `code_outline`, the file expanded.

### 6.2 Lens A — *What's here* (Here)

Content: `HereLens.rows` grouped file → directory, directories = the first two path segments (`crates/tools`, `src/components`) or the package when `files.package` is set (Cargo/npm/Go). Collapsed by default when the directory holds < 3 % of the ranked rows and contains no focus/edited file; `dirs` coverage rows render collapsed with `(N files)`. Per file ≤ 12 symbols, then `+N`. Order: files by best tag rank, symbols by line (60 §21 dev. 11). Row anatomy (mono 11 px, 71 §2.7 chrome register — the whole panel is the machine's voice, no prose register):

```
laptop, panel 640 px
┌ CodeMap · theuth ·· 1 051 files · 13 444 symbols · fresh ······ [here●] fit  change · ⊡ ⤢ ✕ ┐
│ ▾ crates/tools                                                       →61  ~1               │
│   ▾ src/registry.rs                                      →14 ↑9  ◆entry                     │
│   ▎    ToolRegistry           class    L231                                                  │
│   ▎    start                  method   L1366  ↑9                                            │
│   ▎    RegisterTool           class    L231   ↑7                                            │
│   ▸ src/lib.rs                                           →56                                │
│ ▾ crates/runtime                                                     →48                    │
│   ▾ src/lib.rs                                           →48  ◆entry                        │
│        Runtime                class    L22    ↑31                                           │
│        publish                method   L48    ↑12                                           │
│ ▸ crates/codeindex                                        NEW  ~2                           │
│ ▸ crates/desktop/ui  (412 files, 0 ranked)                                                  │
│ … +7 directories                                                                            │
├─────────────────────────────────────────────────────────────────────────────────────────────┤
│ crates/tools/src/registry.rs:1366-1439 · method · start · #3 · →14 ↑9 · import 1.0          │
└─────────────────────────────────────────────────────────────────────────────────────────────┘

phone, 390 px (bottom sheet, one level of indentation, chips wrap)
┌ CodeMap · theuth            here● fit change ✕ ┐
│ crates/tools/src/registry.rs      →14 ↑9 ◆    │
│   ToolRegistry  class  L231                   │
│   start         method L1366 ↑9               │
│   RegisterTool  class  L231  ↑7               │
│ crates/tools/src/lib.rs           →56         │
│ crates/runtime/src/lib.rs         →48 ◆       │
│   Runtime       class  L22   ↑31              │
│ crates/codeindex/…      NEW ~2   (collapsed)  │
│ … +7 directories                               │
└───────────────────────────────────────────────┘
```
`▎` accent bar = focus rows. The detail line (bottom) follows hover on fine pointers and the pinned row otherwise.

### 6.3 Lens B — *How it fits* (Fit)

Content: `FitLens` — focus centre, callers left, callees right, ≤ 15 nodes (`limit`), grouped by file when several symbols of one file call the focus (one node per file, `×N` count). Edge style from provenance: solid `same_file`/`import` (1.0), dashed `package` (0.8) / `unique` (0.6) — the two `GraphEdgeStyle`s (`spec.rs:545-553`), no third. `ambiguous` is a chip `?N` on the focus node. `more_callers`/`more_callees` are `+N more` rows under each column, clickable → the column scrolls (no more nodes are drawn).

```
laptop
 callers (7)                       focus                          callees (4)
 ┌ knowledge/src/tools.rs ┐                                       ┌ registry.rs        ┐
 │ register          ×1   ├──────────┐                            │ RegisterTool       │
 └────────────────────────┘          │      ┌──────────────┐  ┌──▶└────────────────────┘
 ┌ scrape/src/tools.rs    ┐          ├─────▶│ start        │──┤   ┌ lib.rs             ┐
 │ register          ×1   ├──────────┘      │ method  ?3   │  └──▶│ Tool · ToolOutput  │
 └────────────────────────┘                 └──────────────┘      └────────────────────┘
 ┌ codeindex/src/tools.rs ┐ - - - - - - - - ▲   registry.rs:1366-1439
 │ register          ×1   ├  (package 0.8)
 └────────────────────────┘
 +4 more callers                                                   +1 more

phone: header row = focus (path:line · kind · ?3), then two lists
 ┌ start · method · registry.rs:1366 · ?3      ┐
 │ callers (7)                                 │
 │  knowledge/src/tools.rs  register  ×1  ━━━  │   ━━━ solid = import/same_file
 │  scrape/src/tools.rs     register  ×1  ━━━  │   ╍╍╍ dashed = package/unique
 │  codeindex/src/tools.rs  register  ×1  ╍╍╍  │
 │  +4 more                                    │
 │ callees (4)                                 │
 │  registry.rs  RegisterTool             ━━━  │
 └─────────────────────────────────────────────┘
```

### 6.4 Lens C — *What's changing* (Change)

Content: `ChangeLens` as four horizontal bands; at rest **no edges**. Hovering/pinning a `direct` file lights the changed files whose `symbols[].name` ∩ its `via` ≠ ∅ (accent stroke on both, one frame); `<import>` in `via` lights every changed file it imports (from `import_edges`). Band caps 12 / 16 / 16 / 16 with `+N` chips; beyond the cap files collapse into directory chips `crates/desktop/ui (+9)`. Mode chip: `worktree · since 22b3945` / `main..HEAD` / `3 files`. Tests: `✓` when `test_link = "ref"` (names a changed symbol), `◇` when `"subject"` (name-derived), plain when unknown.

```
laptop
 changed (3)      [ index.rs  ~ ]  [ tools.rs  ~ ]  [ rank.rs  + ]                     worktree · since 22b3945
                  ───────────────────────────────────────────────────────────────────────────────────────────
 direct (12)      [ api.rs ] [ lib.rs ] [ outline_hook.rs ] [ bridge.rs ] [ export.rs ] … +7
                  ───────────────────────────────────────────────────────────────────────────────────────────
 transitive (4)   [ main.rs ] [ client.rs ] … +2                                                    depth ≤ 3
                  ───────────────────────────────────────────────────────────────────────────────────────────
 tests (8)        ✓ tests/index.rs   ✓ tests/impact.rs   ◇ tests/fixture.rs   … +5
                  464 ambiguous skipped · 2 unresolved imports

phone: the same four sections as lists; `+N` expands in place
 ┌ changed (3)    worktree                     ┐
 │ ~ crates/codeindex/src/index.rs  write, …   │
 │ ~ crates/codeindex/src/tools.rs  touched, … │
 │ + crates/codeindex/src/rank.rs   load, rank │
 │ direct (12)                                 │
 │   crates/rpc/src/api.rs      via Stats      │
 │   …                                         │
 │ transitive (4) · tests (8)                  │
 └─────────────────────────────────────────────┘
```

### 6.5 Encodings — what each element means, and what is never drawn

| Element | Encodes | Source | Rule |
|---|---|---|---|
| Row / node order | rank | `MapRow.rank` | position only; the number appears in the detail line as `#n`, never on the row |
| `→N` | distinct files importing this file | `importers` | hidden when 0 |
| `↑N` | distinct caller files of this symbol | `callers` | hidden when 0 |
| `NEW` | file mtime < 48 h | `recent` | chip, muted-foreground |
| `◆entry` | manifest entry point | `entry` | chip |
| `test` | `files.is_test` | `is_test` | chip on Here rows; `✓`/`◇` in Change tests band |
| `~` `+` `−` | edited this session (client) · added · deleted | `codemap.edited` · `ChangeFile.status` | one glyph before the label |
| `▎` accent bar | focus (args.focus / found / outlined / pinned) | client | Here rows |
| `danger` stroke | changed | Change band 1 | 65 `paint('danger')` |
| `active` stroke | focus (Fit), direct (Change), hover partner | Fit focus / Change band 2 | `paint('active')` |
| `muted` 50 % | transitive · collapsed directory · stale rows | Change band 3 / Here | `paint('muted')` |
| `done` | test that names a changed symbol | tests band `ref` | `paint('done')` |
| solid / dashed edge | confidence 1.0 / < 1.0 (`package`, `unique`) | `FitEdge.provenance` | number only in the detail line |
| `?N` | ambiguous references named like the focus | `FitLens.ambiguous` | chip on the focus node, never edges |
| `×N` | edge count | `FitEdge.count` | edge label when > 1 |
| `+N` | dropped beyond a cap | `more*`, `dropped` | text row, clickable to scroll, never draws more nodes |
| `stale · N pending` | background reconcile running | `meta.stale/pending` | header chip, pulsing dot (static under reduced motion) |
| banner | `suspect_root` | `meta.hint` verbatim | lenses frozen; **Reconcile** button |

Labels: file = basename, ≤ 24 chars middle-ellipsis (`regist…tion.rs`), full path in `title` and the detail line; directory = the collapsed segments as written; symbol = `name` ≤ 32 chars (Rust methods `Owner::name` when the owner is known, 60 §21 dev. 6); chips ≤ 6 chars; everything mono 11 px. Hover (fine pointer, 120 ms as boards, 71 §0 Q2) / tap → detail line `path:a-b · kind · name · #rank · →N ↑N · provenance confidence`; no popovers.

**Never drawn:** the whole repository; edges in Here; edges at rest in Change; ambiguous refs as edges; more than 15 nodes in Fit or 60 files in Change; rank or confidence numbers on rows/edges; icons per kind; node colours for language or kind; rows with `rank = 0` except focus/edited/changed ones; animation on first paint (M7); anything the index did not say (no client-side guessing of edges from names).

### 6.6 Layout and motion (`lib/codemap.ts`, pure functions, vitest)

- `columnsLayout(fit: FitLens, width): Layout` — three columns at `x = 0 · (W−w)/2 · W−w`, node size from `wrapLabel` at `WRAP = 24` (`graph-layout.ts:81`), rows stacked with `elk.spacing.nodeNode`'s 24 px, focus vertically centred on the taller column; edges as one section `start → end` with a single horizontal bend (`bends: [{x: mid, y: start.y}, {x: mid, y: end.y}]`, the ORTHOGONAL look). O(n log n) for the sort; < 1 ms.
- `bandsLayout(change: ChangeLens, width): Layout` — four rows of pills wrapping at `width`, band labels as `groups` (`GraphSvg` draws groups as rounded rects with a title band, `Graph.tsx:107-110`); edges empty at rest; hover edges rendered by a second `<g>` over the svg, not by relayout.
- Stability: node `id = moniker` (file nodes `<lang>:<path>`), so a delta keeps ids; each `<g data-node>` gets `style="transition: transform 200ms"` and `transform: translate(x,y)`; new nodes fade in 200 ms, removed nodes fade out then unmount. `prefers-reduced-motion` → no transitions. No re-sort on hover; sort keys change only on a lens refetch.
- Here needs no layout: `@tanstack/react-virtual` over the flattened visible rows (already a dependency, used by `SessionList`), row height 22 px, overscan 8.

### 6.7 The panel host and the header

- `HeaderId` gains `'codemap'` (`header.ts:28`); `headerOrder` adds `{ id: 'codemap', slot: panel === 'codemap' ? 'bar' : 'menu', shed: true }` beside `terminals`/`browser`/`artifacts`; `Header.tsx:194` priority becomes `artifacts → codemap → browser → terminals` (the agent's structural work outranks a screenshot; §12 Q3). The button (lucide `Map`) shows a dot while `codemap.last.seq` is newer than what the mounted panel has shown.
- The panel is a right `Sheet` with `ArtifactPanel`'s drag strip and clamps (`ArtifactPanel.tsx:238-261`), width persisted; `Sheet side="bottom"` full height under 768 px. The header row: root name (basename of `root`; a `▾` menu when `codemap.roots.length > 1`) · `indexed_files · symbols` from `codeindex_stats(root)` (one query, `staleTime: Infinity`, invalidated by `CodeReconciled`) · freshness chip · lens tabs · **Snapshot** ⊡ · **Open in new window** ⤢ (`canPopout()`) · ✕.
- Two sessions, one root: each session's slice is independent; two panels may show the same root (fine — read-only).

### 6.8 Interaction that feeds back

Node/row actions (context menu on right-click / `⋯`, Enter on a focused unit opens the same menu):
- **Ask about this** — appends `` `path#L<a>-L<b>` `` (file rows: `` `path` ``) plus a space to the session's draft: `useDrafts.getState().set(key, { ...draft, text: draft.text + anchor })` — a five-line `appendDraft(session, text)` beside `prefillDraft` (`Composer.tsx:76`); running session → the user sends → the steer path as boards do (71 §2.9). The header line is `[codemap <root> · <lens> · <focus>]` only when the draft is empty (one line, `board-ask.ts` grammar family) so the agent knows which root; the prompt block of 60 already tells it what `path#L` means.
- **How it fits** — `pinned(fit, focus = moniker)`; **What changes** — `pinned(change, files = [path])`.
- **Snapshot to board** — `toGraphFence(lens)` builds a `json-render` `Graph` (65 §3.1) with ≤ 12 nodes (`Fit`: focus + best callers/callees; `Change`: changed + top direct; `Here`: top 12 files), `ref = path#L<a>-L<b>` on every node (71 §3.2), `state` per 71 §4.3, `label` ≤ 80. Two actions: **Copy fence** (clipboard) and **Ask to add to board** → `prefillDraft(session, "[codemap …]\n```json-render\n…\n```\nAdd this to the board's Map section.")` — the agent inserts it with `doc_insert` (boards are agent-authored, §2.B). No new RPC; when the client can create boards (§12 Q6) this becomes one call.
- **Export** — Fit and Change are mounted inside `DiagramFrame` (`DiagramFrame.tsx:243`), so Copy PNG / Save PNG… / Save SVG… / Fullscreen come for free; a snapshot placed in a board exports to PDF through 72 §5 (client-rendered SVG asset).
- Keyboard: tabs `1/2/3`, `p` pin/follow, `↑/↓` rows, `←/→` columns in Fit, Enter menu, Escape closes menu then panel; units are `role="button" tabIndex=0` as `GraphSvg` already emits (`Graph.tsx:132-139`, 71 M14).

### 6.9 Pop-out and mobile

- `PopoutTarget |= { kind: 'codemap'; session: string; root: string }`; `idOf` → `${session}/${root}`; `pathOf` → `/codemap/${session}/${encodeURIComponent(root)}`; label `popout-codemap-<safe(session)>-<safe(root)>` (`safe` turns `/Users/x/y` into `_Users_x_y`; Tauri label charset, `popout.ts:45`); `popoutTarget` parses the `codemap` head; `popoutSession` returns `session` so `SessionDeleted` closes it (`App.tsx:299-302`). Route `routes/codemap.$session.$root.tsx` → `PoppedCodeMap` = the same `CodeMapPanel` body without the `Sheet`, full height, `document.title = "CodeMap · <root basename> · <lens>"` (native title via `on_document_title_changed`). The main panel shows nothing special when popped (read-only; unlike the PTY size rule there is no owner).
- Widths: ≥ 1080 px window — lens body + the detail line + a right 280 px "context" column in Fit listing every caller file (the `+N more` expands here instead of scrolling); 768–1080 — body + detail line, no context column; < 768 — lists of §6.2–6.4, no SVG, no hover, tap = pin, no pop-out button (`canPopout()` false on iOS; a narrow desktop window still pops out), 44 px targets, `--kb-inset` respected as NoteChat does.

## 7. Performance contract

| Path | Budget | How it is met | Measured baseline |
|---|---|---|---|
| `codemap_here` warm | **< 30 ms** | one `SELECT` on the render sibling's map cache keyed `(…, budget, focus, edited)` + badge columns; no walk, no rank | map cache: sibling (expected); `ranked_tags` is a lookup (60 §7) |
| `codemap_here` miss | < 80 ms | `ranked_tags LIMIT 2000` (`store.rs:960`) + three indexed `GROUP BY`s for badges; if over budget in E2, the badges become columns written at `apply` time | rank persist is *not* on this path |
| `codemap_fit` | **< 30 ms** | `definition_*` + `refs_to`/`refs_from` `LIMIT 200` on `refs_to`/`refs_name` indexes + one `GROUP BY file` | `code_symbol` callers/callees are the same queries under budget today |
| `codemap_change` range/files | < 120 ms | the `code_impact` path minus `content` rendering | `code_impact(files=…)` 94 ms (60 §22.1) |
| `codemap_change` WORKTREE | < 300 ms on this repo (1 051 files) | HEAD-tree hash compare on `spawn_blocking` (60 §22.3 dev. 1); cached per `(root, git_head, max mtime_ns)` so the second call is a lookup; the panel keeps the previous bands with a `computing…` chip | ~1 000 blob reads; not timed in 60 — E2 records it |
| `codemap_touch(root, paths)` | < 50 ms per file | stat + parse + `apply` + `resolve` for the named files; rank persist deferred to the next tool call when > 4 files touched within 2 s | parse "a fraction of a second" for 300 files (60 §19.1) |
| first paint, 2 000 rows | **< 100 ms after data** | virtualised list (~40 DOM rows); group-by + collapse is one O(n) pass in `useMemo` | React re-render to next frame 15.3 ms (65 as-built) |
| delta applied | **< 16 ms** | rows in a `Map<moniker, MapRow>`; `CodeReconciled` → refetch → diff by key → one `setState`; no relayout in Here, hand layout < 1 ms in Fit/Change | ELK is out of the loop |
| transitions | 200 ms, none under reduced motion | CSS transform on `<g data-node>` | — |
| events | engine: `CodeReconciled` ≥ 250 ms apart per root, `CodeToolRan` one per call; UI: rAF batch (`App.tsx:124`) + `createRenderQueue(…, 200)` per panel | `ToolCallProgress` uses the same 250 ms gate |
| memory | ≤ 2 000 rows × ~250 B ≈ 0.5 MiB per root slice, ≤ 8 roots per session; lens replies dropped on unmount; `edited` ≤ 200 paths | — |
| web workers | **none** | nothing on the client exceeds a frame; the heavy work (diff, rank, parse) is in the engine; the ELK worker remains available if Fit ever needs a real layout (§10) | — |

Behaviour under index states: `stale` → chip, rows drawn, refetch on `CodeReconciled`; `pending > 0` → chip shows the count; `truncated_walk` → chip `walk capped at 50 000 files`; `suspect_root` → banner with `hint`, lenses frozen (no refetch, no touch), rows at 50 %, **Reconcile** → `codemap_touch(root, null)`; `codeindex is disabled` → no `CodeToolRan` ever arrives, no button; root not indexed (`ASK_FAILED`) → one line `run a code_* tool first` (cannot happen via the event path; can via history-load `"?"`).

## 8. Threats and limits

| Threat / limit | Handling |
|---|---|
| Source text is untrusted (28): `signature`, `docs`, paths carry whatever the repo says | Rendered as React text / SVG `<text>` only; no `dangerouslySetInnerHTML`; `ref`s built by the client from `path`+lines, never from index strings; the snapshot fence is validated by `parse.ts` before it is offered |
| A lens RPC pointed at an arbitrary `root` indexes a stranger's tree | Lens RPCs open only roots with an existing database; first indexing stays with the agent's tool call |
| `codemap_touch` with paths outside the root | `root::under_root` (`root.rs:44`) per path; the whole call fails |
| Event flood (a Python cell calling `code_find` in a loop) | one `CodeToolRan` per call is bounded by tool throughput; `CodeReconciled` gated 250 ms; UI refetch gated 200 ms; rows never ride events |
| Two windows both `touch` the same edit | idempotent (mtime unchanged on the second); the engine's per-root `write` lock serialises |
| Popout and main racing lens replies | `seq` on every reply; a reply older than the shown `seq` is dropped |
| Subagent sessions calling `code_*` | `CodeToolRan.session` is the child's; the parent pane's button appears only for the parent's own calls (§12 Q5) |
| Remote engine (relay) | same RPCs over `LinkTransport`; roots are remote paths and are displayed as such; `touch` still works (the engine stats its own disk) |
| Huge Fit (a function with 400 callers) | `limit` 15 with `more_callers`; the context column lists the rest as text |
| Windows paths in `root` labels/URLs | `encodeURIComponent` in the URL, `safe()` in the label; display uses the basename; canonicalisation quirks stay with 58 |
| Wrong answers with a straight face | provenance/confidence styling, `?N` ambiguity, `ambiguous_skipped`, `unresolved_imports`, `stale`, `suspect_root` all visible; no client-side inference |

## 9. What we knowingly give up

Editing from the map; a repository-wide graph; force layouts and any notion of "explore"; symbol-level Here beyond 2 000 rows (files/directories carry the rest); personalised rank computed for the panel (it shows the agent's map, plus stored ranks); edges in Change at rest; shared pin state across windows; iOS pop-out; LLM summaries in the render path; a stored CodeMap (snapshot into a board instead); first-index-from-the-panel; type-aware edges (60 §12).

## 10. Alternatives considered

- **ELK for Fit/Change** — already shipped, 12.7 ms for 6 nodes in the worker; rejected for v1 because a fixed column/band layout is stable by construction (no crossing minimisation can move a node between deltas) and needs no worker round trip; the seam is `layout(props)` → `Layout`, identical shape, so switching a lens to ELK (with `elk.layered.crossingMinimization.semiInteractive` and previous `x/y`) is a one-function change.
- **Treemap for Here** — rank is a dense order, not a meaningful area; treemaps hide labels under 40 px; a tree list keeps 40 rows legible.
- **react-flow / @xyflow (MIT, ~50 kB gz)**, **cytoscape**, **sigma** — a second graph renderer beside `GraphSvg`; 65 §0.4 "one React renderer".
- **d3-hierarchy (ISC, ~15 kB)** for the tree — unnecessary; a flattened list plus `expanded: Set<dir>` is the tree.
- **Rows on the event channel** (a `CodeMapChanged` with the rendered rows) — 2 000 rows × every subscriber through the 1 024-slot broadcast; and popouts would need them again on open. RPC on demand, cached engine-side.
- **Polling `codeindex_stats`** every 500 ms like `BrowserLive` — the browser has no event for "frame changed"; the index does now.
- **Codeindex subscribing to `AgentEvent::ToolCallFinished`** for live edits — `files` are produced files, not edited paths; and a `codeindex → agent` dependency for one field. The UI already knows the edited path from `args`.
- **A `DocKind::CodeMap`** — see 0.1.
- **Empryo / SoulForge's desktop map** — private, BSL 1.1; ideas only (recon §1).
- **A Panes-tree side pane instead of a Sheet** — the chat's `Panes` splits sessions, not tools; `ArtifactPanel` set the precedent; pop-out is the side-by-side answer (§12 Q1).

## 11. Build steps (one commit each, `cargo check --workspace` and the UI trio green; E and U in parallel from §5)

| # | Worker | Delivers | Runnable check |
|---|---|---|---|
| E1 | engine | `event.rs` (`CodeindexEvent`, `CodeToolSummary`), `Codeindex.rt: Option<Runtime>` + `RootIndex.seq`, `CodeToolRan` from all six tools, `CodeReconciled` from `write`/background with the 250 ms gate, `bridge.rs` entries, `export.rs`, `WireEvent` mirror | headless `t --serve --data-dir /tmp/codemap-smoke` (60 §17's recipe); a bun WebSocket client subscribed with `subscribe_events` sees `{"type":"CodeToolRan","tool":"code_find",…}` after `send_input("run code_find(name=\"ToolRegistry\")")`, then `CodeReconciled` after `touch`-ing one file; `cargo test -p codeindex` gains `events_are_published_once_per_call` and `reconciled_is_gated_250ms` |
| E2 | engine | `lens.rs` + actor messages `Here/Fit/Change/Touch` + the four RPCs; store queries `map_rows(limit, focus, edited)`, `neighbourhood(symbol, limit)`, `touch(paths)`; `cache_hit` through the render sibling's cache; monikers synthesised when absent | `cargo test -p codeindex`: `fit_caps_at_limit_and_counts_more`, `touch_reparses_only_named_paths`, `lens_rpcs_never_walk` (a walk counter stays 0); a bun script against the headless engine prints `codemap_here` / `codemap_fit` `meta.ms` for 20 warm calls each on this repo — medians recorded in the as-built against the < 30 ms line; `codemap_change(WORKTREE)` timed once |
| U1 | ui | `SessionSlice.codemap` + reducer (`CodeToolRan`, edit tools, history load), `HeaderId 'codemap'`, `CodeMapPanel` Sheet with Here (virtualised tree, badges, collapse rules, detail line, freshness chips, `suspect_root` banner) over a 2 000-row fixture behind a mocked `Api`, then the real RPC | `reduce.test.ts`: `CodeToolRan` sets `roots`/`last`, `edit` appends `edited`; `codemap.test.ts`: collapse rule, label ellipsis, badge hiding at 0; a vitest with `performance.now()` asserts the fixture's first render < 100 ms (jsdom, generous) and a 50-row delta < 16 ms; browser smoke (`vite --port 51xx` against the live engine): run `code_map` in a scratch chat → button appears → panel shows the tree, chips, detail line |
| U2 | ui | Fit and Change lenses (`columnsLayout`, `bandsLayout`, `GraphSvg` mount, hover partners, transitions, reduced-motion), the lens state machine (`lib/codemap.ts` pure), pin/follow, keyboard | `codemap.test.ts`: state machine table (every §6.1 transition), `columnsLayout` positions are a pure function of sorted input, `bandsLayout` wraps at width; `Fit.test.tsx` renders a 15-node fixture with 7 dashed edges and a `?3` chip; smoke: `code_refs` in the scratch chat switches to Fit and a node glides when the agent calls `code_symbol` on a caller |
| U3 | ui | `PopoutTarget 'codemap'`, route + `PoppedCodeMap`, mobile lists at < 768, `appendDraft` + **Ask about this**, **Snapshot to board** (`toGraphFence` + copy + prefilled ask), `DiagramFrame` mount for export, header button dot | `popout.test.ts`: label/url/round trip for `codemap` incl. a root with spaces; `toGraphFence` output passes `parseSpec` (≤ 12 nodes, `ref` on all); smoke at 1440 / 1024 / 390 px: pop out → second window with the same lens, close on `delete_session`; 390 px shows lists and no pop-out button under an iOS UA; **Ask about this** appends `` `crates/…#L1366-L1439` `` to the composer |
| 6 | either | `docs/design/70-as-built-CODEMAP.md` with the E2 medians, the U1 render numbers, screenshots at three widths, deviations; 60 §10 and 65 §0.6 amendment lines | the numbers sit inside §7's lines or the deviation says why not |

E1/E2 and U1/U2 are independent given §5; U3 depends on U1–U2; E2 needs the sibling fields only for `cache_hit`/badges (fallbacks in §5.4). Suggested order for one worker: E1 → U1 → E2 → U2 → U3 → 6.

## 12. Open (decided by default; overturn in the as-built)

1. **Panel host: `Sheet` overlay (default) vs a real side pane in `Panes`.** A Sheet covers the transcript at panel widths > 50 %; pop-out is the side-by-side answer. Revisit if users keep the panel open while chatting.
2. **Header priority** `artifacts → codemap → browser → terminals` (default) — the shed slot shows one button; the others live in the chevron menu.
3. **`CodeReconciled` is global** (no `session`): a window with two sessions on the same root gets it once; the UI filters by root. Alternative: publish per session that touched the root (more events, no benefit seen).
4. **History-load availability with an unknown root** (`"?"` → `codeindex_stats(null)` default root). Alternative: parse the `root:` line from the tool's `content` text — fragile; not done.
5. **Subagent calls**: the parent pane shows a button only for its own session's calls (default). Alternative: aggregate descendants' roots when the pane subscribes with `descendants`.
6. **Snapshot into a board directly** needs `doc_create(kind, root)` + an insert RPC for the client (72 §7 lists the same gap for exports). Default: ask the agent. Add the RPC when a second client feature wants it.
7. **`recent` window 48 h** (Empryo's) — or "since the session started"? Default: the sibling's 48 h; the `~` glyph already marks this session's edits.
8. **Rank numbers**: never on rows (default). A `#n` in the detail line only.
9. **Fit edge styles**: two (solid ≥ 1.0, dashed < 1.0). A third for `unique 0.6` (dotted) was rejected as a fourth thing to learn; revisit if `package` vs `unique` matters in practice.
10. **WORKTREE cache key** `(root, git_head, max mtime_ns)` — the walk's max mtime is not free (it is the walk). E2 may use `RootIndex.seq` instead (any write invalidates).
11. **ELK for Fit** if a real project shows callers with shared callees crossing badly in fixed columns.
12. **Field names from siblings** (`importers`, `callers`, `recent`, `entry`, `moniker`, `test_link`, `hint`) are taken from the recon brief; E2 aligns `lens.rs` to whatever merges first and the as-built records the final spelling.

## 13. Status

Design drafted 2026-09-28 on `feature/73-codemap-design` from the recon of this tree at `22b39455` (§2.A–B, every claim `file:line`), the Empryo recon (`~/.theuth/artifacts/dbc0092fb984e94e.md`) and the in-flight sibling branches (§2.C). No code. Step E1 can start now; U1 can start now against the §5 fixtures.
