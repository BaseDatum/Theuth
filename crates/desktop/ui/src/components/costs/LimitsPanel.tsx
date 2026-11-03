// 160 §5 / §7: the "Usage limits" card — one card per provider snapshot (subscription windows as RadialBar gauges, rate
// limits and balances as meters, status tag, `source · observed N ago`), theuth budget caps as meters, and providers that
// expose nothing as one muted line each. Never a loading state: no data = one sentence.

import { memo, useMemo, type ReactNode } from 'react'
import { PolarAngleAxis, RadialBar, RadialBarChart } from 'recharts'
import type { Balance } from '@/rpc/bindings/Balance'
import type { BudgetLimit } from '@/rpc/bindings/BudgetLimit'
import type { LimitStatus } from '@/rpc/bindings/LimitStatus'
import type { ProviderLimits } from '@/rpc/bindings/ProviderLimits'
import type { RateLimit } from '@/rpc/bindings/RateLimit'
import type { UsageWindow } from '@/rpc/bindings/UsageWindow'
import { ChartContainer, ChartTooltip, ChartTooltipContent } from '@/components/ui/chart'
import { Panel, Tag, type Tone } from '@/components/costs/ui'
import { ago, compact, int, resetIn, usd, usdShort, when } from '@/lib/costs/format'
import { useUsageLimits } from '@/lib/costs/queries'

const WARN = 'var(--status-warning-fg)'
const DANGER = 'var(--status-danger-fg)'
/** Series colour for a used share: `base` below `warn`, ochre from `warn`, carnelian from `danger`. */
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

const GAUGE_CONFIG = { v: { label: 'used' } }
const GAUGE_SIZE = { width: 70, height: 62 }

/** One subscription window: a 210°→-30° arc clamped at 100 %, the real percent in the middle, label and reset below. */
function Gauge({ w, now }: { w: UsageWindow; now: number }) {
  const real = Math.round(w.used_fraction * 100)
  const reset = w.resets_at_ms == null ? null : resetIn(w.resets_at_ms, now)
  const data = useMemo(() => [{ v: clampPct(w.used_fraction), fill: heat(w.used_fraction, 0.75, 0.9) }], [w.used_fraction])
  return (
    <div className="relative flex w-[76px] shrink-0 flex-col items-center">
      <ChartContainer config={GAUGE_CONFIG} initialDimension={GAUGE_SIZE} className="aspect-auto h-[62px] w-[70px]">
        <RadialBarChart data={data} cx={35} cy={33} innerRadius={23} outerRadius={29} barSize={6} startAngle={210} endAngle={-30}>
          <PolarAngleAxis type="number" domain={[0, 100]} tick={false} />
          <RadialBar dataKey="v" name="used" cornerRadius={4} background isAnimationActive={false} />
          <ChartTooltip
            cursor={false}
            content={
              <ChartTooltipContent
                hideLabel
                hideIndicator
                formatter={() => (
                  <span className="font-mono">
                    used {real}%{reset && ` · resets ${reset}`}
                  </span>
                )}
              />
            }
          />
        </RadialBarChart>
      </ChartContainer>
      <span className="pointer-events-none absolute inset-x-0 top-[23px] text-center font-mono text-[14px] font-semibold">{real}%</span>
      <span className="-mt-0.5 max-w-full truncate text-center text-[11px] whitespace-nowrap text-fg-secondary" title={w.label}>
        {w.label}
      </span>
      {reset && (
        <span className="max-w-full truncate text-center font-mono text-[10px] whitespace-nowrap text-fg-tertiary" title={`resets ${reset}`}>
          {reset}
        </span>
      )}
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
        <div className="flex flex-wrap justify-around gap-1">
          {p.windows.map((w) => (
            <Gauge key={w.id} w={w} now={now} />
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

/** A provider with nothing to meter (no usage API, not configured) takes one line, not a card. */
const bare = (p: ProviderLimits): boolean => (p.status === 'unavailable' || p.status === 'not_configured') && p.windows.length === 0 && p.rates.length === 0 && !p.balance

export const LimitsPanel = memo(function LimitsPanel({ now: pageNow, visible = true }: { now: number; visible?: boolean }) {
  const { data } = useUsageLimits(visible)
  // The page clock ticks per minute, the snapshot every 15 s: never read a snapshot as being from the future.
  const now = Math.max(pageNow, data?.generated_at_ms ?? 0)
  const [cards, lines] = useMemo(() => {
    const ps = data?.providers ?? []
    return [ps.filter((p) => !bare(p)), ps.filter(bare)]
  }, [data])
  const budgets = data?.budgets ?? []
  const empty = cards.length === 0 && lines.length === 0 && budgets.length === 0
  return (
    <Panel title="Usage limits" note={data && `generated ${ago(data.generated_at_ms, now)}`}>
      {!data ? (
        <p className="text-[12px] text-fg-tertiary">Usage limits are not available from this engine yet.</p>
      ) : empty ? (
        <p className="text-[12px] text-fg-tertiary">No provider exposes usage limits here.</p>
      ) : (
        <>
          {(cards.length > 0 || budgets.length > 0) && (
            <div className="grid gap-3 [grid-template-columns:repeat(auto-fit,minmax(min(100%,260px),1fr))]">
              {cards.map((p) => (
                <ProviderCard key={p.id} p={p} now={now} />
              ))}
              {budgets.length > 0 && <BudgetsCard budgets={budgets} now={now} />}
            </div>
          )}
          {lines.length > 0 && (
            <ul className={cards.length > 0 || budgets.length > 0 ? 'mt-2.5 flex flex-col gap-1 text-[11.5px] text-fg-tertiary' : 'flex flex-col gap-1 text-[11.5px] text-fg-tertiary'}>
              {lines.map((p) => (
                <li key={p.id}>
                  <span className="text-fg-secondary">{shownLabel(p)}</span> — {STATUS[p.status][1]}
                  {p.message && ` · ${p.message}`}
                </li>
              ))}
            </ul>
          )}
        </>
      )}
    </Panel>
  )
})
