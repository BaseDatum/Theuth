# 123 — as built, W3 (part-of-whole + misc, Sequence/Kanban/Heatmap extensions)

Branch `feat/123-w3-shapes`: `b0121f0b` (Rust), `438c64d3` (desktop + apps), then this note.

## Built where
- `crates/surface/src/render.rs`: W3 arms + helpers after `placeholder()` — `count_of`/`header` (first line `Name: title — n units`, the TS `describe()` twin, so `<desc>` = line 1), `num`, `fit`, `state_glyph`, `treemap_lines` (groups by total, cells descending, `share%`, labels shrink to width), `sankey_lines` (+ `sankey_stages`: authored `stage` else longest-path depth; node total = max(in, out) as d3), `venn_lines`, `loop_lines` (chain packed to width, `⟳ hub`, `n. · note`), `layers_lines` (one framed row per layer, `bottom-up` reversed, note dim), `journey_lines` (table + `☹ ○ ☺` mood row when it fits, else stage sections; `▲ stage: pain` footnotes), `fishbone_lines`, `kanban_lines`/`kanban_card` (`title (n/limit ⚠)`, `⊘`/`✓`, `▸ row` sub-headers across aligned columns, `────` after `cut`, stacked per row when too narrow), Heatmap arm: `diverging` shades by |v| about 0, signed values, `title`. `sequence.rs::steps_lines`: `[alt: label]` / `[else]` / `[end]` dim lines, framed steps indented 2, `· note`, state glyphs (`✗ ● ◉ ○`).
- `crates/surface/src/export.rs`: W3 region before the `*TypeName*` fallthrough — Kanban, Heatmap, the seven (GFM tables / lists mirroring §4.4), `sequence()` extended (`*[alt: …]*`, notes, glyphs). `is_diagram()`/`caption()` untouched (closer).
- Goldens: 20 files regenerated (`valid-{treemap,sankey,venn,loop,layers,journey,fishbone,kanban-rows,heatmap-diverging,sequence-fragments}-w{60,100}.txt`); no other golden changed. Tests: 10 new (`render.rs` 8, `sequence.rs` 1, `export.rs` 3 incl. "no W3 fixture lowers to `*TypeName*`" and the Sankey cycle/self-flow refusal).
- `crates/desktop/ui/src/components/chat/surface/diagrams/`: `Treemap.tsx` (own `squarify()`, headed group rectangles, labels only ≥ 72×36, tooltip carries the rest), `Sankey.tsx` (d3-sankey, `nodeAlign` = authored stage when every node has one, labels outside bands, own DFS drops cycle-closing / dangling / self flows before d3 sees them), `Venn.tsx` (fixed geometry, lens centroids), `Loop.tsx` (circle, arc arrows — "the one place curves are right" in the header comment), `Layers.tsx`, `Journey.tsx`, `Fishbone.tsx`, `Kanban.tsx` (HTML; `n/limit ⚠`, `⊘`/`✓`, CSS-grid swimlane rows, `data-cut` separator, `data-unit` cards), `Heatmap.tsx` (HTML; diverging two-hue `color-mix`, legend, `data-unit` cells). `Sequence.tsx`: `useUnits`/`svgA11y`/`DiagramTooltip`, `data-unit={i}` beside `data-step`, derived `activations()`, fragments (`data-fragment`, tab, `[else]` dashed divider), notes on card masks, state colours (new `danger`/`success` markers). Each file: pure `<NameSvg>` over an exported pure `nameLayout()`, `<NameView>` in `DiagramFrame`, `nameDesc()`.
- Tests beside each file (`*.test.ts`, vitest): snapshot (`__snapshots__/`), unit contract + a11y, highlight, geometry/layout, streaming tolerance, fence mount.
- `package.json`/`bun.lock`: `d3-sankey ^0.12.3`, `@types/d3-sankey ^0.12.5`. Chunk report: `dist/assets/sankeyLinkHorizontal-*.js` 8.26 kB (2.76 kB gzip) — d3-sankey + the d3-array/d3-shape/d3-path bits it pulls; 492 KB of ESM on disk tree-shakes to it.
- `packages/apps/test/{treemap,sankey,venn,loop,layers,journey,fishbone}.test.tsx`: zod parity (valid accepted, `broken-<name>-*` rejected), `AppRenderer` mount with a test host receiving wire props + `highlight`, a unit press → `@Set($sel, $value)`, text form without a host.

## Unit ids (apps `highlight`, boards)
Treemap: cell `label` (`label#i` on repeats). Sankey: node `id`; flow `from→to` (`#i` on repeats). Venn: set `id`; overlap `sets.join('∩')`. Loop/Layers/Fishbone: index string (+ `effect`). Journey: stage index string (the column is the unit). Sequence: step index string. Kanban: `card.id ?? col:card`. Heatmap: `row:col`.

## Deviations
1. `<desc>` = line 1 of the lowering holds for the seven new components. Sequence/Kanban/Heatmap keep their existing first line (title); Sequence's `<desc>` is `describe('Sequence', …)`; Kanban/Heatmap stay HTML (§5.2) — no `<svg>`, no `DiagramFrame`, aria-labels on cards/cells instead.
2. Arrow keys do nothing in Kanban/Heatmap (`focusUnit` looks for the nearest `<svg>`); Tab/Enter/Space/Esc work. `ponytail:` comments.
3. Sankey: no right label room reserved; last-column labels sit left of their node. A `stage` past the deepest flow is clamped by d3 (`nodeAlign` clamp). Loop: arcs, not orthogonal (§5.4 #1 exception, stated in-file).
4. Venn 2-set labels sit under the outer halves of the circles (48 px beside them is too narrow); still outside their circle (tested).
5. Kanban WIP count is `cards.len()` (fixture: `3/2 ⚠`), done cards included, on both surfaces.
6. Text widths in layouts are `chars × px × 0.6` (no DOM; deterministic).

## Gates
`cargo test -p surface`: 75 passed, 0 failed · desktop `bun run check`: 0 errors · scoped `vitest run src/components/chat/surface src/components/board src/lib/diagram`: 27 files, 226 passed, 11 failed — all 11 are `diagrams/stubs.test.ts` rows for the seven replaced stubs (expected; see Open items); Heatmap + `registry.test.ts` rerun 31/31 · `bun run build`: ✓ 13961 modules · apps `bun run check`: 0 errors, `bun test`: 170 pass / 0 fail (32 new).

## Open items
- Closer: `diagrams/stubs.test.ts` — drop the `STUBS` rows for Treemap, Sankey, Venn, Loop, Layers, Journey, Fishbone (they no longer carry `data-stub`); the W3 views are covered by their own `*.test.ts`. Delete `_stub.tsx` once W1/W2 land too.
- Closer: `export.rs` `is_diagram()` + `caption()` for the W3 components (not touched; `header()` in `render.rs` has the caption text).
- Closer (optional): `lib/diagram/` could export the `tint()` state-paint helper — it is copied in Loop/Layers/Treemap/…; and a `focusUnit` that falls back to the nearest `[data-unit]` container would give Kanban/Heatmap arrow keys.
