// 160 §7.5: spend (or calls, tokens) over time — stacked bars per group (top 7 + other), the previous period as a
// dashed line, a clickable legend, drag across the plot to zoom into a custom range, a brush past 40 buckets.

import { Filter } from 'lucide-react'
import { memo, useMemo, useState } from 'react'
import { Bar, Brush, CartesianGrid, ComposedChart, Line, ReferenceArea, XAxis, YAxis } from 'recharts'
import { type ChartConfig, ChartContainer, ChartLegend, ChartTooltip, ChartTooltipContent } from '@/components/ui/chart'
import type { Dim, Metric, SeriesOut, SeriesRow } from '@/lib/costs/cube'
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

export const fmtMetric = (m: Metric, v: number): string => (m === 'usd' ? usd(v) : m === 'calls' ? int(v) : compact(v))

type Props = {
  out: SeriesOut
  metric: Metric
  groupBy: Dim
  compare: boolean
  rangeLabel: string
  onGroupBy: (d: Dim) => void
  onMetric: (m: Metric) => void
  onCompare: (on: boolean) => void
  onFilter: (dim: Dim, id: string) => void
  /** A dragged selection: the instants from the first bucket's start to the last one's end. */
  onRange: (from: number, to: number) => void
}

const index = (e: { activeTooltipIndex?: unknown } | null | undefined): number | null => {
  const v = Number(e?.activeTooltipIndex)
  return e?.activeTooltipIndex == null || !Number.isFinite(v) ? null : v
}

export const SpendChart = memo(function SpendChart({ out, metric, groupBy, compare, rangeLabel, onGroupBy, onMetric, onCompare, onFilter, onRange }: Props) {
  const [hidden, setHidden] = useState<ReadonlySet<string>>(new Set())
  const [drag, setDrag] = useState<{ a: number; b: number } | null>(null)
  const rows = out.rows
  const config = useMemo(() => {
    const c: ChartConfig = { prev: { label: `previous ${rangeLabel}`, color: 'var(--fg-tertiary)' } }
    for (const s of out.series) c[s.key] = { label: s.label, color: s.color }
    return c
  }, [out.series, rangeLabel])
  const labels = useMemo(() => new Map(rows.map((r) => [r.start, r.label])), [rows])
  const visible = out.series.filter((s) => !hidden.has(s.key))
  const top = visible.at(-1)?.key
  const toggle = (key: string) =>
    setHidden((h) => {
      const n = new Set(h)
      if (!n.delete(key)) n.add(key)
      return n
    })
  const finish = () => {
    if (drag && drag.a !== drag.b) {
      const [i, j] = drag.a < drag.b ? [drag.a, drag.b] : [drag.b, drag.a]
      onRange(rows[i]!.start, rows[j]!.end)
    }
    setDrag(null)
  }
  const empty = rows.every((r) => r.total === 0 && (!compare || r.prev === 0))
  return (
    <Panel
      title={`${metric === 'usd' ? 'Spend' : metric === 'calls' ? 'Calls' : 'Tokens'} over time`}
      actions={
        <div className="flex flex-wrap items-center gap-2">
          <Seg label="Group by" items={GROUPS} value={groupBy} onChange={onGroupBy} />
          <Seg label="Metric" items={METRIC_ITEMS} value={metric} onChange={onMetric} />
          <button
            type="button"
            aria-pressed={compare}
            onClick={() => onCompare(!compare)}
            className="inline-flex items-center gap-1.5 rounded-lg border border-border-strong bg-card px-2.5 py-1 text-xs text-fg-secondary hover:border-fg-tertiary hover:text-foreground"
          >
            <span aria-hidden className={cn('inline-block size-2 rounded-full border border-current', compare && 'bg-current')} />
            vs previous
          </button>
        </div>
      }
    >
      <ChartContainer config={config} className="aspect-auto h-[280px] w-full select-none" initialDimension={{ width: 900, height: 280 }}>
        <ComposedChart
          data={rows}
          margin={{ top: 6, right: 14, bottom: 0, left: 0 }}
          barCategoryGap="18%"
          onMouseDown={(e) => {
            const i = index(e)
            if (i !== null) setDrag({ a: i, b: i })
          }}
          onMouseMove={(e) => {
            const i = index(e)
            if (drag && i !== null && i !== drag.b) setDrag({ ...drag, b: i })
          }}
          onMouseUp={finish}
          onMouseLeave={() => setDrag(null)}
        >
          <CartesianGrid vertical={false} stroke="var(--border-default)" />
          <XAxis dataKey="start" tickFormatter={(v: number) => labels.get(v) ?? ''} tick={{ fill: 'var(--fg-tertiary)', fontSize: 11 }} tickLine={false} axisLine={{ stroke: 'var(--border-strong)' }} minTickGap={18} />
          <YAxis tick={{ fill: 'var(--fg-tertiary)', fontSize: 11, fontFamily: 'var(--font-mono)' }} tickLine={false} axisLine={false} width={56} tickFormatter={(v: number) => (metric === 'usd' ? usdShort(v) : compact(v))} />
          <ChartTooltip
            cursor={{ fill: 'var(--bg-layer-2)', opacity: 0.6 }}
            isAnimationActive={false}
            content={
              <ChartTooltipContent
                className="min-w-[200px]"
                labelFormatter={(_, p) => {
                  const row: SeriesRow | undefined = p[0]?.payload
                  return row?.tip
                }}
                formatter={(value, name, item, i) => {
                  const row: SeriesRow = item.payload
                  const s = name === 'prev' ? null : out.series.find((x) => x.key === name)
                  const last = i === (visible.length + (compare ? 1 : 0)) - 1
                  return (
                    <div className="flex w-full flex-col gap-1">
                      <div className={cn('flex items-center gap-2', name === 'prev' && 'text-fg-tertiary')}>
                        {s ? <Swatch color={s.color} /> : <span className="w-[9px] border-t-2 border-dashed border-fg-tertiary" />}
                        <span className={name === 'prev' ? '' : 'text-fg-secondary'}>{s?.label ?? 'previous period'}</span>
                        <span className="ml-auto font-mono tabular-nums">{fmtMetric(metric, Number(value))}</span>
                      </div>
                      {last && (
                        <div className="mt-1 flex border-t border-border-strong pt-1 font-semibold">
                          Total<span className="ml-auto font-mono tabular-nums">{fmtMetric(metric, row.total)}</span>
                        </div>
                      )}
                    </div>
                  )
                }}
              />
            }
          />
          {out.series.map((s) => (
            <Bar
              key={s.key}
              dataKey={s.key}
              name={s.key}
              stackId="s"
              fill={s.color}
              hide={hidden.has(s.key)}
              radius={s.key === top ? [3, 3, 0, 0] : 0}
              isAnimationActive={false}
              className={s.id === null ? undefined : 'cursor-pointer'}
              onClick={() => s.id !== null && !drag && onFilter(groupBy, s.id)}
            />
          ))}
          {compare && <Line dataKey="prev" name="prev" stroke="var(--fg-tertiary)" strokeDasharray="4 4" dot={false} strokeWidth={1.5} type="linear" isAnimationActive={false} />}
          {drag && drag.a !== drag.b && <ReferenceArea x1={rows[Math.min(drag.a, drag.b)]!.start} x2={rows[Math.max(drag.a, drag.b)]!.start} fill="var(--status-info-fg)" fillOpacity={0.12} stroke="var(--status-info-fg)" strokeOpacity={0.5} />}
          {rows.length > 40 && <Brush dataKey="start" height={18} stroke="var(--border-strong)" fill="var(--bg-elevated)" travellerWidth={7} tickFormatter={() => ''} />}
          <ChartLegend
            content={() => (
              <div className="mt-2 flex flex-wrap gap-x-3 gap-y-1.5 pl-14">
                {out.series.map((s) => (
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
            )}
          />
        </ComposedChart>
      </ChartContainer>
      {empty && <p className="-mt-40 mb-32 text-center text-xs text-fg-tertiary">Nothing in this range{rows.length ? '' : ' yet'}.</p>}
    </Panel>
  )
})
