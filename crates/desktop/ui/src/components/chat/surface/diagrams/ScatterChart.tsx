// 123 W2: `ScatterChart` — scatter | bubble (area ∝ size, a legend of sizes) | beeswarm (one axis, a deterministic
// dodge sweep) on the chart kernel (design 123 §4.2, §5.2 b). Points are units (`id` = index, or `label` when
// given); groups colour from the categorical palette and toggle through the legend; labels draw when ≤ 20 points,
// else only for the hovered/selected point.
import { useCallback, useMemo } from 'react'
import type { z } from 'zod'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { Axis, AXIS_INSET, Gridlines, MONO, type Tick, TICK_PX } from '@/lib/diagram/axes'
import { CHART_W, ChartHost, ChartSvg, HIT, ownState, PAD, titleH, useChart } from '@/lib/diagram/chart/frame'
import { beeswarm, bubbleRadius, extent, type Rect } from '@/lib/diagram/chart/geometry'
import { seriesColor, unitPaint } from '@/lib/diagram/chart/series'
import { describe, num } from '@/lib/diagram/chart/text'
import { LEGEND_H, Legend, type LegendItem, legendWidth } from '@/lib/diagram/legend'
import { linear, niceDomain } from '@/lib/diagram/scale'
import type { DiagramHostProps } from '@/lib/diagram/types'
import type { Units } from '@/lib/diagram/units'
import type { scatterChartProps } from '../catalog'

export type ScatterChartData = z.infer<typeof scatterChartProps>

export interface Dot {
  id: string
  index: number
  x: number
  y: number
  r: number
  group: number
}

export interface ScatterChartLayout {
  W: number
  H: number
  plot: Rect
  variant: NonNullable<ScatterChartData['variant']>
  dots: Dot[]
  xTicks: Tick[]
  yTicks: Tick[]
  showLabels: boolean
  legend: LegendItem[]
  /** Bubble size legend: radius + size value, largest first. */
  sizes: { r: number; size: number }[]
  legendX: number
  legendY: number
}

const PLOT_H = 220
const LABEL_MAX = 20
const DOT_R = 5
const BUBBLE_R = 22

export function pointId(p: ScatterChartData, i: number): string {
  return p.points?.[i]?.label ?? String(i)
}

/** Geometry for `p` with `hidden` groups left out. */
export function layoutScatterChart(p: ScatterChartData, hidden: readonly string[] = [], tokens?: DiagramTokens): ScatterChartLayout {
  const variant = p.variant ?? 'scatter'
  const groups = p.groups ?? []
  const groupIndex = (g: string | null | undefined) => (g ? groups.findIndex((x) => x.id === g) : -1)
  const pts = (p.points ?? [])
    .map((pt, index) => ({ pt, index }))
    .filter(({ pt }) => Number.isFinite(pt.x) && Number.isFinite(pt.y) && !(pt.group && hidden.includes(pt.group)))
  const swarm = variant === 'beeswarm'
  const bubble = variant === 'bubble'
  const left = swarm ? PAD : AXIS_INSET.left
  const bottom = AXIS_INSET.bottom + 8 + LEGEND_H
  const top = PAD + titleH(p.title) + 12
  const W = CHART_W
  const plotH = swarm ? 120 : PLOT_H
  const H = top + plotH + bottom
  const plot: Rect = { x: left, y: top, w: W - left - PAD, h: plotH }
  const inset = bubble ? BUBBLE_R : DOT_R + 2
  const xd = niceDomain(...extent(pts.map(({ pt }) => pt.x), false))
  const yd = niceDomain(...extent(pts.map(({ pt }) => pt.y), false))
  const xs = linear(xd, [plot.x + inset, plot.x + plot.w - inset])
  const ys = linear(yd, [plot.y + plot.h - inset, plot.y + inset])
  const maxSize = Math.max(0, ...pts.map(({ pt }) => pt.size ?? 0))
  const radius = (pt: { size?: number | null }) => (bubble ? bubbleRadius(pt.size ?? 0, maxSize, BUBBLE_R) : DOT_R)
  const offsets = swarm ? beeswarm(pts.map(({ pt }) => xs(pt.x)), DOT_R + 1) : null
  const dots: Dot[] = pts.map(({ pt, index }, k) => ({
    id: pointId(p, index),
    index,
    x: xs(pt.x),
    y: swarm ? plot.y + plot.h / 2 + offsets![k]! : ys(pt.y),
    r: radius(pt),
    group: groupIndex(pt.group),
  }))
  const xTicks: Tick[] = xs.ticks(6).map((v) => ({ at: xs(v), label: num(v) }))
  const yTicks: Tick[] = swarm ? [] : ys.ticks(5).map((v) => ({ at: ys(v), label: num(v) }))
  const legend: LegendItem[] = groups.map((g, i) => ({ id: g.id, label: g.label, color: tokens ? seriesColor(i, null, tokens) : '', shape: 'swatch' as const }))
  const sizes = bubble && maxSize > 0 ? [maxSize, maxSize / 2].map((size) => ({ r: bubbleRadius(size, maxSize, BUBBLE_R), size })) : []
  return {
    W,
    H,
    plot,
    variant,
    dots,
    xTicks,
    yTicks,
    showLabels: pts.length <= LABEL_MAX,
    legend,
    sizes,
    legendX: Math.max(PAD, (W - legendWidth(legend)) / 2),
    legendY: H - LEGEND_H - 4,
  }
}

export function ScatterChartSvg({
  props: p,
  layout: l,
  tokens: t,
  units,
  id,
  legend,
}: {
  props: ScatterChartData
  layout: ScatterChartLayout
  tokens: DiagramTokens
  units: Units
  id: string
  legend?: { hidden: readonly string[]; onToggle: (id: string) => void }
}) {
  const desc = describe('ScatterChart', p.title, p.points?.length ?? 0, 'point')
  const colour = (g: number) => (g < 0 ? t.foreground : seriesColor(g, null, t))
  const items = l.legend.map((it, i) => ({ ...it, color: colour(i) }))
  const swarm = l.variant === 'beeswarm'
  const label = (d: Dot) => p.points?.[d.index]?.label
  const hovered = units.state.hover ?? units.state.selected
  return (
    <ChartSvg name="ScatterChart" id={id} title={p.title} desc={desc} W={l.W} H={l.H} tokens={t} interactive root={units.root}>
      {!swarm && <Gridlines orient="horizontal" at={l.yTicks.map((k) => k.at)} span={[l.plot.x, l.plot.x + l.plot.w]} tokens={t} />}
      <Gridlines orient="vertical" at={l.xTicks.map((k) => k.at)} span={[l.plot.y, l.plot.y + l.plot.h]} tokens={t} />
      {!swarm && <Axis orient="left" at={l.plot.x} span={[l.plot.y, l.plot.y + l.plot.h]} ticks={l.yTicks} tokens={t} title={p.yLabel} />}
      <Axis orient="bottom" at={l.plot.y + l.plot.h} span={[l.plot.x, l.plot.x + l.plot.w]} ticks={l.xTicks} tokens={t} title={p.xLabel} />
      {[...l.dots]
        .sort((a, b) => b.r - a.r)
        .map((d) => {
          const pt = p.points?.[d.index]
          const state = units.paint(d.id, ownState(pt?.state))
          const s = unitPaint(state, colour(d.group), t)
          const text = label(d)
          const show = text && (l.showLabels || hovered === d.id)
          const aria = [text ?? `point ${d.index + 1}`, `${p.xLabel} ${num(pt?.x ?? 0)}`, swarm ? '' : `${p.yLabel} ${num(pt?.y ?? 0)}`, pt?.size != null ? `size ${num(pt.size)}` : ''].filter(Boolean).join(', ')
          return (
            <g key={d.id} opacity={s.opacity} {...units.unit(d.id, aria)}>
              <circle cx={d.x} cy={d.y} r={Math.max(HIT / 2, d.r)} fill="transparent" />
              <circle cx={d.x} cy={d.y} r={d.r} fill={s.fill} fillOpacity={l.variant === 'bubble' ? 0.12 : 0.9} stroke={s.stroke} strokeWidth={s.strokeWidth} />
              {show && (
                <text x={d.x + d.r + 4} y={d.y} dominantBaseline="central" fontFamily={MONO} fontSize={TICK_PX} fill={t.mutedForeground}>
                  {text}
                </text>
              )}
            </g>
          )
        })}
      {l.sizes.length > 0 && (
        <g aria-hidden transform={`translate(${l.plot.x + l.plot.w - BUBBLE_R - PAD} ${l.plot.y + BUBBLE_R + 4})`}>
          {l.sizes.map((s) => (
            <g key={s.size}>
              <circle cx={0} cy={BUBBLE_R - s.r} r={s.r} fill="none" stroke={t.mutedForeground} strokeDasharray="3 2" />
              <text x={BUBBLE_R + 4} y={BUBBLE_R - 2 * s.r} dominantBaseline="central" fontFamily={MONO} fontSize={TICK_PX} fill={t.mutedForeground}>
                {num(s.size)}
              </text>
            </g>
          ))}
        </g>
      )}
      <Legend items={items} x={l.legendX} y={l.legendY} hidden={legend?.hidden} onToggle={legend?.onToggle} tokens={t} />
    </ChartSvg>
  )
}

export function ScatterChartView({ props, highlight, onSelect }: DiagramHostProps<ScatterChartData>) {
  const base = useMemo(() => layoutScatterChart(props), [props])
  const ids = useMemo(() => base.dots.map((d) => d.id), [base])
  const c = useChart({ ids, highlight, onSelect })
  const layout = useMemo(() => (c.legend.hidden.length ? layoutScatterChart(props, c.legend.hidden, c.tokens) : base), [props, c.legend.hidden, c.tokens, base])
  const tip = useCallback(
    (id: string) => {
      const d = layout.dots.find((x) => x.id === id)
      const pt = d && props.points?.[d.index]
      if (!d || !pt) return null
      const group = pt.group ? (props.groups?.find((g) => g.id === pt.group)?.label ?? pt.group) : null
      return {
        title: pt.label ?? `Point ${d.index + 1}`,
        rows: [
          { label: props.xLabel, value: num(pt.x) },
          ...(layout.variant === 'beeswarm' ? [] : [{ label: props.yLabel, value: num(pt.y) }]),
          { label: 'size', value: pt.size != null ? num(pt.size) : null },
          { label: 'group', value: group },
        ],
      }
    },
    [layout, props],
  )
  return (
    <ChartHost name="ScatterChart" id={c.id} units={c.units} tip={tip}>
      <ScatterChartSvg props={props} layout={layout} tokens={c.tokens} units={c.units} id={c.id} legend={{ hidden: c.legend.hidden, onToggle: c.legend.toggle }} />
    </ChartHost>
  )
}
