// 123 W2: `Quadrant` — a fixed 2×2 with quadrant captions, items placed by `x`/`y` in 0..1 (`size` → area), the
// `xTicks`/`yTicks` low/high words at both axis ends (design 123 §4.2, §5.2 b). Each item is a unit (`id`, else
// its label); the twin of `quadrant_of` in `charts.rs` decides which cell an item belongs to.
import { useCallback, useMemo } from 'react'
import type { z } from 'zod'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { MONO, TICK_PX, TITLE_PX } from '@/lib/diagram/axes'
import { ChartHost, ChartSvg, HIT, ownState, PAD, titleH, useChart } from '@/lib/diagram/chart/frame'
import { bubbleRadius, quadrantOf, type Rect } from '@/lib/diagram/chart/geometry'
import { NAME_PX, unitPaint } from '@/lib/diagram/chart/series'
import { describe, num } from '@/lib/diagram/chart/text'
import type { DiagramHostProps } from '@/lib/diagram/types'
import type { Units } from '@/lib/diagram/units'
import type { quadrantProps } from '../catalog'

export type QuadrantData = z.infer<typeof quadrantProps>

export interface QuadrantDot {
  id: string
  index: number
  x: number
  y: number
  r: number
  cell: 0 | 1 | 2 | 3
}

export interface QuadrantLayout {
  W: number
  H: number
  plot: Rect
  dots: QuadrantDot[]
  /** Caption anchors for tl, tr, bl, br. */
  captions: { x: number; y: number; anchor: 'start' | 'end' }[]
}

const SIZE = 320
const LEFT = 56
const BOTTOM = 44
const DOT_R = 5
const MAX_R = 14

export function itemId(p: QuadrantData, i: number): string {
  const it = p.items?.[i]
  return it?.id ?? it?.label ?? String(i)
}

export function layoutQuadrant(p: QuadrantData): QuadrantLayout {
  const items = (p.items ?? []).map((it, index) => ({ it, index })).filter(({ it }) => Number.isFinite(it.x) && Number.isFinite(it.y))
  const top = PAD + titleH(p.title) + 8
  const W = LEFT + SIZE + PAD + 40
  const H = top + SIZE + BOTTOM
  const plot: Rect = { x: LEFT, y: top, w: SIZE + 40, h: SIZE }
  const maxSize = Math.max(0, ...items.map(({ it }) => it.size ?? 0))
  const inset = maxSize > 0 ? MAX_R : DOT_R
  const px = (v: number) => plot.x + inset + Math.min(1, Math.max(0, v)) * (plot.w - 2 * inset)
  const py = (v: number) => plot.y + plot.h - inset - Math.min(1, Math.max(0, v)) * (plot.h - 2 * inset)
  const dots: QuadrantDot[] = items.map(({ it, index }) => ({
    id: itemId(p, index),
    index,
    x: px(it.x),
    y: py(it.y),
    r: it.size != null ? bubbleRadius(it.size, maxSize, MAX_R, DOT_R) : DOT_R,
    cell: quadrantOf(it.x, it.y),
  }))
  const cy = plot.y + plot.h / 2
  const captions: QuadrantLayout['captions'] = [
    { x: plot.x + 8, y: plot.y + 16, anchor: 'start' },
    { x: plot.x + plot.w - 8, y: plot.y + 16, anchor: 'end' },
    { x: plot.x + 8, y: cy + 16, anchor: 'start' },
    { x: plot.x + plot.w - 8, y: cy + 16, anchor: 'end' },
  ]
  return { W, H, plot, dots, captions }
}

export function QuadrantSvg({ props: p, layout: l, tokens: t, units, id }: { props: QuadrantData; layout: QuadrantLayout; tokens: DiagramTokens; units: Units; id: string }) {
  const desc = describe('Quadrant', p.title, p.items?.length ?? 0, 'item')
  const names = p.quadrants?.length === 4 ? p.quadrants : null
  const { plot } = l
  const cx = plot.x + plot.w / 2
  const cy = plot.y + plot.h / 2
  const xt = p.xTicks?.length === 2 ? p.xTicks : null
  const yt = p.yTicks?.length === 2 ? p.yTicks : null
  const hovered = units.state.hover ?? units.state.selected
  return (
    <ChartSvg name="Quadrant" id={id} title={p.title} desc={desc} W={l.W} H={l.H} tokens={t} interactive root={units.root}>
      <g aria-hidden>
        <rect x={plot.x} y={plot.y} width={plot.w} height={plot.h} rx={8} fill={t.card} stroke={t.border} />
        <line x1={cx} y1={plot.y} x2={cx} y2={plot.y + plot.h} stroke={t.border} shapeRendering="crispEdges" />
        <line x1={plot.x} y1={cy} x2={plot.x + plot.w} y2={cy} stroke={t.border} shapeRendering="crispEdges" />
        {names?.map((n, i) => (
          <text key={i} x={l.captions[i]!.x} y={l.captions[i]!.y} textAnchor={l.captions[i]!.anchor} fontFamily={t.font} fontSize={TITLE_PX} fontWeight={600} fill={t.mutedForeground}>
            {n}
          </text>
        ))}
        <text x={cx} y={plot.y + plot.h + 30} textAnchor="middle" fontFamily={t.font} fontSize={TITLE_PX} fill={t.mutedForeground}>
          {p.xLabel}
        </text>
        <text transform={`translate(${plot.x - 40} ${cy}) rotate(-90)`} textAnchor="middle" fontFamily={t.font} fontSize={TITLE_PX} fill={t.mutedForeground}>
          {p.yLabel}
        </text>
        {xt && (
          <>
            <text x={plot.x} y={plot.y + plot.h + 14} fontFamily={MONO} fontSize={TICK_PX} fill={t.mutedForeground}>
              {xt[0]}
            </text>
            <text x={plot.x + plot.w} y={plot.y + plot.h + 14} textAnchor="end" fontFamily={MONO} fontSize={TICK_PX} fill={t.mutedForeground}>
              {xt[1]}
            </text>
          </>
        )}
        {yt && (
          <>
            <text x={plot.x - 6} y={plot.y + plot.h} textAnchor="end" dominantBaseline="central" fontFamily={MONO} fontSize={TICK_PX} fill={t.mutedForeground}>
              {yt[0]}
            </text>
            <text x={plot.x - 6} y={plot.y} textAnchor="end" dominantBaseline="central" fontFamily={MONO} fontSize={TICK_PX} fill={t.mutedForeground}>
              {yt[1]}
            </text>
          </>
        )}
      </g>
      {[...l.dots]
        .sort((a, b) => b.r - a.r)
        .map((d) => {
          const it = p.items?.[d.index]
          const s = unitPaint(units.paint(d.id, ownState(it?.state)), t.foreground, t)
          const label = it?.label ?? d.id
          const right = d.x < cx
          return (
            <g key={d.id} opacity={s.opacity} {...units.unit(d.id, `${label}, ${names?.[d.cell] ?? ['top-left', 'top-right', 'bottom-left', 'bottom-right'][d.cell]}`)}>
              <circle cx={d.x} cy={d.y} r={Math.max(HIT / 2, d.r)} fill="transparent" />
              <circle cx={d.x} cy={d.y} r={d.r} fill={s.fill} fillOpacity={0.9} stroke={s.stroke} strokeWidth={s.strokeWidth} />
              <text x={d.x + (right ? d.r + 5 : -(d.r + 5))} y={d.y} textAnchor={right ? 'start' : 'end'} dominantBaseline="central" fontFamily={t.font} fontSize={NAME_PX - 1} fontWeight={hovered === d.id ? 600 : 500} fill={t.foreground}>
                {label.length > 18 ? `${label.slice(0, 17)}…` : label}
              </text>
            </g>
          )
        })}
    </ChartSvg>
  )
}

export function QuadrantView({ props, highlight, onSelect }: DiagramHostProps<QuadrantData>) {
  const layout = useMemo(() => layoutQuadrant(props), [props])
  const ids = useMemo(() => layout.dots.map((d) => d.id), [layout])
  const c = useChart({ ids, highlight, onSelect })
  const tip = useCallback(
    (id: string) => {
      const d = layout.dots.find((x) => x.id === id)
      const it = d && props.items?.[d.index]
      if (!d || !it) return null
      return {
        title: it.label,
        rows: [
          { label: props.xLabel, value: num(it.x) },
          { label: props.yLabel, value: num(it.y) },
          { label: 'size', value: it.size != null ? num(it.size) : null },
          { label: 'quadrant', value: props.quadrants?.[d.cell] ?? null },
        ],
      }
    },
    [layout, props],
  )
  return (
    <ChartHost name="Quadrant" id={c.id} units={c.units} tip={tip}>
      <QuadrantSvg props={props} layout={layout} tokens={c.tokens} units={c.units} id={c.id} />
    </ChartHost>
  )
}
