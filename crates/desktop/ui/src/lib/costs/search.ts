// 160 D9: the /costs view lives in validated search params — range, granularity, group-by, metric, compare, filters —
// so back/forward and reload restore it. `resolveRange` turns a preset into instants in one IANA zone.

import { type Dim, DIMS, type Filters, GRANS, type Gran, MAX_BUCKETS, type Metric, type Range, type Zone, bucketCount, DAY_MS, HOUR_MS, zone } from './cube'

export const PRESETS = ['today', '24h', '7d', '30d', 'mtd', '90d', 'all'] as const
export type Preset = (typeof PRESETS)[number] | 'custom'
export const PRESET_LABEL: Record<(typeof PRESETS)[number], string> = { today: 'Today', '24h': '24H', '7d': '7D', '30d': '30D', mtd: 'MTD', '90d': '90D', all: 'All' }
export const METRICS: readonly Metric[] = ['usd', 'calls', 'tokens']

/** Every key optional: an absent key is its default (30D, auto granularity, by model, $, compare on, no filters). */
export type CostsSearch = {
  range?: Preset
  /** Custom range instants (ms), `custom` only. */
  from?: number
  to?: number
  gran?: Gran
  by?: Dim
  metric?: Metric
  /** `false` hides the previous-period line (default on). */
  compare?: boolean
  /** The window's `?sim=` flag, kept so navigation inside the page keeps the fixture. */
  sim?: string
} & Filters

const oneOf = <T extends string>(list: readonly T[], v: unknown): T | undefined => (list.includes(v as T) ? (v as T) : undefined)
const ms = (v: unknown): number | undefined => (typeof v === 'number' && Number.isFinite(v) && v >= 0 ? Math.round(v) : typeof v === 'string' && /^\d+$/.test(v) ? Number(v) : undefined)
const ids = (v: unknown): string[] | undefined => {
  const a = typeof v === 'string' ? [v] : Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : []
  return a.length ? [...new Set(a)] : undefined
}

/** `validateSearch` for `/costs`: unknown or malformed values are dropped, defaults are not written. */
export function validateCostsSearch(s: Record<string, unknown>): CostsSearch {
  const out: CostsSearch = {}
  const range = s.range === 'custom' ? 'custom' : oneOf(PRESETS, s.range)
  const from = ms(s.from)
  const to = ms(s.to)
  if (range === 'custom' && from !== undefined && to !== undefined && to > from) Object.assign(out, { range, from, to })
  else if (range && range !== 'custom' && range !== '30d') out.range = range
  const gran = oneOf(GRANS, s.gran)
  if (gran) out.gran = gran
  const by = oneOf(DIMS, s.by)
  if (by && by !== 'model') out.by = by
  const metric = oneOf(METRICS, s.metric)
  if (metric && metric !== 'usd') out.metric = metric
  if (s.compare === false || s.compare === 'false') out.compare = false
  for (const d of DIMS) {
    const v = ids(s[d])
    if (v) out[d] = v
  }
  if (typeof s.q === 'string' && s.q.trim()) out.q = s.q
  if (typeof s.sim === 'string') out.sim = s.sim
  return out
}

/** The filter part of a search. */
export const filtersOf = (s: CostsSearch): Filters => {
  const f: Filters = {}
  for (const d of DIMS) if (s[d]?.length) f[d] = s[d]
  if (s.q) f.q = s.q
  return f
}

const dayOf = (z: Zone, t: number): number => Math.floor(z.local(Math.floor(t / HOUR_MS)) / 24)

/**
 * The instants of a preset ending `now`: Today and MTD from local midnight, 24H the last 24 hours, nD the last n
 * local days including today, All from the ledger's first local day; custom as given.
 */
export function resolveRange(s: CostsSearch, now: number, tz: string, firstMs: number | null): Range {
  const z = zone(tz)
  const today = dayOf(z, now)
  if (s.range === 'custom' && s.from !== undefined && s.to !== undefined) return { from: s.from, to: s.to }
  switch (s.range ?? '30d') {
    case 'today':
      return { from: z.midnight(today), to: now }
    case '24h':
      return { from: now - DAY_MS, to: now }
    case '7d':
      return { from: z.midnight(today - 6), to: now }
    case 'mtd': {
      const d = new Date(today * DAY_MS)
      return { from: z.midnight(Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), 1) / DAY_MS), to: now }
    }
    case '90d':
      return { from: z.midnight(today - 89), to: now }
    case 'all':
      return { from: z.midnight(dayOf(z, Math.min(firstMs ?? now, now))), to: now }
    default:
      return { from: z.midnight(today - 29), to: now }
  }
}

/** §6.6 auto-pick: ≤ 2 days → hour, ≤ 31 days → day, ≤ 180 days → week, else month. */
export function autoGran(r: Range): Gran {
  const days = (r.to - r.from) / DAY_MS
  return days <= 2 ? 'hour' : days <= 31 ? 'day' : days <= 180 ? 'week' : 'month'
}

/** Why a granularity is disabled for a range, or `null` when it is allowed. */
export function granBlock(tz: string, r: Range, g: Gran): string | null {
  const n = bucketCount(zone(tz), r, g)
  if (n > MAX_BUCKETS) return `${n} buckets — more than ${MAX_BUCKETS}; pick a shorter range`
  if (n < 2 && g !== 'hour') return `one bucket — pick a longer range`
  return null
}

/** The granularity in effect: the URL's when allowed, else the auto pick. */
export const effectiveGran = (s: CostsSearch, tz: string, r: Range): Gran => (s.gran && !granBlock(tz, r, s.gran) ? s.gran : autoGran(r))
