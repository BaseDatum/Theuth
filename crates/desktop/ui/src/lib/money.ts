import type { CostBucket } from "@/rpc/bindings/method.agent.output"
import type { Money } from "@/rpc/bindings/method.agent.output"

const NANOS_PER_MILLI = 1_000_000n

const symbols = new Map<string, string | null>()

/**
 * The currency's symbol from the platform's ISO 4217 data (`Intl`, English): `$`, `€`, `£`, `¥`, and
 * look-alikes disambiguated (`CA$`, `A$`, `CN¥`). `null` when `Intl` only knows the code, so the caller
 * prints `XYZ 0.123` instead.
 */
function symbol(currency: string): string | null {
  let s = symbols.get(currency)
  if (s === undefined) {
    try {
      const part = new Intl.NumberFormat('en', { style: 'currency', currency }).formatToParts(0).find((p) => p.type === 'currency')?.value
      s = part && part !== currency ? part : null
    } catch {
      s = null
    }
    symbols.set(currency, s)
  }
  return s
}

/** `$0.123`, `€0.123`, `CA$0.123` (symbol from `Intl`), else `XYZ 0.123`: integer nanos (decimal string) → units with 3 decimals, rounded half up; never a float. */
export function formatMoney(m: Money): string {
  const milli = (BigInt(m.nanos) + NANOS_PER_MILLI / 2n) / NANOS_PER_MILLI
  const units = milli / 1000n
  const frac = (milli % 1000n).toString().padStart(3, '0')
  const s = symbol(m.currency)
  return s ? `${s}${units}.${frac}` : `${m.currency} ${units}.${frac}`
}

/** Nanos (decimal string) → cents, rounded half up; never a float. */
const cents = (m: Money): bigint => (BigInt(m.nanos) + 5_000_000n) / 10_000_000n

/** `$4.12`, `€4.12`, else `XYZ 4.12`: a session total at cents (a ballpark gauge), rounded half up. */
export function formatCents(m: Money): string {
  const c = cents(m)
  const v = `${c / 100n}.${(c % 100n).toString().padStart(2, '0')}`
  const s = symbol(m.currency)
  return s ? `${s}${v}` : `${m.currency} ${v}`
}

/**
 * `formatCents` split for a rolling display (`CostPill`): the symbol (or `XYZ ` code) as text and the cents as a
 * number, exact at two decimals (`4.12`), so `Intl` prints the same digits `formatCents` does.
 */
export function moneyParts(m: Money): { prefix: string; value: number } {
  const s = symbol(m.currency)
  return { prefix: s ?? `${m.currency} `, value: Number(cents(m)) / 100 }
}

/**
 * 51 §6.2 display strings. Returned money and estimates never appear in one string:
 * `primary` is the returned part (`—` when nothing was dispatched) plus the unavailable
 * count; `secondary` is the estimate line (catalog or adapter rates over calls the
 * provider did not price), present only when an estimate exists.
 */
export function costLines(b: CostBucket): { primary: string; secondary?: string } {
  if (b.dispatched === 0) return { primary: 'Returned cost —' }
  const returned = b.returned.length ? b.returned.map(formatMoney).join(' + ') : '—'
  const primary = b.unavailable > 0 ? `Returned cost ${returned} · ${b.unavailable} unavailable` : `Returned cost ${returned}`
  if (!b.estimated_for_unavailable.length) return { primary }
  return { primary, secondary: `Estimated ${b.estimated_for_unavailable.map((m) => `~${formatMoney(m)}`).join(' + ')}` }
}

/**
 * The terms of a bucket: returned money and the estimate. A known-zero returned entry (an omp child's meter before it
 * reports) is dropped when an estimate or unavailable calls exist, so it never reads as the whole cost.
 */
function costParts(b: CostBucket): { returned: Money[]; estimated: Money[] } {
  const estimated = b.estimated_for_unavailable
  const hideZero = estimated.length > 0 || b.unavailable > 0
  return { returned: hideZero ? b.returned.filter((m) => BigInt(m.nanos) !== 0n) : b.returned, estimated }
}

/**
 * A session's (desk's, task's, subagent's) cost as ONE figure (Sky 2026-11-17): returned plus estimated money of every
 * bucket summed; `approx` when any part is an estimate. `null` when nothing is priced or the parts span currencies.
 */
export function costTotal(...bs: CostBucket[]): { money: Money; approx: boolean } | null {
  const parts = bs.flatMap((b) => {
    const { returned, estimated } = costParts(b)
    return [...returned.map((m) => ({ m, est: false })), ...estimated.map((m) => ({ m, est: true }))]
  })
  const currency = parts[0]?.m.currency
  if (currency === undefined || parts.some((p) => p.m.currency !== currency)) return null
  const nanos = parts.reduce((n, p) => n + BigInt(p.m.nanos), 0n)
  return { money: { currency, nanos: String(nanos) }, approx: parts.some((p) => p.est) }
}

/**
 * The buckets' total as one string: `$4.12`, `~$4.12` when part of it is estimated, `—` when nothing is priced. Mixed
 * currencies keep the split (`€0.500 + ~$0.023`): money in two currencies does not add up.
 */
export function costShort(...bs: CostBucket[]): string {
  const t = costTotal(...bs)
  if (t) return `${t.approx ? '~' : ''}${formatCents(t.money)}`
  const terms = bs.flatMap((b) => {
    const { returned, estimated } = costParts(b)
    return [...returned.map(formatMoney), ...estimated.map((m) => `~${formatMoney(m)}`)]
  })
  return terms.length ? terms.join(' + ') : '—'
}

/** The tooltip beside `costShort`: says why a `~` is there; mixed currencies keep `costLines`; else none. */
export function costTitle(b: CostBucket): string | undefined {
  const t = costTotal(b)
  if (t) return t.approx ? 'Approximate: part of this cost is estimated' : undefined
  if (!costParts(b).returned.length && !b.estimated_for_unavailable.length) return undefined
  const { primary, secondary } = costLines(b)
  return secondary ? `${primary}\n${secondary}` : primary
}

/** 141 §7.6: a USD figure a harness reported as a float (omp `progress[].cost`), in `formatCents`'s digits (a session total). */
export const usd = (n: number): string => formatCents({ currency: 'USD', nanos: String(Math.round(n * 1e9)) })
