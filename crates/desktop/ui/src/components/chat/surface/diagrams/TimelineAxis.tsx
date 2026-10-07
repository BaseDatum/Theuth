// 123 W2: `Timeline` with `scale: time` — items (dots) and `until` spans (bars) on a horizontal time axis, packed
// into lanes so labels never overlap (design 123 §4.1, §5.2 b). `list` stays the registry's HTML form. Each item
// is a unit (`id`, else its title). The `<desc>` is the first line of the Rust lowering, which for `Timeline` is
// the first item in date order (`● 2026-10-03T09:12:00Z  Pager fires`), so the a11y text and the terminal agree.
import { useCallback, useMemo } from 'react'
import type { z } from 'zod'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { Axis, Gridlines, type Tick } from '@/lib/diagram/axes'
import { CHART_W, ChartHost, ChartSvg, HIT, PAD, titleH, useChart } from '@/lib/diagram/chart/frame'
import type { Rect } from '@/lib/diagram/chart/geometry'
import { NAME_PX, unitPaint } from '@/lib/diagram/chart/series'
import { shortDate } from '@/lib/diagram/chart/text'
import { formatTick, parseIso, time } from '@/lib/diagram/scale'
import type { DiagramHostProps, UnitState } from '@/lib/diagram/types'
import type { Units } from '@/lib/diagram/units'
import type { catalog } from '../catalog'

export type TimelineData = z.infer<(typeof catalog)['data']['components']['Timeline']['props']>
type Item = TimelineData['items'][number]

export interface TimelineMark {
  id: string
  index: number
  x0: number
  x1: number | null
  lane: number
  y: number
  labelW: number
}

export interface TimelineLayout {
  W: number
  H: number
  plot: Rect
  marks: TimelineMark[]
  ticks: Tick[]
}

const LANE = 28
const CHAR_W = 7

/** Items in date order (stable), as `timeline_order` in `charts.rs`. */
export function orderedItems(p: TimelineData): { item: Item; index: number; at: Date }[] {
  return (p.items ?? [])
    .map((item, index) => ({ item, index, at: parseIso(item.when) }))
    .filter((x): x is { item: Item; index: number; at: Date } => x.at !== null)
    .sort((a, b) => a.at.getTime() - b.at.getTime())
}

/** `when` or `when → until` with a same-day `until` shown as its time — `when()` in `charts.rs`. */
export function whenText(i: Item): string {
  const u = i.until
  if (!u) return i.when
  if (i.when.length > 10 && u.length > 10 && u.slice(0, 10) === i.when.slice(0, 10)) return `${i.when} → ${u.slice(11)}`
  return `${i.when} → ${u}`
}

/** The first line the Rust `Timeline` arm prints for `p` (the `<desc>`). */
export function firstLine(p: TimelineData): string {
  const first = orderedItems(p)[0]?.item ?? p.items?.[0]
  if (!first) return 'Timeline: 0 items'
  const glyph = first.state === 'active' ? '◉' : first.state === 'pending' ? '○' : '●'
  return `${glyph} ${whenText(first)}  ${first.title}`
}

export function markId(p: TimelineData, i: number): string {
  return p.items?.[i]?.id ?? p.items?.[i]?.title ?? String(i)
}

export function layoutTimeline(p: TimelineData): TimelineLayout {
  const items = orderedItems(p)
  let lo = Number.POSITIVE_INFINITY
  let hi = Number.NEGATIVE_INFINITY
  for (const { item, at } of items) {
    lo = Math.min(lo, at.getTime())
    const until = item.until ? parseIso(item.until) : null
    hi = Math.max(hi, at.getTime(), until?.getTime() ?? at.getTime())
  }
  if (!Number.isFinite(lo)) {
    lo = 0
    hi = 86_400_000
  }
  if (hi <= lo) hi = lo + 86_400_000
  const top = PAD + titleH(p.title) + 8
  const W = CHART_W
  const plot: Rect = { x: PAD + 8, y: top, w: W - 2 * PAD - 16, h: 0 }
  const scale = time([new Date(lo), new Date(hi)], [plot.x, plot.x + plot.w - 8])
  // Greedy lanes: an item takes the first lane whose last occupant (dot/bar + label) ends before it starts.
  const laneEnd: number[] = []
  const marks: TimelineMark[] = items.map(({ item, index, at }) => {
    const x0 = scale(at)
    const until = item.until ? parseIso(item.until) : null
    const x1 = until ? Math.max(x0 + 2, scale(until)) : null
    const labelW = Math.min(220, item.title.length * CHAR_W + 12)
    const end = Math.max(x1 ?? x0, x0) + labelW
    let lane = laneEnd.findIndex((e) => e + 8 <= x0)
    if (lane < 0) lane = laneEnd.push(0) - 1
    laneEnd[lane] = end
    return { id: markId(p, index), index, x0, x1, lane, y: plot.y + lane * LANE + LANE / 2, labelW }
  })
  plot.h = Math.max(1, laneEnd.length) * LANE
  const unit = scale.unit(6)
  const ticks: Tick[] = scale.ticks(6).map((d) => ({ at: scale(d), label: formatTick(d, unit) }))
  return { W, H: plot.y + plot.h + 36, plot, marks, ticks }
}

const STATE: Record<string, UnitState> = { done: 'done', active: 'active', pending: 'default' }

export function TimelineAxisSvg({ props: p, layout: l, tokens: t, units, id }: { props: TimelineData; layout: TimelineLayout; tokens: DiagramTokens; units: Units; id: string }) {
  return (
    <ChartSvg name="Timeline" id={id} title={p.title} desc={firstLine(p)} W={l.W} H={l.H} tokens={t} interactive root={units.root}>
      <Gridlines orient="vertical" at={l.ticks.map((k) => k.at)} span={[l.plot.y, l.plot.y + l.plot.h]} tokens={t} />
      <Axis orient="bottom" at={l.plot.y + l.plot.h} span={[l.plot.x, l.plot.x + l.plot.w]} ticks={l.ticks} tokens={t} />
      {l.marks.map((m) => {
        const item = p.items?.[m.index]
        if (!item) return null
        const own = item.state ? STATE[item.state] : null
        const s = unitPaint(units.paint(m.id, own), t.foreground, t)
        const at = parseIso(item.when)
        const until = item.until ? parseIso(item.until) : null
        const withTime = item.when.length > 10
        const dates = at ? `${shortDate(at, withTime)}${until ? ` → ${shortDate(until, (item.until ?? '').length > 10)}` : ''}` : item.when
        const span = m.x1 !== null
        return (
          <g key={m.id} opacity={s.opacity} {...units.unit(m.id, [item.title, dates, item.detail].filter(Boolean).join(', '))}>
            <rect x={m.x0 - HIT / 2} y={m.y - LANE / 2} width={(m.x1 ?? m.x0) - m.x0 + HIT + m.labelW} height={LANE} fill="transparent" />
            {span ? (
              <rect x={m.x0} y={m.y - 6} width={m.x1! - m.x0} height={12} rx={3} fill={s.fill} fillOpacity={0.25} stroke={s.stroke} strokeWidth={s.strokeWidth} />
            ) : (
              <circle cx={m.x0} cy={m.y} r={5} fill={item.state === 'pending' ? t.card : s.stroke} stroke={s.stroke} strokeWidth={s.strokeWidth} />
            )}
            <text x={(m.x1 ?? m.x0) + 9} y={m.y} dominantBaseline="central" fontFamily={t.font} fontSize={NAME_PX - 1} fontWeight={600} fill={t.foreground}>
              {item.title.length > 28 ? `${item.title.slice(0, 27)}…` : item.title}
            </text>
          </g>
        )
      })}
    </ChartSvg>
  )
}

export function TimelineAxisView({ props, highlight, onSelect }: DiagramHostProps<TimelineData>) {
  const layout = useMemo(() => layoutTimeline(props), [props])
  const ids = useMemo(() => layout.marks.map((m) => m.id), [layout])
  const c = useChart({ ids, highlight, onSelect })
  const tip = useCallback(
    (id: string) => {
      const m = layout.marks.find((x) => x.id === id)
      const item = m && props.items?.[m.index]
      if (!m || !item) return null
      const at = parseIso(item.when)
      const until = item.until ? parseIso(item.until) : null
      return {
        title: item.title,
        rows: [
          { label: until ? 'from' : 'when', value: at ? shortDate(at, item.when.length > 10) : item.when },
          { label: 'until', value: until ? shortDate(until, (item.until ?? '').length > 10) : null },
          { label: 'detail', value: item.detail },
          { label: 'state', value: item.state },
        ],
      }
    },
    [layout, props],
  )
  return (
    <ChartHost name="Timeline" id={c.id} units={c.units} tip={tip}>
      <TimelineAxisSvg props={props} layout={layout} tokens={c.tokens} units={c.units} id={c.id} />
    </ChartHost>
  )
}
