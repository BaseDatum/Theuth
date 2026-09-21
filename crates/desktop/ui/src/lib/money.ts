import type { CostBucket } from '@/rpc/bindings/CostBucket'
import type { Money } from '@/rpc/bindings/Money'

const NANOS_PER_MILLI = 1_000_000n

/** `USD 0.123`: integer nanos (decimal string) → units with 3 decimals, rounded half up; never a float. */
export function formatMoney(m: Money): string {
  const milli = (BigInt(m.nanos) + NANOS_PER_MILLI / 2n) / NANOS_PER_MILLI
  const units = milli / 1000n
  const frac = (milli % 1000n).toString().padStart(3, '0')
  return `${m.currency} ${units}.${frac}`
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

/** One bucket for a pill or agent row: returned money, else the estimate as `~USD 0.123`, else `—`. */
export function costShort(b: CostBucket): string {
  if (b.returned.length) return b.returned.map(formatMoney).join(' + ')
  if (b.estimated_for_unavailable.length) return b.estimated_for_unavailable.map((m) => `~${formatMoney(m)}`).join(' + ')
  return '—'
}
