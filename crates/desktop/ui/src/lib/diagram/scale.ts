// Design 123 §5.1 — linear, band and time scales with nice ticks (1-2-5; day / week / month). Pure, no d3.

export interface LinearScale {
  (v: number): number
  domain: [number, number]
  range: [number, number]
  invert(px: number): number
  ticks(count?: number): number[]
}

/** `[lo, hi]` widened to a 1-2-5 step covering `count` ticks; `lo === hi` opens a unit around the value. */
export function niceDomain(lo: number, hi: number, count = 5): [number, number] {
  if (lo > hi) [lo, hi] = [hi, lo]
  if (lo === hi) {
    lo -= 1
    hi += 1
  }
  const step = tickStep(lo, hi, count)
  return [Math.floor(lo / step) * step, Math.ceil(hi / step) * step]
}

/** The 1-2-5 step that yields about `count` ticks over `[lo, hi]`. */
export function tickStep(lo: number, hi: number, count = 5): number {
  const span = Math.abs(hi - lo) || 1
  const raw = span / Math.max(1, count)
  const pow = 10 ** Math.floor(Math.log10(raw))
  const unit = raw / pow
  const mult = unit >= Math.SQRT1_2 * 10 ? 10 : unit >= Math.sqrt(10) ? 5 : unit >= Math.SQRT2 ? 2 : 1
  return mult * pow
}

/** Tick values at the 1-2-5 step inside `[lo, hi]`, inclusive, snapped to clean decimals. */
export function ticks(lo: number, hi: number, count = 5): number[] {
  if (lo > hi) [lo, hi] = [hi, lo]
  const step = tickStep(lo, hi, count)
  const out: number[] = []
  const digits = Math.max(0, -Math.floor(Math.log10(step)))
  for (let v = Math.ceil(lo / step) * step; v <= hi + step / 1e6; v += step) out.push(Number(v.toFixed(digits)))
  return out
}

/** `domain` → `range`, linear; a zero-width domain maps everything to the range start. */
export function linear(domain: [number, number], range: [number, number]): LinearScale {
  const [d0, d1] = domain
  const [r0, r1] = range
  const k = d1 === d0 ? 0 : (r1 - r0) / (d1 - d0)
  const f = ((v: number) => r0 + (v - d0) * k) as LinearScale
  f.domain = domain
  f.range = range
  f.invert = (px) => (k === 0 ? d0 : d0 + (px - r0) / k)
  f.ticks = (count = 5) => ticks(d0, d1, count)
  return f
}

export interface BandScale {
  (key: string): number
  domain: string[]
  range: [number, number]
  /** Width of one band. */
  bandwidth: number
  /** Distance from one band start to the next. */
  step: number
}

/** Categories spread over `range`, `padding` (0..1) of a step left between bands and at both ends. */
export function band(domain: string[], range: [number, number], padding = 0.2): BandScale {
  const n = domain.length
  const [r0, r1] = range
  const step = n ? (r1 - r0) / (n - padding + 2 * padding) : 0
  const bandwidth = step * (1 - padding)
  const index: Record<string, number> = {}
  domain.forEach((k, i) => (index[k] = i))
  const f = ((key: string) => r0 + step * padding + (index[key] ?? 0) * step) as BandScale
  f.domain = domain
  f.range = range
  f.bandwidth = bandwidth
  f.step = step
  return f
}

export interface TimeScale {
  (t: Date | number): number
  domain: [Date, Date]
  range: [number, number]
  invert(px: number): Date
  /** Tick instants at the unit (`day` / `week` / `month`) that yields about `count` ticks. */
  ticks(count?: number): Date[]
  /** The unit `ticks()` picks. */
  unit(count?: number): TimeUnit
}

export type TimeUnit = 'hour' | 'day' | 'week' | 'month' | 'year'

const MS = { hour: 3_600_000, day: 86_400_000, week: 604_800_000, month: 30 * 86_400_000, year: 365 * 86_400_000 }
const UNITS: TimeUnit[] = ['hour', 'day', 'week', 'month', 'year']

/** The coarsest unit that still gives at least `count` ticks over `span` ms (falls back to `year`). */
export function timeUnit(spanMs: number, count = 5): TimeUnit {
  for (const u of UNITS) if (spanMs / MS[u] <= count * 2) return u
  return 'year'
}

/** Instants `[from, to]` → `range`, UTC ticks snapped to unit boundaries (Monday for weeks). */
export function time(domain: [Date, Date], range: [number, number]): TimeScale {
  const d0 = domain[0].getTime()
  const d1 = domain[1].getTime()
  const lin = linear([d0, d1], range)
  const f = ((t: Date | number) => lin(typeof t === 'number' ? t : t.getTime())) as TimeScale
  f.domain = domain
  f.range = range
  f.invert = (px) => new Date(lin.invert(px))
  f.unit = (count = 5) => timeUnit(Math.abs(d1 - d0), count)
  f.ticks = (count = 5) => {
    const unit = f.unit(count)
    const out: Date[] = []
    let cur = floorTo(new Date(Math.min(d0, d1)), unit)
    const end = Math.max(d0, d1)
    for (let guard = 0; cur.getTime() <= end && guard < 1000; guard++) {
      if (cur.getTime() >= Math.min(d0, d1)) out.push(cur)
      cur = step(cur, unit)
    }
    return out
  }
  return f
}

/** `t` rounded down to the start of its `unit` (UTC; weeks start Monday). */
export function floorTo(t: Date, unit: TimeUnit): Date {
  const d = new Date(t.getTime())
  d.setUTCMilliseconds(0)
  d.setUTCSeconds(0)
  d.setUTCMinutes(0)
  if (unit === 'hour') return d
  d.setUTCHours(0)
  if (unit === 'day') return d
  if (unit === 'week') {
    d.setUTCDate(d.getUTCDate() - ((d.getUTCDay() + 6) % 7))
    return d
  }
  d.setUTCDate(1)
  if (unit === 'month') return d
  d.setUTCMonth(0)
  return d
}

function step(t: Date, unit: TimeUnit): Date {
  const d = new Date(t.getTime())
  if (unit === 'hour') d.setUTCHours(d.getUTCHours() + 1)
  else if (unit === 'day') d.setUTCDate(d.getUTCDate() + 1)
  else if (unit === 'week') d.setUTCDate(d.getUTCDate() + 7)
  else if (unit === 'month') d.setUTCMonth(d.getUTCMonth() + 1)
  else d.setUTCFullYear(d.getUTCFullYear() + 1)
  return d
}

/** A tick label for `unit`: `09:00`, `Oct 7`, `Oct 2026`, `2026`. */
export function formatTick(t: Date, unit: TimeUnit): string {
  const mon = t.toLocaleString('en', { month: 'short', timeZone: 'UTC' })
  if (unit === 'hour') return `${String(t.getUTCHours()).padStart(2, '0')}:${String(t.getUTCMinutes()).padStart(2, '0')}`
  if (unit === 'day' || unit === 'week') return `${mon} ${t.getUTCDate()}`
  if (unit === 'month') return `${mon} ${t.getUTCFullYear()}`
  return String(t.getUTCFullYear())
}

/** `crates/surface::validate::iso_datetime` in TS: an ISO date or RFC 3339 datetime as a UTC `Date`, else `null`. */
export function parseIso(s: string): Date | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})(?:[Tt ](\d{2}):(\d{2})(?::(\d{2})(?:\.(\d+))?)?(Z|z|[+-]\d{2}:\d{2})?)?$/.exec(s)
  if (!m) return null
  const [, y, mo, d, h = '0', mi = '0', se = '0', frac = '0', tz] = m
  const t = Date.UTC(+y!, +mo! - 1, +d!, +h, +mi, +se, Math.round(Number(`0.${frac}`) * 1000))
  const date = new Date(t)
  if (date.getUTCMonth() !== +mo! - 1 || date.getUTCDate() !== +d! || +h > 23 || +mi > 59 || +se > 59) return null
  if (tz && tz !== 'Z' && tz !== 'z') {
    const sign = tz.startsWith('-') ? -1 : 1
    const [oh, om] = tz.slice(1).split(':').map(Number)
    return new Date(t - sign * ((oh ?? 0) * 60 + (om ?? 0)) * 60_000)
  }
  return date
}
