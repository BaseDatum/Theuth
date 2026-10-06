# 123 — Diagram grammars: the diagram-design taxonomy as json-render components, one SVG kernel, one skill

Design numbers 120–122 are taken by sibling designs (120 inline sandboxed pages, 121 `spike/121-sck-capture`, 122 `feat/122-web-reading`); this document is 123.

Status: design, 2026-10-07 (main `ebac3603`). Extends 30 (surfaces), 46 (apps), 65 (`Graph`: the one renderer of ours, ELK in a worker, two registrations), 71 (boards: anchors on diagram units), 72 (export), 110 (Slack Block Kit). Amends 00 §10, 30 §0.7/§6.2/§10/§14, 65 §0.3/§0.4/§6, 71 §2.4/§3.3, 72 §3 (§12).

Source material: `cathrynlavery/diagram-design` (MIT, v2.6, clone `/tmp/diagram-design` at `d1376371`) — 44 visual types, 9 semantic patterns, 14 ADRs, per-type references (§2.1). Every claim about theuth cites `path:line` of the working tree at `ebac3603`; where the brief that ordered this document disagreed with the tree, the tree wins and §2.2 says so.

Ruled by Sky 2026-10-06 23:58 on three forks: (1) **taxonomy by layout grammar** — about nineteen new components plus extensions, every one of the 44 types routes to a component in the skill, not one component per type; (2) **an own small SVG chart kernel** in the desktop UI, not recharts in json-render and not openui-only; (3) **grow the existing `visualize` skill**, not a second skill. His words: "make sure we have all of these diagram types as json-render in theuth. It would be great if they could be interactive and very high quality, could also be openui surfaces as well when appropriate. There should be a skill that explains when to use which type of diagram."

Scope: nineteen new `Component` variants in `crates/surface/src/spec.rs`, optional fields on six existing ones, validation, text lowering, Slack and export rows, the desktop kernel and renderers, apps registrations, board hooks, skill and prompt, and the measurement that says whether the model picks them. Not in scope: the axonometric grammars, Wardley, motion in fences, server-side rasterisation (§11).

## 0. Decisions in one screen

1. **Grammar, not type (Sky's ruling 1).** Nineteen new catalog components — `StateMachine`, `Entities`, `Swimlane`, `GraphDelta`, `BarChart`, `LineChart`, `ScatterChart`, `RadarChart`, `Waterfall`, `Funnel`, `Gantt`, `Quadrant`, `Treemap`, `Sankey`, `Venn`, `Loop`, `Layers`, `Journey`, `Fishbone` — plus optional fields on `Graph`, `Sequence`, `Timeline`, `Tree`, `Kanban`, `Heatmap`; 38 → 57 variants in `Component` (`crates/surface/src/spec.rs:39-116`). Every one of the repo's 44 types routes to one of them (§3); a type earns a component only for a layout grammar no component has — the repo's own ADR 0002 bar. Rejected: 44 components (Nested and Tree differ by a `layout`, Medallion is a `Layers`, Deployment a `Graph` with nested groups); one `Diagram(kind, …)` variant (defeats serde's located errors and schemars' per-variant signatures).
2. **One definition, every surface (30 §0.3 holds).** Each variant is one Rust struct with serde + schemars + ts-rs derives (`spec.rs:36-38`), located `SpecError`s (`lib.rs:65-73`), arms in `render.rs`, `slack.rs`, `export.rs`, an `EXAMPLES` entry (`prompt.rs:15`), a zod mirror in `catalog.ts`, a renderer in `registry.tsx`, an apps registration in `library.ts`. None is desktop-only: every one lowers to text (§4.4). Rejected: desktop-only charts with a caveat in the skill.
3. **One SVG kernel of ours for the charts (Sky's ruling 2).** `UI/lib/diagram/` — scales, ticks, axes, bottom-strip legend, masked edge labels, orthogonal paths with r=8 elbows, the unit contract, tooltip, SVG a11y (§5.1) — shared by `BarChart`, `LineChart`, `ScatterChart`, `RadarChart`, `Waterfall`, `Funnel`, `Gantt`, `Quadrant`, `Timeline` (time scale), `Journey`. Deterministic, inline token colours so export carries them (`lib/diagram-export.ts:19,33`), the register of `Graph.tsx`/`Sequence.tsx`. Rejected: recharts in json-render (it stays the engine of the upstream OpenUI charts, `packages/apps/src/library.ts:69-79`; a second chart look, HTML in SVG, no export parity); the full d3 stack; openui-only charts (not on the TUI, not in boards).
4. **Three layout families, one worker.** (a) ELK layered in the existing worker (`lib/graph-layout.worker.ts`, `lib/graph-layout.ts:239`) for `Graph`, `StateMachine`, `Entities` (ports), `Swimlane` (ELK partitioning), `GraphDelta` (one union layout), `Tree` `nodes|org` (`org.eclipse.elk.mrtree`); (b) the chart kernel, synchronous; (c) small own algorithms — squarify, fixed Venn geometry, a circle for `Loop`, nested boxes — and **`d3-sankey`** for `Sankey`, the one new dependency (§2.4). Rejected: an own Sankey layout (ordering and crossing reduction are the whole difficulty); dagre or a second ELK for trees (65 §2.3).
5. **The unit contract is the interactivity (§5.3).** Every drawn unit is `<g data-unit={id} role="button" tabIndex={0}>` with an ≥ 18 px hit area, a tooltip, click-to-select with incident units `active` and the rest dimmed, keyboard and Esc; legends toggle series; `DiagramFrame` (pan/zoom/pinch, Copy PNG, Save PNG/SVG, fullscreen — `components/chat/DiagramFrame.tsx:16-18,172-191`) wraps every diagram component, not only `Graph`/`Sequence`/mermaid as today (`registry.tsx:396,398`). In boards `ref` lights prose ↔ unit ↔ code; in apps `highlight`/`onSelect` (65 §3.5). Rejected: motion in fences (ADR 0001/0003: static by default; walkthroughs are app `$step` state, 65 §0.5); hover-only affordances.
6. **The repo's quality rules become invariants with an enforcement point each (§5.4)** — renderer code, a vitest over the fixtures (theuth's `verify-geometry.py`), or a skill rule; never prose only (ADR 0005). Rejected: the repo's skin (Instrument Serif, Geist, warm paper, coral) — the desktop has a type system and tokens (`lib/diagram-theme.ts:5-17`, 71 §2.7); one palette in the transcript.
7. **Hard caps protect renderers; editorial budgets live in the skill.** The validator refuses what a renderer cannot draw (40 states, 24 entities, 8 lanes, 200 points, 3 Venn sets …; §4.3) with a located error the `<surface_error>` rounds carry back (`crates/agent/src/loop_.rs:2561-2584`; default 3, `crates/agent/src/lib.rs:2426`). The repo's editorial limits (9 nodes, 5 lifelines, 8 bars, 12 tasks …) are a skill table plus "above the budget, split overview and detail". Rejected: editorial limits as validator errors (a 14-task plan is a legitimate answer).
8. **Lowering is the contract on text surfaces; merman is opportunistic.** `StateMachine` and `Entities` emit `stateDiagram-v2` / `erDiagram` through `to_mermaid()` (`lib.rs:204-210`) for channels that carry mermaid; their TUI arm is a typed text form. The bake-off already shows merman 0.7 `ascii` draws flowchart and sequence only (`crates/surface/fixtures/mermaid/BAKEOFF.md`; `mermaid.rs:130-131` pins the state case), so the text form is what the TUI shows and nothing waits on merman. Rejected: a Rust layered layout for the TUI.
9. **`GraphDelta` is two synchronised panels over one layout; its ledger is derived.** The union of `before` and `after` is laid out once; each panel draws its subset so unchanged nodes keep their positions (ADR 0012); the ledger — added, removed, changed when label, kind or group differ — is computed by id, never authored, so it cannot disagree with the pictures. Rejected: an authored `changes[]` (ADR 0012: status describes a difference, not a decoration).
10. **Apps get every diagram component, appended (46 §4, 65 §0.4).** Each is a `defineReactComponent` in `library.ts` with a zod mirror, reactive `highlight` and `onSelect` as `Graph` (`library.ts:159-172`), mounted through the chat's `<NameView>` (`setGraphRenderer` generalised, `library.ts:230`, `AppHost.tsx:44`); names appended after `OptionCard` (`library.ts:99-101`; order is part of the hash). Rejected: generating the zod from ts-rs (46 §4; shared fixtures are the guard, `packages/apps/test/graph.test.tsx:21-60`).
11. **One skill, grown (Sky's ruling 3).** `crates/surface/visualize.md` (182 lines, 11,608 bytes; generated into `~/.theuth/skills/visualize.md`, 408 lines / 21,126 bytes today) gains "Pick the diagram", "Behavioural triggers", "Budgets", "Taste gate", the fence-kind decision, `### Name` sections for 57 components, six worked examples; ≈ 42–45 KB, asserted < 48 KB (§8). Rejected: a second skill (two places to say when to draw; the catalog is generated into this one, `prompt.rs:274-294`).
12. **The prompt grows per grammar, not per component.** The `## Show first, then tell` table (`prompt.rs:186-210`, 21 rows) gains ≤ 10 rows and rewrites 2; `catalog_compact()` grows 1,405 → ≈ 3,470 characters (≈ 400 → ≈ 1,000 tokens; §8.3). Steering is measured: 20 prompts whose right answer is a new grammar, before and after (30 §6.4, 65 step 5). Rejected: 57 table rows (the block is static and cache-safe, 30 §0.8).

## 1. Why

The desktop draws two typed diagrams of ours (`Graph`, `Sequence`); everything else is mermaid or an HTML list. So the model answers "what are the states of a task" with a `stateDiagram-v2` whose states cannot be pointed at from a board, highlighted from an app, or drawn in the TUI (merman declines state, `mermaid.rs:130-131`); "how is spend split by model" with a `Bars` list or prose, because no catalog component has axes; "what did this change do to the topology" with two `Graph`s the reader reconciles by eye. The diagram-design repository is a 44-type taxonomy with budgets, connector rules and selection rules written for the register theuth's replies aim at — editorial, deletion-first, one accent — and Sky asked for all of it.

What the repository is not: a renderer. It makes a coding agent hand-author SVG inside an HTML file and verifies coordinates with Python scripts (ADR 0005, 0011, 0012). theuth wants the model naming states and transitions and the renderer placing them. The borrow is the taxonomy, the budgets, the selection logic and the quality rules — not the authoring model and not the skin (§0.6).

## 2. Facts relied on

### 2.1 The diagram-design repository (`/tmp/diagram-design`, `d1376371`, MIT)

- `skills/diagram-design/SKILL.md`: 391 lines, 30,098 bytes; capped at 40,000 LF bytes by ADR 0004, which also requires the frontmatter `description` to name every type. §1: "The highest-quality move is usually deletion"; "Target density: 4/10 … Above 9 nodes, it's probably two diagrams." §2 don't-draw: lists ("table or bullets"), attribute-only before/after ("table; topology changes → Architecture delta"), one-shape diagrams ("just write the sentence"). §3: 44 rows, each linking a `references/type-*.md` (44 files); "If a 3-column table communicates the same thing, pick the table", "If two types seem useful, pick the dominant axis", "If you're past the complexity budget (§7), split into an overview + detail."
- Nine semantic patterns (`references/semantic-patterns.md`; the left column of §3.1), each routed to a nearest type; a pattern owns primitives and a tighter budget, never a layout grammar (ADR 0002; the type count moved 27 → 44 only for new layout grammars, each with its ADR).
- Budgets: universal (SKILL.md §7) 9 nodes, 12 arrows/transitions, 2 accent elements, 2 callouts; per type (`references/layout-budget.md`) the rows §8.1 reproduces (lifelines 5 with 1 fragment / `alt` 2 regions / nesting 1, lanes 5, quadrant 12, ER 8, nesting 6, tree depth 4, org 12, layers 6, venn 3, pyramid 6, radar 5/5, polar 8/1, bars 8, waterfall 8, treemap 8, line series 5, gantt 12, scatter 30, sankey 3/8/12, fishbone 6 × 3, wardley 9/12, kanban 5/12, journey 6/3, deployment 3/6/8, dependency 9/14, UML 7/8, story map 5/3/12, db schema 5/8/6, architecture delta 8/10/8). 4 px grid: node sizes 80…320, gaps 20…48, padding 8/12/16, radius 4/6/8; data-derived positions off-grid by design.
- Six mandatory connector rules (SKILL.md §6; long form `references/primitives-core.md`): (1) orthogonal only, r=8 elbows, straight only when both ends share x or y; (2) every label on an opaque mask 6–10 px off its stroke, never on the line, ≤ 14 characters; (3) no shared or stacked strokes — parallel routes ≥ 12 px apart, a bridge at a crossing; (4) distinct attach points per connector on a shared edge, ≥ 12 px apart; (5) no transit behind a non-endpoint box; (6) no label mask over a node drawn after it — `scripts/verify-geometry.py` (ADR 0005). Draw order: background → zones → arrows → labels → nodes.
- Design system (SKILL.md §5, `references/style-guide.md`): roles `paper`, `ink`, `muted`/`soft`, `rule`, `accent`/`accent-tint`, `link`; "accent goes on 1–2 elements max"; node treatments focal/backend/store/external/input/optional/security; serif title, sans 600 names, mono only for technical sublabels; legend a horizontal bottom strip; no shadows; anti-patterns (§4) incl. identical boxes, floating legend, vertical text, coral everywhere, "Reproducing Mermaid's renderer layout".
- Animation (`references/animation.md`, ADR 0001/0003): `none|reveal|step|loop`; static by default; `reveal` the only sanctioned autoplay, once; "Source is complete"; never animate coordinates, routes, `viewBox`, sizes or text. Accessible SVG (SKILL.md §12): `role="img"`, `aria-labelledby` → `<title>` (first child, ≤ 60 chars) + `<desc>` (one sentence about the content).
- Per-type rules adopted in §5.4: sequence — dashed lifelines, activation bars `w=8`, return dashed with a filled head, async open head, fragment frame with an operator tab, `alt` 2 regions, time never flows upward; state — start dot r=6, end ringed dot, `event [guard] / action`, self-loops above; ER — two-section box, PK `#`, FK `→`, mono cardinality; swimlane — hairline dividers, one owner per step; gantt — 180 px label column, 40 px rows, 24 px bars, one focal bar, today line; sankey — 3 columns, 12 px bars, filled ribbons with both control points at the midpoint, no arrowheads, one px-per-unit `k`; quadrant — one word per axis tip, items r=4, no item on an axis; architecture — zones at 2 % wash, max 3 ("More and it reads like a swimlane").
- ADRs: 0007 (28 → 38) argues each of Sankey, Fishbone, Wardley, Kanban, Journey, Deployment, Dependency graph, UML class, Story map, Database schema against its nearest type and rejects C4, RAG architecture, mindmap, use case, pie/donut, git graph, packet ("the request names a use case, not a grammar"); 0011: waterfall is a running-total grammar (totals at the floor, floating deltas, the carry), sign by fill weight not hue; 0012: architecture delta = Before · Changes · After with stable ids and derived status ("not an author-selected decoration"); 0013/0014: the axonometric types are projection grammars with reprojection verifiers — read only to refuse them (§11).

### 2.2 theuth today (`ebac3603`; `UI/` = `crates/desktop/ui/src/`)

- Catalog: `Component`, 38 variants, `crates/surface/src/spec.rs:39-116` (":33 thirty-eight"; declaration order is the catalog order, :35) — Card … Calendar as 30 §10 lists them, `Graph` :77, `Sequence` :79. Derives at :36-38; every props struct `deny_unknown_fields`. *Correction: the brief said :39-118.*
- Props this design extends: `GraphNode` :478-491 (`kind` :497-510, `state` :516-527), `GraphEdge` :533-542, `GraphGroup` :559-564 (`id`, `label` only); `SequenceStep` :604-615; `TimelineItem` :397-406 (`when` free text); `TreeItem` :741-749 (`depth` 0..6); `HeatmapProps` :867-876; `KanbanColumn` :926-931, `KanbanCard` :937-944.
- Limits and errors: `lib.rs:41-63` — `MAX_FENCE_BYTES` 256 KiB, `MAX_ELEMENTS` 256, `MAX_DEPTH` 32, `MAX_GRAPH_NODES` 120 (:49), `MAX_GRAPH_EDGES` 240 (:51), `MAX_GRAPH_GROUPS` 12 (:53), `MAX_GRAPH_LABEL_CHARS` 80 (:55), `SEQUENCE_ACTORS` 2..=12 (:57), `SEQUENCE_STEPS` 1..=40 (:59). `SpecError` :68-73; `parse()` :94-116 (strict → `jsonrepair_rs` → `validate`); `check()` :154-168; `to_mermaid()` :204-210. `validate.rs`: `CONTAINERS` :17, ragged `Heatmap` :198-213, `label()` :292-300, `anchor_ref()` :303, `graph()` :313-375, `sequence()` :379. Fixtures: 51 files in `crates/surface/fixtures/json-render/` (34 `VALID` `lib.rs:232-267`, 14 `LOCATED` :278-317).
- Text lowering: `render.rs` `element()` :339, one arm per variant — `Timeline` :547, `Graph` :586-591 (merman, else adjacency list), `Sequence` :593, `Tree` :692, `Heatmap` :786-806 (`SHADES` :787), `Kanban` :841-867. `mermaid.rs:16-51` `mermaid_lines()` returns `Ok(None)` on unsupported types/features and on every ER parse error (:32-39); `Cargo.toml:17` `merman = { version = "0.7", features = ["ascii"] }`. `fixtures/mermaid/BAKEOFF.md`: merman renders flowchart and sequence; state unsupported, class unsupported feature, ER parse error on valid syntax, pie/gantt unsupported; `xychart-beta` renders. *The brief asked step 0 to find out what merman draws; the record already answers (§0.8).*
- Slack (`slack.rs`, 110 §8): 26 of 38 variants have a Block Kit shape (:99-255), 12 return `Unsupported` (:256-267: Graph Sequence Heatmap Kanban Calendar Gauge Excerpt Progress Person FileList Tree Diff) and the fence falls back to `surface::flatten` at 60 columns (`crates/slack/src/bubble.rs:45,164-166,174-214`). `Bars`/`Sparkline` already map to a one-series `data_visualization` via `chart()` (:150-171, :307-340; ≤ 20 points, ≤ 2 charts per message). *Corrections: 26 not 21; the fallback lives in `bubble.rs`, not `channel.rs`; the chart block the brief called a later follow-up exists for one series.*
- Export: `export.rs` `print_markdown()` :18 substitutes a desktop SVG when `is_diagram()` (:68-70: `Graph | Sequence`), else `lower()` :138; `element()` :144-235 has typed arms for 20 variants and `*TypeName*` for the rest (:231-233); 72 §3 is the table.
- Correction loop: `loop_.rs` `check_fences()` :2480, `correction_wanted()` :2561-2563, `correction_message()` :2569-2584, call site :4771-4783; `SurfaceConfig` `lib.rs:2412-2419`, `correct_rounds` 3 (:2426).
- Prompt and skill: `prompt.rs` `EXAMPLES: [(&str, &str); 38]` :15-165 (zipped with the catalog by order, :277); `prompt_block()` :170-172 = `BLOCK_HEAD` :175-179 + `catalog_compact()` + `BLOCK_TAIL` :182-236 (table :186-210, 21 data rows; mermaid line :184); `visualize_skill()` :274-294; tests :377, :438-455, :458-473. `catalog.rs` `catalog_compact()` :65-72, `catalog_schema()` :75-77, `NAMES: [&str; 38]` :114. The generated skill: 408 lines / 21,126 bytes, `## Catalog` ≈ 9.7 KB (≈ 255 bytes per component); the compact line 1,405 characters (`bindings/surface-catalog-compact.txt`, `crates/rpc/src/export.rs:418-426`); the block without the catalog ≈ 7.0 KB. Skills are written on start by `crates/theuth/src/main.rs:1172-1191` (:1183). *Correction: 30 §0.8 cites `main.rs:165-179`; the writer moved.*
- Desktop: `registry.tsx` (631 lines) `defineRegistry` :631; `Timeline` :335, `Graph` :396 (`GraphView`), `Sequence` :398, `Tree` :457, `Heatmap` :521, `Kanban` :595 — HTML/Tailwind for all but the two SVG components. `catalog.ts` (148 lines) hand-written zod; `catalog.test.ts:3` imports `@/rpc/bindings/surface-catalog.json` (from `just ts-desktop`), :120-123 same names as Rust and `toHaveLength(38)`, :125-129 prop parity (*correction: the brief cited :102-122*). `Graph.tsx` (265 lines): `dropDangling` :16, `paint()` :30-41 (tints 0.1/0.12, muted 50 %), `GraphSvg` :94 (`role={onSelect ? 'group' : 'img'} aria-label="Graph"` :126 — no `<title>`/`<desc>`), `<g data-node … role="button" tabIndex=0>` :173-175, `GraphView` :231-264 in `DiagramFrame` :261. `lib/graph-layout.ts` (272 lines): `elk-api` :5, `FONT_PX` 13 :52, `LAYOUT_TIMEOUT_MS` 2000 :57, `structuralHash` :64, `LruCache(64)` :132, ELK options :163-169, the worker :239, `layout()` :259; `lib/graph-layout.worker.ts` (4 lines) only imports `elkjs/lib/elk-worker.min.js` (*correction: the brief placed the layout code in the worker file*). `Sequence.tsx` (157 lines): 176/112/76 px :16-18, `role="group"` :62, `<g data-step … role="button" tabIndex={0}>` :103-105, 18 px hit stroke :112, `SequenceView` :148-155. `DiagramFrame.tsx` (284 lines): pan/zoom/pinch :36-37, Copy PNG / Save PNG… / Save SVG… :172-191, 44 px controls on coarse pointers :202, `DiagramFrame` :243. `lib/diagram-export.ts` `standalone()` :19, `rasterize()` :33. `lib/diagram-theme.ts` `DiagramTokens` :5-17, `lightTokens()` :41. `Mermaid.tsx` strict / `htmlLabels: false` / `look: 'classic'` :20-25. `JsonRenderFence.tsx` `FenceFallback` :24-39. Pins (`package.json`): `elkjs ^0.9.3` :45, `mermaid ^12.0.0` :49, `@json-render/* ^0.20.0` :21-22, `zod ^4.3.6` :68, `vitest ^5.0.0` :82.
- Apps: `library.ts` (532 lines) `CURATED_COMPONENTS` :38-102 (upstream charts :69-79, `Graph` :98, `OptionCards`/`OptionCard` appended :99-101; "order is part of the compatibility hash" :37); `graphProps` :159-172 (`GRAPH_MAX` :149, `superRefine` :173), `setGraphRenderer` :230, the `Graph` component :302; `AppHost.tsx:44` installs `GraphView`; `@OpenUrl` http(s)-only :420-428. `@openuidev/react-ui 0.16.1` (`packages/apps/package.json:32`). Parity: `test/graph.test.tsx:21-24` reads `crates/surface/fixtures/json-render/`; :29-60 accept/reject as Rust.
- Boards: `UI/components/board/units.ts` (203 lines) `unitsOf()` :16-26 (`Graph` → nodes, `Sequence` → steps), `highlightOf()` :29, `focusBy()` :97-109 (`[data-node], [data-step]` :105), `unitOf()` :112-115, `useUnitPress()` :122, `useUnits()` :142. `crates/surface/src/anchor.rs:1-13` the anchor grammar. `crates/document/board.md` (367 lines) "One diagram per idea" :163-176 keeps mermaid `stateDiagram-v2`/`classDiagram` for "a lifecycle or class shape with nothing to point at"; `crates/document/src/tools.rs:279-296` runs `surface::check` on insert/replace.
- Channels: Signal/TUI/`message_user` take `surface::flatten`; Slack Block Kit or flatten; Telegram sends artifacts. The engine has no diagram rasteriser (resvg only inside `show`); 30 §14 keeps "Mermaid PNG to Signal/Slack" unbuilt.
- Gates (read, not guessed): `justfile:18-19` `check` = `cargo check --workspace --all-targets`; :21-22 `test` = `cargo test --workspace`; :33-34 `clippy` = `cargo clippy --workspace --all-targets -- -D warnings`; :79-80 `ts`; :83-86 `ts-desktop` (copies `bindings/` into `crates/desktop/ui/src/rpc/bindings`); :212-213 `desktop-check` = `cd crates/desktop/ui && bun run check && bun run test && bun run build`. `crates/desktop/ui/package.json:7-11`: `"build": "tsc -b && vite build"`, `"test": "vitest run"`, `"check": "tsc -b --noEmit"`. `packages/apps/package.json:21-26`: `"generate": "bun run scripts/generate.ts"`, `"check": "tsc --noEmit"`, `"test": "bun test"`.

### 2.3 elkjs 0.9.3 — what the shipped layout engine offers

Verified in `crates/desktop/ui/node_modules/elkjs/lib/elk-worker.js`: `org.eclipse.elk.layered` supports `org.eclipse.elk.partitioning.activate` / `.partition` (:48667-48668 — same partition, same layer band: a swimlane when the partition is the lane), `org.eclipse.elk.spacing.edgeEdge` (:48561), `spacing.portPort` (:48570), `layered.mergeEdges` default `false` (:48123), `portConstraints` `FIXED_SIDE` (:31797), `layered.cycleBreaking.strategy` (:48133 — cycles broken, not refused); `org.eclipse.elk.mrtree` is registered (:61005-61013). From 65 §2.3 and the 2026-09-23 build: compound nodes, `ORTHOGONAL` routing with bend points, `considerModelOrder`, edge coordinates relative to the LCA container, `elk.bundled.js` cannot run inside a Web Worker (hence `elk-api` + `elk-worker.min.js`), ~1.4 MB minified, tens of milliseconds under ~100 nodes.

### 2.4 Small libraries considered

| Need | Considered | Decision |
|---|---|---|
| Sankey layout | `d3-sankey` 0.12 (ISC; pure layout over `{nodes, links}`, no DOM; `d3-array`/`d3-shape` deps; throws on a circular link) | **Adopt** — the one new dependency; an own layout rejected (ordering/crossing reduction is the hard part); size from step 3's chunk report |
| Scales, ticks | `d3-scale` + `d3-array` | reject — ~150 lines of ours; d3-scale drags d3-time/format/interpolate |
| Treemap | `d3-hierarchy` | reject — squarify is ~80 lines |
| Venn | `venn.js` (area-proportional) | reject — 2 or 3 sets, fixed geometry |
| Charts | recharts (already in apps), visx, nivo | reject for json-render (ruling 2): HTML-in-SVG, animation by default, no `<text>`-only export |
| Trees | dagre, `d3-hierarchy` `tree()` | reject — `org.eclipse.elk.mrtree` ships already |

## 3. Taxonomy: the 44 types → theuth components

One row per repository type (SKILL.md §3 order). "Carries the type" names the prop that makes the component draw that type and not its neighbour. Caps are §4.3's; budgets are the skill's (§8).

| # | Repo type | theuth component | What carries the type |
|---|---|---|---|
| 1 | Architecture | `Graph` | `groups` as tiers/zones (`kind: zone`), `nodes[].tag` for the kind, `sub` for the technical sublabel, `icon` |
| 2 | Architecture delta | `GraphDelta` | `before`/`after` with shared ids; derived ledger (§0.9) |
| 3 | IT current-state | `Graph` `TB` grouped by phase/department, or `Swimlane` with lanes = departments | `groups` + `state: muted` (legacy) / `danger` (end-of-life); or `lanes` |
| 4 | Flowchart | `Graph` `direction: TB` | `kind: circle` start/end, `kind: diamond` decisions, yes/no `edges[].label` |
| 5 | Sequence | `Sequence` | new `fragments` (`alt|opt|loop|par`), `steps[].note` |
| 6 | State machine | `StateMachine` | `states[].kind: initial|final|choice`, `transitions[].guard` |
| 7 | ER / data model | `Entities` `detail: model` | `fields[].key`, `relations[].kind: one-one|one-many|many-many` (crow's feet) |
| 8 | Timeline | `Timeline` `scale: time` | ISO `when` positions the item; `until` spans |
| 9 | Swimlane | `Swimlane` | `lanes`, `steps[].lane`; cross-lane edges are the handoffs |
| 10 | Quadrant (+ consultant 2×2) | `Quadrant` | `items[].x/y` in 0..1; `quadrants` names the four cells (the consultant variant: four named cells, no items) |
| 11 | Radar / spider | `RadarChart` | `axes`, `series` |
| 12 | Polar chart | `RadarChart` `variant: polar` | one series as radial bars over cyclic categories |
| 13 | Loop / flywheel | `Loop` | `steps` on a circle, `hub` centred |
| 14 | Nested | `Tree` `layout: nested` | the same `depth` data drawn as containment |
| 15 | Tree | `Tree` `layout: nodes` | parent → children boxes (`mrtree`) |
| 16 | Org chart | `Tree` `layout: org` | top-down, `detail` as the role line |
| 17 | Layer stack | `Layers` | `layers[]`, `items` inside a band |
| 18 | Exploded axonometric | — **refused** (§11) | projection grammar; `show` an SVG/PNG made elsewhere |
| 19 | Axonometric plan | — **refused** (§11) | site/floor grammar; same fallback |
| 20 | Venn | `Venn` | `sets` (2 or 3), `overlaps` labelled by set ids |
| 21 | Pyramid / funnel | `Funnel` `variant: funnel|pyramid` | `stages[].value` for drop-off; no values for a ranked hierarchy |
| 22 | Bar (+ dumbbell) | `BarChart` | `variant: grouped|stacked|dumbbell|horizontal`; `Bars` stays for the quick list (Slack maps it natively, `slack.rs:150-154`) |
| 23 | Waterfall | `Waterfall` | `steps[].kind: delta|total|subtotal`; running total computed |
| 24 | Treemap (marimekko refused) | `Treemap` | `items[].value`, `group` headers; a marimekko is `BarChart` stacked |
| 25 | Heatmap | `Heatmap` | new `palette: diverging` for signed cells |
| 26 | Line (+ slopegraph, bump; ridgeline/streamgraph refused) | `LineChart` | `variant: line|area|slope|bump|step` |
| 27 | Gantt | `Gantt` | `tasks[].start/end`, `group` phase bands, `depends`, `today` |
| 28 | Scatter (+ bubble, beeswarm) | `ScatterChart` | `variant: scatter|bubble|beeswarm`; `size`, `groups` |
| 29 | High-level | `Graph` `TB` | one group per tier |
| 30 | Process | `Swimlane`, or `Steps` when there is one actor | `steps[].kind: data` for the handed-over artifact |
| 31 | Medallion | `Layers` | one layer per tier, `note` for the quality level |
| 32 | Data flow | `Swimlane` (role lanes) or `Graph` `LR` | `lanes` = roles, `kind: data` steps |
| 33 | DP integration | `Graph` `LR`, three groups | `groups` sources → core → consumers |
| 34 | DP security matrix | `Table` (✓ / — / RW cells; a skill convention) or `Heatmap` | nothing new |
| 35 | Sankey | `Sankey` | `flows[].value` = band width; `nodes[].stage` fixes the column |
| 36 | Fishbone | `Fishbone` | `effect`, `causes[].category/items` |
| 37 | Wardley map | **deferred** (§11): `ScatterChart` with `groups` = evolution stage + prose | later a `Wardley` or `stages`+`links` on `ScatterChart` |
| 38 | Kanban | `Kanban` | new `columns[].limit` (WIP), `cards[].state: blocked` |
| 39 | User journey | `Journey` | `stages`, `rows`, `mood` curve, `pains` |
| 40 | Deployment | `Graph` | `groups[].parent` (zone › host › pod), `sub` for `:8080 ×3`, `tag` for the artifact |
| 41 | Dependency graph | `Graph` | ELK breaks cycles (§2.3); nothing new |
| 42 | UML class | `Entities` `detail: class` | `methods`, `relations[].kind: inherits|implements|composes|aggregates|depends` |
| 43 | Story map | `Kanban` | new `rows` (releases), `cards[].row`, `cut` (release line); columns = backbone |
| 44 | Database schema | `Entities` `detail: schema` | `fields[].type/key/nullable` in mono columns; `from_field`/`to_field` FKs |

Count: 19 new components, 6 extended, 2 refused, 1 deferred, 3 answered by existing components (`Table`, `Steps`, `Bars`).

### 3.1 The nine semantic patterns → recipes (skill section "Behavioural triggers")

| Pattern (repo) | theuth recipe |
|---|---|
| Fan-in queue / bottleneck | `Graph` `LR`: sources → one `kind: hexagon` queue node with `sub` capacity (`8/hour`, `3 slots`) → the service → two outcome edges (`active` admitted, `muted` deferred) |
| Stage framework with semantic slots | `Table` stages × slots with explicit `—` cells; `Swimlane` only when rows are owners |
| Unstructured input → structured artifact | `Graph` `LR`: source → named transformation → artifact; provenance as ≤ 2 `kind: note` nodes |
| Paired policy-evaluation traces | `Table` rule × trace with `PASS` / `FAIL` / `SKIPPED` / `NOT REACHED` + an `Alert` naming the first divergence |
| Secure paved road | `Graph` with `groups[].kind: boundary` (dashed); permitted edges `active`, forbidden `danger` with `arrow: none` stopping at the boundary |
| Governance / control catalog | `Layers`: one layer per enforcement surface, `items` = controls, `note` = actor and timing |
| Compensating security layers | `Layers` top-down, `note` = residual risk; `state: danger` on the layer with the gap |
| Traceable block decomposition | `Tree` `layout: nodes` with `ref` on every node — theuth's board anchor model (71 §2.3) |
| Lifecycle phase map | `StateMachine`: 4–5 phases `LR`, wait/retry states `muted`, terminal states `kind: final` |

## 4. Shape: the wire

Conventions for every diagram component (new and extended): `title?` (≤ 80); on every unit that can be pointed at, `id?` (unique within the component; required where edges reference it), `state?: default|active|done|muted|danger` (the `GraphState` vocabulary, `spec.rs:516-527`), `ref?` (the 71 §2.3 anchor grammar, `validate.rs:303`). `deny_unknown_fields` everywhere; labels ≤ 80 through `label()` (`validate.rs:292-300`).

### 4.1 Extensions to existing components (all optional; nothing existing changes meaning)

```
Graph       += groups[].parent?: id (nested zones, depth ≤ 3; a cycle or depth 4 is a located error)
               groups[].kind?: zone|boundary|cluster        // boundary draws dashed
               nodes[].sub?: string (≤ 40, mono)             // port, version, capacity
               nodes[].tag?: string (≤ 12, rectangular rx 2) // SERVICE, DB, QUEUE
               nodes[].icon?: GraphIcon                     // closed enum, 24 names (§4.2 note)
               nodes[].kind += hexagon | circle | note
               edges[].state?: GraphState; edges[].arrow?: forward|both|none
               legend?: [{label, state?|style?|kind?}] (≤ 5)
               title?
Sequence    += fragments?: [{kind: alt|opt|loop|par, from, to, label?, else?}] (≤ 2; from/to = step indices, from ≤ to, no nesting)
               steps[].note?: string (≤ 80; ≤ 2 notes per sequence)      steps[].state?: GraphState
               (activations are derived: a call opens, the matching return closes)
Timeline    += scale?: list|time; items[].until?; items[].id?; items[].ref?; title?
               (scale=time: every `when` (and `until`) must be an ISO date or datetime — located error otherwise)
Tree        += layout?: list|nodes|nested|org; items[].id?; items[].state?; items[].ref?; title?
Kanban      += columns[].limit?: integer (WIP; a column over its limit is marked)
               cards[].state?: default|blocked|done; cards[].id?; cards[].ref?
               rows?: [string]; cards[].row?: index; cut?: index (story map: the release line after row `cut`)
               title?
Heatmap     += palette?: sequential|diverging; title?
```
`GraphIcon`: server database cloud user users globe lock shield key queue cpu box package file folder terminal laptop phone mail clock settings code git-branch bot browser — a closed enum drawn from `lucide-react` (already a UI dependency); the TUI prints nothing for it. Caps unchanged.

### 4.2 The nineteen new components

Rust-ish, as 65 §3.1 wrote `Graph`; doc comments become the schemars descriptions the skill prints. `State = GraphState`.

```
StateMachine(states: [State_], transitions: [Transition], direction?: LR|TB, title?)
  State_     { id, label, kind?: initial|final|state|choice, sub?, state?, ref? }   // ≤ 1 initial
  Transition { from, to, label?, guard?, style?: solid|dashed, state? }            // label = event, guard = [cond]

Entities(entities: [Entity], relations: [Relation], detail?: model|schema|class, title?)
  Entity   { id, label, kind?: entity|table|class|interface|enum, fields: [Field], methods?: [Method], state?, ref? }
  Field    { name, type?, key?: pk|fk|uk, nullable?: bool, note? }
  Method   { name, sig? }
  Relation { from, to, kind: one-one|one-many|many-many|inherits|implements|composes|aggregates|depends|references,
             label?, from_field?, to_field? }                                       // *_field ∈ that entity's fields

Swimlane(lanes: [{id, label}], steps: [LaneStep], edges: [LaneEdge], direction?: LR|TB, title?)
  LaneStep { id, lane, label, kind?: step|decision|start|end|data, sub?, state?, ref? }
  LaneEdge { from, to, label?, style?: solid|dashed }

Gantt(tasks: [Task], today?: date, title?)
  Task { id, label, start: date, end: date, group?, state?, progress?: 0..1, depends?: [id], ref? }

Layers(layers: [Layer], direction?: top-down|bottom-up, title?)
  Layer { label, items?: [string], note?, state?, ref? }

Quadrant(xLabel, yLabel, items: [QItem], quadrants?: [tl, tr, bl, br], xTicks?: [low, high], yTicks?: [low, high], title?)
  QItem { id?, label, x: 0..1, y: 0..1, size?, state?, ref? }

BarChart(labels: [string], series: [Series], variant?: grouped|stacked|dumbbell|horizontal, xLabel?, yLabel?, unit?, title?)
LineChart(labels: [string], series: [Series], variant?: line|area|slope|bump|step, xLabel?, yLabel?, unit?, title?)
  Series { name, values: [number], state? }          // values.len() == labels.len(); dumbbell/slope need exactly 2

ScatterChart(points: [Pt], xLabel, yLabel, groups?: [{id, label}], variant?: scatter|bubble|beeswarm, title?)
  Pt { x, y, label?, size?, group?, state?, ref? }   // beeswarm ignores y

RadarChart(axes: [string], series: [Series], max?, variant?: radar|polar, title?)   // polar: exactly 1 series, radial bars

Funnel(stages: [Stage], variant?: funnel|pyramid, unit?, title?)
  Stage { label, value?, note?, state? }

Treemap(items: [Cell], title?)
  Cell { label, value: > 0, group?, state?, ref? }   // groups = nested rectangles with a header

Sankey(nodes: [SNode], flows: [Flow], unit?, title?)
  SNode { id, label, stage?: integer, state? }       // stage fixes the column, else derived
  Flow  { from, to, value: > 0, label?, state? }     // from ≠ to; no cycles (§4.3)

Waterfall(steps: [WStep], unit?, title?)
  WStep { label, value, kind?: delta|total|subtotal, state? }   // total = absolute level; the carry is computed

Venn(sets: [{id, label, size?}], overlaps?: [{sets: [id], label}], title?)   // exactly 2 or 3 sets; fixed geometry

Loop(steps: [LStep], hub?, title?)
  LStep { label, note?, state?, ref? }               // 2..8, clockwise from the top

Journey(stages: [string], rows: [JRow], mood?: [number], pains?: [{stage: index, text}], title?)
  JRow { label, cells: [string] }                    // cells.len() == stages.len(); mood one per stage, −1..1

Fishbone(effect, causes: [{category, items: [string]}], title?)

GraphDelta(before: GraphSide, after: GraphSide, direction?: LR|TB, title?)
  GraphSide { nodes: [GraphNode], edges: [GraphEdge], groups?: [GraphGroup] }   // ids are the identity across sides
```
Compact signatures follow from `catalog_compact()` (`catalog.rs:65-72`), e.g. `StateMachine(states[{id, label, kind?, ref?, state?, sub?}], transitions[{from, to, guard?, label?, state?, style?}], direction?, title?)`. Appendix A has one minimal element per component.

### 4.3 Validation: hard caps and located errors (`validate.rs`; constants beside `MAX_GRAPH_*` in `lib.rs`)

| Component | Caps | Structural checks (each a `SpecError` at the field's JSON Pointer) |
|---|---|---|
| StateMachine | 40 states, 80 transitions | from/to ∈ states; ≤ 1 `initial`; unique ids; `guard`, `sub` ≤ 40 |
| Entities | 24 entities, 32 fields each (renderer shows 12 + "n more"), 48 relations | from/to ∈ entities; `from_field`/`to_field` ∈ that entity's fields; `methods` only on `class|interface`; unique ids and field names |
| Swimlane | 8 lanes, 40 steps, 80 edges | `step.lane` ∈ lanes; edge from/to ∈ steps |
| Gantt | 60 tasks | ISO date or RFC 3339; `end ≥ start`; `depends` ∈ tasks, no self; `progress` 0..=1 |
| Layers | 12 layers × 12 items | — |
| Quadrant | 40 items | `x`,`y` ∈ 0..=1; `quadrants` exactly 4; `xTicks`/`yTicks` exactly 2 |
| BarChart / LineChart | 8 series × 40 labels (Line: 60) | `values.len() == labels.len()`; `dumbbell`/`slope` exactly 2 series / 2 labels; `bump` values integer ranks ≥ 1 |
| ScatterChart | 200 points, 8 groups | `bubble` needs `size` on every point; `group` ∈ groups |
| RadarChart | 3..=8 axes, 5 series | `values.len() == axes.len()`; `polar` exactly 1 series |
| Funnel | 10 stages | values on all stages or none |
| Treemap | 60 items, 12 groups | `value > 0` |
| Sankey | 24 nodes, 48 flows | from/to ∈ nodes; `from ≠ to`; `value > 0`; no cycle (DFS in Rust naming the closing flow — d3-sankey's own rule, so TUI and desktop agree) |
| Waterfall | 20 steps | a `total` value is an absolute level; totals first/last recommended by the skill, not required |
| Venn | exactly 2 or 3 sets | overlap `sets` ⊆ ids, 2..=3 distinct, overlaps distinct |
| Loop | 2..=8 steps | — |
| Journey | 8 stages, 4 rows | `cells.len() == stages.len()`; `mood.len() == stages.len()`, each −1..=1; `pains[].stage` < stages.len() |
| Fishbone | 8 causes × 6 items | — |
| GraphDelta | `MAX_GRAPH_*` per side | each side passes `graph()` (`validate.rs:313-375`) |
| Graph (ext.) | `parent` depth ≤ 3; `legend` ≤ 5 | parent ∈ groups, no cycle; `tag` ≤ 12, `sub` ≤ 40 |
| Sequence (ext.) | ≤ 2 fragments, ≤ 2 notes | `from ≤ to` < steps.len(); fragments disjoint |
| Timeline (ext.) | — | `scale: time` → every `when`/`until` parses as a date or datetime |
| Kanban (ext.) | — | `cards[].row`, `cut` < rows.len() |

Message shapes follow the existing ones (`/elements/s/props/transitions/3/to: unknown state "x"`, `/elements/b/props/series/1/values: 5 values, 6 labels`, `/elements/k/props/flows/4: circular flow a → b → a`); tests pin paths, not wording (30 §5). Over a cap is an error, not a truncation — the model gets the round (65 §3.2).

### 4.4 Text lowering (`render.rs`, one arm per component: the TUI, Signal, `message_user`, the Slack `flatten` fallback)

| Component | TUI form (width-aware; `markdown::layout_table` where "table" is named) |
|---|---|
| StateMachine | one line per transition `● draft → review  submit [complete]`, `◉` initial, `◎` final; `to_mermaid` → `stateDiagram-v2` for channels that carry mermaid |
| Entities | one table per entity (`field  type  key`), then relation lines `orders ─┼< items  (one-many)`; `to_mermaid` → `erDiagram` (`classDiagram` for `class`) |
| Swimlane | bold lane headers, steps indented under their lane, `→` lines for cross-lane edges |
| Gantt | `label   2026-10-01 → 10-14  ▓▓▓▓░░░░` on one scale fitted to the width; `│` today column |
| Layers | one framed row per layer, items comma-joined, `note` dim |
| Quadrant | a 2×2 of cells listing their items (`tl · Do first: A, B`); axis labels as the caption |
| BarChart | `Bars`-style rows per label, one glyph run per series (`▓ ▒ ░`), legend line |
| LineChart | sparkline glyphs per series (`▁▂▃▅▇`) with first/last values; `slope` as `a  12 → 18  b`; `bump` as rank columns |
| ScatterChart | table `label  x  y [size] [group]` |
| RadarChart | table `axis  s1  s2`, `max` in the header |
| Funnel | rows `label  value  ▓▓▓▓▓` proportional |
| Treemap | `label  value  share%` descending, group headers |
| Sankey | `from → to  value` grouped by stage, node totals as headers |
| Waterfall | table `step  Δ  running total`, totals bold |
| Venn | sets with sizes, then `A ∩ B: label` |
| Loop | `1. step → 2. step → … ↺ 1.` plus the hub line |
| Journey | table stage × row; mood as a `☹ ○ ☺` row; pains as footnotes |
| Fishbone | `effect` header, one bold category per bone, items indented |
| GraphDelta | `Before: n nodes · After: m nodes`, the ledger `+ added  − removed  ~ changed (what)`, then the `after` adjacency list |
| Graph ext. | `sub` in parentheses, `tag` in brackets, nested groups as deeper indentation, `state: danger` edges as `✗→` |
| Sequence ext. | `[alt: label]` … `[else]` … `[end]` lines around the steps; notes as indented `· note` |
| Timeline ext. | `scale: time` prints the ISO date and sorts by it; `until` as `→ until` |
| Tree ext. | all four layouts print today's indented list |
| Kanban ext. | `title (3/2 ⚠)` over `limit`; `⊘` before a blocked card; rows as sub-headers, `────` after `cut` |

### 4.5 Slack, export, prompt examples

- Slack (`slack.rs`): every new diagram kind joins the `Unsupported` list (:256-267), so the fence takes `surface::flatten` at 60 columns in a code block (`bubble.rs:174-214`) — the §4.4 text. One exception, one arm in step 5: `BarChart`/`LineChart` with **one** series reuse `chart()` (:307-340) under its caps; multi-series `data_visualization` is a design-110 follow-up on the pinned slack-morphism fork (§13).
- Export (72 §3): `is_diagram()` (`export.rs:68-70`) grows to the SVG family (19 + `Graph`/`Sequence`), so a desktop SVG asset is substituted with a caption (`*StateMachine: 6 states, 9 transitions*`); without an asset each component gets a markdown arm mirroring §4.4 as GFM. The `*TypeName*` fallback (:231-233) is never the final state for a diagram component: a test asserts no diagram fixture lowers to it.
- Prompt: `EXAMPLES` 38 → 57 in catalog order (Appendix A); `prompt.rs:377` and `:458` pin them.

## 5. Desktop rendering

### 5.1 The kernel (`UI/lib/diagram/`)

| File | What |
|---|---|
| `scale.ts` | linear, band, time scales with nice ticks (1-2-5; day/week/month) — charts, Gantt, Timeline time |
| `axes.tsx` | axes as `<line>` + `<text>` ticks (mono 10 px), titles (sans 11 px), gridlines at `border` 40 % |
| `legend.tsx` | horizontal bottom strip, swatch + label per series/state; click toggles (local), `aria-pressed` |
| `label.tsx` | edge/flow label on an opaque `card` mask 6–10 px off its stroke, never on the line, ≤ 14 chars shown (full text in the tooltip) |
| `path.ts` | ELK sections → one `d` with r=8 elbows; `bridge()` hops at crossings; markers by `arrow` |
| `units.ts` | `useUnits()`: hover → tooltip, click → selection, Esc, Tab/Enter/arrows; `data-unit`; `highlight`/`onSelect` plumbing |
| `tooltip.tsx` | one portal tooltip: label, `sub`, values + unit, note, dates, guard |
| `svg-a11y.ts` | `role="img"` + `aria-labelledby` → `<title>` (first child) + `<desc>` = the first line of the §4.4 lowering, so the a11y text and the terminal agree; `role="group"` when `onSelect` is wired (as `Graph.tsx:126`) |
| `squarify.ts`, `venn.ts`, `circle.ts` | squarified treemap (~80 lines), two/three-circle geometry, equal arcs for `Loop` |
| `elk.ts` | `graph-layout.ts` generalised: `toElk`/`fromElk` for nodes, ports, partitions, compound groups; hash, LRU, 2 s `LayoutTimeout` and the worker kept (`graph-layout.ts:64,132,239-270`; `Graph` unchanged) |

### 5.2 Layout families and the per-component structure

- (a) **ELK layered** in the worker, memoised by structural hash: `StateMachine` (`initial`/`final` as 16 px dots, ELK self-loops, `choice` a diamond); `Entities` (each field a port, `FIXED_SIDE`, `spacing.portPort` 12; crow's-foot and UML markers by `kind`); `Swimlane` (`partitioning.activate`, `partition` = lane index, lane bands from the layer extents); `GraphDelta` (union laid out once, two panels at the same coordinates, the ledger strip under them); `Tree` `nodes|org` (`mrtree`).
- (b) **Chart kernel**, synchronous on the main thread: `BarChart`, `LineChart`, `ScatterChart` (beeswarm = a deterministic 1-D collision sweep), `RadarChart` (polar coordinates), `Waterfall` (running total → floating rects + carry connectors), `Funnel`, `Gantt` (time scale, 40 px rows, 24 px bars, group bands, `today` line, `depends` as orthogonal connectors), `Quadrant`, `Journey` (stage grid + mood polyline), `Timeline` `scale: time`.
- (c) **Own small algorithms**: `Treemap` (squarify, headed group rectangles), `Venn` (fixed geometry, overlap labels at the lens centroids), `Loop` (equal arcs, `hub` centred), `Layers`/`Fishbone`/`Tree` `nested` (boxes); **`d3-sankey`** for `Sankey` (`nodeAlign` by `stage`, `nodeWidth` 12, `nodePadding` 16; ribbons as filled paths, both control points at the midpoint, no arrowheads).
- Every component is `<NameSvg layout props tokens highlight? onSelect?>` — pure, snapshot-testable with a fixed layout — plus `<NameView>` wrapping worker/memo/`DiagramFrame`, as `GraphSvg`/`GraphView` (`Graph.tsx:94,231`) and `SequenceSvg`/`SequenceView` (`Sequence.tsx:34,148`). `registry.tsx` arms become `Name: ({ props, loading }) => <NameView … />`; `Timeline`/`Tree`/`Kanban`/`Heatmap` keep their HTML form for the default layout and draw SVG only for `scale: time` and `layout: nodes|org|nested`. Every SVG component sits in `DiagramFrame` (`DiagramFrame.tsx:243`), not only `Graph`/`Sequence`/mermaid as today.

### 5.3 The unit contract (this is the interactivity)

1. Every unit is `<g data-unit={id} role="button" tabIndex={0} aria-label=…>` with an ≥ 18 px hit area (a transparent stroke or padded rect, as `Sequence.tsx:112`); with the frame's padding that meets the 44 pt touch target on iOS.
2. Hover (fine pointers) and focus show the tooltip: full label, `sub`, value and unit, note, dates, guard.
3. Click or Enter/Space selects: the unit and its incident edges/flows/relations draw `active`, the rest dims to 50 % (the `muted` paint, `Graph.tsx:39`); Esc or a second press clears; selection is local state, never a model turn.
4. Tab moves through units in authored order; arrows follow incident edges (ELK family) or the axis (charts).
5. Legend entries toggle a series or a state locally; export respects the toggle.
6. `DiagramFrame`: pan, wheel/pinch zoom about the pointer, double-tap home, fullscreen, Copy PNG, Save PNG…/Save SVG… (standalone SVG with inline token colours, `diagram-export.ts:19,33`).
7. In boards `ref` lights prose ↔ unit ↔ code and the tour numbers units (§7); in apps `highlight` recolours and `onSelect` runs with the unit id as `$value` (65 §3.5).
8. No motion in fences (ADR 0001/0003); `prefers-reduced-motion` also disables the 120 ms selection transition. Walkthroughs are app `$step` state (65 §0.5).
9. Determinism: same props → same SVG (`considerModelOrder`, the structural-hash memo `graph-layout.ts:64`, no randomness in any own algorithm).

### 5.4 Quality invariants adopted from the repository

| # | Invariant | Enforced by |
|---|---|---|
| 1 | Orthogonal connectors, r=8 elbows; straight only when ends share x or y | `path.ts` over ELK `ORTHOGONAL` sections |
| 2 | Edge labels on an opaque mask 6–10 px off the stroke, never on the line, ≤ 14 chars shown | `label.tsx`; geometry vitest: mask ∩ stroke = ∅ |
| 3 | No shared or stacked strokes: `spacing.edgeEdge: 12`, `mergeEdges` stays `false`; bridge at a crossing | `elk.ts` options; `path.ts bridge()` |
| 4 | Distinct attach points on a shared side ≥ 12 px apart: ports `FIXED_SIDE`, `spacing.portPort: 12` | `elk.ts` |
| 5 | No edge segment through a non-endpoint node; no label mask over a later node | geometry vitest over every valid fixture's fixed layout (theuth's `verify-geometry.py`) |
| 6 | Draw order groups → edges → labels → nodes | each `<NameSvg>`; snapshots |
| 7 | 4 px grid for sizes, gaps, padding (`nodeNode: 24`, `nodeNodeBetweenLayers: 40`, bars 24 px, rows 40 px); data positions off-grid | kernel constants |
| 8 | `active` is the one accent; ≤ 2 `active` units per diagram | renderer + skill rule |
| 9 | Legend a horizontal bottom strip, never inside the drawing | `legend.tsx` |
| 10 | Sans 600 names (13 px `FONT_PX`), mono `sub`/`tag`/ticks/guards (11/10 px), never vertical text | `Label`, `axes.tsx`; snapshots |
| 11 | No shadows; fills only as state tints 0.10–0.12 (`Graph.tsx:33-37`); radius ≤ 8 | tokens + snapshots |
| 12 | `min-width` = viewBox width inside `overflow-x: auto` (phones scroll the diagram, not the page) | `DiagramFrame` surface |
| 13 | `role="img"` + `aria-labelledby` → `<title>` first child + `<desc>`; every unit focusable | `svg-a11y.ts`; a11y vitest |
| 14 | Sankey ribbons: filled paths, control points at the midpoint, no arrowheads, one `k`, rounding reconciled at the node | `Sankey.tsx` + d3-sankey |
| 15 | Sequence: time never flows upward; return dashed + filled head; async open head; fragment frame, `alt` ≤ 2 regions | `Sequence.tsx` |

### 5.5 Theme, streaming, mobile

- Theme: `lib/diagram-theme.ts` tokens for light/dark as inline attributes (`lightTokens()` :41 for print). Not the repository's fonts, paper or coral (71 §2.7 mapped the board onto the desktop's tokens). The repo's node treatments map onto `state`/`kind` (focal → `active`, store → `cylinder`, optional → `dashed`, security → `boundary`).
- Streaming (30 §4, 65 §3.4): an open fence renders progressively; ELK-family components drop dangling references as `dropDangling` does (`Graph.tsx:16`) and keep the last good layout; the chart kernel redraws inline; skeleton via `FenceFallback` while the turn runs (`JsonRenderFence.tsx:24-39`); 2 s time-box → the §4.4 text, never a blank card.
- Mobile/iOS: units tappable, pan/pinch from `DiagramFrame`, controls 44 px on coarse pointers (`DiagramFrame.tsx:202`); one simulator check per wave.

## 6. OpenUI apps

- Registration (46 §4, 65 §3.5): each new component is a `defineReactComponent` in `packages/apps/src/library.ts` with a zod mirror (structural rules as `superRefine`, as `graphProps` :173), reactive `highlight: string[]` and `onSelect: Action` (unit id as `$value`); names appended after `OptionCard` (:99-101) in §10's order; `setGraphRenderer` (:230) generalised to `setDiagramRenderer(name, component)` from `AppHost.tsx:44`. `bun run generate` rewrites `generated/`; the hash changes once per landed wave and re-checks committed apps on open (46 §8.3). Parity: `packages/apps/test/diagrams.test.tsx` reads the same `crates/surface/fixtures/json-render/` files (as `graph.test.tsx:21-24`).
- A diagram is an **app** when (skill rule): live data feeds the props (`Query`), a press must do something (`Action`), or the reader steps through states. The static **`openui` fence** is the middle ground when the reader flips `Tabs`/`Steps` or picks `OptionCards` locally with no live data (`OpenUIFence.tsx`; `Query`/`Mutation`/`@ToAssistant` inert there). Otherwise **json-render**.
- Four live recipes (sketches; `@OpenUrl` is http(s)-only, `AppHost.tsx:425-427`, so in-app navigation stays `@ToAssistant`):

```openui
# 1 — routines as a Gantt with Run-now
$st = Query(schedule_status)
root = Stack([Gantt(@Map($st.rows, {id: $.name, label: $.name, start: $.last_run, end: $.next_fire, state: $.running ? "active" : "default"}), @Now(), "Routines", null, Action([@Set($pick, $value)])),
              Buttons([Button("Run now", Action([@ToAssistant("fire schedule " + $pick)]))])])
# 2 — a task's lifecycle with the live state highlighted
$task = Query(task_inbox, {id: "01a1…"})
root = StateMachine(states, transitions, "LR", "Task lifecycle", [$task.state], Action([@Set($sel, $value)]))
# 3 — spend by model → session as a Sankey
$cost = Query(session_cost_by_model)
root = Sankey(@Map($cost.models, {id: $.id, label: $.name, stage: 0}) + @Map($cost.sessions, {id: $.id, label: $.title, stage: 1}),
              @Map($cost.edges, {from: $.model, to: $.session, value: $.usd}), "USD", "Spend")
# 4 — the inbox as a Kanban with a move per card
$inbox = Query(task_inbox)
root = Kanban(@Map(["todo","doing","blocked"], {title: $, limit: 5, cards: @Filter($inbox.tasks, state == $)}), null, null, "Inbox", null,
              Action([Mutation(task_move, {task: $value, state: "doing"})]))
```
Positional order follows `catalog.txt`; the `apps` skill (`crates/app/apps.md`, `app::skill()` `crates/app/src/lib.rs:54`) carries the final forms once step 4 verifies them against the parser.

## 7. Boards

- `ref?` on every unit (§4), validated by `anchor_ref`; `data-unit` is the one attribute the reader learns units through: `units.ts` `unitsOf()` (:16-26) becomes a per-renderer `units(props)` exported beside each `<NameSvg>`, so `highlightOf`, `focusBy` (:97-109, today `[data-node], [data-step]`), `unitOf` (:112-115) and `diagramStops` (:35) have one code path for 21 diagram components. `Graph`/`Sequence` move to `data-unit` in step 0 (old attributes kept one release as aliases).
- `crates/document/board.md` "One diagram per idea" (:163-176) gains one paragraph: lifecycles are a `StateMachine`, schemas an `Entities`, handoffs a `Swimlane`, plans a `Gantt`, a change's topology a `GraphDelta` — every unit with `ref`; mermaid only for un-anchored sketches. One diagram per idea stands.
- `code_impact` → `GraphDelta` is the natural "what this change did" picture: `before` = the referenced definitions as they were, `after` = changed/added ones with `state` by status (board.md's recipe :333-339 already colours by status); the derived ledger heads the diff board's Checks. `surface::check` already runs on insert/replace (`crates/document/src/tools.rs:279-296`).

## 8. The skill and the prompt (Sky's ruling 3)

### 8.1 What `crates/surface/visualize.md` gains

(a) **Pick the diagram** — §3 condensed to "what you are showing → component + the prop that carries it", 44 rows; (b) **Behavioural triggers** — §3.1's nine recipes; (c) **Budgets** — the repository's editorial limits as one table (9 nodes / 12 arrows, 5 lifelines, 5 lanes, 8 bars, 5 line series, 12 Gantt tasks, 30 points, 3 Sankey stages / 8 nodes / 12 flows, 6 bones × 3, 5 columns / 12 cards, 6 stages / 3 rows, 6 layers, 3 Venn sets, 8 entities, 12 quadrant items, 8 treemap cells, 8 waterfall bars) with "above the budget, split overview and detail"; (d) **Taste gate** — the remove test (remove a node? merge two? remove an arrow? remove a label?), "would a table or a sentence do?", one accent (`active` on ≤ 2 units), the legend covers every state used and nothing more, never a one-shape diagram; (e) **Fence kind** — json-render for a static picture → `openui` fence when the reader flips locally → `app` for live data or an action → a board for three or more parts about code; (f) generated `### Name` sections for 57 components (`prompt.rs:274-294`; `EXAMPLES` 38 → 57); (g) six worked examples: `StateMachine` lifecycle, `Entities` schema, `Swimlane` handoff, `Gantt` plan, `GraphDelta` before/after, `Sankey` spend. The "Choose the block" rows for `stateDiagram-v2` and `erDiagram`/`classDiagram` route to `StateMachine`/`Entities`; mermaid keeps `mindmap` and un-anchored sketches.

Size: today 21,126 bytes generated (11,608 template + ≈ 9.7 KB catalog). After: catalog ≈ 14.5 KB (57 × ≈ 255 bytes), routing table ≈ 5 KB, triggers ≈ 2 KB, budgets ≈ 2.5 KB, taste gate + fence kind ≈ 2 KB, six examples ≈ 7 KB → ≈ 42–45 KB. The repository caps its router at 40,000 bytes (ADR 0004) because it loads on every invocation; theuth's skill loads on demand, so the cap is discipline, not context: **cap at 48 KB**, asserted in `skill_is_generated_and_valid` (`prompt.rs:438`) — over it, move prose to this document, never cut the routing table.

### 8.2 What the `## Show first, then tell` block gains (`prompt.rs:182-236`)

Rows per grammar, ≤ 4-word labels, ≤ 10 new; two rows rewritten (:192 lifecycle → `StateMachine`; :193 schema → `Entities`); the mermaid line (:184) drops `stateDiagram-v2, classDiagram, erDiagram, gantt, timeline` and keeps `sequenceDiagram` (un-anchored sketches) and `mindmap`.

| The answer is… | Draw |
|---|---|
| states and transitions of one thing | json-render StateMachine |
| entities, tables, classes and relations | json-render Entities (detail: model / schema / class) |
| who does which step; handoffs | json-render Swimlane |
| tasks over dates; a plan with overlap | json-render Gantt; dated events: Timeline (scale: time) |
| quantities by category or over time | json-render BarChart / LineChart (Bars for a quick list) |
| two variables; position; priority | json-render ScatterChart / Quadrant |
| where a quantity goes; what adds up | json-render Sankey / Waterfall / Treemap / Funnel |
| what a change did to a topology | json-render GraphDelta |
| causes of one effect; a cycle; stacked levels | json-render Fishbone / Loop / Layers |
| an experience across stages; overlap of sets | json-render Journey / Venn |

### 8.3 Growth, measured

`catalog_compact()` today: 1,405 characters, 38 entries (≈ 400 tokens). With the §4.1 extensions (+303 characters) and the 19 signatures (+1,766 with separators) it is ≈ 3,470 characters (≈ 1,000 tokens); the ten rows add ≈ 700 characters. The block is static and cache-safe (30 §0.8); step 4 records the measured numbers. Steering is measured (30 §6.4, 65 step 5): 20 prompts whose right answer is a new grammar (five lifecycles, five schemas, five plans/handoffs, five quantities) and 10 that should stay `Graph`/`Table`/prose; count the pick before and after; target ≥ 16/20 and ≤ 1/10 regressions, else tune the rows once and re-measure.

## 9. Verification

- `crates/surface` (`cargo test -p surface`; `just test` for the workspace): per component a `valid-*.json` and a `broken-*.json` with its path in `LOCATED` (`lib.rs:278-317`); `valid_fixtures_parse_strictly_cover_the_catalog_and_round_trip` (`lib.rs:328`) keeps every variant covered; `NAMES` (`catalog.rs:114`) pinned at 57; `render.rs` snapshots at widths 60 and 100; `to_mermaid()` goldens for `StateMachine`/`Entities`; `export.rs` goldens and "no diagram fixture lowers to `*TypeName*`"; `slack.rs` goldens; `prompt.rs` tests (:377, :438, :458) incl. the 48 KB cap.
- Desktop (`just ts-desktop`, then `just desktop-check` = `cd crates/desktop/ui && bun run check && bun run test && bun run build`): `catalog.test.ts:122` pin 38 → 57 and prop parity; `<NameSvg>` snapshots with a fixed layout per fixture; the geometry vitest (§5.4 #2, #5); the a11y vitest (role, `<title>` first, `<desc>`, every `data-unit` focusable); the unit-contract vitest (select → incident `active`, Esc clears, legend toggle); dangling references dropped while `loading`.
- Apps (`cd packages/apps && bun run check && bun test`; `bun run generate` diff reviewed): the shared-fixture parity test; the four §6 recipes pass `check.ts` and step in jsdom.
- Live smoke per wave, in the as-built: desktop — a streamed fence of each component renders, selects, exports; TUI (`t -p '…'`) — the §4.4 lowering at 100 and 60 columns; one iOS-simulator tap; Slack `message_user` — a `StateMachine` as the code-block fallback, a one-series `BarChart` as a chart block.

## 10. Build steps

omp-sized waves off `main`, one worktree per worker, crate-level gates (no `cargo fmt --all` from a worker), an integration tree, `merge --no-ff`. Steps 1–3 run in parallel once step 0 lands. The **shared files** — `spec.rs` enum, `catalog.rs NAMES`, `render.rs` match, `export.rs` arms + `type_name`, `slack.rs` Unsupported list, `prompt.rs EXAMPLES`, `registry.tsx`, `catalog.ts`, `library.ts CURATED_COMPONENTS`, the three fixture dirs — follow one rule: **append only, in §0.1's order** (ELK, chart, part-of-whole families); each wave inserts at its own position, the integrator resolves adjacent-line conflicts and re-runs the counts (57 / 57 / 57).

| # | Step | Gate (done when) |
|---|---|---|
| 0 | Kernel + contract: `lib/diagram/*`, `elk.ts` from `graph-layout.ts`, `data-unit` + generic `units.ts`, `DiagramFrame` for every SVG component, the §4.1 extensions end to end, geometry/a11y/unit vitests over today's fixtures | `cargo test -p surface` green with the extension fixtures; `just desktop-check` green; boards still highlight and tour (`units.test.ts`, `tour.test.ts`); `docs/design/123-as-built-KERNEL.md` |
| 1 | ELK family: `StateMachine`, `Entities`, `Swimlane`, `GraphDelta`, `Tree` `nodes|org` — Rust, desktop, apps | fixtures valid/broken + snapshots; geometry vitest on the new fixtures; a `GraphDelta` fixture keeps identical coordinates for an unchanged node on both sides; apps parity |
| 2 | Chart family: `BarChart`, `LineChart`, `ScatterChart`, `RadarChart`, `Waterfall`, `Funnel`, `Gantt`, `Quadrant`, `Timeline` time | same gates; `Waterfall` running total recomputed in a test; `Gantt` ISO errors located |
| 3 | Part-of-whole + misc: `Treemap`, `Sankey` (`d3-sankey` added to `crates/desktop/ui/package.json`), `Venn`, `Loop`, `Layers`, `Journey`, `Fishbone`, `Kanban` rows/cut | same gates; the Sankey cycle fixture is refused in Rust and never reaches d3-sankey; d3-sankey's size in the chunk report |
| 4 | Skill + prompt + measurement: §8.1, the ten rows, `EXAMPLES` 57, the 48 KB cap, `catalog_compact()` sizes, the 20 + 10 prompts | `skill_is_generated_and_valid`; counts in `123-as-built-SKILL.md`; ≥ 16/20 and ≤ 1/10, or one re-tune |
| 5 | Closers: Slack goldens + the one-series `chart()` arm, export rows, TUI snapshots, iOS tap check, `board.md` paragraph, `apps.md` recipes verified against the parser, as-built | on the integration tree: `just check`, `just clippy`, `cargo test -p surface -p agent -p document`, `just ts-desktop` + `just desktop-check`, `packages/apps` check + test; counts 57 / 57 / 57 |

## 11. Not built / refused, and when

- **Exploded axonometric, axonometric plan** — refused: projection grammars for physical objects and sites with reprojection verifiers (ADR 0013/0014); not an answer shape this harness gives. Fallback: `show` an SVG/PNG made elsewhere.
- **Wardley map** — deferred: `ScatterChart` with `groups` = evolution stage + prose; a `Wardley` (or `stages` + `links` on `ScatterChart`) when a real answer needs the value-chain links.
- **Marimekko, ridgeline, streamgraph** — refused (rare; `BarChart` stacked, openui `AreaChart`, or prose).
- **Motion in fences** — refused (ADR 0001/0003); walkthroughs are apps.
- **Server-side PNG of diagrams for Slack/Signal/Telegram** — not here: needs layout in the engine or a connected desktop renderer; the sibling inline-pages design may supply one (30 §14 keeps the item).
- **Multi-series Slack `data_visualization`** — a design-110 follow-up (§13).
- **One component per type; recharts in json-render; a second skill** — refused by Sky's rulings (§0). **Proportional-area Venn, mindmap, pie/donut, C4 as a type, git graph** — ADR 0007's rejected list holds here too (a pie is a `Bars` or a `Treemap`).

## 12. Amendments to 00/30/65/71/72

- **00 §10** (`00-architecture.md:789`): the `surface` row gains "nineteen diagram grammars (123)"; no new crate; `d3-sankey` is a UI package.
- **30 §0.7**: the 65 exception becomes "the diagram family (65, 123) has renderers of ours over one SVG kernel; everything else stays library components". **30 §6.2**: §8.2's rows. **30 §10**: thirty-eight → fifty-seven. **30 §14**: add "multi-series Slack charts (110 follow-up)".
- **65 §0.3/§0.4**: one kernel, twenty-one renderers, each registered twice. **65 §6** "Expose ELK sub-algorithms" → answered: `mrtree` for `Tree`, partitioning for `Swimlane`, never as a `Graph` prop. **65 "Not in v1"** seams consumed: nested groups, icons, state/class/ER semantics as typed components.
- **71 §2.4**: `Sequence` gains fragments/notes; mermaid for state/class/ER → `StateMachine`/`Entities` with `ref`. **71 §3.3** and `board.md` "One diagram per idea": the §7 paragraph.
- **72 §3**: one lowering row per new component; `is_diagram()` is the SVG family. **46**: unchanged in principle.

## 13. Open questions for Sky

1. Fold `Bars` into `BarChart` `variant: horizontal` later? Recommend **keep**: Slack maps `Bars` natively (`slack.rs:150-154`).
2. Skill byte cap at **48 KB** (asserted) — or none, since the skill loads on demand?
3. `GraphDelta` in step 1 (where its union layout lives) or step 3? Recommend step 1.
4. Is the `icon` enum worth its prompt bytes? Default yes; drop if step 4 shows no uptake.
5. Multi-series Slack `data_visualization` for `BarChart`/`LineChart` as a 110 follow-up — wanted?
6. Wardley at all?

## Appendix A — one minimal JSON example per new component (`prompt.rs` `EXAMPLES` entries, catalog order)

```json
{"type": "StateMachine", "props": {"states": [{"id": "todo", "label": "Todo", "kind": "initial"}, {"id": "done", "label": "Done", "kind": "final"}], "transitions": [{"from": "todo", "to": "done", "label": "finish", "guard": "checks green"}]}}
{"type": "Entities", "props": {"entities": [{"id": "user", "label": "User", "fields": [{"name": "id", "type": "uuid", "key": "pk"}]}, {"id": "session", "label": "Session", "fields": [{"name": "user_id", "type": "uuid", "key": "fk"}]}], "relations": [{"from": "user", "to": "session", "kind": "one-many", "from_field": "id", "to_field": "user_id"}], "detail": "schema"}}
{"type": "Swimlane", "props": {"lanes": [{"id": "dev", "label": "Developer"}, {"id": "ci", "label": "CI"}], "steps": [{"id": "push", "lane": "dev", "label": "Push", "kind": "start"}, {"id": "test", "lane": "ci", "label": "Run tests"}], "edges": [{"from": "push", "to": "test"}]}}
{"type": "GraphDelta", "props": {"before": {"nodes": [{"id": "api", "label": "API"}, {"id": "db", "label": "Postgres"}], "edges": [{"from": "api", "to": "db"}]}, "after": {"nodes": [{"id": "api", "label": "API"}, {"id": "cache", "label": "Redis"}, {"id": "db", "label": "Postgres"}], "edges": [{"from": "api", "to": "cache"}, {"from": "cache", "to": "db"}]}}}
{"type": "BarChart", "props": {"labels": ["Mon", "Tue", "Wed"], "series": [{"name": "Tests", "values": [12, 18, 9]}, {"name": "Fails", "values": [1, 0, 2]}], "unit": "runs"}}
{"type": "LineChart", "props": {"labels": ["v1", "v2", "v3"], "series": [{"name": "p95 ms", "values": [410, 380, 290]}], "yLabel": "ms"}}
{"type": "ScatterChart", "props": {"points": [{"x": 2, "y": 40, "label": "api"}, {"x": 5, "y": 12, "label": "worker"}], "xLabel": "Effort", "yLabel": "Value"}}
{"type": "RadarChart", "props": {"axes": ["Speed", "Cost", "Safety", "Coverage"], "series": [{"name": "Plan A", "values": [4, 2, 5, 3]}], "max": 5}}
{"type": "Waterfall", "props": {"steps": [{"label": "Start", "value": 100, "kind": "total"}, {"label": "Hires", "value": 12}, {"label": "Leavers", "value": -5}, {"label": "End", "value": 107, "kind": "total"}], "unit": "people"}}
{"type": "Funnel", "props": {"stages": [{"label": "Visited", "value": 1200}, {"label": "Signed up", "value": 300}, {"label": "Paid", "value": 45}]}}
{"type": "Gantt", "props": {"tasks": [{"id": "a", "label": "Kernel", "start": "2026-10-08", "end": "2026-10-10"}, {"id": "b", "label": "Charts", "start": "2026-10-10", "end": "2026-10-14", "depends": ["a"], "state": "active"}], "today": "2026-10-11"}}
{"type": "Quadrant", "props": {"xLabel": "Effort", "yLabel": "Impact", "items": [{"label": "Cache", "x": 0.2, "y": 0.8, "state": "active"}, {"label": "Rewrite", "x": 0.9, "y": 0.7}]}}
{"type": "Treemap", "props": {"items": [{"label": "agent", "value": 362, "group": "crates"}, {"label": "ui", "value": 210, "group": "desktop"}]}}
{"type": "Sankey", "props": {"nodes": [{"id": "opus", "label": "Opus", "stage": 0}, {"id": "s1", "label": "Session 1", "stage": 1}], "flows": [{"from": "opus", "to": "s1", "value": 12.5}], "unit": "USD"}}
{"type": "Venn", "props": {"sets": [{"id": "a", "label": "Has tests"}, {"id": "b", "label": "Has docs"}], "overlaps": [{"sets": ["a", "b"], "label": "Release-ready"}]}}
{"type": "Loop", "props": {"steps": [{"label": "Measure"}, {"label": "Decide"}, {"label": "Build"}, {"label": "Ship"}], "hub": "Learning"}}
{"type": "Layers", "props": {"layers": [{"label": "Apps", "items": ["desktop", "TUI"]}, {"label": "Engine", "items": ["agent", "tools"]}, {"label": "Store", "items": ["sqlite"]}]}}
{"type": "Journey", "props": {"stages": ["Install", "First chat"], "rows": [{"label": "Does", "cells": ["downloads", "asks"]}], "mood": [0.2, 0.8], "pains": [{"stage": 0, "text": "notarisation wait"}]}}
{"type": "Fishbone", "props": {"effect": "Slow replies", "causes": [{"category": "Model", "items": ["long context"]}, {"category": "Tools", "items": ["cold ELK"]}]}}
```
Each validates under §4.3; each is also the `### Name` example the skill prints.
