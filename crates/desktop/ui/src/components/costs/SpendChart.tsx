// 160 §7.5: spend (or calls, tokens) over time — stacked bars per group (top 6 + other), the previous period as a
// dashed line, a clickable legend, drag across the plot to zoom into a custom range, a brush past 40 buckets.
// §6.5: the header controls, the legend's toggles and the drag highlight answer at once (urgent state, read through
// context by small components); the memoised chart body re-renders only for its own (deferred) data.

import { Filter } from 'lucide-react'
import { createContext, memo, useCallback, useContext, useDeferredValue, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { Bar, Brush, CartesianGrid, ComposedChart, Line, ReferenceArea, XAxis, YAxis } from 'recharts'
import { type ChartConfig, ChartContainer, ChartLegend, ChartTooltip, ChartTooltipContent } from '@/components/ui/chart'
import type { Dim, Metric, Series, SeriesOut, SeriesRow } from '@/lib/costs/cube'
import { compact, int, usd, usdShort } from '@/lib/costs/format'
import { cn } from '@/lib/utils'
import { Panel, Seg, Swatch } from './ui'

const GROUPS: { value: Dim; label: string }[] = [
  { value: 'model', label: 'Model' },
  { value: 'provider', label: 'Provider' },
  { value: 'owner', label: 'Owner' },
  { value: 'kind', label: 'Kind' },
  { value: 'via', label: 'Via' },
  { value: 'basis', label: 'Basis' },
  { value: 'project', label: 'Project' },
]
const METRIC_ITEMS: { value: Metric; label: string }[] = [
  { value: 'usd', label: '$' },
  { value: 'calls', label: 'Calls' },
  { value: 'tokens', label: 'Tokens' },
]
/** Past this many buckets the bars are thin: square tops, no per-bar radius path. */
const THIN = 60
const MARGIN = { top: 6, right: 14, bottom: 0, left: 0 }
const X_TICK = { fill: 'var(--fg-tertiary)', fontSize: 11 }
const Y_TICK = { fill: 'var(--fg-tertiary)', fontSize: 11, fontFamily: 'var(--font-mono)' }
const X_LINE = { stroke: 'var(--border-strong)' }
const CURSOR = { fill: 'var(--bg-layer-2)', opacity: 0.6 }
const TOP_RADIUS: [number, number, number, number] = [3, 3, 0, 0]

export const fmtMetric = (m: Metric, v: number): string => (m === 'usd' ? usd(v) : m === 'calls' ? int(v) : compact(v))

/** What the chart body shows: `groupBy`/`metric`/`compare` are the (deferred) view `out` was computed for. */
export type ChartView = { groupBy: Dim; metric: Metric; compare: boolean }

type Props = {
  out: SeriesOut
  /** The view `out` belongs to (deferred). */
  view: ChartView
  /** The controls' current values (urgent): the header follows these at once. */
  ctl: ChartView
  rangeLabel: string
  onGroupBy: (d: Dim) => void
  onMetric: (m: Metric) => void
  onCompare: (on: boolean) => void
  onFilter: (dim: Dim, id: string) => void
  /** A dragged selection: the instants from the first bucket's start to the last one's end. */
  onRange: (from: number, to: number) => void
}

type Drag = { a: number; b: number } | null
const DragCtx = createContext<Drag>(null)
const LegendCtx = createContext<{ hidden: ReadonlySet<string>; toggle: (key: string) => void }>({ hidden: new Set(), toggle: () => undefined })

const tipIndex = (e: { activeTooltipIndex?: unknown } | null | undefined): number | null => {
  const v = Number(e?.activeTooltipIndex)
  return e?.activeTooltipIndex == null || !Number.isFinite(v) ? null : v
}

export const SpendChart = memo(function SpendChart({ out, view, ctl, rangeLabel, onGroupBy, onMetric, onCompare, onFilter, onRange }: Props) {
  // Hidden series belong to one grouping; a new grouping starts with everything shown.
  const [hiddenBy, setHiddenBy] = useState<{ by: Dim; keys: ReadonlySet<string> }>({ by: view.groupBy, keys: new Set() })
  const hidden = hiddenBy.by === view.groupBy ? hiddenBy.keys : EMPTY
  const bodyHidden = useDeferredValue(hidden)
  const toggle = useCallback(
    (key: string) =>
      setHiddenBy((h) => {
        const keys = new Set(h.by === view.groupBy ? h.keys : EMPTY)
        if (!keys.delete(key)) keys.add(key)
        return { by: view.groupBy, keys }
      }),
    [view.groupBy],
  )
  const legend = useMemo(() => ({ hidden, toggle }), [hidden, toggle])

  const [drag, setDrag] = useState<Drag>(null)
  const dragRef = useRef<Drag>(null)
  const rowsRef = useRef(out.rows)
  rowsRef.current = out.rows
  const onRangeRef = useRef(onRange)
  onRangeRef.current = onRange
  const put = (d: Drag) => {
    dragRef.current = d
    setDrag(d)
  }
  const mouse = useMemo(
    () => ({
      down: (e: { activeTooltipIndex?: unknown } | null) => {
        const i = tipIndex(e)
        if (i !== null) put({ a: i, b: i })
      },
      move: (e: { activeTooltipIndex?: unknown } | null) => {
        const d = dragRef.current
        const i = tipIndex(e)
        if (d && i !== null && i !== d.b) put({ a: d.a, b: i })
      },
      up: () => {
        const d = dragRef.current
        const rows = rowsRef.current
        if (d && d.a !== d.b) {
          const [i, j] = d.a < d.b ? [d.a, d.b] : [d.b, d.a]
          onRangeRef.current(rows[i]!.start, rows[j]!.end)
        }
        put(null)
      },
      leave: () => put(null),
    }),
    [],
  )
  const empty = out.rows.every((r) => r.total === 0 && (!view.compare || r.prev === 0))
  return (
    <Panel
      title={`${ctl.metric === 'usd' ? 'Spend' : ctl.metric === 'calls' ? 'Calls' : 'Tokens'} over time`}
      actions={
        <div className="flex max-w-full min-w-0 flex-wrap items-center gap-2">
          <Seg label="Group by" items={GROUPS} value={ctl.groupBy} onChange={onGroupBy} fade />
          <Seg label="Metric" items={METRIC_ITEMS} value={ctl.metric} onChange={onMetric} />
          <button
            type="button"
            aria-pressed={ctl.compare}
            onClick={() => onCompare(!ctl.compare)}
            className="inline-flex items-center gap-1.5 rounded-lg border border-border-strong bg-card px-2.5 py-1 text-xs text-fg-secondary hover:border-fg-tertiary hover:text-foreground"
          >
            <span aria-hidden className={cn('inline-block size-2 rounded-full border border-current', ctl.compare && 'bg-current')} />
            vs previous
          </button>
        </div>
      }
    >
      <DragCtx value={drag}>
        <LegendCtx value={legend}>
          <Body out={out} view={view} hidden={bodyHidden} rangeLabel={rangeLabel} mouse={mouse} onFilter={onFilter} dragging={dragRef} />
        </LegendCtx>
      </DragCtx>
      {empty && <p className="-mt-40 mb-32 text-center text-xs text-fg-tertiary">Nothing in this range{out.rows.length ? '' : ' yet'}.</p>}
    </Panel>
  )
})

const EMPTY: ReadonlySet<string> = new Set()

type Mouse = { down: (e: { activeTooltipIndex?: unknown } | null) => void; move: (e: { activeTooltipIndex?: unknown } | null) => void; up: () => void; leave: () => void }

const CHART_H = 244
/** The main chart's last measured width (module-level: the next mount starts at it). */
let lastWidth = 0
/**
 * The main chart's width before it has been measured: the last mount's, else the sidebar inset's content box minus the
 * page and card padding (max 1600 px, px-3/md:px-4, card px-3.5 + 1 px border).
 */
function mainChartWidth(): number {
  if (lastWidth) return lastWidth
  const inset = typeof document === 'undefined' ? null : document.querySelector<HTMLElement>('[data-slot="sidebar-inset"]')
  if (!inset) return 900
  const w = Math.min(inset.clientWidth, 1600)
  return Math.max(200, w - 2 * (window.innerWidth >= 768 ? 16 : 12) - 2 * 15)
}

/** The chart itself: re-renders only when its data, view or (deferred) hidden set change. */
const Body = memo(function Body({ out, view, hidden, rangeLabel, mouse, onFilter, dragging }: { out: SeriesOut; view: ChartView; hidden: ReadonlySet<string>; rangeLabel: string; mouse: Mouse; onFilter: (dim: Dim, id: string) => void; dragging: { current: Drag } }) {
  const { groupBy, metric, compare } = view
  const rows = out.rows
  const config = useMemo(() => {
    const c: ChartConfig = { prev: { label: `previous ${rangeLabel}`, color: 'var(--fg-tertiary)' } }
    for (const s of out.series) c[s.key] = { label: s.label, color: s.color }
    return c
  }, [out.series, rangeLabel])
  const labels = useMemo(() => new Map(rows.map((r) => [r.start, r.label])), [rows])
  const xFmt = useCallback((v: number) => labels.get(v) ?? '', [labels])
  const yFmt = useCallback((v: number) => (metric === 'usd' ? usdShort(v) : compact(v)), [metric])
  const visible = out.series.filter((s) => !hidden.has(s.key))
  // Square tops past 60 buckets: no per-bar radius path when the bars are thin.
  const top = rows.length > THIN ? null : visible.at(-1)?.key
  const tooltip = useMemo(
    () => (
      <ChartTooltipContent
        className="min-w-[200px]"
        labelFormatter={(_, p) => {
          const row: SeriesRow | undefined = p[0]?.payload
          return row?.tip
        }}
        formatter={(value, name, item, i) => {
          const row: SeriesRow = item.payload
          const s = name === 'prev' ? null : out.series.find((x) => x.key === name)
          const last = i === visible.length + (compare ? 1 : 0) - 1
          const zero = name !== 'prev' && !Number(value)
          return (
            <div className="flex w-full flex-col gap-1">
              {!zero && (
                <div className={cn('flex items-center gap-2', name === 'prev' && 'text-fg-tertiary')}>
                  {s ? <Swatch color={s.color} /> : <span className="w-[9px] border-t-2 border-dashed border-fg-tertiary" />}
                  <span className={name === 'prev' ? '' : 'text-fg-secondary'}>{s?.label ?? 'previous period'}</span>
                  <span className="ml-auto font-mono tabular-nums">{fmtMetric(metric, Number(value))}</span>
                </div>
              )}
              {last && (
                <div className="mt-1 flex border-t border-border-strong pt-1 font-semibold">
                  Total<span className="ml-auto font-mono tabular-nums">{fmtMetric(metric, row.total)}</span>
                </div>
              )}
            </div>
          )
        }}
      />
    ),
    [out.series, visible.length, compare, metric],
  )
  const legend = useMemo(() => <LegendItems series={out.series} groupBy={groupBy} compare={compare} rangeLabel={rangeLabel} onFilter={onFilter} />, [out.series, groupBy, compare, rangeLabel, onFilter])
  // R2: the legend renders through ChartLegend into a node below the chart (Legend `portal`), so Recharts never measures
  // it and re-lays the plot out; and the chart starts at its real width, so the container's first measure changes nothing.
  const [legendEl] = useState(() => (typeof document === 'undefined' ? null : document.createElement('div')))
  const [initial] = useState(() => ({ width: mainChartWidth(), height: CHART_H }))
  const wrap = useRef<HTMLDivElement>(null)
  useLayoutEffect(() => {
    if (wrap.current) lastWidth = wrap.current.clientWidth
  })
  return (
    <div ref={wrap}>
    <ChartContainer config={config} className="aspect-auto h-[244px] w-full select-none" initialDimension={initial}>
      <ComposedChart data={rows} margin={MARGIN} barCategoryGap={rows.length > THIN ? '8%' : '18%'} onMouseDown={mouse.down} onMouseMove={mouse.move} onMouseUp={mouse.up} onMouseLeave={mouse.leave}>
        <CartesianGrid vertical={false} stroke="var(--border-default)" />
        <XAxis dataKey="start" tickFormatter={xFmt} tick={X_TICK} tickLine={false} axisLine={X_LINE} minTickGap={18} />
        <YAxis tick={Y_TICK} tickLine={false} axisLine={false} width={56} tickFormatter={yFmt} />
        <ChartTooltip cursor={CURSOR} isAnimationActive={false} content={tooltip} />
        {out.series.map((s) => (
          <Bar
            key={s.key}
            dataKey={s.key}
            name={s.key}
            stackId="s"
            fill={s.color}
            hide={hidden.has(s.key)}
            radius={s.key === top ? TOP_RADIUS : 0}
            isAnimationActive={false}
            className={s.id === null ? undefined : 'cursor-pointer'}
            onClick={() => s.id !== null && !dragging.current && onFilter(groupBy, s.id)}
          />
        ))}
        {compare && <Line dataKey="prev" name="prev" stroke="var(--fg-tertiary)" strokeDasharray="4 4" dot={false} activeDot={false} strokeWidth={1.5} type="linear" isAnimationActive={false} />}
        <DragArea rows={rows} />
        {rows.length > 40 && <Brush dataKey="start" height={18} stroke="var(--border-strong)" fill="var(--bg-elevated)" travellerWidth={7} tickFormatter={() => ''} />}
        <ChartLegend content={legend} portal={legendEl} />
      </ComposedChart>
    </ChartContainer>
    <div ref={(n) => void (n && legendEl && legendEl.parentNode !== n && n.appendChild(legendEl))} />
    </div>
  )
})

/** The drag highlight: a `ReferenceArea` that follows the urgent drag state without re-rendering the bars. */
function DragArea({ rows }: { rows: SeriesRow[] }) {
  const d = useContext(DragCtx)
  if (!d || d.a === d.b || !rows[d.a] || !rows[d.b]) return null
  return <ReferenceArea x1={rows[Math.min(d.a, d.b)]!.start} x2={rows[Math.max(d.a, d.b)]!.start} fill="var(--status-info-fg)" fillOpacity={0.12} stroke="var(--status-info-fg)" strokeOpacity={0.5} />
}

/** Legend entries (rendered through `ChartLegend`): a click toggles the series at once, the filter icon filters. */
function LegendItems({ series, groupBy, compare, rangeLabel, onFilter }: { series: Series[]; groupBy: Dim; compare: boolean; rangeLabel: string; onFilter: (dim: Dim, id: string) => void }) {
  const { hidden, toggle } = useContext(LegendCtx)
  return (
    <div className="mt-2 flex flex-wrap gap-x-3 gap-y-1.5 pl-14">
      {series.map((s) => (
        <span key={s.key} className={cn('inline-flex items-center gap-1 text-xs text-fg-secondary', hidden.has(s.key) && 'opacity-35')}>
          <button type="button" aria-pressed={!hidden.has(s.key)} onClick={() => toggle(s.key)} className="inline-flex items-center gap-1.5 rounded-sm outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50">
            <Swatch color={s.color} />
            {s.label}
          </button>
          {s.id !== null && (
            <button type="button" title={`Filter to ${s.label}`} aria-label={`Filter to ${s.label}`} onClick={() => onFilter(groupBy, s.id!)} className="rounded-sm p-0.5 text-fg-tertiary outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50">
              <Filter className="size-3" />
            </button>
          )}
        </span>
      ))}
      {compare && (
        <span className="inline-flex items-center gap-1.5 text-xs text-fg-secondary">
          <span aria-hidden className="w-3 border-t-2 border-dashed border-fg-tertiary" />
          previous {rangeLabel}
        </span>
      )}
    </div>
  )
}
