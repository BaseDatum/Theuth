# `lib/diagram/` — the SVG diagram kernel (design 123 §5.1)

Shared by every diagram component of `components/chat/surface/diagrams/*` (and `Graph.tsx`). **Every export below is
FROZEN for wave 123**: workers W1–W3 consume these signatures and may not change them. Additive exceptions: W1 may add
exports to `elk.ts` (ports, partitions, markers); W2 owns a new `chart/` directory under this one and may not change
kernel exports. Anything else goes to the closer as an "Open item".

Tests: `kernel.test.ts` (pure functions). Node environment, no DOM — hooks and tsx components are exercised through
`renderToStaticMarkup` in the component tests.

## `types.ts`
- `type DiagramHostProps<P> = { props: P; highlight?: readonly string[]; onSelect?: (id: string) => void; loading?: boolean }` — what every `<NameView>` takes (apps pass `highlight`/`onSelect`, the registry passes `loading`).
- `type Loose<T>` — ts-rs `T | null` fields made optional (the JSON the model writes).
- `interface Size { w; h }`, `interface Point { x; y }`.
- `type UnitState = 'default' | 'active' | 'done' | 'muted' | 'danger'`.

## `scale.ts`
- `linear(domain: [number, number], range: [number, number]): LinearScale` — `(v) → px`, `.invert(px)`, `.ticks(count = 5)`, `.domain`, `.range`.
- `band(domain: string[], range, padding = 0.2): BandScale` — `(key) → band start`, `.bandwidth`, `.step`.
- `time(domain: [Date, Date], range): TimeScale` — `(Date | ms) → px`, `.invert`, `.ticks(count = 5)` snapped to hour/day/week(Mon)/month/year in UTC, `.unit(count)`.
- `ticks(lo, hi, count = 5): number[]`, `tickStep(lo, hi, count): number`, `niceDomain(lo, hi, count): [lo, hi]` — d3's 1-2-5 thresholds.
- `timeUnit(spanMs, count = 5): TimeUnit`, `floorTo(date, unit): Date`, `formatTick(date, unit): string` (`09:05`, `Oct 7`, `Oct 2026`, `2026`).
- `parseIso(s): Date | null` — the TS twin of `crates/surface::validate::iso_datetime` (ISO date or RFC 3339).

## `axes.tsx`
- `<Axis orient="left" | "bottom" at span=[from, to] ticks=[{ at, label }] tokens title? />` — `<line>` + mono 10 px tick labels, sans 11 px title.
- `<Gridlines orient="horizontal" | "vertical" at=number[] span tokens />` — `border` at 40 %.
- `AXIS_INSET = { left: 48, bottom: 40, leftNoTitle: 32, bottomNoTitle: 22 }`, `TICK_PX = 10`, `TITLE_PX = 11`, `MONO`.

## `legend.tsx`
- `useLegend(): { hidden: readonly string[]; toggle(id); visible(id): boolean }` — local toggle state.
- `<Legend items=[{ id, label, color, shape?: 'swatch' | 'line', dashed? }] x y hidden? onToggle? tokens />` — bottom strip; entries are `role="button" aria-pressed` when `onToggle` is given; `data-legend={id}`.
- `legendWidth(items): number`, `LEGEND_H = 22`, `LEGEND_PX = 11`.

## `label.tsx`
- `<MaskedLabel text full? at={Point} along="horizontal" | "vertical" side?="before" | "after" tokens fill? />` — label on a `card` mask `LABEL_GAP` (8 px) off the stroke, ≤ 14 chars shown, full text in `<title>`.
- `truncate(text, max = 14): string`, `labelBox(shown): Size`, `LABEL_PX = 12`, `LABEL_MAX_CHARS = 14`, `LABEL_GAP = 8`.

## `path.ts`
- `orthogonalPath(points: readonly Point[], r = 8): string` — one `d`, interior bends rounded (radius shrinks on short segments); straight runs stay straight.
- `sectionsPath(sections: { start; bends; end }[], r = 8): string` — ELK sections → one `d`.
- `markers(arrow: 'forward' | 'both' | 'none' | null | undefined, markerId: string | null): { markerStart?; markerEnd? }`.
- `pointAt(points, t: 0..1): Point`, `ELBOW_R = 8`, `type Arrow`.

## `units.ts` — the unit contract (§5.3)
- `UNIT_ATTR = 'data-unit'` — the one attribute boards learn units through (`board/units.ts` also accepts the legacy `data-node`/`data-step` one release).
- `useUnits({ ids, incident?, highlight?, onSelect? }): Units` where `Units = { state: { selected, hover }; paint(id, own?): UnitState; unit(id, ariaLabel): UnitAttrs; root: { onKeyDown }; dispatch }`.
  - `unit(id, label)` spreads onto the unit `<g>`: `data-unit`, `role="button"`, `tabIndex=0`, `aria-label`, `aria-pressed`, cursor + 120 ms transition (off under `prefers-reduced-motion`), click / Enter / Space select, arrows move focus through `ids`, hover (fine pointers) and focus feed `state.hover`.
  - `root` spreads onto the `<svg>`: Esc clears.
  - `paint(id, own)`: while something is selected, it and `incident(selected)` are `active`, the rest `muted`; otherwise `highlight` ids are `active`; a unit's own `danger`/`done` wins, own `muted` stays.
- Pure, tested: `unitsReduce(state, action)`, `paintOf(id, own, state, incident, highlight?)`, `UNITS_IDLE`, `unitTransition()`, `focusUnit(from: Element, id)`.

## `tooltip.tsx`
- `<DiagramTooltip hover={id | null} root={Element | null} title rows?=[{ label, value? }] />` — one portal tooltip positioned from the hovered unit's bounding box (so pan/zoom never misplaces it); `role="tooltip"`.

## `a11y.ts`
- `svgA11y(base: string, interactive: boolean): { svg: { role: 'img' | 'group'; 'aria-labelledby' }; titleId; descId }` — render `<title id={titleId}>` as the svg's first child, `<desc id={descId}>` second.
- `describe(name, title, n, unit): string` — the `<desc>` text, `Gantt: Wave 123 — 6 tasks` (first line of the §4.4 lowering).
- `countOf(n, unit): string` (`1 task`, `6 tasks`, `1 entity`), `unitLabel(label, detail?, state?): string`.

## `elk.ts` — ELK plumbing (W1 may add, not change)
- `elkLayout<T>(hash: string, build: () => ElkNode, map: (root: ElkNode) => T): Promise<T>` — LRU (64) by hash, `document.fonts.ready` before `build`, in-flight sharing, the worker (`../graph-layout.worker.ts`), 2 s `LayoutTimeout`.
- `flattenElk(root: ElkNode): Layout` — nested children and container-relative edges to root coordinates; ids `n:`, `g:`, `e:<index>`, `l:<index>`.
- `nodeSize(label, extraLines = 0): Size`, `labelSize(text): Size | undefined`, `textWidth(text, px = 13): number`, `wrapLabel(text): string[]`, `norm(s): string`, `fnv(s): string`.
- `ELK_OPTIONS`, `GROUP_OPTIONS`, `WRAP = 24`, `FONT_PX = 13`, `LINE_PX = 17`, `PAD_X = 12`, `PAD_Y = 8`, `MIN_SIZE`, `LAYOUT_TIMEOUT_MS = 2000`.
- `class LruCache<V>`, `class LayoutTimeout`, types `Layout`, `LayoutNode`, `LayoutEdge`, `LayoutGroup`.
- `lib/graph-layout.ts` is now a thin `Graph`-specific layer over this file (`GraphData`, `structuralHash`, `toElk`, `layout`) and re-exports the names `Graph.tsx` / `codemap/layout.ts` import.

## `../diagram-theme.ts`
- `DiagramTokens.series: [string × 6]` — the categorical palette (`--chart-1..6` when the theme defines them, else fixed fallbacks), for series and legends.
