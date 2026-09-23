# 65 — Graph: a typed box-and-arrow component for replies and apps; mermaid 12 on the desktop

Status: design, 2026-09-23. Extends 30 (surfaces: the `surface` crate, json-render catalog, mermaid on every surface), 46 (apps: OpenUI library, reactive props, one-press actions), 58 (cross-platform WebView floors). Amends 30 §0.7 ("two libraries, zero custom renderers"), 30 §1 (desktop row), 30 §8.3 (mermaid as built). Ruled by Sky 2026-09-23 after the cloudlands-fe comparison: "I definitely want the Graph component"; no fixture bake-off of alternative mermaid renderers.

Scope: one new catalog component, `Graph`, that the model emits inside a ```` ```json-render ```` fence (static) or an OpenUI app (reactive, steppable), rendered from one Rust definition on every surface; and the desktop's mermaid path moved to mermaid 12 (bundled ELK), loaded lazily, rendered through a coalescing queue, with the platform floors that release requires.

Not in v1 (and the seams left for them): camera/zoom-to-node per step (an `focus` prop on the apps `Graph`, the pan/zoom frame already has the transform); manual node positions (`x`/`y` on `GraphNode`, ignored by the layout when absent); nested groups deeper than one level (`group` stays a flat id; a `parent` on `GraphGroup` later); node icons (a `kind`-keyed icon map in the React renderer); Rust-side layout for the TUI (the lowering to mermaid text is the seam — a Rust layered layout would replace merman there, nothing else moves); sequence/state/class/ER semantics (mermaid keeps those; `Graph` is boxes and arrows).

## 0. Decisions in one screen

1. **`Graph` is a catalog component, not a diagram format.** `Graph(nodes, edges, groups?, direction?)` joins `Timeline`, `Bars`, `Checklist` in `crates/surface/src/spec.rs` (`Component` enum, spec.rs:39) with the same derives (serde + schemars + ts-rs, spec.rs:343-345), the same located `SpecError`s, the same one `<surface_error>` correction round (30 §5). The model writes it where it writes every other json-render block. Rejected: a `` ```diagram `` fence with its own schema and parser (cloudlands-fe `messageParser.ts:121`, `incrementalDiagramJson.ts`) — a second wire format for one component; 30 §0.2 "no new wire types" holds.
2. **One Rust definition, every surface.** TUI, Slack, Signal and `message_user` get `Graph` for free: `render.rs` lowers it to mermaid flowchart text and hands it to the existing `mermaid_lines` (mermaid.rs:16, merman ASCII), falling back to an adjacency list when merman declines or the width is too small (mermaid.rs:25-35 already does this for fences). Rejected: a desktop-only component (30 §0.3 — the catalog is one definition; a component the TUI cannot show is a component the prompt must caveat).
3. **Layout is ELK, off the main thread, and shared.** The desktop renderer runs `elkjs` layered layout in a Web Worker, memoised by a structural hash of the props; mermaid 12 depends on the same `elkjs ^0.9.3`, so one copy ships, in a lazy chunk fetched the first time a diagram appears. Rejected: dagre (weak compound graphs — the single largest source of "mermaid looks broken"; 65 §2.3); an in-house layered/force/tree engine (cloudlands-fe `layout-engine.ts`, 4,374 lines; 00 §1.2 has no written reason that survives comparison with a library); layout in Rust for the desktop (30 §0.2: rendering is a frontend concern; the TUI has merman).
4. **One React renderer, two registrations.** `components/chat/surface/Graph.tsx` renders the json-render `Graph` (registry.tsx:254 `defineRegistry`) and is the same component `packages/apps/src/library.ts` registers via `defineComponent` for OpenUI (46 §4: "only include theuth-specific display components when an existing upstream component cannot express a real screen" — none draws a graph). The apps registration adds two props the static one lacks: reactive `highlight: string[]` and `onSelect: Action`. Rejected: aligning the two catalogs by generation (46 §4 explicitly dropped that requirement); a shared fixture set asserts they accept and reject the same documents instead (§5).
5. **Walkthroughs are app state, not schema.** Step-through diagrams (cloudlands-fe `DiagramState`: visible/highlighted sets, camera, narrative, transition) are `$step = 0`, a `Buttons` Prev/Next, a `TextContent` narrative and `highlight` derived from the step. Nothing new in the wire, the parser or the renderer beyond `highlight`. Rejected: `states[]` on the component (a state machine inside a display component, unrenderable on text surfaces).
6. **A `Graph` is a sink** (30 §0.10, 28). Nodes carry text, a shape hint, a group and a semantic `state`; no `href`, no `src`, no file/symbol bindings. In apps, `onSelect` is an ordinary 46 §6 action (`@Set`, `@ToAssistant`, `@OpenUrl`) — the press is the approval. Rejected: cloudlands-fe's `bindings` (click → open file/symbol/note): a navigation side channel a spec must not have.
7. **Mermaid 12, directly.** `@streamdown/mermaid` is deleted: it is an 80-line static wrapper around `mermaid.render` (vercel/streamdown `packages/streamdown-mermaid/index.ts`) that pins mermaid 11 and makes it non-lazy. `Mermaid.tsx` does `await import('mermaid')` itself, keeps `securityLevel: 'strict'`, `htmlLabels: false`, `suppressErrorRendering`, `maxEdges: 1000`, gets ELK as the default layout for flowchart/state/class/ER, and renders through a one-in-flight + trailing-snapshot queue instead of once per source change. Rejected: `@mermaid-js/layout-elk` on mermaid 11 (cloudlands-fe `MermaidRenderer.svelte:7`) — a bolt-on for a floor we would raise anyway; post-processing mermaid's SVG in the DOM (cloudlands-fe, ~2,600 lines keyed to mermaid diagram types) — 00 §1.2, and it breaks on every mermaid upgrade.
8. **Raise the floors mermaid 12 requires, once, for the whole UI.** ES2024 / Safari 17.4+: `crates/desktop/ui/vite.config.ts:68` `build.target` `safari15` → `safari17`; `tauri.ios.conf.json:10` `minimumSystemVersion` `17.0` → `17.4`; macOS 15 (tauri.macos.conf.json:40) and WebView2 already clear; Linux WebKitGTK verified on the release box (58). Rejected: polyfilling — esbuild lowers syntax to a target but cannot supply `Object.groupBy`, `Promise.withResolvers`, Set methods or the RegExp `v` flag.

## 1. Why

- The chat has two ways to draw and both are second-rate on their own axis: mermaid fences look like 2015 dagre on anything with subgraphs, and json-render has no component for the most common picture in an engineering answer — boxes and arrows. The visualize prompt (30 §6.2) tells the model to draw architecture, dependency and flow "when a reader would sketch it on a whiteboard"; today that sketch can only be mermaid, which the TUI shows as ASCII only when merman parses it and which an app cannot highlight or step.
- cloudlands-fe (`~/Develop/cloudlands-fe`, Svelte 5 + Electron) solved the same two problems with ~11k lines: a `DiagramPrimitive` JSON schema with 8 grammars, states, bindings and a 4,374-line layout engine; plus 2,600 lines of DOM repair on mermaid's SVG. The features worth having — semantic node states, groups, step-through, clickable nodes in an interactive surface — map onto seams theuth already owns (catalog, apps state, actions). The costs — a second wire format, a layout engine, DOM surgery — are what 00 §1.2 and 30 §0.2/§0.7 exist to refuse.
- Mermaid 12.0.0 (2026-09-10) bundles ELK and makes it the default for the diagram types that were ugly. The only thing between the desktop and that is a wrapper package and two platform floors.

## 2. Facts relied on

### 2.1 theuth today
- Desktop mermaid: `crates/desktop/ui/src/components/chat/Mermaid.tsx:21-24` builds `createMermaidPlugin({config: {securityLevel:'strict', htmlLabels:false, flowchart:{htmlLabels:false}, suppressErrorRendering:true, maxEdges:1000}})`; `useRender` (:26-49) calls `.render()` in a `useEffect` on every `chart` change and drops stale results with a `live` flag — the work still runs at stream rate. `MessageAssistant.tsx:17` imports `MermaidFence` statically and the plugin imports `mermaid` statically, so mermaid is in the main chunk (30 §1 says "lazy mermaid@11"; it is not). Pan/zoom/fullscreen/export as built in 30 §8.3.
- Pins (`crates/desktop/ui/package.json`): `mermaid ^11.12.2`, `@streamdown/mermaid ^1.0.2`, `streamdown ^2.6.0`, `@json-render/core|react ^0.20.0`, `react 19`, `vite ^8.3.0`. Floors: `vite.config.ts:68` `build: { target: 'safari15' }`; `tauri.ios.conf.json:10` `17.0`; `gen/apple/desktop.xcodeproj/project.pbxproj:307,369` `IPHONEOS_DEPLOYMENT_TARGET = 17.0`; `tauri.macos.conf.json:40` `15.0`.
- `crates/surface`: limits `lib.rs:34-40` (`MAX_FENCE_BYTES` 256 KiB, `MAX_ELEMENTS` 256, `MAX_DEPTH` 32, `MAX_MERMAID_BYTES` 128 KiB); `parse()` lib.rs:71 strict → jsonrepair → `validate`; `check()` lib.rs:131. `spec.rs:39` `Component` enum, `TimelineProps` :346 as the derive template. `validate.rs:12` `CONTAINERS`, element cap :63. `catalog.rs:65` `catalog_compact()` (prompt signatures), :75 `catalog_schema()`, :114 `NAMES: [&str; 18]` (the test that pins the catalog). `render.rs:20` `spec_lines`, :31 `fence()`, :513 the `Timeline` arm (glyph vocabulary `◉ ○`). `mermaid.rs:16` `mermaid_lines(src, width) -> Result<Option<Vec<Line>>>`, `None` on `UnsupportedDiagram`/too narrow (:25-35).
- Desktop json-render: `components/chat/surface/registry.tsx:1,254` `defineRegistry(catalog, { components })`, `Timeline` :191 as the template; `catalog.ts` from ts-rs bindings; `JsonRenderFence.tsx` R3/R4 repair and the `ErrorCard`.
- Apps: `packages/apps/src/library.ts:26-53` `CURATED_COMPONENTS` (21 upstream names; order is part of the compatibility hash), `renderer.tsx:100` `AppRenderer({library})`; 46 §4 as-built: `bun run generate` → `generated/library.json`, `catalog.txt`, `library-hash.txt`; a library change invalidates every committed app's `library_hash` (46 §8.3).

### 2.2 mermaid 12.0.0 (release notes, 2026-09-10)
- Breaking: ES2024, Safari 17.4+, Node 22.12+. ELK bundled (`elkjs ^0.9.3`) and default for flowchart, state, class, ER, requirement, use case, agentflow; `layout: dagre` restores the old one; ELK is its own chunk in ESM builds. `defaultRenderer` removed. New default appearance `redux-color` theme + `neo` look; `theme: default` + `look: classic` restore the old. `dist/mermaid.esm.min.mjs` trips Vite's es-module-lexer — import the `mermaid` specifier (we do). `@mermaid-js/layout-elk` remains only for the tiny build.
- `@streamdown/mermaid`: published 1.0.2 depends on `mermaid ^11.12.2`, main on `^11.16.1`; no issue or PR in vercel/streamdown mentions 12; the wrapper uses nothing 12 removed. Their open PR #632 ("pause between renders so a streaming diagram does not hold up the stream") is the queue of §4.3, independently.

### 2.3 Why ELK over dagre (and why cloudlands needed 2,600 lines without it)
ELK layered (`org.eclipse.elk.layered`) does compound graphs natively — a subgraph is a node with children, edges route around and into it — where dagre's compound support makes edges cut cluster boxes; layer-sweep crossing minimisation; `ORTHOGONAL` edge routing with bend points; port-side constraints; `considerModelOrder` so nodes appear in authored order; `mergeEdges`. Cost: ~1.4 MB minified (GWT-compiled), tens of ms for graphs under ~100 nodes, and it must not run on the main thread during a stream. Every geometry repair in cloudlands-fe `mermaid-path-geometry.ts` / `mermaid-cluster-membership.ts` is a dagre artefact or a label-measurement artefact (fonts not loaded before `getBBox`).

### 2.4 cloudlands-fe, for the record
`src/lib/utils/messageParser.ts:121,134` two fences; `src/lib/components/markdown/MermaidRenderer.svelte` 2,593 lines (ELK registered :7, post-processing :1319-1490); `src/lib/components/diagrams/layout-engine.ts` 4,374 lines (`computeLayout` :251, layered :986, force :1634, orthogonal channel routing); `DiagramRenderer.svelte` 2,161 lines, SVG edge layer :1517 with HTML nodes in `<foreignObject>`; `StreamingDiagramRenderer.svelte` 90 lines (keep last good; the pattern §4.3 keeps); `progressive-render-queue.ts` 65 lines (one in-flight + trailing, 80 ms; the queue §4.3 adopts). `src/shared/types/notes-primitives.ts:212-400` `DiagramPrimitive` (8 grammars, `DiagramState`, `DiagramBinding`).

## 3. Shape: the `Graph` component

### 3.1 Spec (Rust, `crates/surface/src/spec.rs`; ts-rs exports to `bindings/`)
```
Graph(nodes: [GraphNode], edges: [GraphEdge], groups?: [GraphGroup], direction?: LR|TB)
GraphNode  { id, label, kind?: box|rounded|pill|diamond|cylinder|actor, group?: id, state?: GraphState }
GraphEdge  { from: id, to: id, label?, style?: solid|dashed }
GraphGroup { id, label }
GraphState = default | active | done | muted | danger      // Timeline/Badge vocabulary, render.rs:517 glyphs
```
Compact signature for the prompt (catalog.rs:65): `Graph(nodes[{id,label,kind?,group?,state?}], edges[{from,to,label?,style?}], groups?[{id,label}], direction?)`. `#[serde(deny_unknown_fields)]` like every props struct.

### 3.2 Validation (`validate.rs`, located errors)
`/elements/<id>/props/edges/3/from: unknown node "db2"`; duplicate node id; `group` naming no group; edge `from == to` allowed and drawn as a self-loop; caps `MAX_GRAPH_NODES` 120, `MAX_GRAPH_EDGES` 240, `MAX_GRAPH_GROUPS` 12, label ≤ 80 chars (longer is an error, not a truncation — the model gets the round). Constants next to `MAX_ELEMENTS` in lib.rs. Parity fixtures in `crates/surface/fixtures/json-render/graph-*.json` (valid: architecture with 2 groups; dependency LR with dashed edges; danger path; invalid: unknown endpoint, dup id, over-cap).

### 3.3 Text surfaces (`render.rs`, new `graph.rs` for the lowering)
`to_mermaid(&GraphProps) -> String`: `flowchart LR|TB`, `subgraph <id>["<label>"] … end`, node shapes `[ ]`, `( )`, `([ ])`, `{ }`, `[( )]`, `[/ /]`… mapped from `kind`, edges `-->` / `-.->`, labels quoted and escaped (30 §6.3 "mermaid rules that avoid errors" is the escape list). Then `mermaid_lines(&text, width)`: `Some(lines)` → the TUI draws it; `None` → adjacency list, one line per node `◉ api → db (writes), queue` with the state glyph, groups as indented headers. Slack/Signal (`flatten.rs`) always take the adjacency list (box drawing survives per 30 §9, but a 40-node ASCII graph does not fit a phone). The lowering is also the fixture oracle: every valid fixture's `to_mermaid()` must parse under merman.

### 3.4 Desktop (`components/chat/surface/Graph.tsx`, `lib/graph-layout.worker.ts`)
- Props → structural hash (ids, kinds, groups, edges, direction, label text) → worker `layout(hash, graph)`; result `{nodes:[{id,x,y,w,h}], edges:[{id,sections:[{start,bends,end}], labelPos}], groups:[{id,x,y,w,h}], size}` cached by hash in a module `Map` (≤ 64 entries, LRU). Unchanged hash re-renders without touching the worker. Node sizes come from canvas `measureText` with the app font after `document.fonts.ready`, wrapped at 24ch; never `getBBox`.
- ELK options: `elk.algorithm: layered`, `elk.direction: RIGHT|DOWN`, `elk.edgeRouting: ORTHOGONAL`, `elk.layered.considerModelOrder.strategy: NODES_AND_EDGES`, `elk.hierarchyHandling: INCLUDE_CHILDREN`, `elk.spacing.nodeNode: 24`, `elk.layered.spacing.nodeNodeBetweenLayers: 40`, `elk.padding` on groups for the title band. Time-box 2 s per layout; on timeout the card shows the §3.3 adjacency list (the same text the TUI would) — never a blank card, never a spinner past 2 s.
- SVG: groups (rounded rect, title band), edges (`<path>` from sections, arrowhead marker, dashed by `style`), nodes (shape by `kind`, Tailwind token classes by `state`: `active` accent ring, `done` success, `muted` 50 % opacity, `danger` destructive), labels as SVG `<text>` lines (no `<foreignObject>`: export and strict CSP stay simple). The frame — pan/zoom, fullscreen `Dialog`, Copy PNG / Save PNG… / Save SVG… — is extracted from `Mermaid.tsx` into `components/chat/DiagramFrame.tsx` and used by both; `lib/mermaid-export.ts` becomes `lib/diagram-export.ts` with the same `standalone()`/`rasterize()`.
- Streaming (30 §4 R4): the open fence renders through `jsonrepair`; a `Graph` whose edges reference not-yet-streamed nodes renders the nodes it has and drops dangling edges until the fence closes (validation errors are for closed fences). Layout requests coalesce through the same queue as mermaid (§4.3); the last good layout stays on screen.

### 3.5 Apps (`packages/apps/src/library.ts`, `renderer.tsx`)
`defineComponent("Graph", { nodes, edges, groups?, direction?, highlight?: reactive string[], onSelect?: Action })` with zod schemas mirroring §3.1; the renderer maps `highlight` to `state: active` on those ids (a node's own `state` wins for `danger`/`done`), `onSelect(nodeId)` runs the action with `{id}` in scope. Adding a component changes `library-hash.txt` (46 §4): every committed app re-checks on next open, which is the documented behaviour, not a migration. Walkthrough shape the `apps` skill will carry:
```openui
root = Stack([title, graph, narrative, nav])
$step = 0
steps = [{ids: ["client"], text: "The client…"}, {ids: ["client","api"], text: "…"}, {ids: ["api","db"], text: "…"}]
graph = Graph(nodes, edges, null, "LR", steps[$step].ids, Action([@Set($selected, id)]))
narrative = TextContent(steps[$step].text)
nav = Buttons([Button("Back", Action([@Set($step, @Max($step - 1, 0))]), "secondary"), Button("Next", Action([@Set($step, @Min($step + 1, 2))]))])
```
Whether `steps[$step].ids` indexes as written is a fact of OpenUI Lang the step-4 spike verifies; if not, `@Filter` over a steps table is the fallback and the skill example changes, not the component.

### 3.6 Prompt and skill (30 §6.2 block, `visualize.md`)
Catalog line gains `Graph(...)`. One rule added to "Choose the block": *Boxes and arrows — architecture, dependencies, data flow, a pipeline — are a `Graph` in a json-render block; it draws on every surface and an app can highlight it. Mermaid stays for sequence, state, class and ER diagrams, and for anything that needs mermaid's own syntax.* One worked example (the "Architecture answer" of 30 §6.3 rewritten as a `Graph`). Steering is measured (30 §6.4): 10 architecture/flow prompts before and after, count `Graph` vs mermaid; the rule is tuned until ≥ 8/10 pick `Graph` for box-and-arrow and 10/10 keep mermaid for sequence.

## 4. Shape: mermaid 12 on the desktop

### 4.1 Dependency
`package.json`: `mermaid ^12.0.0`; remove `@streamdown/mermaid` (streamdown core 2.6 lists it `workspace:*` — verify `bun install` still resolves without it; if streamdown insists, keep the package installed and unused, which costs bytes on disk only). `elkjs` is reached through mermaid; `Graph.tsx` imports `elkjs/lib/elk.bundled.js` from the same resolved copy (one chunk; the build report in step 1 proves it).

### 4.2 `Mermaid.tsx`
```ts
const mermaid = () => import('mermaid').then(m => { m.default.initialize(CONFIG); return m.default })   // once, memoised
CONFIG = { startOnLoad:false, securityLevel:'strict', htmlLabels:false, suppressErrorRendering:true, maxEdges:1000,
           theme:'base', themeVariables: fromTokens(), look:'classic' /* §6 open: neo */, fontFamily: var(--font-sans) }
```
`htmlLabels` root-level only (the flowchart sub-key is deprecated in 12). `layout` unset → ELK by default; a diagram's own frontmatter may still say `layout: dagre`. First render awaits `document.fonts.ready` so ELK measures the real font.

### 4.3 Render queue (`lib/render-queue.ts`, ~60 lines, shared by mermaid and Graph)
One in-flight render, one latest trailing request, 80 ms minimum spacing; `isCurrent()` handed to the renderer so a superseded result is dropped before it touches state; the last good SVG/layout stays mounted; a failed render on an open fence is silent, on a closed fence shows the 30 §8 error card. Not a resetting debounce — a diagram that streams for ten seconds still updates every 80 ms.

### 4.4 Floors
`vite.config.ts:68` `target: 'safari17'`; `tauri.ios.conf.json` and both `IPHONEOS_DEPLOYMENT_TARGET` → `17.4`; a note in 58's platform table: Linux WebKitGTK ≥ 2.44 (Ubuntu 24.04, matching the glibc 2.39 floor), verified by rendering one flowchart on the Linux release host through the packaged app.

## 5. Verification
- `cargo test -p surface`: graph fixtures valid/invalid with the exact `SpecError` paths; `to_mermaid()` of every valid fixture parses under merman; `catalog.rs:114` `NAMES` grows to 19 and `catalog_compact()` contains the `Graph(` signature; render snapshot of the TUI adjacency list at width 40 and the ASCII graph at width 100.
- Desktop vitest: registry keys == `ComponentType` from bindings (existing test, now including `Graph`); `Graph.tsx` with the worker mocked renders the 3 valid fixtures; structural hash stable under label-only whitespace changes; queue test: 50 requests in 400 ms → ≤ 6 renders, last one wins.
- Apps: the shared fixtures under `packages/apps/test/fixtures/graph/` are the same files (symlink or copy checked by a test) and the zod schema accepts/rejects each as Rust does; the §3.5 walkthrough app passes `check.ts` and steps in the browser.
- Measured (M3 Max, release build), recorded in the as-built: elk layout of the 40-node/2-group fixture in the worker; unchanged-hash re-render; mermaid 12 first render of a 20-node flowchart with subgraphs cold (chunk fetch) and warm; main chunk size before/after (mermaid must leave it); one flowchart on iOS 17.4 simulator, Windows and the Linux host.

## 6. Open
- `look: classic` vs `neo` for mermaid 12 — decided by eye on the desktop, not by fixture corpus (Sky, 2026-09-23). `theme: base` + tokens either way.
- Expose ELK sub-algorithms on `Graph` (`layout?: layered|stress|mrtree`)? Not until a reply needs one.
- Should the apps zod schema be generated from the ts-rs `GraphProps` rather than mirrored? 46 §4 says no generation across catalogs; the parity fixtures are the guard. Revisit if a third consumer appears.
- Streamdown core's hard dependency on `@streamdown/mermaid` (vercel/streamdown #501) — if a future streamdown drops it, nothing here changes.

## 7. What we knowingly give up
Mermaid-quality curved edges and label placement on `Graph` (ELK orthogonal is plainer and more legible); click-to-open-file on nodes (a sink stays a sink); nested groups; any diagram type that is not boxes and arrows (mermaid has them); iOS 17.0–17.3 (Safari below 17.4 cannot run mermaid 12; the app already requires 17.0 and Apple's 17.4 uptake is near-total).

## 8. Amendments
- **30 §0.7** appended: *Exception, 65: `Graph` is the one component with a renderer of ours, because no library component draws a typed node/edge graph, and its layout is `elkjs` (the engine mermaid 12 bundles), not code of ours. `@streamdown/mermaid` is gone; `Mermaid.tsx` imports mermaid 12 lazily.*
- **30 §1** desktop row: `Streamdown` + `@streamdown/code`; **mermaid 12 (ELK default), lazy, strict, 80 ms render queue**; `@json-render/react` with 29 custom components.
- **30 §6.2** prompt block and `visualize.md`: `Graph(...)` in the catalog line and the rule of §3.6.
- **58** platform table: WebView floors Safari 17.4 / iOS 17.4 / WebKitGTK 2.44 / WebView2 evergreen, reason "mermaid 12 (65 §4.4)".
- **00**: no crate, no dependency-line change (elkjs/mermaid are UI packages under `crates/desktop/ui`).

## 9. Build steps

| # | Step | Done when |
|---|---|---|
| 1 | Mermaid 12: remove `@streamdown/mermaid`, lazy import, `CONFIG` of §4.2, `render-queue.ts`, floors of §4.4, `DiagramFrame.tsx` extracted | `bun run build` chunk report: `mermaid` and `elk` absent from the main chunk; a flowchart with two subgraphs renders with ELK on macOS, Windows, Linux host, iOS 17.4 sim; 50 streamed edits → ≤ 6 renders; existing mermaid vitest + accessibility tests green |
| 2 | `surface`: `GraphProps` + validation + caps + fixtures + `to_mermaid()` + TUI/flatten rendering + catalog/prompt/skill text | `cargo test -p surface` green with the §5 cases; `t -p 'draw the surface crate as a Graph'` shows an ASCII graph in the TUI at 100 cols and the adjacency list at 40; Slack `message_user` shows the list |
| 3 | Desktop `Graph.tsx` + layout worker + hash cache, registered in `registry.tsx`; bindings regenerated | the 3 fixtures render; pan/zoom/fullscreen/export parity with mermaid; §5 numbers recorded; unchanged re-render < 1 ms, 40-node layout < 100 ms warm |
| 4 | Apps `Graph` (`library.ts`, renderer, `check.ts`, regenerate `library.json`/`catalog.txt`/hash), `apps` skill example, §3.5 walkthrough app | `bun run generate` diff reviewed; walkthrough app publishes, steps through 3 states, `onSelect` reaches `@ToAssistant`; hash change noted in the as-built |
| 5 | Steering measurement (§3.6) and prompt tuning | 10/10 box-and-arrow prompts → `Graph`, 10/10 sequence prompts → mermaid, or the rule is rewritten and re-measured once |

## 10. Status
Design drafted 2026-09-23 by the orchestrator from the cloudlands-fe comparison, the streamdown and mermaid 12 recon (`facts/mermaid-12-vs-streamdown-and-theuth-webview-floors`) and Sky's rulings (`decisions/theuth-chat-diagrams-path-2026-09-23`). Not yet on main; step 1 can start the moment it is.
