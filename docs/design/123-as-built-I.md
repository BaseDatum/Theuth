# 123 — as built, I (closer: integration, Phase I fixes, test wave)

Branch `feat/123-diagram-grammars` from the merge `a49adcc0`: `6e8b506c` (export/Slack + apps `check.ts`), `63952bc8` (fmt), `d709f4eb` (desktop tests, boards, label placement), `0d3188d3` (clippy), then this note.

## Merge state resolved
- `export.rs`/`render.rs`: the three wave regions were disjoint and compiled; `UPDATE_GOLDEN=1 … valid_fixtures_match_golden_files` changed no golden.
- `diagrams/_stub.tsx` deleted (no importer); `stubs.test.ts` deleted, superseded by `fixtures.test.ts`. `registry.tsx`/`diagram-hosts.ts` import the final Views; `tsc` clean. One `bun install` (W3's `@types/d3-sankey` was only in `bun.lock`; lockfiles unchanged).

## Hand-offs closed
- C dev. 1 (apps 15) — kept; paragraph below. `visualize.md` "Which fence" and `apps.md` already say it; no `Surface*` names.
- C dev. 7 — `parse.ts` not extended (no worker asked); Rust `check()` authoritative.
- Slack one-series arm — `slack.rs`: `BarChart`/`LineChart` with one series → `chart()` (`bar`/`line`, title = `title ?? series.name`); more series stay `Unsupported`. Goldens `fixtures/slack/{bar-chart-one-series,line-chart-one-series,unsupported-bar-chart-stacked}`.
- `export.rs`: `is_diagram()` = Graph, Sequence, the 19, `Tree` (layout ≠ list), `Timeline` (`scale: time`); `caption()` = `Name: n units[, m links]` for all 23. New `Tree` GFM arm (nested list) — `*Tree*` was the real fallthrough the new fixture-walk test `no_diagram_fixture_lowers_to_its_type_name_and_every_diagram_has_a_caption` found.
- W4: `check.ts` `SELECT_AT` scopes `$value` inside `onSelect` for every `DIAGRAMS` name (positional = wire props + `highlight` + 1; `mappedProps.onSelect` wins); Graph keeps 5. Test in `graph.test.tsx`.
- W1 boards: `blocks.ts` `Diagram` ∪ StateMachine/Entities/Swimlane/GraphDelta/Tree (nodes/org/nested); `units.ts` `unitsOf` arms (ids as the Views emit them; GraphDelta `after ∪ before`; Tree `id ?? i:<index>`); `Tour.tsx` `DiagramView` mounts the five. Test in `units.test.ts`.
- `no-emoji.test.ts` was red on the merge: Kanban `⚠` → lucide `TriangleAlert`; Swimlane's Rust-twin glyph table `// no-emoji: allow`.
- W3 optional items not done (Open items).

## Tests added (`surface/diagrams/`)
- `geometry.test.ts` — the real ELK (`elkjs/lib/elk.bundled.js`, worker options) over every ELK-family element of every `valid-*` fixture (12 cases, 6 types): sections orthogonal, ending on a node border; no segment through a non-endpoint node; every label mask (`maskRect`, as drawn) off its stroke and over no node. Tree connectors synthesised as `TreeNodesSvg` draws them.
- `fixtures.test.ts` (90 tests) — the render proof: all 74 `valid-*` through `JsonRenderFence`; no boundary fallback, no `data-stub`; one `data-streamdown="<kind>-block"` frame per drawn diagram (23 kinds); one labelled `<svg>` per synchronous diagram with `aria-labelledby`, `<title>` first, `<desc>`, every `data-unit` `role="button" tabindex="0" aria-label`; Kanban/Heatmap units on `div`/`td`; the ELK family's pure `<NameSvg>` at `fixedLayout` + `useUnits` (static markup runs no effect, so the worker path yields only its frame); Tree dispatch; determinism.
- Found + fixed: under real ELK `valid-graph-extended` put the `SQL` mask over node `q`. `Graph.tsx` `edgeLabelAt` now reads `e.label` (ELK's placed box): nearest segment, mask on ELK's side (`side`), still `LABEL_GAP` off the stroke; `EdgePath`/Entities pass `side`; `fixedLayout` keeps the old placement so no snapshot moved; `label.tsx` `maskRect()` (README).

## Apps register 15, not 19 (KEEP)
Upstream OpenUI already defines `BarChart`, `LineChart`, `ScatterChart`, `RadarChart` (recharts) and `library.ts` is keyed by name, upstream first — re-registering would change every existing app. The four chart grammars reach apps as **fence and board components** (json-render fences, boards' `ref`-lit units); the other fifteen are apps components under their catalog names with `highlight`/`onSelect` appended. `CURATED_COMPONENTS` = 71 (56 + 15). No `Surface*` aliases invented.

## Counts
Rust `NAMES` 57 · desktop `catalog.test.ts` 57 · apps 71 (15 new) · `EXAMPLES` 57 · fixtures 74 `valid-*`, 62 `broken-*`, 5 `graph-*`, 148 goldens, slack 18 (3 new) · skill `/tmp/123-visualize.md` **48,036 B** (cap 49,152) · `catalog_compact()` **3,531** · `prompt_block()` 11,586.

## Gates (this tree, verbatim)
`cargo test -p surface -p agent -p document`: surface 90 passed; agent 414 passed / 1 ignored; document 45; actor 11; 0 failed · `cargo check --workspace --all-targets`: Finished (1 warning, dead `placeholder()` → removed) · `cargo clippy -p surface --all-targets -- -D warnings`: Finished after 3 fixes (`!(x > 0.0)` → `is_nan() || <= 0.0` ×2, `nth(0)` → `next()`); `fmt --check` clean · `just ts-desktop`: 553 files both dirs, no tracked drift · desktop `check` 0 errors, `test` (FULL) **166 files / 1171 passed**, `build` ✓ 13968 modules 24.43 s (`sankeyLinkHorizontal` 8.26 kB / 2.76 gzip) · apps `check` 0 errors, `test` **226 pass / 0 fail** (24 files).

PHASE I DONE 0d3188d3

## Phase II
Skill dumped via `cargo run -q -p surface --example prompt_block skill` (48,036 B; `catalog` 3,531; `block` 11,586). Render proof = `fixtures.test.ts`. Steering run stays owed (W4 had no `openai/gpt-5.6-sol` credentials); no model called.

## Decisions
- `role`: `svgA11y` gives `img` without live units, `group` with them (focusable children need `group`); W2 charts always `group`, W1/W3 `img` in a fence. The a11y test accepts both.
- Captions are counts, never the drawn title.

## Open items (Sky)
- Apps naming ruling; steering rerun with credentials; iOS tap check; live desktop smoke (chat → paste the skill's six worked examples → pan/zoom/select/export).
- `svgA11y` role unification (one line per View + ~20 snapshots). W4: `Table` cell number coercion in `validate.rs` (contract change). W3: `focusUnit` fallback to the nearest `[data-unit]` (Kanban/Heatmap arrows), shared `tint()`.
