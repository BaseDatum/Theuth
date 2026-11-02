// 160 §7.7: by model, owner, provider (donut), kind and project for the range and filters; a click adds the filter.

import { memo, useMemo } from 'react'
import { Bar, BarChart, Cell, LabelList, Pie, PieChart, XAxis, YAxis } from 'recharts'
import { ChartContainer, ChartTooltip, ChartTooltipContent } from '@/components/ui/chart'
import { type BreakdownRow, breakdown, type Dim, type Filters, type Index, OTHER_COLOR, type Range } from '@/lib/costs/cube'
import { compact, int, usd, usdShort } from '@/lib/costs/format'
import { OwnerGlyph, Panel, Swatch } from './ui'

const TOP = 10
const PIE = ['var(--chart-1)', 'var(--chart-5)', 'var(--chart-3)', 'var(--chart-7)', 'var(--chart-2)', 'var(--chart-8)', 'var(--chart-4)']

type Datum = BreakdownRow & { text: string }

function Tip({ row }: { row: BreakdownRow }) {
  return (
    <div className="grid gap-1">
      <b className="font-medium">{row.label}</b>
      {(
        [
          ['cost', usd(row.usd)],
          ['calls', int(row.calls)],
          ['per call', row.calls ? usd(row.usd / row.calls) : '—'],
          ['tokens', compact(row.tokens)],
          ...(row.failed ? [['failed', int(row.failed)]] : []),
        ] as [string, string][]
      ).map(([k, v]) => (
        <div key={k} className="flex gap-4 text-fg-secondary">
          {k}
          <span className="ml-auto font-mono text-foreground tabular-nums">{v}</span>
        </div>
      ))}
    </div>
  )
}

/** Horizontal bars of the top rows, `$` labels (or the call count for an unpriced row). */
function HBars({ rows, color, onPick }: { rows: BreakdownRow[]; color: string; onPick: (id: string) => void }) {
  const data: Datum[] = useMemo(() => rows.slice(0, TOP).map((r) => ({ ...r, text: r.usd > 0 ? usdShort(r.usd) : `${int(r.calls)} calls` })), [rows])
  if (!data.length) return <p className="py-6 text-center text-xs text-fg-tertiary">Nothing in this range.</p>
  return (
    <>
      <ChartContainer config={{ usd: { label: 'cost', color } }} className="aspect-auto w-full" style={{ height: data.length * 26 + 8 }} initialDimension={{ width: 420, height: data.length * 26 + 8 }}>
        <BarChart data={data} layout="vertical" margin={{ top: 0, right: 64, bottom: 0, left: 0 }} barCategoryGap={6}>
          <XAxis type="number" hide domain={[0, 'dataMax']} />
          <YAxis type="category" dataKey="label" width={150} interval={0} tick={(p: { x?: number | string; y?: number | string; payload?: { value?: unknown; index?: number } }) => {
            const full = String(p.payload?.value ?? '')
            const label = full.length > 22 ? `${full.slice(0, 21)}…` : full
            const kind = data[p.payload?.index ?? -1]?.kind
            // ≈ 6.6 px a glyph at 12 px: the owner mark sits just left of the right-aligned label.
            const gx = -4 - label.length * 6.6 - 8
            return (
              <g transform={`translate(${Number(p.x)},${Number(p.y)})`}>
                <text x={-4} dy={4} textAnchor="end" fill="var(--fg-secondary)" fontSize={12}>
                  {label}
                </text>
                {kind === 'agent' && <circle cx={gx} cy={0} r={3} fill="none" stroke="var(--fg-secondary)" />}
                {kind === 'routine' && <rect x={gx - 3} y={-3} width={6} height={6} fill="var(--fg-secondary)" />}
              </g>
            )
          }} axisLine={false} tickLine={false} />
          <ChartTooltip cursor={{ fill: 'var(--bg-layer-2)' }} isAnimationActive={false} content={<ChartTooltipContent hideLabel hideIndicator formatter={(_, __, item) => <Tip row={item.payload} />} />} />
          <Bar dataKey="usd" fill={color} radius={[0, 4, 4, 0]} barSize={12} minPointSize={2} isAnimationActive={false} className="cursor-pointer" onClick={(d) => onPick(d.payload.id)}>
            <LabelList dataKey="text" position="right" style={{ fill: 'var(--fg-secondary)', fontSize: 11, fontFamily: 'var(--font-mono)' }} />
          </Bar>
        </BarChart>
      </ChartContainer>
      {rows.length > TOP && <p className="mt-1 text-[11px] text-fg-tertiary">+{rows.length - TOP} more · {usdShort(rows.slice(TOP).reduce((s, r) => s + r.usd, 0))}</p>}
    </>
  )
}

function Donut({ rows, onPick }: { rows: BreakdownRow[]; onPick: (id: string) => void }) {
  const priced = rows.filter((r) => r.usd > 0)
  const color = (i: number) => (i < PIE.length ? PIE[i]! : OTHER_COLOR)
  const config = useMemo(() => Object.fromEntries(rows.map((r, i) => [r.id, { label: r.label, color: color(i) }])), [rows])
  if (!rows.length) return <p className="py-6 text-center text-xs text-fg-tertiary">Nothing in this range.</p>
  return (
    <div className="flex flex-wrap items-center gap-3.5">
      <ChartContainer config={config} className="aspect-square h-[150px] w-[150px]" initialDimension={{ width: 150, height: 150 }}>
        <PieChart>
          <ChartTooltip isAnimationActive={false} content={<ChartTooltipContent hideLabel hideIndicator formatter={(_, __, item) => <Tip row={item.payload} />} />} />
          <Pie data={priced.length ? priced : [{ id: 'none', label: 'unpriced', usd: 1 }]} dataKey="usd" nameKey="id" innerRadius={48} outerRadius={70} paddingAngle={2} stroke="none" isAnimationActive={false} onClick={(d) => priced.length && onPick(d.payload.id)} className="cursor-pointer">
            {(priced.length ? priced : [null]).map((r) => (
              <Cell key={r?.id ?? 'none'} fill={r ? color(rows.indexOf(r)) : 'var(--bg-layer-2)'} />
            ))}
          </Pie>
        </PieChart>
      </ChartContainer>
      <ul className="flex min-w-[180px] flex-1 flex-col gap-2">
        {rows.map((r, i) => (
          <li key={r.id}>
            <button type="button" onClick={() => onPick(r.id)} className="flex w-full items-center gap-2 rounded-sm text-left text-sm hover:text-foreground">
              <Swatch color={r.usd > 0 ? color(i) : 'var(--bg-layer-2)'} />
              <span className="min-w-0 flex-1 truncate">{r.label}</span>
              <span className="font-mono text-xs tabular-nums">{r.usd > 0 ? usdShort(r.usd) : 'unpriced'}</span>
              <span className="w-24 text-right font-mono text-xs whitespace-nowrap text-fg-tertiary tabular-nums">{int(r.calls)} calls</span>
            </button>
          </li>
        ))}
      </ul>
    </div>
  )
}

type Props = { ix: Index; r: Range; filters: Filters; rangeLabel: string; onFilter: (dim: Dim, id: string) => void }

export const Breakdowns = memo(function Breakdowns({ ix, r, filters, rangeLabel, onFilter }: Props) {
  const by = useMemo(() => {
    const out = {} as Record<'model' | 'owner' | 'provider' | 'kind' | 'project', BreakdownRow[]>
    for (const d of ['model', 'owner', 'provider', 'kind', 'project'] as const) out[d] = breakdown(ix, r, filters, d)
    return out
  }, [ix, r, filters])
  const owners = by.owner
  return (
    <div className="grid gap-3 [grid-template-columns:repeat(auto-fit,minmax(min(100%,340px),1fr))]">
      <Panel title={`By model · ${rangeLabel}`}>
        <HBars rows={by.model} color="var(--chart-1)" onPick={(id) => onFilter('model', id)} />
      </Panel>
      <Panel title={`By owner · ${rangeLabel}`} note={owners.length ? <span className="inline-flex items-center gap-1.5"><OwnerGlyph kind="agent" />agent <OwnerGlyph kind="routine" />routine</span> : undefined}>
        <HBars rows={owners} color="var(--chart-2)" onPick={(id) => onFilter('owner', id)} />
      </Panel>
      <Panel title={`By provider · ${rangeLabel}`}>
        <Donut rows={by.provider} onPick={(id) => onFilter('provider', id)} />
      </Panel>
      <Panel title={`By kind · ${rangeLabel}`}>
        <HBars rows={by.kind} color="var(--chart-3)" onPick={(id) => onFilter('kind', id)} />
      </Panel>
      <Panel title={`By project · ${rangeLabel}`}>
        <HBars rows={by.project} color="var(--chart-7)" onPick={(id) => onFilter('project', id)} />
      </Panel>
    </div>
  )
})
