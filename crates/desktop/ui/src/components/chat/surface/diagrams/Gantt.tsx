// 123 W2: `Gantt` — tasks on a time scale (day / week / month ticks chosen from the span), 40 px rows, 24 px bars,
// groups as bands, `progress` as an inner fill, `depends` as orthogonal arrows whose vertical run picks an x no
// intermediate bar occupies, a `today` line (design 123 §4.2, §5.2 b). Each task is a unit (`id`); selecting one
// lights what it depends on and what depends on it. Dangling `depends` are dropped (an open fence).
import { useCallback, useMemo } from 'react'
import type { z } from 'zod'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { Axis, Gridlines, MONO, type Tick, TICK_PX } from '@/lib/diagram/axes'
import { CHART_W, ChartHost, ChartSvg, ownState, PAD, titleH, useChart } from '@/lib/diagram/chart/frame'
import { endOf, type Rect, timeSpan } from '@/lib/diagram/chart/geometry'
import { NAME_PX, unitPaint } from '@/lib/diagram/chart/series'
import { describe, shortDate } from '@/lib/diagram/chart/text'
import { orthogonalPath } from '@/lib/diagram/path'
import { formatTick, parseIso, time } from '@/lib/diagram/scale'
import type { DiagramHostProps, Point } from '@/lib/diagram/types'
import type { Units } from '@/lib/diagram/units'
import type { ganttProps } from '../catalog'

export type GanttData = z.infer<typeof ganttProps>

export interface GanttRow {
  id: string
  index: number
  label: string
  rect: Rect
  /** Progress fill width. */
  done: number
  y: number
}

export interface GanttArrow {
  from: string
  to: string
  points: Point[]
}

export interface GanttLayout {
  W: number
  H: number
  plot: Rect
  rows: GanttRow[]
  groups: { label: string; y: number; h: number }[]
  arrows: GanttArrow[]
  ticks: Tick[]
  today: number | null
  labelX: number
}

export const ROW = 40
export const BAR = 24
const HEAD = 24
const LABEL_MIN = 96
const LABEL_MAX = 180

/** Task order: groups in first-appearance order, ungrouped tasks last (as `charts.rs`). */
function ordered(tasks: GanttData['tasks']): { task: GanttData['tasks'][number]; index: number; group: string | null }[] {
  const groups: string[] = []
  for (const t of tasks) if (t.group && !groups.includes(t.group)) groups.push(t.group)
  const out: { task: GanttData['tasks'][number]; index: number; group: string | null }[] = []
  for (const g of groups) tasks.forEach((task, index) => task.group === g && out.push({ task, index, group: g }))
  tasks.forEach((task, index) => !task.group && out.push({ task, index, group: null }))
  return out
}

export function layoutGantt(p: GanttData): GanttLayout {
  const tasks = (p.tasks ?? []).filter((t) => t.id && parseIso(t.start) && endOf(t.end))
  const span = timeSpan(tasks, [p.today]) ?? [new Date(0), new Date(86_400_000)]
  const seq = ordered(tasks)
  const grouped = seq.some((s) => s.group !== null)
  const labelW = Math.min(LABEL_MAX, Math.max(LABEL_MIN, ...tasks.map((t) => t.label.length * 7 + (grouped ? 12 : 0) + 8)))
  const left = PAD + labelW
  const top = PAD + titleH(p.title) + HEAD
  const W = Math.max(CHART_W, left + PAD + 320)
  const plotX = left
  const plotW = W - left - PAD
  const scale = time(span, [plotX, plotX + plotW])
  const rows: GanttRow[] = []
  const groups: GanttLayout['groups'] = []
  let y = top
  let current: string | null | undefined
  for (const { task, index, group } of seq) {
    if (grouped && group !== current) {
      current = group
      if (group !== null) {
        groups.push({ label: group, y, h: 0 })
        y += HEAD
      }
    }
    const x0 = scale(parseIso(task.start)!)
    const x1 = Math.max(x0 + 2, scale(endOf(task.end)!))
    const progress = task.progress ?? (task.state === 'done' ? 1 : 0)
    rows.push({ id: task.id, index, label: task.label, rect: { x: x0, y: y + (ROW - BAR) / 2, w: x1 - x0, h: BAR }, done: (x1 - x0) * Math.min(1, Math.max(0, progress)), y })
    const g = groups[groups.length - 1]
    if (g && group !== null) g.h = y + ROW - g.y
    y += ROW
  }
  const H = y + 32
  const plot: Rect = { x: plotX, y: top, w: plotW, h: y - top }
  const byId = new Map(rows.map((r) => [r.id, r]))
  const arrows: GanttArrow[] = []
  for (const r of rows) {
    for (const dep of p.tasks?.[r.index]?.depends ?? []) {
      const from = byId.get(dep)
      if (!from || from === r) continue
      arrows.push({ from: dep, to: r.id, points: route(from, r, rows, plot) })
    }
  }
  const unit = scale.unit(6)
  const ticks: Tick[] = scale.ticks(6).map((d) => ({ at: scale(d), label: formatTick(d, unit) }))
  const todayDate = p.today ? parseIso(p.today) : null
  return { W, H, plot, rows, groups, arrows, ticks, today: todayDate ? scale(todayDate) : null, labelX: PAD + (grouped ? 12 : 0) }
}

/** `from`'s end → `to`'s start: straight when they meet on one row, else one vertical run at a free x. */
function route(from: GanttRow, to: GanttRow, rows: readonly GanttRow[], plot: Rect): Point[] {
  const a: Point = { x: from.rect.x + from.rect.w, y: from.rect.y + from.rect.h / 2 }
  const b: Point = { x: to.rect.x, y: to.rect.y + to.rect.h / 2 }
  if (a.y === b.y) return [a, b]
  const lo = Math.min(from.y, to.y)
  const hi = Math.max(from.y, to.y)
  const between = rows.filter((r) => r.y > lo && r.y < hi).map((r) => r.rect)
  const free = (x: number) => x >= plot.x && x <= plot.x + plot.w && between.every((r) => x <= r.x - 4 || x >= r.x + r.w + 4)
  const candidates = [(a.x + b.x) / 2, a.x + 8, b.x - 8, ...between.flatMap((r) => [r.x - 8, r.x + r.w + 8]), plot.x + plot.w - 4, plot.x + 4]
  const x = candidates.find(free) ?? (a.x + b.x) / 2
  return [a, { x, y: a.y }, { x, y: b.y }, b]
}

export function GanttSvg({ props: p, layout: l, tokens: t, units, id }: { props: GanttData; layout: GanttLayout; tokens: DiagramTokens; units: Units; id: string }) {
  const desc = describe('Gantt', p.title, p.tasks?.length ?? 0, 'task')
  const marker = `gantt-arrow-${id}`
  return (
    <ChartSvg name="Gantt" id={id} title={p.title} desc={desc} W={l.W} H={l.H} tokens={t} interactive root={units.root}>
      <defs>
        <marker id={marker} viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto">
          <path d="M0 0 L10 5 L0 10 z" fill={t.mutedForeground} />
        </marker>
      </defs>
      {l.groups.map((g) => (
        <g key={g.label} aria-hidden>
          <rect x={PAD} y={g.y} width={l.W - 2 * PAD} height={g.h} rx={4} fill={t.muted} fillOpacity={0.25} />
          <text x={PAD + 8} y={g.y + HEAD / 2} dominantBaseline="central" fontFamily={t.font} fontSize={NAME_PX - 1} fontWeight={600} fill={t.mutedForeground}>
            {g.label}
          </text>
        </g>
      ))}
      <Gridlines orient="vertical" at={l.ticks.map((k) => k.at)} span={[l.plot.y, l.plot.y + l.plot.h]} tokens={t} />
      <Axis orient="bottom" at={l.plot.y + l.plot.h} span={[l.plot.x, l.plot.x + l.plot.w]} ticks={l.ticks} tokens={t} />
      {l.today !== null && (
        <g aria-hidden>
          <line x1={l.today} y1={l.plot.y - 4} x2={l.today} y2={l.plot.y + l.plot.h} stroke={t.accent} strokeWidth={1.5} strokeDasharray="4 3" />
          <text x={l.today + 4} y={l.plot.y - 6} fontFamily={MONO} fontSize={TICK_PX} fill={t.accent}>
            today
          </text>
        </g>
      )}
      <g aria-hidden>
        {l.arrows.map((a) => {
          const lit = units.state.selected !== null && (units.state.selected === a.from || units.state.selected === a.to)
          return <path key={`${a.from}>${a.to}`} d={orthogonalPath(a.points)} fill="none" stroke={lit ? t.accent : t.mutedForeground} strokeWidth={lit ? 1.5 : 1} markerEnd={`url(#${marker})`} opacity={units.state.selected !== null && !lit ? 0.4 : 1} />
        })}
      </g>
      {l.rows.map((r) => {
        const task = p.tasks?.[r.index]
        const s = unitPaint(units.paint(r.id, ownState(task?.state)), t.foreground, t)
        const start = parseIso(task?.start ?? '')
        const end = parseIso(task?.end ?? '')
        const dates = start && end ? `${shortDate(start, task!.start.length > 10)} → ${shortDate(end, task!.end.length > 10)}` : ''
        return (
          <g key={r.id} opacity={s.opacity} {...units.unit(r.id, [r.label, dates, task?.progress != null ? `${Math.round(task.progress * 100)}% done` : null].filter(Boolean).join(', '))}>
            <rect x={PAD} y={r.y} width={l.W - 2 * PAD} height={ROW} fill="transparent" />
            <text x={l.labelX} y={r.y + ROW / 2} dominantBaseline="central" fontFamily={t.font} fontSize={NAME_PX} fontWeight={600} fill={t.foreground}>
              {r.label.length > 24 ? `${r.label.slice(0, 23)}…` : r.label}
            </text>
            <rect x={r.rect.x} y={r.rect.y} width={r.rect.w} height={r.rect.h} rx={4} fill={s.fill} fillOpacity={s.fillOpacity} stroke={s.stroke} strokeWidth={s.strokeWidth} />
            {r.done > 0 && <rect x={r.rect.x} y={r.rect.y} width={r.done} height={r.rect.h} rx={4} fill={s.stroke} fillOpacity={0.35} />}
          </g>
        )
      })}
    </ChartSvg>
  )
}

export function GanttView({ props, highlight, onSelect }: DiagramHostProps<GanttData>) {
  const layout = useMemo(() => layoutGantt(props), [props])
  const ids = useMemo(() => layout.rows.map((r) => r.id), [layout])
  const incident = useCallback((id: string) => layout.arrows.filter((a) => a.from === id || a.to === id).map((a) => (a.from === id ? a.to : a.from)), [layout])
  const c = useChart({ ids, incident, highlight, onSelect })
  const tip = useCallback(
    (id: string) => {
      const r = layout.rows.find((x) => x.id === id)
      const task = r && props.tasks?.[r.index]
      if (!r || !task) return null
      const start = parseIso(task.start)
      const end = parseIso(task.end)
      return {
        title: task.label,
        rows: [
          { label: 'start', value: start ? shortDate(start, task.start.length > 10) : task.start },
          { label: 'end', value: end ? shortDate(end, task.end.length > 10) : task.end },
          { label: 'progress', value: task.progress != null ? `${Math.round(task.progress * 100)}%` : null },
          { label: 'depends', value: task.depends?.length ? task.depends.map((d) => props.tasks.find((x) => x.id === d)?.label ?? d).join(', ') : null },
        ],
      }
    },
    [layout, props],
  )
  return (
    <ChartHost name="Gantt" id={c.id} units={c.units} tip={tip}>
      <GanttSvg props={props} layout={layout} tokens={c.tokens} units={c.units} id={c.id} />
    </ChartHost>
  )
}
