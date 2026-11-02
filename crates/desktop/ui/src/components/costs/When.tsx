// 160 §7.8–7.9: when the money goes (hour × weekday heat grid in local time, an hour-of-day profile) and where the
// tokens go (the input mix as a 100 % area, output tokens per bucket).

import { memo, useCallback, useEffect, useMemo, useState } from 'react'
import { Area, AreaChart, Bar, BarChart, Scatter, ScatterChart, XAxis, YAxis, usePlotArea } from 'recharts'
import { ChartContainer, ChartTooltip, ChartTooltipContent } from '@/components/ui/chart'
import { DOW, type Filters, type Gran, type HeatCell, heat, type Index, type Range, tokenSeries, type TokenRow } from '@/lib/costs/cube'
import { compact, int, pct, usd, usdShort } from '@/lib/costs/format'
import { Panel } from './ui'

const pad2 = (n: number) => String(n).padStart(2, '0')
const TICK = { fill: 'var(--fg-tertiary)', fontSize: 10, fontFamily: 'var(--font-mono)' }

type HeatDatum = HeatCell & { a: number }

/** Reports the plot area's cell size (one hour wide, one weekday tall) once per resize — one store read for all 168 cells. */
function CellSize({ onSize }: { onSize: (w: number, h: number) => void }) {
  const p = usePlotArea()
  const w = p ? p.width / 24 : 0
  const h = p ? p.height / 7 : 0
  useEffect(() => onSize(w, h), [w, h, onSize])
  return null
}

/** The empty grid (all 168 cells) as one path from the plot area; only cells with spend are Scatter points. */
function EmptyCells() {
  const p = usePlotArea()
  if (!p) return null
  const cw = p.width / 24
  const ch = p.height / 7
  const w = Math.max(2, cw - 3)
  const h = Math.max(2, ch - 3)
  let d = ''
  for (let row = 0; row < 7; row++)
    for (let col = 0; col < 24; col++) d += `M${(p.x + col * cw + (cw - w) / 2).toFixed(1)},${(p.y + row * ch + (ch - h) / 2).toFixed(1)}h${w.toFixed(1)}v${h.toFixed(1)}h${(-w).toFixed(1)}Z`
  return <path d={d} fill="var(--bg-layer-2)" fillOpacity={0.6} />
}

type Props = { ix: Index; r: Range; gran: Gran; filters: Filters; rangeLabel: string }
const SPEND_CFG = { usd: { label: 'spend', color: 'var(--chart-1)' } }

/** The heat grid ignores the granularity, so it takes no `gran` and skips a granularity change. */
export const When = memo(function When({ ix, r, filters, rangeLabel }: Omit<Props, 'gran'>) {
  const { cells, hours } = useMemo(() => {
    const g = heat(ix, r, filters)
    const max = Math.max(0, ...g.map((c) => c.usd))
    const hours = Array.from({ length: 24 }, (_, h) => ({ h, label: `${pad2(h)}:00`, usd: 0, calls: 0 }))
    for (const c of g) {
      hours[c.h]!.usd += c.usd
      hours[c.h]!.calls += c.calls
    }
    return { cells: g.filter((c) => c.usd > 0 || c.calls > 0).map((c): HeatDatum => ({ ...c, a: max > 0 ? Math.sqrt(c.usd / max) : 0 })), hours }
  }, [ix, r, filters])
  const [size, setSize] = useState<[number, number]>([0, 0])
  const onSize = useCallback((w: number, h: number) => setSize((s) => (s[0] === w && s[1] === h ? s : [w, h])), [])
  /** A heat cell sized from the plot area minus a 3 px gap, so it never overflows. */
  const square = useCallback(
    (p: { cx?: number; cy?: number; payload?: HeatDatum }) => {
      if (p.cx === undefined || p.cy === undefined || !p.payload || !size[0]) return <g />
      const w = Math.max(2, size[0] - 3)
      const h = Math.max(2, size[1] - 3)
      const on = p.payload.a > 0
      return <rect x={p.cx - w / 2} y={p.cy - h / 2} width={w} height={h} rx={Math.min(3, w / 4)} fill={on ? 'var(--chart-1)' : 'var(--bg-layer-2)'} fillOpacity={on ? 0.18 + 0.82 * p.payload.a : 0.6} />
    },
    [size],
  )
  return (
    <Panel title="When · hour × weekday" note={`local time · ${rangeLabel}`}>
      <ChartContainer config={SPEND_CFG} className="aspect-auto h-[190px] w-full" initialDimension={{ width: 420, height: 190 }}>
        <ScatterChart margin={{ top: 6, right: 16, bottom: 0, left: 0 }} accessibilityLayer={false}>
          <XAxis type="number" dataKey="h" domain={[-0.5, 23.5]} ticks={[0, 3, 6, 9, 12, 15, 18, 21]} tickFormatter={(h: number) => pad2(h)} tick={TICK} tickLine={false} axisLine={false} />
          <YAxis type="number" dataKey="wd" domain={[-0.5, 6.5]} ticks={[0, 1, 2, 3, 4, 5, 6]} interval={0} reversed tickFormatter={(i: number) => DOW[i] ?? ''} tick={TICK} tickLine={false} axisLine={false} width={34} />
          <ChartTooltip
            cursor={false}
            isAnimationActive={false}
            content={
              <ChartTooltipContent
                hideIndicator
                labelFormatter={(_, p) => {
                  const c: HeatDatum | undefined = p[0]?.payload
                  return c ? `${DOW[c.wd]} ${pad2(c.h)}:00` : ''
                }}
                formatter={(_, __, item, i) => {
                  const c: HeatDatum = item.payload
                  return i === 0 ? (
                    <span className="font-mono">
                      {usd(c.usd)} · {int(c.calls)} calls
                    </span>
                  ) : null
                }}
              />
            }
          />
          <EmptyCells />
          <Scatter data={cells} shape={square} isAnimationActive={false} />
          <CellSize onSize={onSize} />
        </ScatterChart>
      </ChartContainer>
      <ChartContainer config={SPEND_CFG} className="mt-2 aspect-auto h-[64px] w-full" initialDimension={{ width: 420, height: 64 }}>
        <BarChart data={hours} margin={{ top: 2, right: 16, bottom: 0, left: 34 }} barCategoryGap={2}>
          <XAxis dataKey="label" hide />
          <ChartTooltip cursor={{ fill: 'var(--bg-layer-2)' }} isAnimationActive={false} content={<ChartTooltipContent hideIndicator formatter={(v, _, item) => <span className="font-mono">{usd(Number(v))} · {int(item.payload.calls)} calls</span>} />} />
          <Bar dataKey="usd" fill="var(--chart-1)" fillOpacity={0.7} radius={[2, 2, 0, 0]} isAnimationActive={false} />
        </BarChart>
      </ChartContainer>
      <p className="mt-1 text-[11px] text-fg-tertiary">hour-of-day profile · peak {usdShort(Math.max(0, ...hours.map((h) => h.usd)))} at {pad2(hours.reduce((b, h) => (h.usd > hours[b]!.usd ? h.h : b), 0))}:00</p>
    </Panel>
  )
})

const MIX = { cacheRead: { label: 'cache read', color: 'var(--chart-1)' }, cacheWrite: { label: 'cache write', color: 'var(--chart-2)' }, uncached: { label: 'uncached', color: 'var(--chart-5)' }, output: { label: 'output', color: 'var(--chart-3)' } }

export const Tokens = memo(function Tokens({ ix, r, gran, filters }: Props) {
  const rows = useMemo(() => tokenSeries(ix, { ...r, gran, filters }), [ix, r, gran, filters])
  const t = rows.reduce((a, x) => ({ read: a.read + x.cacheRead, input: a.input + x.cacheRead + x.cacheWrite + x.uncached }), { read: 0, input: 0 })
  const tip = <ChartTooltipContent labelFormatter={(_, p) => { const row: TokenRow | undefined = p[0]?.payload; return row?.tip }} formatter={(v, name) => (
    <div className="flex w-full items-center gap-2">
      <span className="size-2.5 rounded-[2px]" style={{ background: MIX[name as keyof typeof MIX]?.color }} />
      <span className="text-fg-secondary">{MIX[name as keyof typeof MIX]?.label}</span>
      <span className="ml-auto font-mono tabular-nums">{compact(Number(v))}</span>
    </div>
  )} />
  return (
    <Panel title={`Input mix · cache hit ${t.input ? pct(t.read / t.input) : '—'}`} note={`share of input tokens per ${gran}`}>
      {rows.length === 0 ? (
        <p className="py-16 text-center text-xs text-fg-tertiary">No tokens in this range.</p>
      ) : (
        <>
          <ChartContainer config={MIX} className="aspect-auto h-[150px] w-full" initialDimension={{ width: 420, height: 150 }}>
            <AreaChart data={rows} stackOffset="expand" margin={{ top: 6, right: 18, bottom: 0, left: 0 }}>
              <XAxis dataKey="label" tick={{ ...TICK, fontFamily: undefined }} tickLine={false} axisLine={false} minTickGap={24} />
              <YAxis tickFormatter={(v: number) => `${Math.round(v * 100)}%`} tick={TICK} tickLine={false} axisLine={false} width={40} />
              <ChartTooltip isAnimationActive={false} content={tip} />
              <Area type="monotone" dataKey="cacheRead" stackId="1" stroke="var(--chart-1)" fill="var(--chart-1)" fillOpacity={0.55} isAnimationActive={false} />
              <Area type="monotone" dataKey="cacheWrite" stackId="1" stroke="var(--chart-2)" fill="var(--chart-2)" fillOpacity={0.55} isAnimationActive={false} />
              <Area type="monotone" dataKey="uncached" stackId="1" stroke="var(--chart-5)" fill="var(--chart-5)" fillOpacity={0.55} isAnimationActive={false} />
            </AreaChart>
          </ChartContainer>
          <ChartContainer config={MIX} className="mt-2 aspect-auto h-[64px] w-full" initialDimension={{ width: 420, height: 64 }}>
            <BarChart data={rows} margin={{ top: 2, right: 18, bottom: 0, left: 40 }}>
              <XAxis dataKey="label" hide />
              <ChartTooltip cursor={{ fill: 'var(--bg-layer-2)' }} isAnimationActive={false} content={tip} />
              <Bar dataKey="output" fill="var(--chart-3)" radius={[2, 2, 0, 0]} isAnimationActive={false} />
            </BarChart>
          </ChartContainer>
          <p className="mt-1 text-[11px] text-fg-tertiary">output tokens per {gran} · {compact(rows.reduce((s, x) => s + x.output, 0))} total</p>
        </>
      )}
    </Panel>
  )
})
