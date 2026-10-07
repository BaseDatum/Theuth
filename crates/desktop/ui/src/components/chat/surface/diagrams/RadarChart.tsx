// 123 W2: `RadarChart` — radar (a polygon per series over ≤ 8 spokes, `max` or the data max as the outer ring) |
// polar (radial bars, one series) in polar coordinates (design 123 §4.2, §5.2 b). Units are the vertices
// (`axis/series`) — or the sectors in `polar` (`axis`); the legend toggles series.
import { useCallback, useMemo } from 'react'
import type { z } from 'zod'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { MONO, TICK_PX } from '@/lib/diagram/axes'
import { ChartHost, ChartSvg, HIT, ownState, PAD, titleH, useChart } from '@/lib/diagram/chart/frame'
import { polar, sector } from '@/lib/diagram/chart/geometry'
import { NAME_PX, seriesColor, unitPaint } from '@/lib/diagram/chart/series'
import { describe, num } from '@/lib/diagram/chart/text'
import { LEGEND_H, Legend, type LegendItem, legendWidth } from '@/lib/diagram/legend'
import type { DiagramHostProps, Point } from '@/lib/diagram/types'
import type { Units } from '@/lib/diagram/units'
import type { radarChartProps } from '../catalog'
import { expandHighlight } from './BarChart'

export type RadarChartData = z.infer<typeof radarChartProps>

export interface RadarVertex extends Point {
  id: string
  axis: string
  series: number
  value: number
  /** `polar`: the sector path. */
  d?: string
}

export interface RadarChartLayout {
  W: number
  H: number
  cx: number
  cy: number
  r: number
  max: number
  polar: boolean
  rings: number[]
  spokes: { label: string; end: Point; anchor: Point }[]
  series: { index: number; name: string; vertices: RadarVertex[]; d: string }[]
  legend: LegendItem[]
  legendX: number
  legendY: number
}

const R = 110
const RINGS = 4

export function layoutRadarChart(p: RadarChartData, hidden: readonly string[] = [], tokens?: DiagramTokens): RadarChartLayout {
  const axes = p.axes ?? []
  const all = p.series ?? []
  const shown = all.map((s, i) => ({ s, i })).filter(({ s }) => !hidden.includes(s.name))
  const isPolar = p.variant === 'polar'
  const dataMax = Math.max(0, ...shown.flatMap(({ s }) => (s.values ?? []).filter(Number.isFinite)))
  const max = p.max && p.max > 0 ? p.max : dataMax || 1
  const W = 2 * (R + 72) + 2 * PAD
  const top = PAD + titleH(p.title)
  const cx = W / 2
  const cy = top + R + 32
  const H = cy + R + 40 + LEGEND_H + 8
  const n = Math.max(1, axes.length)
  const spokes = axes.map((label, i) => ({ label, end: polar(cx, cy, R, i, n), anchor: polar(cx, cy, R + 16, i, n) }))
  const series = shown.map(({ s, i }) => {
    const vertices: RadarVertex[] = []
    axes.forEach((axis, k) => {
      const v = s.values?.[k]
      if (typeof v !== 'number' || !Number.isFinite(v)) return
      const f = Math.min(1, Math.max(0, v / max))
      const pt = polar(cx, cy, R, k, n, f)
      vertices.push({ id: isPolar ? axis : `${axis}/${s.name}`, axis, series: i, value: v, ...pt, ...(isPolar ? { d: sector(cx, cy, R, k, n, f) } : {}) })
    })
    const d = vertices.length ? `${vertices.map((v, k) => `${k ? 'L' : 'M'}${v.x} ${v.y}`).join(' ')} Z` : ''
    return { index: i, name: s.name, vertices, d }
  })
  const legend: LegendItem[] = all.map((s, i) => ({ id: s.name, label: s.name, color: tokens ? seriesColor(i, ownState(s.state), tokens) : '', shape: 'line' as const }))
  return {
    W,
    H,
    cx,
    cy,
    r: R,
    max,
    polar: isPolar,
    rings: Array.from({ length: RINGS }, (_, k) => (R * (k + 1)) / RINGS),
    spokes,
    series,
    legend,
    legendX: Math.max(PAD, (W - legendWidth(legend)) / 2),
    legendY: H - LEGEND_H - 4,
  }
}

export function radarIds(l: RadarChartLayout): string[] {
  const ids: string[] = []
  for (let k = 0; k < l.spokes.length; k++) for (const s of l.series) if (s.vertices[k]) ids.push(s.vertices[k]!.id)
  return ids
}

export function RadarChartSvg({
  props: p,
  layout: l,
  tokens: t,
  units,
  id,
  legend,
}: {
  props: RadarChartData
  layout: RadarChartLayout
  tokens: DiagramTokens
  units: Units
  id: string
  legend?: { hidden: readonly string[]; onToggle: (id: string) => void }
}) {
  const desc = describe('RadarChart', p.title, p.series?.length ?? 0, 'series', 'series')
  const colour = (si: number) => seriesColor(si, ownState(p.series?.[si]?.state), t)
  const items = l.legend.map((it, i) => ({ ...it, color: colour(i) }))
  const n = Math.max(1, l.spokes.length)
  return (
    <ChartSvg name="RadarChart" id={id} title={p.title} desc={desc} W={l.W} H={l.H} tokens={t} interactive root={units.root}>
      <g aria-hidden>
        {l.rings.map((r, k) => (
          <polygon key={r} points={l.spokes.map((_, i) => polar(l.cx, l.cy, r, i, n)).map((q) => `${q.x},${q.y}`).join(' ')} fill="none" stroke={t.border} strokeOpacity={k === l.rings.length - 1 ? 1 : 0.4} />
        ))}
        {l.spokes.map((s) => (
          <line key={s.label} x1={l.cx} y1={l.cy} x2={s.end.x} y2={s.end.y} stroke={t.border} strokeOpacity={0.4} />
        ))}
        {l.spokes.map((s) => (
          <text key={s.label} x={s.anchor.x} y={s.anchor.y} textAnchor={Math.abs(s.anchor.x - l.cx) < 1 ? 'middle' : s.anchor.x < l.cx ? 'end' : 'start'} dominantBaseline="central" fontFamily={t.font} fontSize={NAME_PX - 2} fontWeight={600} fill={t.foreground}>
            {s.label}
          </text>
        ))}
        <text x={l.cx + 4} y={l.cy - l.r - 4} fontFamily={MONO} fontSize={TICK_PX} fill={t.mutedForeground}>
          {num(l.max)}
        </text>
      </g>
      {!l.polar &&
        l.series.map((s) => {
          const c = colour(s.index)
          const dim = units.state.selected !== null && !s.vertices.some((v) => units.paint(v.id, null) === 'active')
          return <path key={s.name} d={s.d} fill={c} fillOpacity={0.12} stroke={c} strokeWidth={2} strokeLinejoin="round" opacity={dim ? 0.5 : 1} />
        })}
      {l.series.map((s) =>
        s.vertices.map((v) => {
          const own = ownState(p.series?.[s.index]?.state)
          const state = units.paint(v.id, own === 'done' || own === 'danger' ? own : null)
          const paint = unitPaint(state, colour(s.index), t)
          return (
            <g key={v.id} opacity={paint.opacity} {...units.unit(v.id, `${v.axis}, ${s.name}: ${num(v.value)}`)}>
              {l.polar && v.d ? (
                <path d={v.d} fill={paint.fill} fillOpacity={0.3} stroke={paint.stroke} strokeWidth={paint.strokeWidth} strokeLinejoin="round" />
              ) : (
                <circle cx={v.x} cy={v.y} r={HIT / 2} fill="transparent" />
              )}
              {!l.polar && <circle cx={v.x} cy={v.y} r={state === 'active' ? 5 : 3.5} fill={t.card} stroke={paint.stroke} strokeWidth={paint.strokeWidth} />}
            </g>
          )
        }),
      )}
      <Legend items={items} x={l.legendX} y={l.legendY} hidden={legend?.hidden} onToggle={legend?.onToggle} tokens={t} />
    </ChartSvg>
  )
}

export function RadarChartView({ props, highlight, onSelect }: DiagramHostProps<RadarChartData>) {
  const base = useMemo(() => layoutRadarChart(props), [props])
  const ids = useMemo(() => radarIds(base), [base])
  const incident = useCallback((id: string) => ids.filter((o) => o !== id && o.slice(0, o.indexOf('/')) === id.slice(0, id.indexOf('/'))), [ids])
  const lit = useMemo(() => expandHighlight(highlight, ids), [highlight, ids])
  const c = useChart({ ids, incident, highlight: lit, onSelect })
  const layout = useMemo(() => (c.legend.hidden.length ? layoutRadarChart(props, c.legend.hidden, c.tokens) : base), [props, c.legend.hidden, c.tokens, base])
  const tip = useCallback(
    (id: string) => {
      for (const s of layout.series) {
        const v = s.vertices.find((x) => x.id === id)
        if (v) return { title: v.axis, rows: [{ label: s.name, value: `${num(v.value)} / ${num(layout.max)}` }] }
      }
      return null
    },
    [layout],
  )
  return (
    <ChartHost name="RadarChart" id={c.id} units={c.units} tip={tip}>
      <RadarChartSvg props={props} layout={layout} tokens={c.tokens} units={c.units} id={c.id} legend={{ hidden: c.legend.hidden, onToggle: c.legend.toggle }} />
    </ChartHost>
  )
}
