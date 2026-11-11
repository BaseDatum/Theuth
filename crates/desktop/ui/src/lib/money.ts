import type { CostBucket } from '@/rpc/bindings/CostBucket'
import type { Money } from '@/rpc/bindings/Money'

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

/**
 * `formatMoney` split for a rolling display (`CostPill`): the symbol (or `XYZ ` code) as text and the half-up-rounded
 * milli-units as a number, exact at three decimals (`0.123`), so `Intl` prints the same digits `formatMoney` does.
 */
export function moneyParts(m: Money): { prefix: string; value: number } {
  const milli = (BigInt(m.nanos) + NANOS_PER_MILLI / 2n) / NANOS_PER_MILLI
  const s = symbol(m.currency)
  return { prefix: s ?? `${m.currency} `, value: Number(milli) / 1000 }
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

/** `costLines` as one tooltip: the returned line, then the estimate line. */
export function costTitle(b: CostBucket): string {
  const { primary, secondary } = costLines(b)
  return secondary ? `${primary}\n${secondary}` : primary
}

/**
 * The terms a one-line cost shows: returned money and the estimate side by side. A known-zero returned entry (an omp
 * child's meter before it reports) is dropped when an estimate or unavailable calls exist, so it never hides them.
 */
export function costParts(b: CostBucket): { returned: Money[]; estimated: Money[] } {
  const estimated = b.estimated_for_unavailable
  const hideZero = estimated.length > 0 || b.unavailable > 0
  return { returned: hideZero ? b.returned.filter((m) => BigInt(m.nanos) !== 0n) : b.returned, estimated }
}

/** One bucket for a pill or agent row: `$0.100 + ~$0.023` (returned, then the tilde-marked estimate), either alone, else `—`. */
export function costShort(b: CostBucket): string {
  const { returned, estimated } = costParts(b)
  const terms = [...returned.map(formatMoney), ...estimated.map((m) => `~${formatMoney(m)}`)]
  return terms.length ? terms.join(' + ') : '—'
}

/** 141 §7.6: a USD figure a harness reported as a float (omp `progress[].cost`), in `formatMoney`'s digits. */
export const usd = (n: number): string => formatMoney({ currency: 'USD', nanos: String(Math.round(n * 1e9)) })
