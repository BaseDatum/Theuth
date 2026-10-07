// 123 W2: `LineChart` — line | area | slope | bump | step on the chart kernel (design 123 §4.2, §5.2 b). Every
// point is a unit (`label/series`); the series stroke is drawn under its points; markers appear when a series has
// ≤ 24 points. `bump` inverts the axis (rank 1 at the top); `slope` labels both ends.
import { useCallback, useMemo } from 'react'
import type { z } from 'zod'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { Axis, AXIS_INSET, Gridlines, MONO, type Tick, TICK_PX } from '@/lib/diagram/axes'
import { CHART_W, ChartHost, ChartSvg, HIT, ownState, PAD, titleH, useChart } from '@/lib/diagram/chart/frame'
import { areaPath, extent, linePath, type Rect } from '@/lib/diagram/chart/geometry'
import { NAME_PX, seriesColor, unitPaint } from '@/lib/diagram/chart/series'
import { describe, num, withUnit } from '@/lib/diagram/chart/text'
import { LEGEND_H, Legend, type LegendItem, legendWidth } from '@/lib/diagram/legend'
import { linear, niceDomain } from '@/lib/diagram/scale'
import type { DiagramHostProps, Point } from '@/lib/diagram/types'
import type { Units } from '@/lib/diagram/units'
import type { lineChartProps } from '../catalog'
import { expandHighlight } from './BarChart'

export type LineChartData = z.infer<typeof lineChartProps>

export interface LinePoint extends Point {
  id: string
  label: string
  series: number
  value: number
}

export interface LineSeries {
  index: number
  name: string
  points: LinePoint[]
  d: string
  area: string | null
  markers: boolean
}

export interface LineChartLayout {
  W: number
  H: number
  plot: Rect
  variant: NonNullable<LineChartData['variant']>
  series: LineSeries[]
  valueTicks: Tick[]
  catTicks: Tick[]
  legend: LegendItem[]
  legendX: number
  legendY: number
}

const PLOT_H = 200
const MARKER_MAX = 24

/** Geometry for `p` with `hidden` series left out; tolerant of short `values` (an open fence). */
export function layoutLineChart(p: LineChartData, hidden: readonly string[] = [], tokens?: DiagramTokens): LineChartLayout {
  const labels = p.labels ?? []
  const all = p.series ?? []
  const variant = p.variant ?? 'line'
  const shown = all.map((s, i) => ({ s, i })).filter(({ s }) => !hidden.includes(s.name))
  const values: number[] = []
  for (const { s } of shown) for (const v of s.values ?? []) if (Number.isFinite(v)) values.push(v)
  const bump = variant === 'bump'
  const [lo, hi] = bump ? [1, Math.max(1, ...values)] : niceDomain(...extent(values, variant === 'area'))
  const slope = variant === 'slope'
  const left = slope ? 48 : p.yLabel || p.unit ? AXIS_INSET.left : AXIS_INSET.leftNoTitle
  const right = slope ? 48 : PAD
  const bottom = (p.xLabel ? AXIS_INSET.bottom : AXIS_INSET.bottomNoTitle) + 8 + LEGEND_H
  const top = PAD + titleH(p.title) + 12
  const W = Math.max(CHART_W, left + right + labels.length * 24)
  const H = top + PLOT_H + bottom
  const plot: Rect = { x: left, y: top, w: W - left - right, h: PLOT_H }
  const xs = linear([0, Math.max(1, labels.length - 1)], [plot.x, plot.x + plot.w])
  const ys = bump ? linear([lo, hi], [plot.y, plot.y + plot.h]) : linear([lo, hi], [plot.y + plot.h, plot.y])
  const series: LineSeries[] = shown.map(({ s, i }) => {
    const points: LinePoint[] = []
    labels.forEach((label, li) => {
      const v = s.values?.[li]
      if (typeof v !== 'number' || !Number.isFinite(v)) return
      points.push({ id: `${label}/${s.name}`, label, series: i, value: v, x: xs(li), y: ys(v) })
    })
    const step = variant === 'step'
    return {
      index: i,
      name: s.name,
      points,
      d: linePath(points, step),
      area: variant === 'area' ? areaPath(points, ys(Math.max(lo, 0)), step) : null,
      markers: points.length <= MARKER_MAX,
    }
  })
  const valueTicks: Tick[] = bump
    ? Array.from({ length: Math.min(hi, 12) }, (_, k) => k + 1).map((v) => ({ at: ys(v), label: `#${v}` }))
    : ys.ticks(5).map((v) => ({ at: ys(v), label: num(v) }))
  const catTicks: Tick[] = labels.map((l, i) => ({ at: xs(i), label: l }))
  const legend: LegendItem[] = all.map((s, i) => ({ id: s.name, label: s.name, color: tokens ? seriesColor(i, ownState(s.state), tokens) : '', shape: 'line' as const }))
  return { W, H, plot, variant, series, valueTicks, catTicks, legend, legendX: Math.max(PAD, (W - legendWidth(legend)) / 2), legendY: H - LEGEND_H - 4 }
}

export function lineIds(l: LineChartLayout): string[] {
  const ids: string[] = []
  const n = Math.max(0, ...l.series.map((s) => s.points.length))
  for (let i = 0; i < n; i++) for (const s of l.series) if (s.points[i]) ids.push(s.points[i]!.id)
  return ids
}

export function LineChartSvg({
  props: p,
  layout: l,
  tokens: t,
  units,
  id,
  legend,
}: {
  props: LineChartData
  layout: LineChartLayout
  tokens: DiagramTokens
  units: Units
  id: string
  legend?: { hidden: readonly string[]; onToggle: (id: string) => void }
}) {
  const desc = describe('LineChart', p.title, p.series?.length ?? 0, 'series', 'series')
  const unit = p.unit ?? undefined
  const colour = (si: number) => seriesColor(si, ownState(p.series?.[si]?.state), t)
  const items = l.legend.map((it, i) => ({ ...it, color: colour(i) }))
  const slope = l.variant === 'slope'
  return (
    <ChartSvg name="LineChart" id={id} title={p.title} desc={desc} W={l.W} H={l.H} tokens={t} interactive root={units.root}>
      {!slope && <Gridlines orient="horizontal" at={l.valueTicks.map((k) => k.at)} span={[l.plot.x, l.plot.x + l.plot.w]} tokens={t} />}
      {!slope && <Axis orient="left" at={l.plot.x} span={[l.plot.y, l.plot.y + l.plot.h]} ticks={l.valueTicks} tokens={t} title={p.yLabel ?? unit} />}
      <Axis orient="bottom" at={l.plot.y + l.plot.h} span={[l.plot.x, l.plot.x + l.plot.w]} ticks={l.catTicks} tokens={t} title={p.xLabel} />
      {slope && <line x1={l.plot.x + l.plot.w} y1={l.plot.y} x2={l.plot.x + l.plot.w} y2={l.plot.y + l.plot.h} stroke={t.border} shapeRendering="crispEdges" />}
      {l.series.map((s) => {
        const c = colour(s.index)
        const dim = units.state.selected !== null && !s.points.some((pt) => units.paint(pt.id, null) === 'active')
        return (
          <g key={s.name} opacity={dim ? 0.5 : 1}>
            {s.area && <path d={s.area} fill={c} fillOpacity={0.12} stroke="none" />}
            <path d={s.d} fill="none" stroke={c} strokeWidth={2} strokeLinejoin="round" strokeLinecap="round" />
          </g>
        )
      })}
      {l.series.map((s) =>
        s.points.map((pt, i) => {
          const own = ownState(p.series?.[s.index]?.state)
          const state = units.paint(pt.id, own === 'done' || own === 'danger' ? own : null)
          const paint = unitPaint(state, colour(s.index), t)
          const end = slope && (i === 0 || i === s.points.length - 1)
          return (
            <g key={pt.id} opacity={paint.opacity} {...units.unit(pt.id, `${pt.label}, ${s.name}: ${withUnit(pt.value, unit)}`)}>
              <circle cx={pt.x} cy={pt.y} r={HIT / 2} fill="transparent" />
              {(s.markers || state === 'active') && <circle cx={pt.x} cy={pt.y} r={state === 'active' ? 5 : 3.5} fill={t.card} stroke={paint.stroke} strokeWidth={paint.strokeWidth} />}
              {end && (
                <text x={pt.x + (i === 0 ? -10 : 10)} y={pt.y} textAnchor={i === 0 ? 'end' : 'start'} dominantBaseline="central" fontFamily={MONO} fontSize={TICK_PX} fill={t.mutedForeground}>
                  {withUnit(pt.value, unit)}
                </text>
              )}
              {l.variant === 'bump' && i === s.points.length - 1 && (
                <text x={pt.x + 10} y={pt.y} dominantBaseline="central" fontFamily={t.font} fontSize={NAME_PX - 2} fontWeight={600} fill={t.foreground}>
                  {s.name}
                </text>
              )}
            </g>
          )
        }),
      )}
      <Legend items={items} x={l.legendX} y={l.legendY} hidden={legend?.hidden} onToggle={legend?.onToggle} tokens={t} />
    </ChartSvg>
  )
}

export function LineChartView({ props, highlight, onSelect }: DiagramHostProps<LineChartData>) {
  const base = useMemo(() => layoutLineChart(props), [props])
  const ids = useMemo(() => lineIds(base), [base])
  const incident = useCallback((id: string) => ids.filter((o) => o !== id && o.slice(0, o.indexOf('/')) === id.slice(0, id.indexOf('/'))), [ids])
  const lit = useMemo(() => expandHighlight(highlight, ids), [highlight, ids])
  const c = useChart({ ids, incident, highlight: lit, onSelect })
  const layout = useMemo(() => (c.legend.hidden.length ? layoutLineChart(props, c.legend.hidden, c.tokens) : base), [props, c.legend.hidden, c.tokens, base])
  const tip = useCallback(
    (id: string) => {
      for (const s of layout.series) {
        const pt = s.points.find((x) => x.id === id)
        if (pt) return { title: pt.label, rows: [{ label: s.name, value: layout.variant === 'bump' ? `#${num(pt.value)}` : withUnit(pt.value, props.unit ?? undefined) }] }
      }
      return null
    },
    [layout, props.unit],
  )
  return (
    <ChartHost name="LineChart" id={c.id} units={c.units} tip={tip}>
      <LineChartSvg props={props} layout={layout} tokens={c.tokens} units={c.units} id={c.id} legend={{ hidden: c.legend.hidden, onToggle: c.legend.toggle }} />
    </ChartHost>
  )
}
