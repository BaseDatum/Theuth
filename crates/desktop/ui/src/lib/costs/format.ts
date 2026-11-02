// 160: number formatting for the costs page — money with a precision that fits its size, compact token counts, ages.

const USD2 = new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD', minimumFractionDigits: 2, maximumFractionDigits: 2 })
const INT = new Intl.NumberFormat('en-US', { maximumFractionDigits: 0 })

/** `$14,179.85`; sub-cent amounts keep two significant digits (`$0.0042`), zero is `$0.00`. */
export function usd(v: number): string {
  const a = Math.abs(v)
  if (a > 0 && a < 0.01) return `${v < 0 ? '-' : ''}$${a.toPrecision(2)}`
  return USD2.format(v)
}

/** Axis / label money: `$1.35k`, `$12.4k`, `$2.10M`, `$740`, `$3.20`. */
export function usdShort(v: number): string {
  const a = Math.abs(v)
  if (a >= 1e6) return `$${(v / 1e6).toFixed(2)}M`
  if (a >= 1e4) return `$${(v / 1e3).toFixed(1)}k`
  if (a >= 1e3) return `$${(v / 1e3).toFixed(2)}k`
  if (a >= 100) return `$${v.toFixed(0)}`
  if (a > 0 && a < 0.01) return usd(v)
  return `$${v.toFixed(2)}`
}

/** `25.2B`, `146.9M`, `12.0k`, `742`. */
export function compact(n: number): string {
  const a = Math.abs(n)
  if (a >= 1e9) return `${(n / 1e9).toFixed(2)}B`
  if (a >= 1e6) return `${(n / 1e6).toFixed(1)}M`
  if (a >= 1e3) return `${(n / 1e3).toFixed(1)}k`
  return INT.format(n)
}

export const int = (n: number): string => INT.format(n)

/** `12 %` style share with one decimal under 10 %. */
export const pct = (f: number): string => `${f < 0.1 ? (f * 100).toFixed(1) : Math.round(f * 100)}%`

/** `9 s ago`, `4 min ago`, `3 h ago`, `2 d ago`; `just now` under 5 s; future instants read `in …`. */
export function ago(ms: number, now: number): string {
  const s = Math.round((now - ms) / 1000)
  const a = Math.abs(s)
  const t = a < 60 ? `${a} s` : a < 3600 ? `${Math.round(a / 60)} min` : a < 86400 ? `${Math.round(a / 3600)} h` : `${Math.round(a / 86400)} d`
  if (a < 5) return 'just now'
  return s >= 0 ? `${t} ago` : `in ${t}`
}

/** `16:40` today, `Thu 09:00` within a week, `12 Nov` later. Local time. */
export function when(ms: number, now: number): string {
  const d = new Date(ms)
  const hm = `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
  if (new Date(now).toDateString() === d.toDateString()) return hm
  if (Math.abs(ms - now) < 6 * 86_400_000) return `${d.toLocaleDateString('en-GB', { weekday: 'short' })} ${hm}`
  return d.toLocaleDateString('en-GB', { day: 'numeric', month: 'short' })
}

/** A limit reset in one short line: `in 48m`, `in 1h 48m` within a day, else `Thu 19:18` / `12 Nov`; `now` once passed. */
export function resetIn(ms: number, now: number): string {
  const m = Math.round((ms - now) / 60_000)
  if (m <= 0) return 'now'
  if (m < 60) return `in ${m}m`
  if (m < 1440) return `in ${Math.floor(m / 60)}h ${m % 60}m`
  return when(ms, now)
}
