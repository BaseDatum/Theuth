// 215 value surface: what a subscription's usage would have cost at API rates. A call through an OAuth plan carries no
// provider price, so the ledger prices it from theuth's table (basis `estimated`); summing those cells for a provider
// that is logged in with no key of its own is exactly "what the plan covered". Pure: the card feeds it the held cube
// index, `list_logins`, `list_providers` and the `usage_limits` snapshot.

import type { LoginInfo, ProviderInfo, UsageLimits, UsageWindow } from '@/rpc/bindings/method.agent.output'
import { DAY_MS, HOUR_MS, type Index } from '@/lib/costs/cube'

/** The plan's product name per login namespace; anything else shows the namespace. */
export const PLAN_NAME: Readonly<Record<string, string>> = {
  anthropic: 'Claude',
  openai: 'ChatGPT',
  xai: 'Grok',
  cursor: 'Cursor',
  google: 'Gemini',
  'github-copilot': 'Copilot',
}

export const planName = (provider: string): string => PLAN_NAME[provider] ?? provider

export type SubscriptionValue = {
  provider: string
  /** "Claude", "ChatGPT", … */
  name: string
  account: string | null
  /** `usage_limits` plan name (`max`, `pro`), when the provider reports one. */
  plan: string | null
  /** USD at API rates, the current local calendar month. */
  month: number
  /** USD at API rates, the rolling last 30 days. */
  last30: number
  /** Calls in the last 30 days. */
  calls30: number
  /** The plan's usage windows (5-hour, weekly…), from `usage_limits`. */
  windows: UsageWindow[]
}

/**
 * The providers whose usage a subscription pays for: a login with tokens on disk (`logged_in`) and no key of the
 * provider's own (a provider row with `has_key` would mix API-billed calls into the same cells). Engine order.
 */
export function subscriptionProviders(logins: readonly LoginInfo[], providers: readonly ProviderInfo[] | undefined): LoginInfo[] {
  const keyed = new Set((providers ?? []).filter((p) => p.has_key).map((p) => p.name))
  return logins.filter((l) => l.logged_in && !keyed.has(l.name))
}

/** The local month number (`year * 12 + month`) of civil day `day`. */
const monthOfDay = (day: number): number => {
  const d = new Date(day * DAY_MS)
  return d.getUTCFullYear() * 12 + d.getUTCMonth()
}

/**
 * One row per subscription provider: estimated spend this local month and over the last 30 days (cells whose UTC hour
 * starts within `[now - 30 d, now]`), plus the plan's windows. A provider the cube never saw sums to 0.
 */
export function subscriptionValue(
  ix: Index,
  logins: readonly LoginInfo[],
  providers: readonly ProviderInfo[] | undefined,
  limits: UsageLimits | undefined,
  now: number,
): SubscriptionValue[] {
  const subs = subscriptionProviders(logins, providers)
  if (!subs.length) return []
  const estimated = ix.dict.basis.at.get('estimated')
  const nowHour = Math.floor(now / HOUR_MS)
  const fromHour = Math.floor((now - 30 * DAY_MS) / HOUR_MS)
  const thisMonth = monthOfDay(Math.floor(ix.zone.local(nowHour) / 24))
  const at = new Map<number, number>()
  const sums = subs.map(() => ({ month: 0, last30: 0, calls30: 0 }))
  subs.forEach((l, k) => {
    const p = ix.dict.provider.at.get(l.name)
    if (p !== undefined) at.set(p, k)
  })
  if (estimated !== undefined && at.size) {
    const prov = ix.col.provider
    const basis = ix.col.basis
    for (let i = 0; i < ix.n; i++) {
      if (basis[i] !== estimated) continue
      const k = at.get(prov[i]!)
      if (k === undefined) continue
      const h = ix.hour[i]!
      if (h > nowHour) continue
      const usd = ix.nanos[i]! / 1e9
      const s = sums[k]!
      if (ix.month[i] === thisMonth) s.month += usd
      if (h >= fromHour) {
        s.last30 += usd
        s.calls30 += ix.calls[i]!
      }
    }
  }
  return subs.map((l, k) => {
    const rows = (limits?.providers ?? []).filter((r) => r.provider === l.name && r.auth !== 'api_key')
    return {
      provider: l.name,
      name: planName(l.name),
      account: l.account,
      plan: rows.find((r) => r.plan)?.plan ?? null,
      ...sums[k]!,
      windows: rows.flatMap((r) => r.windows),
    }
  })
}

/** The card's headline for one row: "Your Claude subscription covered $41.20 of usage at API rates this month". */
export function coveredLine(v: Pick<SubscriptionValue, 'name' | 'month'>, money: (usd: number) => string): string {
  return `Your ${v.name} subscription covered ${money(v.month)} of usage at API rates this month`
}
