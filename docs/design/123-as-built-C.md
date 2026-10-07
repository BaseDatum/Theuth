# 123 — as built, C (frozen wire, shared seams, kernel, stubs)

Branch `feat/123-diagram-grammars`: `08f4ecce` (Rust wire), `2ab95414` (desktop + apps), then this note. Bindings are gitignored and generated here — **copy** `bindings/` and `crates/desktop/ui/src/rpc/bindings/` from `/Users/Sky.Moore/Develop/theuth-123`; never run `just ts-desktop`.

## Frozen wire
`Component` 38 → 57 after `Calendar`; regions `// ---- 123 ELK family (W1) ----` / `chart family (W2)` / `part-of-whole + misc (W3)` mark every shared seam: StateMachine, Entities, Swimlane, GraphDelta · BarChart, LineChart, ScatterChart, RadarChart, Waterfall, Funnel, Gantt, Quadrant · Treemap, Sankey, Venn, Loop, Layers, Journey, Fishbone. Props per §4.2 (`xLabel`/`xTicks` camelCase, `from_field` snake), §4.1 extensions. Caps: `pub const` in `lib.rs` after `MAX_EXCERPT_BYTES`.

## Built where
- `crates/surface`: `spec.rs`; `validate.rs` (every §4.3 rule, `graph_side` shared by Graph/GraphDelta, `iso_datetime`, 7 unit tests); `lib.rs` (`VALID` 59, `LOCATED` 61); `render.rs` `placeholder()` arms `*Name: n units*`; `export.rs` `type_name`; `slack.rs` Unsupported; `catalog.rs` 57; `prompt.rs` EXAMPLES 57 (Appendix A verbatim, all valid); `graph.rs` shapes for hexagon/circle/note. Fixtures: 25 `valid-*` (19 + 6 extension), 47 `broken-*`, 50 placeholder goldens.
- `crates/desktop/ui/src`: `lib/diagram/{types,scale,path,units,a11y,elk}.ts`, `{axes,legend,label,tooltip}.tsx`, `kernel.test.ts`, **`README.md` = every frozen signature**; `graph-layout.ts` a thin layer over `elk.ts` (same exports); `diagram-theme.ts` `series: [6]` (+ `doc-export.ts` recolours them); `surface/catalog.ts` 57 mirrors, `catalog.test.ts` 57; `registry.tsx` (19 arms; Tree `layout≠list` → `TreeDiagramView`, Timeline `scale: time` → `TimelineAxisView`); `surface/Tag.tsx` (Badge body out of registry); `diagrams/_stub.tsx`, 19 `diagrams/<Name>.tsx` stubs, `TreeDiagram.tsx`, `TimelineAxis.tsx`, `Kanban.tsx`/`Heatmap.tsx` (moved verbatim), `stubs.test.ts`; `surface/diagram-hosts.ts` `registerDiagramHosts()` ← `AppHost.tsx`, `OpenUIFence.tsx`; `board/units.ts` matches `data-unit` first, `data-node`/`data-step` as aliases.
- `packages/apps`: `src/diagrams.ts` (zod mirrors + `superRefine` at the crate's paths/messages, `DIAGRAMS`, `diagramText`); `src/library.ts` (15 names after `OptionCard`, `defineDiagram()`, `setDiagramRenderer`/`diagramRenderer`, `setGraphRenderer` alias, `DiagramHostProps<P>`); `index.ts`; `test/diagrams.test.tsx`; `graph.test.tsx` signature pin, `library.test.ts` reactive list; `test/fixtures/graph/graph-architecture.json` re-synced (drifted on main); `generated/*` regenerated.

## Deviations
1. **Apps register 15, not 19**: upstream OpenUI already has `BarChart`/`LineChart`/`ScatterChart`/`RadarChart` (recharts) and the library is keyed by name, upstream first. Closer/Sky: keep (design §0.3) or add `Surface*` variants.
2. Graph apps schema appends `legend`, `title` AFTER `highlight`, `onSelect` so positional `Graph(nodes, edges, groups, direction, highlight, onSelect)` (`apps.md:173`) keeps working; the 15 new ones: catalog order, then `highlight`, `onSelect`.
3. Series palette added to `DiagramTokens` (§5.5 silent) — W2 needs it and does not own `diagram-theme.ts`.
4. `a11y.ts` not `svg-a11y.ts`; `useUnits` arrows walk `ids` in authored order (incident following: W1).
5. Own ISO parser, no date crate; offsets validated not applied (`ponytail:` comment).
6. `title` ≤ 80 everywhere; `sub`/`guard` ≤ 40; `tag` ≤ 12. Sequence `else` ∈ `from+1..=to`, `alt` only; overlapping fragments error at the later one.
7. Desktop `parse.ts` structural checks not extended to the 19 (Rust `check()` is authoritative). `renderer.tsx` untouched (host resolved per component).

## Gates
`cargo test -p surface`: 63 passed, 0 failed · `cargo check --workspace --all-targets`: clean (no match fixes needed outside `surface`) · `just ts-desktop`: 553 files · desktop `check` 0 errors, `test` 149 files / 903 tests passed (`mobile-zoom`, `pairing` flake under full-machine load, pass alone), `build` ✓ 1m 25s · apps `check` 0 errors, `test` 138 pass / 0 fail.

## Ownership (brief 123-C)
- **W1**: `diagrams/{StateMachine,Entities,Swimlane,GraphDelta,TreeDiagram}.tsx`, `Graph.tsx` + extensions, `lib/graph-layout*.ts`, `lib/diagram/elk.ts` (additive), Rust arms + `to_mermaid()` for its four, its fixtures/goldens/tests, `packages/apps/test/` for its components.
- **W2**: `diagrams/{BarChart,LineChart,ScatterChart,RadarChart,Waterfall,Funnel,Gantt,Quadrant,TimelineAxis}.tsx`, `lib/diagram/chart/*` (new; no kernel export changes), Rust arms for its eight + Timeline time, fixtures/tests.
- **W3**: `diagrams/{Treemap,Sankey,Venn,Loop,Layers,Journey,Fishbone,Kanban,Heatmap}.tsx`, `Sequence.tsx`, `d3-sankey` dep, Rust arms for its seven + Sequence/Kanban/Heatmap lowering, fixtures/tests.
- **W4**: `visualize.md`, `prompt.rs` (rows + assembly, NOT `EXAMPLES`), `board.md`, `apps.md`, `123-as-built-SKILL.md`, `scripts/`.
- **Closer only**: everything else C listed above (`spec.rs`, `validate.rs`, `catalog.rs`, `lib.rs`, `catalog.ts`, `registry.tsx`, `diagram-hosts.ts`, `Tag.tsx`, `library.ts`, `diagrams.ts`, `AppHost.tsx`, `OpenUIFence.tsx`, `lib/diagram/*` bar the two exceptions, `board/units.ts`, `_stub.tsx`).

## Open items
- W1/W3: add `data-unit={id}` beside `data-node` (`Graph.tsx`) / `data-step` (`Sequence.tsx`); use `useUnits`/`svgA11y`.
- W1–W3: replace your stubs (keep `<Name>View` + `DiagramHostProps`) and `render.rs` `placeholder(…)` arms; regenerate goldens (`UPDATE_GOLDEN=1 cargo test -p surface valid_fixtures_match_golden_files`); add `export.rs` arms; `stubs.test.ts` checks `data-stub` + a11y — tell the closer when yours drops `data-stub`.
- W2: `TimelineAxis.tsx`; colours from `tokens.series`. W3: `Kanban.tsx` limit/state/rows/cut/title, `Heatmap.tsx` diverging/title.
- W4: the prompt table still routes lifecycles/schemas to mermaid.
- Closer: deviations 1, 7; delete `_stub.tsx` when unused; Slack one-series `chart()` arm; `export.rs` `is_diagram()` + captions + the "no `*TypeName*`" test; `bun run generate` after any `library.ts` change.
