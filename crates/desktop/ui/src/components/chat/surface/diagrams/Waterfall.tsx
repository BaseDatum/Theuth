// 123 W2: `Waterfall` — the running total is computed here (`runningTotal`, the twin of `running_total` in
// `charts.rs`; one test pins both to the same sequence): `delta` bars float from the previous level, `total` /
// `subtotal` bars stand on the baseline, dim connectors carry the level across (design 123 §4.2, §5.2 b). Each
// step is a unit whose id is its index as a string (the apps contract, `diagrams.ts`).
import { useCallback, useMemo } from 'react'
import type { z } from 'zod'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { Axis, AXIS_INSET, Gridlines, MONO, type Tick, TICK_PX } from '@/lib/diagram/axes'
import { CHART_W, ChartHost, ChartSvg, HIT, ownState, PAD, titleH, useChart } from '@/lib/diagram/chart/frame'
import { extent, type Rect, runningTotal } from '@/lib/diagram/chart/geometry'
import { unitPaint } from '@/lib/diagram/chart/series'
import { describe, signed, withUnit } from '@/lib/diagram/chart/text'
import { band, linear, niceDomain } from '@/lib/diagram/scale'
import type { DiagramHostProps } from '@/lib/diagram/types'
import type { Units } from '@/lib/diagram/units'
import type { waterfallProps } from '../catalog'

export type WaterfallData = z.infer<typeof waterfallProps>

export interface WaterfallBar {
  id: string
  index: number
  label: string
  value: number
  total: number
  kind: 'delta' | 'total' | 'subtotal'
  rect: Rect
}

export interface WaterfallLayout {
  W: number
  H: number
  plot: Rect
  bars: WaterfallBar[]
  /** Connectors at the running total from one bar's end to the next bar's start. */
  connectors: { x1: number; x2: number; y: number }[]
  valueTicks: Tick[]
  catTicks: Tick[]
  zero: number
  showValues: boolean
}

const PLOT_H = 200

export function layoutWaterfall(p: WaterfallData): WaterfallLayout {
  const steps = (p.steps ?? []).filter((s) => typeof s.value === 'number' && Number.isFinite(s.value))
  const totals = runningTotal(steps)
  const levels = [0, ...totals]
  const [lo, hi] = niceDomain(...extent(levels))
  const left = AXIS_INSET.left
  const bottom = AXIS_INSET.bottomNoTitle + 8
  const top = PAD + titleH(p.title) + 16
  const W = Math.max(CHART_W, left + PAD + steps.length * 56)
  const H = top + PLOT_H + bottom
  const plot: Rect = { x: left, y: top, w: W - left - PAD, h: PLOT_H }
  const cats = band(
    steps.map((_, i) => String(i)),
    [plot.x, plot.x + plot.w],
    0.3,
  )
  const ys = linear([lo, hi], [plot.y + plot.h, plot.y])
  const bars: WaterfallBar[] = steps.map((s, i) => {
    const kind = s.kind ?? 'delta'
    const from = kind === 'delta' ? (totals[i - 1] ?? 0) : 0
    const to = totals[i]!
    const x = cats(String(i))
    return { id: String(i), index: i, label: s.label, value: s.value, total: to, kind, rect: { x, y: Math.min(ys(from), ys(to)), w: cats.bandwidth, h: Math.max(1, Math.abs(ys(from) - ys(to))) } }
  })
  const connectors = bars.slice(0, -1).map((b, i) => ({ x1: b.rect.x + b.rect.w, x2: bars[i + 1]!.rect.x, y: ys(b.total) }))
  return {
    W,
    H,
    plot,
    bars,
    connectors,
    valueTicks: ys.ticks(5).map((v) => ({ at: ys(v), label: withUnit(v) })),
    catTicks: steps.map((s, i) => ({ at: cats(String(i)) + cats.bandwidth / 2, label: s.label })),
    zero: ys(Math.min(Math.max(0, lo), hi)),
    showValues: steps.length <= 12,
  }
}

export function WaterfallSvg({ props: p, layout: l, tokens: t, units, id }: { props: WaterfallData; layout: WaterfallLayout; tokens: DiagramTokens; units: Units; id: string }) {
  const desc = describe('Waterfall', p.title, p.steps?.length ?? 0, 'step')
  const unit = p.unit ?? undefined
  return (
    <ChartSvg name="Waterfall" id={id} title={p.title} desc={desc} W={l.W} H={l.H} tokens={t} interactive root={units.root}>
      <Gridlines orient="horizontal" at={l.valueTicks.map((k) => k.at)} span={[l.plot.x, l.plot.x + l.plot.w]} tokens={t} />
      <Axis orient="left" at={l.plot.x} span={[l.plot.y, l.plot.y + l.plot.h]} ticks={l.valueTicks} tokens={t} title={unit} />
      <Axis orient="bottom" at={l.plot.y + l.plot.h} span={[l.plot.x, l.plot.x + l.plot.w]} ticks={l.catTicks} tokens={t} />
      <g aria-hidden>
        {l.connectors.map((c, i) => (
          <line key={i} x1={c.x1} y1={c.y} x2={c.x2} y2={c.y} stroke={t.mutedForeground} strokeWidth={1} strokeDasharray="3 2" />
        ))}
      </g>
      {l.bars.map((b) => {
        const step = p.steps?.[b.index]
        const own = ownState(step?.state)
        const base = b.kind === 'delta' ? (b.value >= 0 ? t.success : t.danger) : t.foreground
        const s = unitPaint(units.paint(b.id, own), base, t)
        const total = b.kind !== 'delta'
        const padY = Math.max(0, (HIT - b.rect.h) / 2)
        return (
          <g key={b.id} opacity={s.opacity} {...units.unit(b.id, `${b.label}: ${total ? withUnit(b.total, unit) : signed(b.value)}, total ${withUnit(b.total, unit)}`)}>
            <rect x={b.rect.x} y={b.rect.y - padY} width={b.rect.w} height={b.rect.h + 2 * padY} fill="transparent" />
            <rect x={b.rect.x} y={b.rect.y} width={b.rect.w} height={b.rect.h} rx={2} fill={s.fill} fillOpacity={total ? 0.25 : s.fillOpacity} stroke={s.stroke} strokeWidth={total ? Math.max(1.5, s.strokeWidth) : s.strokeWidth} />
            {l.showValues && (
              <text x={b.rect.x + b.rect.w / 2} y={b.value >= 0 || total ? b.rect.y - 4 : b.rect.y + b.rect.h + TICK_PX} textAnchor="middle" fontFamily={MONO} fontSize={TICK_PX} fontWeight={total ? 600 : undefined} fill={total ? t.foreground : t.mutedForeground}>
                {total ? withUnit(b.total, unit) : signed(b.value)}
              </text>
            )}
          </g>
        )
      })}
      <line x1={l.plot.x} y1={l.zero} x2={l.plot.x + l.plot.w} y2={l.zero} stroke={t.border} shapeRendering="crispEdges" />
    </ChartSvg>
  )
}

export function WaterfallView({ props, highlight, onSelect }: DiagramHostProps<WaterfallData>) {
  const layout = useMemo(() => layoutWaterfall(props), [props])
  const ids = useMemo(() => layout.bars.map((b) => b.id), [layout])
  const c = useChart({ ids, highlight, onSelect })
  const tip = useCallback(
    (id: string) => {
      const b = layout.bars.find((x) => x.id === id)
      if (!b) return null
      const unit = props.unit ?? undefined
      return { title: b.label, rows: [{ label: b.kind === 'delta' ? 'Δ' : b.kind, value: b.kind === 'delta' ? signed(b.value) : withUnit(b.value, unit) }, { label: 'running total', value: withUnit(b.total, unit) }] }
    },
    [layout, props.unit],
  )
  return (
    <ChartHost name="Waterfall" id={c.id} units={c.units} tip={tip}>
      <WaterfallSvg props={props} layout={layout} tokens={c.tokens} units={c.units} id={c.id} />
    </ChartHost>
  )
}
