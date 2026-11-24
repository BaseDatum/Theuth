// 215 value surface: "Your Claude subscription covered $X of usage at API rates this month", per subscription login,
// with that plan's usage windows. Reads the held cube index (no fetch), the login and provider lists once, and the
// limits snapshot `LimitsPanel` already polls (cache only: no second poller).

import { useQuery } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { useMemo } from 'react'
import { settings } from '@/components/chat/slash'
import { Button } from '@/components/ui/button'
import type { Index } from '@/lib/costs/cube'
import { int, resetIn, usd } from '@/lib/costs/format'
import { LIMITS_KEY } from '@/lib/costs/queries'
import { coveredLine, type SubscriptionValue as Row, subscriptionValue } from '@/lib/onboarding/subscription-value'
import { queryKeys, useApi } from '@/lib/rpc'
import { useQuerySupport } from '@/rpc/capabilities'
import type { UsageLimits, UsageWindow } from '@/rpc/bindings/method.agent.output'
import { Panel } from './ui'

const STALE = 60_000

export function SubscriptionValue({ ix, now }: { ix: Index; now: number }) {
  const api = useApi()
  const navigate = useNavigate()
  const canLogins = useQuerySupport(['list_logins'])
  const canProviders = useQuerySupport(['list_providers'])
  const logins = useQuery({ enabled: canLogins, queryKey: queryKeys.logins, queryFn: () => api.list_logins(), staleTime: STALE })
  const providers = useQuery({ enabled: canProviders, queryKey: queryKeys.providers, queryFn: () => api.list_providers(), staleTime: STALE })
  const limits = useQuery<UsageLimits>({ queryKey: LIMITS_KEY, queryFn: () => api.usage_limits(), enabled: false }).data
  const rows = useMemo(
    () => (logins.data ? subscriptionValue(ix, logins.data, canProviders ? providers.data : undefined, limits, now) : []),
    [ix, logins.data, canProviders, providers.data, limits, now],
  )
  if (!canLogins || !logins.data) return null
  return (
    <div data-tour="costs-subscription-value">
      <Panel title="Subscription value" note="what your plan's usage would cost at API rates">
        {rows.length === 0 ? (
          <div className="flex flex-wrap items-center gap-x-3 gap-y-2 text-[13px] text-fg-secondary">
            <p className="min-w-0 flex-1">
              With a Claude or ChatGPT plan logged in, calls run on your subscription instead of a per-token bill; this card then shows what they would have cost at API rates.
            </p>
            <Button variant="outline" size="sm" onClick={() => void settings(navigate, 'providers')}>
              Connect a plan
            </Button>
          </div>
        ) : (
          <div className="grid gap-3 [grid-template-columns:repeat(auto-fit,minmax(min(100%,320px),1fr))]">
            {rows.map((r) => (
              <Plan key={r.provider} r={r} now={now} />
            ))}
          </div>
        )}
      </Panel>
    </div>
  )
}

function Plan({ r, now }: { r: Row; now: number }) {
  const plan = r.plan && `${r.plan.charAt(0).toUpperCase()}${r.plan.slice(1)} plan`
  return (
    <div className="flex min-w-0 flex-col gap-2.5 rounded-[10px] border border-border px-3 py-2.5">
      <div className="min-w-0">
        <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
          <span className="font-mono text-[22px] font-semibold tabular-nums">{usd(r.month)}</span>
          <span className="text-[11.5px] text-fg-tertiary">this month</span>
          {(plan || r.account) && (
            <span className="ml-auto flex min-w-0 items-center gap-1.5 self-center">
              {plan && <span className="shrink-0 rounded-md bg-layer-2 px-1.5 py-0.5 text-[11px] font-medium text-fg-secondary">{plan}</span>}
              {r.account && <span className="min-w-0 truncate text-[11px] text-fg-tertiary" title={r.account}>{r.account}</span>}
            </span>
          )}
        </div>
        <p className="mt-0.5 text-[13px] text-fg-secondary">{coveredLine(r, usd)}.</p>
        <p className="mt-1 text-[12px] text-fg-tertiary tabular-nums">
          {usd(r.last30)} over the last 30 days, {int(r.calls30)} {r.calls30 === 1 ? 'call' : 'calls'}
        </p>
      </div>
      {r.windows.length > 0 && (
        <div className="flex flex-col gap-2">
          {r.windows.map((w) => (
            <Window key={w.id} w={w} now={now} />
          ))}
        </div>
      )}
    </div>
  )
}

/** One plan window as a quiet bar: label, percent used and when it resets. */
function Window({ w, now }: { w: UsageWindow; now: number }) {
  const pct = Math.round(w.used_fraction * 100)
  const color = w.used_fraction >= 1 ? 'var(--status-danger-fg)' : w.used_fraction >= 0.9 ? 'var(--status-warning-fg)' : 'var(--fg-accent)'
  return (
    <div className="flex flex-col gap-[3px] text-[12px] text-fg-secondary">
      <div className="flex items-baseline gap-2">
        <span className="min-w-0 truncate">{w.label}</span>
        <span className="ml-auto font-mono text-[11px] whitespace-nowrap tabular-nums">
          {pct}% used{w.resets_at_ms != null && ` · resets ${resetIn(w.resets_at_ms, now)}`}
        </span>
      </div>
      <div className="h-1 overflow-hidden rounded-full bg-layer-2" role="meter" aria-label={`${w.label} used`} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.min(100, pct)}>
        <div className="h-full rounded-full" style={{ width: `${Math.min(100, Math.max(0, pct))}%`, background: color }} />
      </div>
    </div>
  )
}
