// 160 §5 / §7: the "Usage limits" card — one card per provider that reports a real limit (subscription windows as pace
// tiles: usage so far against the even pace, projected to the reset; rate limits and balances as meters, status tag,
// `source · observed N ago`) and theuth's spending caps as meters. A provider with nothing measured (no meter,
// unknown/empty schema, a stale placeholder without last values) is not shown at all; routine runs (dreaming,
// schedules) are not spending caps. Never a loading state: no data = one sentence.

import { memo, useMemo, type ReactNode } from 'react'
import { ComposedChart, Line, ReferenceArea, ReferenceDot, ReferenceLine, XAxis, YAxis } from 'recharts'
import type { Balance } from "@/rpc/bindings/method.agent.output"
import type { BudgetLimit } from "@/rpc/bindings/method.agent.output"
import type { LimitStatus } from "@/rpc/bindings/method.agent.output"
import type { ProviderLimits } from "@/rpc/bindings/method.agent.output"
import type { RateLimit } from "@/rpc/bindings/method.agent.output"
import type { UsageWindow } from "@/rpc/bindings/method.agent.output"
import { ChartContainer, ChartTooltip, ChartTooltipContent } from '@/components/ui/chart'
import { Panel, Tag, type Tone } from '@/components/costs/ui'
import { ago, compact, int, resetIn, usd, usdShort, when } from '@/lib/costs/format'
import { pace, type Pace } from '@/lib/costs/pace'
import { useUsageLimits } from '@/lib/costs/queries'

const WARN = 'var(--status-warning-fg)'
const DANGER = 'var(--status-danger-fg)'
/** Series colour for a used share: `base` below `warn`, the warning blue from `warn`, carnelian from `danger`. */
const heat = (share: number, warn: number, danger: number, base = 'var(--chart-1)'): string => (share >= danger ? DANGER : share >= warn ? WARN : base)
const clampPct = (share: number): number => Math.min(100, Math.max(0, share * 100))

const STATUS: Record<LimitStatus, [Tone, string]> = {
  ok: ['ok', 'ok'],
  warn: ['warn', 'warn'],
  limited: ['danger', 'limited'],
  stale: ['muted', 'stale'],
  error: ['danger', 'error'],
  unavailable: ['muted', 'no api'],
  not_configured: ['muted', 'not configured'],
}

/** The status tag; `warn` names the hottest window (`opus 78%`) when there is one. */
function StatusTag({ p }: { p: ProviderLimits }) {
  const [tone, text] = STATUS[p.status]
  if (p.status !== 'warn' || p.windows.length === 0) return <Tag tone={tone}>{text}</Tag>
  const hot = p.windows.reduce((a, b) => (b.used_fraction > a.used_fraction ? b : a))
  const name = hot.label.split(' · ').at(-1)?.toLowerCase() ?? hot.label
  return <Tag tone="warn">{`${name} ${Math.round(hot.used_fraction * 100)}%`}</Tag>
}

const PACE_CONFIG = { v: { label: 'used' } }
const PACE_SIZE = { width: 200, height: 84 }
const PACE_MARGIN = { top: 4, right: 4, bottom: 4, left: 4 }
const MUTED = 'var(--fg-tertiary)'
const OK = 'var(--status-success-fg)'

/** The one-line verdict: over pace (and when 100 % is hit), cutting close, or under pace — by the projection at reset. */
function Verdict({ p, now }: { p: Pace; now: number }) {
  if (p.projected == null) return <span className="text-fg-tertiary">· too early to project</span>
  const end = Math.round(p.projected * 100)
  if (p.projected >= 1) {
    const hits = p.used >= 1 ? ' · limit reached' : p.hitsAt != null ? ` · hits 100% ${resetIn(p.hitsAt, now)}` : ''
    return <span style={{ color: DANGER }}>{`▲ on track for ${end}%${hits}`}</span>
  }
  if (p.projected >= 0.9) return <span style={{ color: WARN }}>{`● cutting close · ${end}% by reset`}</span>
  return <span style={{ color: OK }}>{`▼ under pace · ${end}% by reset`}</span>
}

/** The pace chart: even pace (dashed) to 100 % at reset, the limit, your usage (solid), the projection (dotted) to reset. */
function PaceChart({ p, color, tip }: { p: Pace; color: string; tip: string }) {
  const end = p.projected ?? p.used
  const ymax = Math.max(1.05, end * 1.05)
  return (
    <ChartContainer config={PACE_CONFIG} initialDimension={PACE_SIZE} className="aspect-auto h-[84px] w-full">
      <ComposedChart data={p.line} margin={PACE_MARGIN}>
        <XAxis dataKey="t" type="number" domain={[p.start, p.reset]} hide allowDataOverflow />
        <YAxis type="number" domain={[0, ymax]} hide allowDataOverflow />
        {end > 1 && <ReferenceArea x1={p.start} x2={p.reset} y1={1} y2={ymax} fill={DANGER} fillOpacity={0.06} stroke="none" />}
        <ReferenceLine y={1} stroke={DANGER} strokeOpacity={0.45} />
        <ReferenceLine segment={[{ x: p.start, y: 0 }, { x: p.reset, y: 1 }]} stroke={MUTED} strokeDasharray="4 3" strokeWidth={1.2} />
        <ReferenceLine x={p.at} stroke="var(--border)" />
        {p.projected != null && <ReferenceLine segment={[{ x: p.at, y: p.used }, { x: p.reset, y: p.projected }]} stroke={color} strokeDasharray="1.5 3" strokeWidth={2} />}
        <Line dataKey="v" type="linear" stroke={color} strokeWidth={2} dot={false} activeDot={false} isAnimationActive={false} />
        <ReferenceDot x={p.at} y={p.used} r={3} fill={color} stroke="none" />
        <ChartTooltip cursor={false} content={<ChartTooltipContent hideLabel hideIndicator formatter={() => <span className="font-mono">{tip}</span>} />} />
      </ComposedChart>
    </ChartContainer>
  )
}

/**
 * One subscription window: the real percent, label and share of time gone; the verdict; the pace chart from window start
 * to reset. Without a reset time or window length there is no pace: percent and label only.
 */
function PaceTile({ w, now }: { w: UsageWindow; now: number }) {
  const real = Math.round(w.used_fraction * 100)
  const reset = w.resets_at_ms == null ? null : resetIn(w.resets_at_ms, now)
  const p = useMemo(() => pace(w, now), [w, now])
  const head = (
    <div className="flex min-w-0 items-baseline gap-1.5">
      <span className="font-mono text-[18px] font-semibold">{real}%</span>
      <span className="min-w-0 truncate text-[11.5px] text-fg-secondary" title={w.label}>
        {w.label}
      </span>
      {p ? (
        <span className="ml-auto font-mono text-[10.5px] whitespace-nowrap text-fg-tertiary">{Math.round(p.elapsed * 100)}% of time gone</span>
      ) : (
        reset && <span className="ml-auto font-mono text-[10.5px] whitespace-nowrap text-fg-tertiary">resets {reset}</span>
      )}
    </div>
  )
  if (!p) return <div className="min-w-0 rounded-[10px] border border-border px-2.5 py-2">{head}</div>
  const color = heat(p.projected ?? p.used, 0.9, 1)
  const tip = [`used ${real}%`, p.projected != null && `projected ${Math.round(p.projected * 100)}%`, reset && `resets ${reset}`].filter(Boolean).join(' · ')
  return (
    <div className="flex min-w-0 flex-col rounded-[10px] border border-border px-2.5 pt-2 pb-1.5">
      {head}
      <div className="mt-0.5 truncate font-mono text-[11px]">
        <Verdict p={p} now={now} />
      </div>
      <PaceChart p={p} color={color} tip={tip} />
      <div className="flex justify-between font-mono text-[10px] text-fg-tertiary">
        <span>start</span>
        <span>now</span>
        <span>resets {reset}</span>
      </div>
    </div>
  )
}

/** A progress bar (not a chart): label left, mono value right, a 4 px track with `share` filled in `color`. */
function Meter({ label, right, share, color }: { label: ReactNode; right: ReactNode; share: number; color: string }) {
  return (
    <div className="flex flex-col gap-[3px] text-[12px] text-fg-secondary">
      <div className="flex flex-wrap items-baseline gap-x-2">
        <span className="min-w-0 truncate">{label}</span>
        <span className="ml-auto font-mono whitespace-nowrap text-fg-tertiary">{right}</span>
      </div>
      <div className="h-1 overflow-hidden rounded-[2px] bg-layer-2">
        <div className="h-full rounded-[2px]" style={{ width: `${clampPct(share)}%`, background: color }} />
      </div>
    </div>
  )
}

function RateMeter({ r }: { r: RateLimit }) {
  const share = r.limit > 0 ? (r.limit - r.remaining) / r.limit : 0
  return (
    <Meter
      label={
        <>
          {r.label}
          {r.scope && <span className="ml-1.5 text-fg-tertiary">{r.scope}</span>}
        </>
      }
      right={`${compact(r.remaining)} / ${compact(r.limit)} free`}
      share={share}
      color={heat(share, 0.85, 0.95)}
    />
  )
}

/** `label · remaining of limit currency`, plus a meter when both the cap and a used amount are known. */
function BalanceLine({ b }: { b: Balance }) {
  const money = b.currency === 'USD'
  const f = money ? usd : int
  const unit = money ? '' : ` ${b.currency}`
  const of = b.limit != null ? ` of ${f(b.limit)}` : ''
  const amount = b.remaining != null ? `${f(b.remaining)}${of}${unit}` : b.used != null ? `${f(b.used)} used${of}${unit}` : '—'
  const used = b.used ?? (b.remaining != null && b.limit != null ? b.limit - b.remaining : null)
  const share = b.limit != null && b.limit > 0 && used != null ? used / b.limit : null
  if (share == null) return <div className="text-[12px] text-fg-secondary">{`${b.label} · ${amount}`}</div>
  return <Meter label={b.label} right={amount} share={share} color={heat(share, 0.85, 0.95)} />
}

function ProviderCard({ p, now }: { p: ProviderLimits; now: number }) {
  const nextReset = p.rates.reduce<number | null>((m, r) => (r.resets_at_ms != null && (m == null || r.resets_at_ms < m) ? r.resets_at_ms : m), null)
  const footer = [p.source, p.observed_at_ms != null && `observed ${ago(p.observed_at_ms, now)}`, nextReset != null && `resets ${ago(nextReset, now)}`].filter(Boolean).join(' · ')
  return (
    <div className="flex min-w-0 flex-col gap-2.5 rounded-xl border border-border bg-background p-3">
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <span className="text-[13px] font-semibold">{shownLabel(p)}</span>
        <Tag>{p.plan ? `${authLabel(p.auth)} · ${p.plan}` : authLabel(p.auth)}</Tag>
        {p.account && <span className="min-w-0 truncate text-[11px] text-fg-tertiary">{p.account}</span>}
        <div className="flex-1" />
        <StatusTag p={p} />
      </div>
      {p.windows.length > 0 && (
        <div className="grid gap-2.5 [grid-template-columns:repeat(auto-fit,minmax(200px,1fr))]">
          {p.windows.map((w) => (
            <PaceTile key={w.id} w={w} now={now} />
          ))}
        </div>
      )}
      {p.rates.map((r) => (
        <RateMeter key={r.id} r={r} />
      ))}
      {p.balance && <BalanceLine b={p.balance} />}
      {p.message && <div className={p.status === 'error' ? 'text-[11.5px] text-danger' : 'text-[11.5px] text-fg-tertiary'}>{p.message}</div>}
      <div className="text-[11px] text-fg-tertiary">{footer}</div>
    </div>
  )
}

// The engine's custom-provider auth value stays `gateway` (persisted row ids and labels, 162 D1); users read `custom`.
const authLabel = (auth: string): string => (auth === 'api_key' ? 'key' : auth === 'gateway' ? 'custom' : auth)
const shownLabel = (p: ProviderLimits): string => p.label.replace(/ · gateway$/, ' · custom')

function BudgetsCard({ budgets, now }: { budgets: BudgetLimit[]; now: number }) {
  const soonest = budgets.reduce<number | null>((m, b) => (b.resets_at_ms != null && (m == null || b.resets_at_ms < m) ? b.resets_at_ms : m), null)
  return (
    <div className="flex min-w-0 flex-col gap-2.5 rounded-xl border border-border bg-background p-3">
      <div className="flex items-center gap-2">
        <span className="text-[13px] font-semibold">theuth budgets</span>
        <Tag>caps</Tag>
      </div>
      {budgets.map((b) => {
        const share = b.cap_usd > 0 ? b.spent_usd / b.cap_usd : 0
        return (
          <Meter
            key={b.id}
            label={
              <>
                {b.label}
                <span className="ml-1.5 text-fg-tertiary">{b.period}</span>
              </>
            }
            right={`${usdShort(b.spent_usd)} of ${usdShort(b.cap_usd)} · ${Math.round(share * 100)}%`}
            share={share}
            color={heat(share, 0.75, 0.9, 'var(--chart-2)')}
          />
        )
      })}
      {soonest != null && <div className="text-[11px] text-fg-tertiary">resets {when(soonest, now)}</div>}
    </div>
  )
}

/**
 * A provider earns a card only with something measured: a window, a rate bucket with a limit, or a balance with a value.
 * Status alone (stale, error, no api, not configured) is not a limit; a stale card with last-known values still shows.
 */
export const hasLimits = (p: ProviderLimits): boolean =>
  p.windows.length > 0 || p.rates.some((r) => r.limit > 0) || (p.balance != null && (p.balance.remaining != null || p.balance.used != null || p.balance.limit != null))

/** User-facing spending caps only: a routine's run budget (`routine:<name>`, e.g. dreaming's night) is not one. */
export const isSpendingCap = (b: BudgetLimit): boolean => !b.id.startsWith('routine:') && b.cap_usd > 0

export const LimitsPanel = memo(function LimitsPanel({ now: pageNow, visible = true }: { now: number; visible?: boolean }) {
  const { data } = useUsageLimits(visible)
  // The page clock ticks per minute, the snapshot every 15 s: never read a snapshot as being from the future.
  const now = Math.max(pageNow, data?.generated_at_ms ?? 0)
  const cards = useMemo(() => (data?.providers ?? []).filter(hasLimits), [data])
  const budgets = useMemo(() => (data?.budgets ?? []).filter(isSpendingCap), [data])
  return (
    <Panel title="Usage limits" note={data && `generated ${ago(data.generated_at_ms, now)}`}>
      {!data ? (
        <p className="text-[12px] text-fg-tertiary">Usage limits are not available from this engine yet.</p>
      ) : (
        <>
          {cards.length === 0 && <p className={budgets.length > 0 ? 'mb-2.5 text-[12px] text-fg-tertiary' : 'text-[12px] text-fg-tertiary'}>No usage limits reported by your providers.</p>}
          {(cards.length > 0 || budgets.length > 0) && (
            <div className="grid gap-3 [grid-template-columns:repeat(auto-fit,minmax(min(100%,260px),1fr))]">
              {cards.map((p) => (
                <ProviderCard key={p.id} p={p} now={now} />
              ))}
              {budgets.length > 0 && <BudgetsCard budgets={budgets} now={now} />}
            </div>
          )}
        </>
      )}
    </Panel>
  )
})
