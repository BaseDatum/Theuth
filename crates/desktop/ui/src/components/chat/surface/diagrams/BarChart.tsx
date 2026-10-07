// 123 W2: `BarChart` — grouped | stacked | dumbbell | horizontal on the chart kernel (design 123 §4.2, §5.2 b).
// `layoutBarChart` is the pure geometry, `BarChartSvg` the pure drawing (snapshot-tested with `staticUnits()`),
// `BarChartView` the host the registry and the apps mount. Every bar is a unit (`label/series`); highlighting a
// bare label lights all its bars; selecting one lights its category.
import { useCallback, useMemo } from 'react'
import type { z } from 'zod'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { Axis, AXIS_INSET, Gridlines, MONO, type Tick, TICK_PX } from '@/lib/diagram/axes'
import { CHART_W, ChartHost, ChartSvg, HIT, ownState, PAD, titleH, useChart } from '@/lib/diagram/chart/frame'
import { extent, type Rect } from '@/lib/diagram/chart/geometry'
import { seriesColor, unitPaint } from '@/lib/diagram/chart/series'
import { describe, withUnit } from '@/lib/diagram/chart/text'
import { LEGEND_H, Legend, type LegendItem, legendWidth } from '@/lib/diagram/legend'
import { band, linear, niceDomain } from '@/lib/diagram/scale'
import type { DiagramHostProps, UnitState } from '@/lib/diagram/types'
import type { Units } from '@/lib/diagram/units'
import type { barChartProps } from '../catalog'

export type BarChartData = z.infer<typeof barChartProps>

export interface Bar {
  id: string
  label: string
  series: number
  value: number
  rect: Rect
  /** Where the value label sits. */
  at: { x: number; y: number }
}

export interface BarChartLayout {
  W: number
  H: number
  plot: Rect
  horizontal: boolean
  dumbbell: boolean
  bars: Bar[]
  /** Dumbbell connectors, one per label with both ends present. */
  links: { label: string; x1: number; y1: number; x2: number; y2: number }[]
  valueTicks: Tick[]
  catTicks: Tick[]
  /** Pixel position of the zero line along the value axis. */
  zero: number
  showValues: boolean
  legend: LegendItem[]
  legendX: number
  legendY: number
}

const ROW = 28
const PLOT_H = 200

/** Geometry for `p` with `hidden` series left out; tolerant of short `values` (an open fence). */
export function layoutBarChart(p: BarChartData, hidden: readonly string[] = [], tokens?: DiagramTokens): BarChartLayout {
  const labels = p.labels ?? []
  const all = p.series ?? []
  const shown = all.map((s, i) => ({ s, i })).filter(({ s }) => !hidden.includes(s.name))
  const variant = p.variant ?? 'grouped'
  const dumbbell = variant === 'dumbbell' && shown.length === 2
  const stacked = variant === 'stacked'
  const horizontal = variant === 'horizontal'
  const value = (si: number, li: number): number | null => {
    const v = all[si]?.values?.[li]
    return typeof v === 'number' && Number.isFinite(v) ? v : null
  }
  // Value domain: stacked sums positives and negatives separately.
  const values: number[] = []
  for (let li = 0; li < labels.length; li++) {
    let pos = 0
    let neg = 0
    for (const { i } of shown) {
      const v = value(i, li)
      if (v === null) continue
      if (stacked) {
        if (v >= 0) pos += v
        else neg += v
      } else values.push(v)
    }
    if (stacked) values.push(pos, neg)
  }
  const [lo, hi] = niceDomain(...extent(values))
  const left = p.yLabel || horizontal ? AXIS_INSET.left : AXIS_INSET.leftNoTitle
  const bottom = (p.xLabel ? AXIS_INSET.bottom : AXIS_INSET.bottomNoTitle) + 8 + LEGEND_H
  const top = PAD + titleH(p.title) + 12
  const perLabel = dumbbell || stacked ? 1 : Math.max(1, shown.length)
  const W = horizontal ? CHART_W : Math.max(CHART_W, left + PAD + labels.length * (perLabel * 14 + 16))
  const plotH = horizontal ? Math.max(120, labels.length * ROW) : PLOT_H
  const H = top + plotH + bottom
  const plot: Rect = { x: left, y: top, w: W - left - PAD, h: plotH }
  const cats = band(
    labels.map((_, i) => String(i)),
    horizontal ? [plot.y, plot.y + plot.h] : [plot.x, plot.x + plot.w],
    0.25,
  )
  const val = linear([lo, hi], horizontal ? [plot.x, plot.x + plot.w] : [plot.y + plot.h, plot.y])
  const zero = val(Math.min(Math.max(0, lo), hi))
  const bars: Bar[] = []
  const links: BarChartLayout['links'] = []
  const slot = cats.bandwidth / perLabel
  for (let li = 0; li < labels.length; li++) {
    const label = labels[li]!
    const start = cats(String(li))
    let pos = 0
    let neg = 0
    const ends: { x: number; y: number }[] = []
    shown.forEach(({ s, i }, k) => {
      const v = value(i, li)
      if (v === null) return
      const id = `${label}/${s.name}`
      let a: number
      let b: number
      if (stacked) {
        if (v >= 0) {
          a = pos
          pos += v
          b = pos
        } else {
          b = neg
          neg += v
          a = neg
        }
      } else {
        a = Math.min(0, v)
        b = Math.max(0, v)
      }
      const across = dumbbell || stacked ? start : start + k * slot
      const thick = dumbbell ? Math.min(cats.bandwidth, 10) : stacked ? cats.bandwidth : Math.max(2, slot - 2)
      const lenA = val(a)
      const lenB = val(b)
      let rect: Rect
      let at: { x: number; y: number }
      if (dumbbell) {
        const c = val(v)
        rect = horizontal ? { x: c - 5, y: start + cats.bandwidth / 2 - 5, w: 10, h: 10 } : { x: start + cats.bandwidth / 2 - 5, y: c - 5, w: 10, h: 10 }
        at = horizontal ? { x: c, y: rect.y - 6 } : { x: rect.x + 5 + (k === 0 ? -12 : 12), y: c }
        ends.push({ x: rect.x + 5, y: rect.y + 5 })
      } else if (horizontal) {
        rect = { x: Math.min(lenA, lenB), y: across + (stacked ? 0 : 1), w: Math.abs(lenB - lenA), h: thick }
        at = { x: v >= 0 ? rect.x + rect.w + 4 : rect.x - 4, y: rect.y + rect.h / 2 }
      } else {
        rect = { x: across + (stacked ? 0 : 1), y: Math.min(lenA, lenB), w: thick, h: Math.abs(lenB - lenA) }
        at = { x: rect.x + rect.w / 2, y: v >= 0 ? rect.y - 4 : rect.y + rect.h + TICK_PX }
      }
      bars.push({ id, label, series: i, value: v, rect, at })
    })
    if (dumbbell && ends.length === 2) links.push({ label, x1: ends[0]!.x, y1: ends[0]!.y, x2: ends[1]!.x, y2: ends[1]!.y })
  }
  const valueTicks: Tick[] = val.ticks(horizontal ? 6 : 5).map((v) => ({ at: val(v), label: withUnit(v, undefined) }))
  const catTicks: Tick[] = labels.map((l, i) => ({ at: cats(String(i)) + cats.bandwidth / 2, label: l }))
  const legend: LegendItem[] = all.map((s, i) => ({ id: s.name, label: s.name, color: tokens ? seriesColor(i, ownState(s.state), tokens) : '', shape: 'swatch' as const }))
  return {
    W,
    H,
    plot,
    horizontal,
    dumbbell,
    bars,
    links,
    valueTicks,
    catTicks,
    zero,
    showValues: bars.length <= 12 && !dumbbell,
    legend,
    legendX: Math.max(PAD, (W - legendWidth(legend)) / 2),
    legendY: H - LEGEND_H - 4,
  }
}

/** The ids of all bars, authored order (label-major). */
export function barIds(l: BarChartLayout): string[] {
  return l.bars.map((b) => b.id)
}

export function BarChartSvg({
  props: p,
  layout: l,
  tokens: t,
  units,
  id,
  legend,
}: {
  props: BarChartData
  layout: BarChartLayout
  tokens: DiagramTokens
  units: Units
  id: string
  legend?: { hidden: readonly string[]; onToggle: (id: string) => void }
}) {
  const desc = describe('BarChart', p.title, p.series?.length ?? 0, 'series', 'series')
  const colour = (si: number) => seriesColor(si, ownState(p.series?.[si]?.state), t)
  const unit = p.unit ?? undefined
  const items = l.legend.map((it, i) => ({ ...it, color: colour(i) }))
  return (
    <ChartSvg name="BarChart" id={id} title={p.title} desc={desc} W={l.W} H={l.H} tokens={t} interactive root={units.root}>
      <Gridlines orient={l.horizontal ? 'vertical' : 'horizontal'} at={l.valueTicks.map((k) => k.at)} span={l.horizontal ? [l.plot.y, l.plot.y + l.plot.h] : [l.plot.x, l.plot.x + l.plot.w]} tokens={t} />
      {l.horizontal ? (
        <>
          <Axis orient="left" at={l.plot.x} span={[l.plot.y, l.plot.y + l.plot.h]} ticks={l.catTicks} tokens={t} title={p.yLabel} />
          <Axis orient="bottom" at={l.plot.y + l.plot.h} span={[l.plot.x, l.plot.x + l.plot.w]} ticks={l.valueTicks} tokens={t} title={p.xLabel ?? unit} />
        </>
      ) : (
        <>
          <Axis orient="left" at={l.plot.x} span={[l.plot.y, l.plot.y + l.plot.h]} ticks={l.valueTicks} tokens={t} title={p.yLabel ?? unit} />
          <Axis orient="bottom" at={l.plot.y + l.plot.h} span={[l.plot.x, l.plot.x + l.plot.w]} ticks={l.catTicks} tokens={t} title={p.xLabel} />
        </>
      )}
      {l.links.map((k) => (
        <line key={k.label} x1={k.x1} y1={k.y1} x2={k.x2} y2={k.y2} stroke={t.mutedForeground} strokeWidth={1.5} />
      ))}
      {l.bars.map((b) => {
        const own = ownState(p.series?.[b.series]?.state)
        const state: UnitState = units.paint(b.id, own === 'active' || own === 'done' || own === 'danger' ? own : null)
        const s = unitPaint(state, colour(b.series), t)
        const { rect } = b
        const padX = Math.max(0, (HIT - rect.w) / 2)
        const padY = Math.max(0, (HIT - rect.h) / 2)
        return (
          <g key={b.id} opacity={s.opacity} {...units.unit(b.id, `${b.label}, ${p.series?.[b.series]?.name ?? ''}: ${withUnit(b.value, unit)}`)}>
            <rect x={rect.x - padX} y={rect.y - padY} width={rect.w + 2 * padX} height={rect.h + 2 * padY} fill="transparent" />
            {l.dumbbell ? (
              <circle cx={rect.x + rect.w / 2} cy={rect.y + rect.h / 2} r={5} fill={s.fill} fillOpacity={0.9} stroke={s.stroke} strokeWidth={s.strokeWidth} />
            ) : (
              <rect x={rect.x} y={rect.y} width={rect.w} height={rect.h} rx={2} fill={s.fill} fillOpacity={s.fillOpacity} stroke={s.stroke} strokeWidth={s.strokeWidth} />
            )}
            {l.showValues && (
              <text x={b.at.x} y={b.at.y} textAnchor={l.horizontal ? (b.value >= 0 ? 'start' : 'end') : 'middle'} dominantBaseline={l.horizontal ? 'central' : undefined} fontFamily={MONO} fontSize={TICK_PX} fill={t.mutedForeground}>
                {withUnit(b.value, unit)}
              </text>
            )}
          </g>
        )
      })}
      {l.dumbbell &&
        l.bars.map((b) => (
          <text key={`${b.id}-v`} x={b.at.x} y={b.at.y} textAnchor={l.horizontal ? 'middle' : b.at.x < b.rect.x ? 'end' : 'start'} dominantBaseline="central" fontFamily={MONO} fontSize={TICK_PX} fill={t.mutedForeground}>
            {withUnit(b.value, unit)}
          </text>
        ))}
      <line
        x1={l.horizontal ? l.zero : l.plot.x}
        y1={l.horizontal ? l.plot.y : l.zero}
        x2={l.horizontal ? l.zero : l.plot.x + l.plot.w}
        y2={l.horizontal ? l.plot.y + l.plot.h : l.zero}
        stroke={t.border}
        strokeWidth={1}
        shapeRendering="crispEdges"
      />
      <Legend items={items} x={l.legendX} y={l.legendY} hidden={legend?.hidden} onToggle={legend?.onToggle} tokens={t} />
    </ChartSvg>
  )
}

/** `highlight` entries naming a bare label light every bar of that label. */
export function expandHighlight(highlight: readonly string[] | undefined, ids: readonly string[]): readonly string[] | undefined {
  if (!highlight?.length) return highlight
  return ids.filter((id) => highlight.includes(id) || highlight.includes(id.slice(0, id.indexOf('/'))))
}

export function BarChartView({ props, highlight, onSelect }: DiagramHostProps<BarChartData>) {
  const base = useMemo(() => layoutBarChart(props), [props])
  const ids = useMemo(() => barIds(base), [base])
  const incident = useCallback((id: string) => ids.filter((o) => o !== id && o.slice(0, o.indexOf('/')) === id.slice(0, id.indexOf('/'))), [ids])
  const lit = useMemo(() => expandHighlight(highlight, ids), [highlight, ids])
  const c = useChart({ ids, incident, highlight: lit, onSelect })
  const layout = useMemo(() => (c.legend.hidden.length ? layoutBarChart(props, c.legend.hidden, c.tokens) : base), [props, c.legend.hidden, c.tokens, base])
  const tip = useCallback(
    (id: string) => {
      const b = layout.bars.find((x) => x.id === id)
      if (!b) return null
      return { title: b.label, rows: [{ label: props.series?.[b.series]?.name ?? '', value: withUnit(b.value, props.unit ?? undefined) }] }
    },
    [layout, props],
  )
  return (
    <ChartHost name="BarChart" id={c.id} units={c.units} tip={tip}>
      <BarChartSvg props={props} layout={layout} tokens={c.tokens} units={c.units} id={c.id} legend={{ hidden: c.legend.hidden, onToggle: c.legend.toggle }} />
    </ChartHost>
  )
}
