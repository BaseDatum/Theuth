// 123 W2: the chart family — pure `<NameSvg>` snapshots at the fixtures' fixed layouts, the a11y contract (`<title>`
// first, `<desc>` = the Rust lowering's first line, every unit focusable), geometry invariants where edges exist
// (Gantt arrows never run through an intermediate bar), the Waterfall running total pinned to the Rust sequence,
// streaming tolerance, and the full mount through `JsonRenderFence` (hooks, `DiagramFrame`, no `data-stub`).
import { readFileSync } from 'node:fs'
import { type ComponentType, createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { staticUnits } from '@/lib/diagram/chart/frame'
import { beeswarm, quadrantOf, runningTotal, segmentCrosses } from '@/lib/diagram/chart/geometry'
import { describe as describeChart, num } from '@/lib/diagram/chart/text'
import type { Units } from '@/lib/diagram/units'
import { JsonRenderFence } from '../JsonRenderFence'
import { parseSpec } from '../parse'
import { type BarChartData, BarChartSvg, barIds, expandHighlight, layoutBarChart } from './BarChart'
import { type FunnelData, FunnelSvg, layoutFunnel } from './Funnel'
import { type GanttData, GanttSvg, layoutGantt } from './Gantt'
import { layoutLineChart, type LineChartData, LineChartSvg } from './LineChart'
import { layoutQuadrant, type QuadrantData, QuadrantSvg } from './Quadrant'
import { layoutRadarChart, type RadarChartData, RadarChartSvg } from './RadarChart'
import { layoutScatterChart, type ScatterChartData, ScatterChartSvg } from './ScatterChart'
import { firstLine, layoutTimeline, type TimelineData, TimelineAxisSvg, whenText } from './TimelineAxis'
import { layoutWaterfall, type WaterfallData, WaterfallSvg } from './Waterfall'

vi.mock('@/lib/notify', () => ({ notify: Object.assign(vi.fn(), { message: vi.fn(), error: vi.fn() }) }))
vi.mock('@/lib/rpc', () => ({ useApi: () => ({}) }))
vi.mock('@/components/ui/tooltip', () => ({ Tooltip: ({ children }: { children: unknown }) => children, TooltipTrigger: ({ children }: { children: unknown }) => children, TooltipContent: () => null }))
vi.mock('@/lib/theme', () => ({ subscribeDark: () => () => {} }))
vi.mock('@/lib/diagram-theme', () => ({ diagramTokens: () => TOKENS }))

const TOKENS: DiagramTokens = {
  background: '#e8dcc8',
  foreground: '#1a1612',
  primary: '#1a1612',
  muted: '#d4c4a8',
  mutedForeground: '#6b5e4f',
  border: '#d4c4a8',
  card: '#f3eadf',
  accent: '#b4531e',
  success: '#2f5d4a',
  danger: '#9b2d1f',
  font: 'sans-serif',
  series: ['#b4531e', '#2f5d4a', '#3b5b8c', '#8a6d1f', '#7a3e7a', '#6b5e4f'],
}

const RUST = new URL('../../../../../../../surface/fixtures/json-render/', import.meta.url)
const source = (name: string) => readFileSync(new URL(`${name}.json`, RUST), 'utf8')
function propsOf<T>(name: string): T {
  const spec = parseSpec(source(name))
  if ('error' in spec) throw new Error(`${name}: ${spec.error}`)
  return Object.values(spec.spec.elements)[0]!.props as T
}
const units = staticUnits()

/** `[fixture, render it statically, expected <desc>]`. */
const CHARTS: [string, (name: string) => string, string][] = [
  ['valid-bar-chart', (n) => svg(BarChartSvg, propsOf<BarChartData>(n), layoutBarChart(propsOf(n), [], TOKENS)), 'BarChart: CI runs per day — 2 series'],
  ['valid-bar-chart-dumbbell', (n) => svg(BarChartSvg, propsOf<BarChartData>(n), layoutBarChart(propsOf(n), [], TOKENS)), 'BarChart: Latency before and after — 2 series'],
  ['valid-bar-chart-horizontal', (n) => svg(BarChartSvg, propsOf<BarChartData>(n), layoutBarChart(propsOf(n), [], TOKENS)), 'BarChart: 1 series'],
  ['valid-line-chart', (n) => svg(LineChartSvg, propsOf<LineChartData>(n), layoutLineChart(propsOf(n), [], TOKENS)), 'LineChart: p95 latency by release — 2 series'],
  ['valid-line-chart-slope', (n) => svg(LineChartSvg, propsOf<LineChartData>(n), layoutLineChart(propsOf(n), [], TOKENS)), 'LineChart: Error rate per surface — 3 series'],
  ['valid-line-chart-bump', (n) => svg(LineChartSvg, propsOf<LineChartData>(n), layoutLineChart(propsOf(n), [], TOKENS)), 'LineChart: Model ranking by week — 3 series'],
  ['valid-line-chart-step', (n) => svg(LineChartSvg, propsOf<LineChartData>(n), layoutLineChart(propsOf(n), [], TOKENS)), 'LineChart: 1 series'],
  ['valid-scatter-chart', (n) => svg(ScatterChartSvg, propsOf<ScatterChartData>(n), layoutScatterChart(propsOf(n), [], TOKENS)), 'ScatterChart: Effort vs value — 5 points'],
  ['valid-scatter-chart-bubble', (n) => svg(ScatterChartSvg, propsOf<ScatterChartData>(n), layoutScatterChart(propsOf(n), [], TOKENS)), 'ScatterChart: Crates by size — 4 points'],
  ['valid-scatter-chart-beeswarm', (n) => svg(ScatterChartSvg, propsOf<ScatterChartData>(n), layoutScatterChart(propsOf(n), [], TOKENS)), 'ScatterChart: 8 points'],
  ['valid-radar-chart', (n) => svg(RadarChartSvg, propsOf<RadarChartData>(n), layoutRadarChart(propsOf(n), [], TOKENS)), 'RadarChart: Plan comparison — 2 series'],
  ['valid-radar-chart-polar', (n) => svg(RadarChartSvg, propsOf<RadarChartData>(n), layoutRadarChart(propsOf(n), [], TOKENS)), 'RadarChart: Hours by area — 1 series'],
  ['valid-waterfall', (n) => svg(WaterfallSvg, propsOf<WaterfallData>(n), layoutWaterfall(propsOf(n))), 'Waterfall: Headcount 2026 — 6 steps'],
  ['valid-waterfall-deltas', (n) => svg(WaterfallSvg, propsOf<WaterfallData>(n), layoutWaterfall(propsOf(n))), 'Waterfall: Build time change — 5 steps'],
  ['valid-funnel', (n) => svg(FunnelSvg, propsOf<FunnelData>(n), layoutFunnel(propsOf(n))), 'Funnel: Signup funnel — 4 stages'],
  ['valid-funnel-pyramid', (n) => svg(FunnelSvg, propsOf<FunnelData>(n), layoutFunnel(propsOf(n))), 'Funnel: Test pyramid — 3 stages'],
  ['valid-gantt', (n) => svg(GanttSvg, propsOf<GanttData>(n), layoutGantt(propsOf(n))), 'Gantt: Wave 123 — 6 tasks'],
  ['valid-gantt-hours', (n) => svg(GanttSvg, propsOf<GanttData>(n), layoutGantt(propsOf(n))), 'Gantt: Release day — 4 tasks'],
  ['valid-quadrant', (n) => svg(QuadrantSvg, propsOf<QuadrantData>(n), layoutQuadrant(propsOf(n))), 'Quadrant: What to do first — 5 items'],
  ['valid-quadrant-bare', (n) => svg(QuadrantSvg, propsOf<QuadrantData>(n), layoutQuadrant(propsOf(n))), 'Quadrant: 5 items'],
  ['valid-timeline-time', (n) => svg(TimelineAxisSvg, propsOf<TimelineData>(n), layoutTimeline(propsOf(n))), '● 2026-10-03T09:12:00Z  Pager fires'],
  ['valid-timeline-time-dates', (n) => svg(TimelineAxisSvg, propsOf<TimelineData>(n), layoutTimeline(propsOf(n))), '● 2026-10-07  Contract'],
]

function svg<P, L>(Svg: ComponentType<{ props: P; layout: L; tokens: DiagramTokens; units: Units; id: string }>, props: P, layout: L): string {
  return renderToStaticMarkup(createElement(Svg, { props, layout, tokens: TOKENS, units, id: 'fixed' }))
}

test.each(CHARTS)('%s: the pure svg matches its snapshot', (name, render) => {
  expect(render(name)).toMatchSnapshot()
})

test.each(CHARTS)('%s: a11y — title first, desc is the Rust first line, every unit a focusable button', (name, render, desc) => {
  const html = render(name)
  const m = /<svg([^>]*)>(.*)<\/svg>/s.exec(html)!
  expect(m[1]).toMatch(/role="img"|role="group"/)
  expect(m[1]).toMatch(/aria-labelledby="[^"]+-fixed-title [^"]+-fixed-desc"/)
  expect(m[2]).toMatch(/^<title id="[^"]+-title">/)
  expect(m[2]).toContain(`<desc id="${/aria-labelledby="[^ ]+ ([^"]+)"/.exec(m[1]!)![1]}">${desc}</desc>`)
  const unitTags = [...html.matchAll(/<g [^>]*data-unit="[^"]*"[^>]*>/g)].map((x) => x[0])
  expect(unitTags.length).toBeGreaterThan(0)
  for (const g of unitTags) {
    expect(g).toContain('role="button"')
    expect(g).toContain('tabindex="0"')
    expect(g).toMatch(/aria-label="[^"]+"/)
  }
  // No shadows, inline token colours only.
  expect(html).not.toMatch(/filter=|drop-shadow|class="/)
  // The wrapper scrolls horizontally and the svg keeps its viewBox width.
  expect(html).toMatch(/^<div style="overflow-x:auto"><svg [^>]*style="min-width:\d+px;height:auto;display:block"/)
})

test('desc matches the Rust describe() for singular and plural', () => {
  expect(describeChart('Gantt', null, 1, 'task')).toBe('Gantt: 1 task')
  expect(describeChart('BarChart', 'T', 1, 'series', 'series')).toBe('BarChart: T — 1 series')
  expect(num(42)).toBe('42')
  expect(num(0.25)).toBe('0.25')
  expect(num(1.5)).toBe('1.5')
  expect(whenText({ when: '2026-10-03T10:05:00Z', until: '2026-10-03T11:30:00Z', title: 'x' })).toBe('2026-10-03T10:05:00Z → 11:30:00Z')
  expect(whenText({ when: '2026-10-08', until: '2026-10-14', title: 'x' })).toBe('2026-10-08 → 2026-10-14')
  expect(firstLine(propsOf<TimelineData>('valid-timeline-time-dates'))).toBe('● 2026-10-07  Contract')
})

test('the Waterfall running total is the sequence the Rust test pins', () => {
  const p = propsOf<WaterfallData>('valid-waterfall')
  expect(runningTotal(p.steps)).toEqual([100, 112, 107, 107, 111, 111])
  const l = layoutWaterfall(p)
  // Deltas float from the previous level, totals stand on the baseline.
  const hires = l.bars[1]!
  const jan = l.bars[0]!
  expect(hires.rect.y + hires.rect.h).toBeCloseTo(jan.rect.y, 5)
  expect(jan.rect.y + jan.rect.h).toBeCloseTo(l.zero, 5)
  expect(l.connectors).toHaveLength(5)
})

test('Gantt: arrows run orthogonally and no vertical run crosses an intermediate bar; dangling depends are dropped', () => {
  const p = propsOf<GanttData>('valid-gantt')
  const l = layoutGantt(p)
  expect(l.arrows.length).toBe(8)
  const rects = Object.fromEntries(l.rows.map((r) => [r.id, r.rect]))
  for (const a of l.arrows) {
    for (let i = 1; i < a.points.length; i++) {
      const s = a.points[i - 1]!
      const e = a.points[i]!
      expect(s.x === e.x || s.y === e.y, 'orthogonal').toBe(true)
      for (const r of l.rows) {
        if (r.id === a.from || r.id === a.to) continue
        expect(segmentCrosses(s, e, r.rect), `${a.from}→${a.to} crosses ${r.id}`).toBe(false)
      }
    }
  }
  expect(l.groups.map((g) => g.label)).toEqual(['step 0', 'workers', 'step 5'])
  expect(l.today).not.toBeNull()
  expect(rects.c!.w).toBeGreaterThan(0)
  const open = layoutGantt({ ...p, tasks: [...p.tasks, { id: 'x', label: 'Streaming', start: '2026-10-10', end: '2026-10-11', depends: ['nope'] }] })
  expect(open.arrows.length).toBe(8)
  expect(open.rows.map((r) => r.id)).toContain('x')
})

test('BarChart: value labels only up to 12 bars; a bare label highlight lights all its bars; hidden series leave', () => {
  const p = propsOf<BarChartData>('valid-bar-chart')
  const l = layoutBarChart(p, [], TOKENS)
  expect(l.showValues).toBe(true)
  expect(l.bars).toHaveLength(10)
  // Stacked: Failed sits on top of Passed.
  expect(l.bars[1]!.rect.y + l.bars[1]!.rect.h).toBeCloseTo(l.bars[0]!.rect.y, 5)
  expect(expandHighlight(['Thu'], barIds(l))).toEqual(['Thu/Passed', 'Thu/Failed'])
  expect(layoutBarChart(p, ['Failed'], TOKENS).bars).toHaveLength(5)
  const hp = propsOf<BarChartData>('valid-bar-chart-horizontal')
  const many = layoutBarChart(hp, [], TOKENS)
  // A highlighted label paints its bar with the accent (a series' own done/danger would win, 65 §3.5).
  const html = renderToStaticMarkup(createElement(BarChartSvg, { props: hp, layout: many, tokens: TOKENS, units: staticUnits(expandHighlight(['Beta'], barIds(many))), id: 'h' }))
  expect(html.match(new RegExp(`stroke="${TOKENS.accent}" stroke-width="2"`, 'g'))).toHaveLength(1)
  expect(many.showValues).toBe(false)
  expect(many.horizontal).toBe(true)
  // Streaming: a series shorter than the labels draws what it has.
  const open = layoutBarChart({ ...p, series: [{ name: 'Passed', values: [1, 2] }] }, [], TOKENS)
  expect(open.bars).toHaveLength(2)
})

test('LineChart: markers up to 24 points, bump inverts the axis, slope has no left axis', () => {
  const line = layoutLineChart(propsOf<LineChartData>('valid-line-chart'), [], TOKENS)
  expect(line.series.every((s) => s.markers)).toBe(true)
  expect(line.series[0]!.area).not.toBeNull()
  const bump = layoutLineChart(propsOf<LineChartData>('valid-line-chart-bump'), [], TOKENS)
  const opus = bump.series[0]!.points
  expect(opus[0]!.y).toBeLessThan(opus[2]!.y) // rank 1 above rank 2
  const long = layoutLineChart({ labels: Array.from({ length: 30 }, (_, i) => `d${i}`), series: [{ name: 's', values: Array.from({ length: 30 }, (_, i) => i) }] }, [], TOKENS)
  expect(long.series[0]!.markers).toBe(false)
  const slope = renderToStaticMarkup(createElement(LineChartSvg, { props: propsOf<LineChartData>('valid-line-chart-slope'), layout: layoutLineChart(propsOf('valid-line-chart-slope'), [], TOKENS), tokens: TOKENS, units, id: 's' }))
  expect(slope).toContain('2.4 %')
  expect(slope).toContain('1.1 %')
})

test('Scatter: beeswarm dots never overlap, bubble sizes map to area, labels hide past 20 points', () => {
  const ys = beeswarm([0, 0, 0, 2, 100], 6)
  expect(ys[0]).toBe(0)
  const pts = [0, 0, 0, 2, 100].map((x, i) => ({ x, y: ys[i]! }))
  for (let i = 0; i < pts.length; i++) for (let j = i + 1; j < pts.length; j++) expect(Math.hypot(pts[i]!.x - pts[j]!.x, pts[i]!.y - pts[j]!.y)).toBeGreaterThanOrEqual(12 - 1e-6)
  const bubble = layoutScatterChart(propsOf<ScatterChartData>('valid-scatter-chart-bubble'), [], TOKENS)
  const agent = bubble.dots.find((d) => d.id === 'agent')!
  const markdown = bubble.dots.find((d) => d.id === 'markdown')!
  expect(agent.r).toBeGreaterThan(markdown.r)
  expect(agent.r ** 2 / markdown.r ** 2).toBeCloseTo(24000 / 2100, 5)
  expect(bubble.sizes).toHaveLength(2)
  const many = layoutScatterChart({ xLabel: 'x', yLabel: 'y', points: Array.from({ length: 21 }, (_, i) => ({ x: i, y: i, label: `p${i}` })) }, [], TOKENS)
  expect(many.showLabels).toBe(false)
  const swarm = layoutScatterChart(propsOf<ScatterChartData>('valid-scatter-chart-beeswarm'), [], TOKENS)
  expect(swarm.yTicks).toHaveLength(0)
  expect(layoutScatterChart(propsOf<ScatterChartData>('valid-scatter-chart'), ['ui'], TOKENS).dots).toHaveLength(2)
})

test('Radar: the ring is max or the data max; polar draws one sector per axis', () => {
  const radar = layoutRadarChart(propsOf<RadarChartData>('valid-radar-chart'), [], TOKENS)
  expect(radar.max).toBe(5)
  const safety = radar.series[0]!.vertices[2]! // Plan A, Safety 5 of 5: on the outer ring
  expect(Math.hypot(safety.x - radar.cx, safety.y - radar.cy)).toBeCloseTo(radar.r, 1)
  const polar = layoutRadarChart(propsOf<RadarChartData>('valid-radar-chart-polar'), [], TOKENS)
  expect(polar.max).toBe(14)
  expect(polar.series[0]!.vertices.every((v) => v.d?.startsWith('M'))).toBe(true)
  expect(polar.series[0]!.vertices.map((v) => v.id)).toEqual(['Rust', 'TS', 'Docs', 'Review', 'Ops', 'Design'])
})

test('Quadrant: the midline is right / top, as the Rust quadrant_of', () => {
  expect(quadrantOf(0.2, 0.8)).toBe(0)
  expect(quadrantOf(0.5, 0.6)).toBe(1)
  expect(quadrantOf(0.3, 0.3)).toBe(2)
  expect(quadrantOf(0.8, 0.2)).toBe(3)
  const l = layoutQuadrant(propsOf<QuadrantData>('valid-quadrant'))
  expect(l.dots.map((d) => d.id)).toEqual(['cache', 'rewrite', 'docs', 'theme', 'Tests'])
  const tests = l.dots[4]!
  expect(tests.r).toBeGreaterThan(l.dots[0]!.r)
  expect(tests.x).toBeGreaterThanOrEqual(l.plot.x + l.plot.w / 2)
})

test('Funnel: widths descend by value with conversion between stages; pyramid bands are equal without values', () => {
  const f = layoutFunnel(propsOf<FunnelData>('valid-funnel'))
  const widths = f.bands.map((b) => Number(b.points.split(' ')[1]!.split(',')[0]) - Number(b.points.split(' ')[0]!.split(',')[0]))
  expect(widths[0]).toBeGreaterThan(widths[1]!)
  expect(widths[1]).toBeGreaterThan(widths[2]!)
  expect(f.bands.map((b) => b.pct)).toEqual([null, 25, 40, 38])
  const p = layoutFunnel(propsOf<FunnelData>('valid-funnel-pyramid'))
  expect(p.pyramid).toBe(true)
  expect(p.bands.map((b) => b.pct)).toEqual([null, null, null])
})

test('Timeline time: items sort by date into non-overlapping lanes; spans are bars', () => {
  const l = layoutTimeline(propsOf<TimelineData>('valid-timeline-time-dates'))
  expect(l.marks.map((m) => m.id)).toEqual(['c', 'w', 'Closer'])
  expect(l.marks[1]!.x1).not.toBeNull()
  const end = (m: (typeof l.marks)[number]) => (m.x1 ?? m.x0) + m.labelW
  for (const a of l.marks) for (const b of l.marks) if (a !== b && a.lane === b.lane) expect(end(a) + 8 <= b.x0 || end(b) + 8 <= a.x0, `${a.id} / ${b.id} share a lane`).toBe(true)
})

const fence = (name: string) => renderToStaticMarkup(createElement(JsonRenderFence, { code: source(name), isIncomplete: false, language: 'json-render' }))

test.each(CHARTS.map(([n]) => n))('%s mounts through the registry inside a DiagramFrame, no stub', (name) => {
  const html = fence(name)
  expect(html).not.toContain('data-stub')
  expect(html).toMatch(/data-streamdown="[a-z]+-block"/)
  expect(html).toMatch(/<svg[^>]*data-chart="[A-Za-z]+"/)
  expect(html).toMatch(/aria-labelledby="[^"]+-title [^"]+-desc"/)
  expect((html.match(/data-unit="/g) ?? []).length).toBeGreaterThan(0)
})

test('an open fence with a dangling depends and a short series renders without throwing', () => {
  const code = `{"root":"g","elements":{"g":{"type":"Gantt","props":{"tasks":[{"id":"a","label":"A","start":"2026-10-08","end":"2026-10-10"},{"id":"b","label":"B","start":"2026-10-10","end":"2026-10-12","depends":["zz","a"]}]}}}}`
  const html = renderToStaticMarkup(createElement(JsonRenderFence, { code, isIncomplete: true, language: 'json-render' }))
  expect(html).toContain('data-unit="b"')
  const bar = `{"root":"b","elements":{"b":{"type":"BarChart","props":{"labels":["a","b","c"],"series":[{"name":"s","values":[1`
  const partial = renderToStaticMarkup(createElement(JsonRenderFence, { code: bar, isIncomplete: true, language: 'json-render' }))
  expect(partial).toContain('data-chart="BarChart"')
})
