# 123 as-built — W2, chart family

Branch `feat/123-w2-charts`. Two workers: the original W2 (commits `3402e7a8`, `0c594848`) and its successor (this note, from `0705d012` on). The successor found the work functionally complete, fixed the one red test, ran every gate and wrote this note.

## What the predecessor built

**Rust (`crates/surface`)** — `src/render/charts.rs` (new): `bar_chart` (rows per label, one glyph run per series, legend line, width-aware), `line_chart` (sparklines; `slope` as `a  12 → 18  b`; `bump` as rank columns), `scatter_chart` / `radar_chart` tables, `waterfall` (`running_total()`, TS twin), `funnel` (bars + conversion %), `gantt` (`label  2026-10-01 → 10-14  ▓▓▓▓░░░░`, width-fitted, `│` today), `quadrant` (2×2 box grid), `timeline_order` / `when` (Timeline `scale: time` sorted, dates shown), `markdown()` (GFM tables for `export.rs`). Arms in the `// ---- 123 chart family (W2) ----` regions of `render.rs` / `export.rs`. 13 fixtures beyond C's (`valid-{bar-chart-dumbbell,bar-chart-horizontal,line-chart-bump,line-chart-slope,line-chart-step,radar-chart-polar,scatter-chart-beeswarm,scatter-chart-bubble,waterfall-deltas,funnel-pyramid,gantt-hours,quadrant-bare,timeline-time-dates}.json`), 44 goldens at 60/100 columns, 6 unit tests in `charts.rs`.

**Desktop (`crates/desktop/ui`)** — `src/lib/diagram/chart/` (new, on the frozen kernel only): `frame.tsx` (`ChartSvg` shell: `<title>`/`<desc>` first, `min-width` = viewBox width in an `overflow-x: auto` div; `useChart` = tokens + `useUnits` + `useLegend`; `ChartHost` = `DiagramFrame` + tooltip; `staticUnits()` for hook-free tests), `geometry.ts` (extent, `runningTotal`, line/area/step paths, beeswarm, polar/sector, bubble radius, `timeSpan`/`endOf`, `quadrantOf`, `conversion`, `elbow`, `segmentCrosses`), `series.ts` (palette/own-state colour, `unitPaint`: outline + 0.12 tint, `muted` 50 %), `text.ts` (`describe`/`num`/`withUnit`/`signed`/`shortDate`, twins of `charts.rs`). Nine Views replacing C's stubs, each `layoutX()` + pure `<XSvg>` + `<XView>`: `BarChart` (grouped | stacked | dumbbell | horizontal, value labels ≤ 12 bars, unit suffix, legend toggle), `LineChart` (line | area | slope | bump | step, markers ≤ 24 points), `ScatterChart` (scatter | bubble + size legend | beeswarm, labels ≤ 20 points), `RadarChart` (radar | polar), `Waterfall` (floating deltas, grounded totals, connectors), `Funnel` (conversion % | pyramid), `Gantt` (day/week/month ticks, group bands, progress, `depends` elbows, today), `Quadrant`, `TimelineAxis` (time axis, `until` spans, lanes). `diagrams/charts.test.ts`: 22 `<XSvg>` snapshots, a11y per fixture (title first, `<desc>` = Rust first line, every unit a focusable button), Gantt arrow geometry, Waterfall running total pinned to Rust, per-variant tests, registry mount in a `DiagramFrame` without `data-stub`, open-fence tolerance.

**Apps (`packages/apps`)** — `test/chart-parity.tsx` (the `graph.test.tsx` shape as a `parity(case)` helper: zod mirror accepts `valid-<prefix>*`, rejects `broken-<prefix>-*` at the Rust path, `AppRenderer` mount with reactive `highlight` and a unit press → `onSelect` `$value`) and `test/{waterfall,funnel,gantt,quadrant}.test.tsx`.

## What the successor did

- Ran all gates; the only red was 4 stale snapshots (`valid-funnel`, `valid-funnel-pyramid`, `valid-waterfall`, `valid-waterfall-deltas`): the predecessor had switched Funnel/Waterfall unit ids from labels to indices after taking them (the apps parity tests already expect indices). Refreshed with `vitest -u`; commit `0705d012`.
- Audited each View against the common requirements (unit contract via `useUnits`, `<title>`/`<desc>`, `DiagramFrame` via `ChartHost`, reduced motion in `units.ts`, legend toggles, streaming tolerance tests) — nothing missing.
- Nothing in the Rust side needed changes.

## Decisions / deviations

- Unit ids: `BarChart`/`LineChart`/`RadarChart` radar `label/series`; `RadarChart` polar the axis; `ScatterChart`/`Quadrant` `id ?? label`; `Gantt` the task id; `Funnel`/`Waterfall` the stage/step **index** (labels may repeat; the Rust rows are positional); Timeline `id ?? title`. A bare label in `highlight` lights every bar/point of that label (`expandHighlight`).
- `<desc>` for Timeline `scale: time` is the first item row (`● 2026-10-07  Contract`), since the Rust Timeline arm has no `Name: title — n items` header.
- A date-only Gantt/Timeline `end` is inclusive (runs to the next midnight) in both Rust (`end_of`) and TS (`endOf`).
- `BarChart` `dumbbell` falls back to grouped when the visible series count is not 2 (legend toggle can hide one).
- Scoped vitest only (`src/components/chat/surface src/lib/diagram`), per the build-coordination rule; the full `bun run test` was not run.

## Gates (verbatim counts)

- `cargo test -p surface`: `ok. 69 passed; 0 failed`.
- `crates/desktop/ui`: `bun run check` clean; `bunx vitest run src/components/chat/surface src/lib/diagram`: `Test Files 2 failed | 9 passed (11)`, `Tests 13 failed | 210 passed (223)` → after the snapshot refresh `charts.test.ts` is `77 passed (77)`; the remaining 9 failures are all `stubs.test.ts` (Open items). `bun run build`: `✓ 13718 modules transformed … ✓ built in 40.15s`.
- `packages/apps`: `bun run check` clean; `bun test`: `177 pass, 0 fail, 857 expect() calls, 12 files`.

## Open items

- **Closer, `crates/desktop/ui/src/components/chat/surface/diagrams/stubs.test.ts`**: drop the 9 W2 rows from `STUBS` (`valid-bar-chart`, `valid-line-chart`, `valid-scatter-chart`, `valid-radar-chart`, `valid-waterfall`, `valid-funnel`, `valid-gantt`, `valid-quadrant`, `valid-timeline-time`) — those fixtures now render real Views without `data-stub`; `charts.test.ts` covers their a11y and mount.
- **Closer, `crates/surface/src/slack.rs`**: the one-series Slack `chart()` arm; `render::charts::markdown()` and `describe()` are `pub(crate)` and ready.
- Apps names `BarChart`/`LineChart`/`ScatterChart`/`RadarChart` remain OpenUI's (recharts); the four W2 Views reach apps only via json-render fences and boards, as C recorded.
