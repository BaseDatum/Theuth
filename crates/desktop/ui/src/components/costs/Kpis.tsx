// 160 §7.4: six fixed-size KPI cards; the figures tick with NumberFlow so a live update moves digits, not layout (§6.8).

import NumberFlow from '@number-flow/react'
import { memo, type ReactNode, useMemo } from 'react'
import { Area, AreaChart } from 'recharts'
import { ChartContainer, ChartTooltip, ChartTooltipContent } from '@/components/ui/chart'
import { BASES, BASIS_COLOR, BASIS_LABEL, DAY_MS, type Filters, HOUR_MS, type Index, previousPeriod, type Range, series, totals } from '@/lib/costs/cube'
import { compact, int, pct, usd, usdShort } from '@/lib/costs/format'
import { autoGran } from '@/lib/costs/search'
import { cn } from '@/lib/utils'

const USD = { style: 'currency', currency: 'USD', minimumFractionDigits: 2, maximumFractionDigits: 2 } as const
const COMPACT = { notation: 'compact', maximumFractionDigits: 2 } as const
const MONTHS = ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December']
const SPARK_CFG = { v: { label: 'spend', color: 'var(--chart-1)' } }

function Card({ title, value, wrap = false, children }: { title: string; value: ReactNode; wrap?: boolean; children?: ReactNode }) {
  return (
    <div className="flex min-h-[138px] min-w-0 flex-col rounded-xl border border-border bg-card px-3.5 py-3">
      <h3 className="flex items-center gap-[7px] font-mono text-[10px] font-medium tracking-[1.4px] text-fg-secondary uppercase">
        <span className="inline-block size-[5px] bg-fg-tertiary" />
        {title}
      </h3>
      <div className={cn('mt-1.5 font-mono text-[20px] leading-tight font-semibold tracking-[-0.3px] tabular-nums', wrap ? 'line-clamp-2' : 'truncate')}>{value}</div>
      <div className="mt-0.5 flex min-h-0 flex-1 flex-col gap-0.5 overflow-hidden text-[11.5px] text-fg-tertiary">{children}</div>
    </div>
  )
}

const M = ({ children }: { children: ReactNode }) => <span className="font-mono tabular-nums text-fg-secondary">{children}</span>

type Props = { ix: Index; r: Range; filters: Filters; now: number; rangeLabel: string }

export const Kpis = memo(function Kpis({ ix, r, filters, now, rangeLabel }: Props) {
  const z = ix.zone
  const k = useMemo(() => {
    const t = totals(ix, r, filters)
    const prev = totals(ix, previousPeriod(r), filters)
    const today = Math.floor(z.local(Math.floor(now / HOUR_MS)) / 24)
    const midnight = z.midnight(today)
    const nowH = Math.floor(now / HOUR_MS) * HOUR_MS
    const d = new Date(today * DAY_MS)
    const monthStart = z.midnight(Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), 1) / DAY_MS)
    const lastMonthStart = z.midnight(Date.UTC(d.getUTCFullYear(), d.getUTCMonth() - 1, 1) / DAY_MS)
    const nextMonthStart = z.midnight(Date.UTC(d.getUTCFullYear(), d.getUTCMonth() + 1, 1) / DAY_MS)
    const spend = (from: number, to: number) => totals(ix, { from, to }, filters).usd
    const thisHour = spend(nowH, nowH + HOUR_MS)
    const lastHour = spend(nowH - HOUR_MS, nowH)
    const mtd = spend(monthStart, now)
    const spark = series(ix, { ...r, gran: autoGran(r), groupBy: 'basis', metric: 'usd', filters, topN: 4 }).rows.map((x) => ({ start: x.start, tip: x.tip, v: x.total }))
    return {
      t,
      delta: prev.usd > 0 ? (t.usd - prev.usd) / prev.usd : null,
      today: spend(midnight, now),
      yesterday: spend(z.midnight(today - 1), midnight),
      thisHour,
      // The last 60 minutes at hour precision: the previous hour plus the elapsed part of this one.
      burn: (lastHour + thisHour) / (1 + (now - nowH) / HOUR_MS),
      mtd,
      projected: (mtd * (nextMonthStart - monthStart)) / Math.max(HOUR_MS, now - monthStart),
      lastMonth: spend(lastMonthStart, monthStart),
      month: MONTHS[d.getUTCMonth()]!,
      lastMonthName: MONTHS[(d.getUTCMonth() + 11) % 12]!,
      spark,
    }
  }, [ix, r, filters, now, z])
  const { t } = k
  const priced = t.usdBy[0] + t.usdBy[1] + t.usdBy[2]
  const hit = t.input > 0 ? t.cacheRead / t.input : 0
  const bases = BASES.slice(0, 3)
    .map((b, i) => ({ b, usd: t.usdBy[i]! }))
    .filter((x) => x.usd > 0)
    .sort((a, b) => b.usd - a.usd)
  return (
    <div className="grid grid-cols-2 gap-3 @2xl:grid-cols-3 @7xl:grid-cols-6">
      <Card title={`Spend · ${rangeLabel}`} value={<NumberFlow value={t.usd} format={USD} locales="en-US" />}>
        <span>
          {k.delta === null ? (
            'no earlier data to compare'
          ) : (
            <>
              <span className={cn('rounded-md px-1.5 py-px font-mono text-[11px]', k.delta >= 0 ? 'bg-warning-bg text-fg-accent' : 'bg-success-bg text-success')}>
                {k.delta >= 0 ? '▲' : '▼'} {pct(Math.abs(k.delta))}
              </span>{' '}
              vs previous {rangeLabel}
            </>
          )}
        </span>
        <ChartContainer config={SPARK_CFG} className="mt-auto aspect-auto h-[30px] w-full" initialDimension={{ width: 160, height: 30 }}>
          <AreaChart data={k.spark} margin={{ top: 2, right: 0, bottom: 0, left: 0 }}>
            <defs>
              <linearGradient id="costs-spark" x1="0" y1="0" x2="0" y2="1">
                <stop offset="0%" stopColor="var(--chart-1)" stopOpacity={0.35} />
                <stop offset="100%" stopColor="var(--chart-1)" stopOpacity={0} />
              </linearGradient>
            </defs>
            <ChartTooltip cursor={false} isAnimationActive={false} content={<ChartTooltipContent hideIndicator labelFormatter={(_, p) => { const row: { tip: string } | undefined = p[0]?.payload; return row?.tip }} formatter={(v) => <span className="font-mono">{usd(Number(v))}</span>} />} />
            <Area type="monotone" dataKey="v" name="v" stroke="var(--chart-1)" strokeWidth={1.5} fill="url(#costs-spark)" isAnimationActive={false} />
          </AreaChart>
        </ChartContainer>
      </Card>
      <Card title="Today" value={<NumberFlow value={k.today} format={USD} locales="en-US" />}>
        <span>
          this hour <M>{usd(k.thisHour)}</M> · burn <M>{usdShort(k.burn)}/h</M>
        </span>
        <span>
          yesterday <M>{usd(k.yesterday)}</M>
        </span>
      </Card>
      <Card title={k.month} value={<NumberFlow value={k.mtd} format={USD} locales="en-US" />}>
        <span>
          projected <M>{usd(k.projected)}</M> at this pace
        </span>
        <span>
          {k.lastMonthName} <M>{usd(k.lastMonth)}</M>
        </span>
      </Card>
      <Card title="Calls" value={<NumberFlow value={t.calls} locales="en-US" />}>
        <span>
          <M>{t.calls > 0 ? usd(t.usd / t.calls) : '—'}</M> per call · <M>{int(t.failed)}</M> failed
        </span>
        {t.repriced > 0 && (
          <span>
            <M>{int(t.repriced)}</M> priced at read time
          </span>
        )}
      </Card>
      <Card title="Tokens" value={<NumberFlow value={t.input + t.output} format={COMPACT} locales="en-US" />}>
        <span>
          <M>{compact(t.output)}</M> out · cache hit <M>{pct(hit)}</M>
        </span>
        {t.cacheSavedUsd > 0 && (
          <span>
            cache saved ≈ <M>{usdShort(t.cacheSavedUsd)}</M>
          </span>
        )}
      </Card>
      <Card
        title="Basis"
        wrap
        value={
          <span className="text-[15px] font-semibold">
            {bases.length === 0 ? (
              <span className="text-fg-tertiary">no priced calls</span>
            ) : (
              bases.slice(0, 2).map((x, i) => (
                <span key={x.b}>
                  {i > 0 && ' · '}
                  {usdShort(x.usd)} <span className="font-normal text-fg-tertiary">{x.b === 'estimated' ? 'est.' : x.b}</span>
                </span>
              ))
            )}
          </span>
        }
      >
        <div className="my-1.5 flex h-1.5 overflow-hidden rounded-[3px] bg-layer-2" role="img" aria-label={BASES.slice(0, 3).map((b, i) => `${BASIS_LABEL[b]} ${usd(t.usdBy[i]!)}`).join(', ')}>
          {BASES.slice(0, 3).map((b, i) => (
            <div key={b} style={{ width: `${priced > 0 ? (t.usdBy[i]! / priced) * 100 : 0}%`, background: BASIS_COLOR[b] }} />
          ))}
        </div>
        <span>
          known <M>{usd(t.usdBy[0])}</M> · <M>{int(t.callsBy[3])}</M> calls unpriced
        </span>
        <span title="Failed and cancelled attempts carry no tokens; a cancelled stream may still have been billed upstream.">unpriced is a lower bound</span>
      </Card>
    </div>
  )
})
