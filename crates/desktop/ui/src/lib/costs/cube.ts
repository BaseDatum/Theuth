// 160 D3: the whole ledger as an hourly cube, held and sliced in the client. Pure (no React): `merge` folds a
// `cost_cube` answer into the held cube, `index` turns it into typed columns with per-cell local time keys for one
// IANA zone, and every query below is one pass over those columns.

import type { CostBasis } from '@/rpc/bindings/CostBasis'
import type { CostCells } from '@/rpc/bindings/CostCells'
import type { CostCube } from '@/rpc/bindings/CostCube'
import type { CostRoot } from '@/rpc/bindings/CostRoot'
import type { SpendOwnerKind } from '@/rpc/bindings/SpendOwnerKind'

export const HOUR_MS = 3_600_000
export const DAY_MS = 86_400_000
/** §6.6 hard cap: no chart draws more buckets than this. */
export const MAX_BUCKETS = 400
/** 160 R2: Hour is offered for ranges up to 7 local days (≤ 168 buckets). */
export const MAX_HOUR_DAYS = 7

export const BASES: readonly CostBasis[] = ['known', 'reported', 'estimated', 'unpriced']
export const BASIS_LABEL: Record<CostBasis, string> = { known: 'Known', reported: 'Reported', estimated: 'Estimated', unpriced: 'Unpriced' }
/** D2: known = malachite, reported = lapis, estimated = sand, unpriced = slate. */
export const BASIS_COLOR: Record<CostBasis, string> = { known: 'var(--chart-4)', reported: 'var(--chart-3)', estimated: 'var(--chart-2)', unpriced: 'var(--chart-6)' }

export const DIMS = ['model', 'provider', 'owner', 'kind', 'via', 'basis', 'project'] as const
export type Dim = (typeof DIMS)[number]
export type Metric = 'usd' | 'calls' | 'tokens'
export type Gran = 'hour' | 'day' | 'week' | 'month'
export const GRANS: readonly Gran[] = ['hour', 'day', 'week', 'month']

/** Selected ids per dimension (absent or empty = all) and a session title search. */
export type Filters = Partial<Record<Dim, string[]>> & { q?: string }

const EMPTY_CELLS: CostCells = { hour: [], root: [], model: [], provider: [], kind: [], via: [], basis: [], nanos: [], calls: [], failed: [], repriced: [], input: [], output: [], cache_read: [], cache_write: [], cache_saved_nanos: [], approx_nanos: [] }
const COLS = Object.keys(EMPTY_CELLS) as (keyof CostCells)[]

// ---------------------------------------------------------------------------------------------------------------- merge

/**
 * Fold an answer into the held cube. A full answer replaces it; a delta of the same epoch replaces every cell of its
 * `hours`, upserts `roots` by `ix` and adopts the (complete) dictionaries and meta. `null` = the delta cannot apply
 * (nothing held, or another epoch): fetch a full snapshot.
 */
export function merge(held: CostCube | undefined, next: CostCube): CostCube | null {
  if (next.full) return next
  if (!held || held.epoch !== next.epoch) return null
  if (next.rev <= held.rev) return held
  const gone = new Set(next.hours)
  const keep: number[] = []
  for (let i = 0; i < held.cells.hour.length; i++) if (!gone.has(held.cells.hour[i]!)) keep.push(i)
  const cells = {} as CostCells
  for (const c of COLS) {
    const a = held.cells[c]
    const out = new Array<number>(keep.length + next.cells[c].length)
    for (let j = 0; j < keep.length; j++) out[j] = a[keep[j]!]!
    for (let j = 0; j < next.cells[c].length; j++) out[keep.length + j] = next.cells[c][j]!
    cells[c] = out
  }
  const roots = new Map(held.roots.map((r) => [r.ix, r]))
  for (const r of next.roots) roots.set(r.ix, r)
  return { ...next, full: true, hours: [], roots: [...roots.values()].sort((a, b) => a.ix - b.ix), cells }
}

/** An empty cube (no engine answer yet, or a genuinely empty ledger). */
export const emptyCube = (): CostCube => ({
  epoch: '',
  rev: 0,
  full: true,
  hours: [],
  providers: [],
  kinds: [],
  vias: [],
  models: [],
  roots: [],
  cells: EMPTY_CELLS,
  meta: { engine: '', first_call_ms: null, built_at_ms: 0, pre_ledger_sessions: 0, unpriced: [], price_table: '' },
})

// ----------------------------------------------------------------------------------------------------------------- time

/**
 * Local time for one IANA zone at hour precision (D4). A civil day is a day number (days since 1970-01-01 of the local
 * date), so day / ISO week / month keys are plain integers whatever the DST offset. `local` is cached per UTC hour.
 */
export type Zone = {
  tz: string
  /** The local civil day number and hour (0–23) of UTC hour `h`, packed `day * 24 + hour`. */
  local(h: number): number
  /** The instant (ms) of local midnight starting civil day `day`. */
  midnight(day: number): number
}

const zones = new Map<string, Zone>()

export function zone(tz: string): Zone {
  let z = zones.get(tz)
  if (z) return z
  const fmt = new Intl.DateTimeFormat('en-US', { timeZone: tz, hourCycle: 'h23', year: 'numeric', month: 'numeric', day: 'numeric', hour: 'numeric', minute: 'numeric' })
  /** The local wall time of `t` read as if it were UTC. */
  const wall = (t: number): number => {
    const p: Record<string, number> = {}
    for (const { type, value } of fmt.formatToParts(t)) if (type !== 'literal') p[type] = Number(value)
    return Date.UTC(p.year!, p.month! - 1, p.day!, p.hour! % 24, p.minute!)
  }
  const cache = new Map<number, number>()
  z = {
    tz,
    local(h) {
      let v = cache.get(h)
      if (v === undefined) {
        const w = wall(h * HOUR_MS)
        v = Math.floor(w / DAY_MS) * 24 + Math.floor((w % DAY_MS) / HOUR_MS)
        cache.set(h, v)
      }
      return v
    },
    midnight(day) {
      const t = day * DAY_MS
      const first = t - (wall(t) - t)
      return t - (wall(first) - first)
    },
  }
  zones.set(tz, z)
  return z
}

export const defaultTz = (): string => Intl.DateTimeFormat().resolvedOptions().timeZone
/** Monday = 0 … Sunday = 6 of a civil day number (1970-01-01 was a Thursday). */
export const weekday = (day: number): number => (((day + 3) % 7) + 7) % 7
export const monday = (day: number): number => day - weekday(day)
const civil = (day: number): Date => new Date(day * DAY_MS)
const monthOf = (day: number): number => {
  const d = civil(day)
  return d.getUTCFullYear() * 12 + d.getUTCMonth()
}
const monthStartDay = (month: number): number => Date.UTC(Math.floor(month / 12), month % 12, 1) / DAY_MS

// ---------------------------------------------------------------------------------------------------------------- index

/** One dimension's dictionary: stable ids (URL filter values) and display labels, indexed by the cell column value. */
export type Dict = { ids: string[]; labels: string[]; at: Map<string, number> }

/** The cube as typed columns: measures, dimension indices (model by canonical name, owner/project via the root) and local time keys. */
export type Index = {
  cube: CostCube
  tz: string
  zone: Zone
  n: number
  hour: Uint32Array
  root: Uint32Array
  col: Record<Dim, Uint32Array>
  nanos: Float64Array
  calls: Float64Array
  failed: Float64Array
  repriced: Float64Array
  input: Float64Array
  output: Float64Array
  cacheRead: Float64Array
  cacheWrite: Float64Array
  cacheSaved: Float64Array
  approx: Float64Array
  /** `input + output` per cell (the Tokens metric). */
  tokens: Float64Array
  /** Local civil day number, its ISO week's Monday (a day number), month number (`year * 12 + month`) and local hour of each cell. */
  day: Int32Array
  week: Int32Array
  month: Int32Array
  lhour: Uint8Array
  dict: Record<Dim, Dict>
  /** `roots[ix]`; `undefined` for an index the cube never sent. */
  roots: (CostRoot | undefined)[]
  /** Per root ix: owner and project dictionary indices. */
  rootOwner: Uint32Array
  rootProject: Uint32Array
  /** Per owner dictionary index: the owner kind (agent / user / routine / other). */
  ownerKind: SpendOwnerKind[]
  minHour: number
  maxHour: number
}

function dict(): Dict {
  return { ids: [], labels: [], at: new Map() }
}
function put(d: Dict, id: string, label: string): number {
  let i = d.at.get(id)
  if (i === undefined) {
    i = d.ids.length
    d.ids.push(id)
    d.labels.push(label)
    d.at.set(id, i)
  }
  return i
}

const f64 = (a: readonly number[]) => Float64Array.from(a)
const u32 = (a: readonly number[]) => Uint32Array.from(a)

/** Build the typed index of `cube` for zone `tz` (default: this browser's). */
export function index(cube: CostCube, tz: string = defaultTz()): Index {
  const z = zone(tz)
  const c = cube.cells
  const n = c.hour.length
  const d = { model: dict(), provider: dict(), owner: dict(), kind: dict(), via: dict(), basis: dict(), project: dict() } satisfies Record<Dim, Dict>
  // Small dictionaries: cube index → our index (models collapse onto their canonical name).
  const modelOf = u32(cube.models.map((m) => put(d.model, m.name, m.name)))
  for (const p of cube.providers) put(d.provider, p, p)
  for (const k of cube.kinds) put(d.kind, k, k)
  for (const v of cube.vias) put(d.via, v, v)
  for (const b of BASES) put(d.basis, b, BASIS_LABEL[b])
  const roots: (CostRoot | undefined)[] = []
  for (const r of cube.roots) roots[r.ix] = r.root
  let maxRoot = roots.length
  for (let i = 0; i < n; i++) if (c.root[i]! >= maxRoot) maxRoot = c.root[i]! + 1
  const rootOwner = new Uint32Array(maxRoot)
  const rootProject = new Uint32Array(maxRoot)
  const ownerKind: SpendOwnerKind[] = []
  for (let r = 0; r < maxRoot; r++) {
    const root = roots[r]
    const kind: SpendOwnerKind = root?.owner_kind ?? 'other'
    const name = root?.owner ?? 'other'
    const o = put(d.owner, `${kind}:${name}`, name)
    ownerKind[o] = kind
    rootOwner[r] = o
    rootProject[r] = put(d.project, root?.project ?? '', root?.project ?? 'no project')
  }
  const hour = u32(c.hour)
  const root = u32(c.root)
  const model = new Uint32Array(n)
  const owner = new Uint32Array(n)
  const project = new Uint32Array(n)
  const day = new Int32Array(n)
  const week = new Int32Array(n)
  const month = new Int32Array(n)
  const tokens = new Float64Array(n)
  const lhour = new Uint8Array(n)
  const months = new Map<number, number>()
  let minHour = Infinity
  let maxHour = -Infinity
  for (let i = 0; i < n; i++) {
    model[i] = modelOf[c.model[i]!] ?? put(d.model, `#${c.model[i]}`, `model ${c.model[i]}`)
    owner[i] = rootOwner[root[i]!]!
    project[i] = rootProject[root[i]!]!
    const h = hour[i]!
    const l = z.local(h)
    const dd = Math.floor(l / 24)
    day[i] = dd
    week[i] = monday(dd)
    tokens[i] = c.input[i]! + c.output[i]!
    let m = months.get(dd)
    if (m === undefined) months.set(dd, (m = monthOf(dd)))
    month[i] = m
    lhour[i] = l % 24
    if (h < minHour) minHour = h
    if (h > maxHour) maxHour = h
  }
  return {
    cube,
    tz,
    zone: z,
    n,
    hour,
    root,
    col: { model, provider: u32(c.provider), owner, kind: u32(c.kind), via: u32(c.via), basis: u32(c.basis), project },
    nanos: f64(c.nanos),
    calls: f64(c.calls),
    failed: f64(c.failed),
    repriced: f64(c.repriced),
    input: f64(c.input),
    output: f64(c.output),
    cacheRead: f64(c.cache_read),
    cacheWrite: f64(c.cache_write),
    cacheSaved: f64(c.cache_saved_nanos),
    approx: f64(c.approx_nanos),
    tokens,
    day,
    week,
    month,
    lhour,
    dict: d,
    roots,
    rootOwner,
    rootProject,
    ownerKind,
    minHour: n ? minHour : 0,
    maxHour: n ? maxHour : 0,
  }
}

// -------------------------------------------------------------------------------------------------------------- filters

type Check = { col: Uint32Array; mask: Uint8Array; dim: Dim | 'q' }

/** Active filters as (column, allowed-mask) pairs; the title search is a mask over roots. */
function compile(ix: Index, f: Filters): Check[] {
  const out: Check[] = []
  for (const dim of DIMS) {
    const ids = f[dim]
    if (!ids?.length) continue
    const mask = new Uint8Array(ix.dict[dim].ids.length)
    for (const id of ids) {
      const k = ix.dict[dim].at.get(id)
      if (k !== undefined) mask[k] = 1
    }
    out.push({ col: ix.col[dim], mask, dim })
  }
  const q = f.q?.trim().toLowerCase()
  if (q) {
    const mask = new Uint8Array(ix.rootOwner.length)
    for (let r = 0; r < mask.length; r++) {
      const root = ix.roots[r]
      if (root && (sessionLabel(root).toLowerCase().includes(q) || root.id.startsWith(q))) mask[r] = 1
    }
    out.push({ col: ix.root, mask, dim: 'q' })
  }
  return out
}

const passes = (checks: Check[], i: number): boolean => {
  for (let k = 0; k < checks.length; k++) if (!checks[k]!.mask[checks[k]!.col[i]!]) return false
  return true
}

/** "Deleted session · 3 Oct" for a deleted root (H1: its title is not kept), else the title (or "Untitled"). */
export function sessionLabel(root: CostRoot): string {
  if (root.deleted) return `Deleted session · ${shortDate(root.created_at_ms)}`
  return root.title || 'Untitled'
}

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec']
const MONTHS_LONG = ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December']
export const DOW = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun']
const shortDate = (ms: number): string => {
  const d = new Date(ms)
  return `${d.getDate()} ${MONTHS[d.getMonth()]}`
}

// ---------------------------------------------------------------------------------------------------------------- range

/** A half-open instant range `[from, to)`; cells count when their UTC hour starts inside `[floor(from), ceil(to))` hours. */
export type Range = { from: number; to: number }
const hours = (r: Range): [number, number] => [Math.floor(r.from / HOUR_MS), Math.ceil(r.to / HOUR_MS)]

// -------------------------------------------------------------------------------------------------------------- buckets

export type Bucket = { key: number; start: number; end: number; label: string; tip: string }

/** The bucket axis of a range at a granularity: every bucket, empty or not, plus the per-cell bucket offset. */
type Axis = {
  buckets: Bucket[]
  /** Bucket offset of cell `i` relative to the first bucket (negative = before the range). */
  at(i: number): number
  /** First UTC hour of bucket offset `k` (any integer). */
  hourAt(k: number): number
}

const pad2 = (n: number) => String(n).padStart(2, '0')

function axis(ix: Index, r: Range, gran: Gran): Axis {
  const z = ix.zone
  const [h0] = hours(r)
  const d0 = Math.floor(z.local(h0) / 24)
  const label = (k: number): { start: number; label: string; tip: string } => {
    if (gran === 'hour') {
      const l = z.local(h0 + k)
      const dd = civil(Math.floor(l / 24))
      const hh = `${pad2(l % 24)}:00`
      return { start: (h0 + k) * HOUR_MS, label: hh, tip: `${DOW[weekday(Math.floor(l / 24))]} ${dd.getUTCDate()} ${MONTHS[dd.getUTCMonth()]} · ${hh}` }
    }
    if (gran === 'month') {
      const m = monthOf(d0) + k
      return { start: z.midnight(monthStartDay(m)), label: `${MONTHS[m % 12]} ${Math.floor(m / 12)}`, tip: `${MONTHS_LONG[m % 12]} ${Math.floor(m / 12)}` }
    }
    const day = gran === 'day' ? d0 + k : monday(d0) + 7 * k
    const dd = civil(day)
    const s = `${dd.getUTCDate()} ${MONTHS[dd.getUTCMonth()]}`
    return { start: z.midnight(day), label: s, tip: gran === 'day' ? `${DOW[weekday(day)]} ${s} ${dd.getUTCFullYear()}` : `Week of ${s} ${dd.getUTCFullYear()}` }
  }
  const count = bucketCount(ix.zone, r, gran)
  const buckets: Bucket[] = []
  for (let k = 0; k < Math.min(count, MAX_BUCKETS); k++) {
    const b = label(k)
    buckets.push({ key: k, start: b.start, end: 0, label: b.label, tip: b.tip })
  }
  for (let k = 0; k < buckets.length; k++) buckets[k]!.end = k + 1 < buckets.length ? buckets[k + 1]!.start : Math.min(label(k + 1).start, r.to)
  const m0 = monthOf(d0)
  const w0 = monday(d0)
  return {
    buckets,
    at:
      gran === 'hour'
        ? (i) => ix.hour[i]! - h0
        : gran === 'day'
          ? (i) => ix.day[i]! - d0
          : gran === 'week'
            ? (i) => (ix.week[i]! - w0) / 7
            : (i) => ix.month[i]! - m0,
    hourAt: (k) => Math.floor(label(k).start / HOUR_MS),
  }
}

/** How many buckets `r` spans at `gran` (for the ≤ 400 guard). */
export function bucketCount(z: Zone, r: Range, gran: Gran): number {
  const [h0, h1] = hours(r)
  if (h1 <= h0) return 0
  if (gran === 'hour') return h1 - h0
  const d0 = Math.floor(z.local(h0) / 24)
  const d1 = Math.floor(z.local(h1 - 1) / 24)
  if (gran === 'day') return d1 - d0 + 1
  if (gran === 'week') return (monday(d1) - monday(d0)) / 7 + 1
  return monthOf(d1) - monthOf(d0) + 1
}

// --------------------------------------------------------------------------------------------------------------- series

export type Series = { key: string; id: string | null; label: string; color: string }
export type SeriesRow = { label: string; tip: string; start: number; end: number; total: number; prev: number } & Record<string, number | string>
export type SeriesOut = { buckets: Bucket[]; series: Series[]; rows: SeriesRow[]; total: number; prevTotal: number }
export type SeriesQuery = Range & { gran: Gran; groupBy: Dim; metric: Metric; filters: Filters; topN?: number }

/** Rank colours: the minerals minus slate, which "other" keeps. */
const RANK = ['var(--chart-1)', 'var(--chart-2)', 'var(--chart-3)', 'var(--chart-4)', 'var(--chart-5)', 'var(--chart-7)', 'var(--chart-8)']
export const OTHER_COLOR = 'var(--chart-6)'

/**
 * Stacked series over the range: one row per bucket with `s0…` per group (top `topN`, default 6, by range total + `other`),
 * `total`, and `prev` = the same metric over the equal number of buckets just before (the previous-period line).
 */
export function series(ix: Index, q: SeriesQuery): SeriesOut {
  const ax = axis(ix, q, q.gran)
  const B = ax.buckets.length
  const [h0, h1] = hours(q)
  const p0 = ax.hourAt(-B)
  const checks = compile(ix, q.filters)
  const g = ix.col[q.groupBy]
  const K = ix.dict[q.groupBy].ids.length
  const acc = new Float64Array(B * K)
  const byGroup = new Float64Array(K)
  const prev = new Float64Array(B)
  // The metric's column and divisor, chosen once (not per cell).
  const col = q.metric === 'usd' ? ix.nanos : q.metric === 'calls' ? ix.calls : ix.tokens
  const div = q.metric === 'usd' ? 1e9 : 1
  for (let i = 0; i < ix.n; i++) {
    const h = ix.hour[i]!
    if (h < p0 || h >= h1 || !passes(checks, i)) continue
    const k = ax.at(i)
    const v = col[i]! / div
    if (h >= h0) {
      if (k < 0 || k >= B) continue
      acc[k * K + g[i]!]! += v
      byGroup[g[i]!]! += v
    } else if (k < 0 && k >= -B) prev[k + B]! += v
  }
  const order = [...byGroup.keys()].filter((v) => byGroup[v]! > 0).sort((a, b) => byGroup[b]! - byGroup[a]!)
  const topN = q.topN ?? 6
  const top = order.slice(0, topN)
  const rest = order.length > top.length
  const d = ix.dict[q.groupBy]
  const series: Series[] = top.map((v, n) => ({ key: `s${n}`, id: d.ids[v]!, label: d.labels[v]!, color: q.groupBy === 'basis' ? BASIS_COLOR[d.ids[v] as CostBasis] : RANK[n % RANK.length]! }))
  if (rest) series.push({ key: 'other', id: null, label: 'other', color: OTHER_COLOR })
  const slot = new Int32Array(K).fill(rest ? top.length : -1)
  top.forEach((v, n) => (slot[v] = n))
  let total = 0
  let prevTotal = 0
  const rows = ax.buckets.map((b, k) => {
    const row: SeriesRow = { label: b.label, tip: b.tip, start: b.start, end: b.end, total: 0, prev: prev[k]! }
    for (const s of series) row[s.key] = 0
    for (let v = 0; v < K; v++) {
      const x = acc[k * K + v]!
      if (!x || slot[v]! < 0) continue
      const key = series[slot[v]!]!.key
      row[key] = (row[key] as number) + x
      row.total += x
    }
    total += row.total
    prevTotal += row.prev
    return row
  })
  return { buckets: ax.buckets, series, rows, total, prevTotal }
}

// --------------------------------------------------------------------------------------------------------------- totals

export type Totals = {
  usd: number
  calls: number
  failed: number
  repriced: number
  input: number
  output: number
  cacheRead: number
  cacheWrite: number
  cacheSavedUsd: number
  approxUsd: number
  /** USD and calls per basis, in `BASES` order. */
  usdBy: [number, number, number, number]
  callsBy: [number, number, number, number]
}

/** Every measure summed over the range and filters. */
export function totals(ix: Index, r: Range, filters: Filters): Totals {
  const [h0, h1] = hours(r)
  const checks = compile(ix, filters)
  const t: Totals = { usd: 0, calls: 0, failed: 0, repriced: 0, input: 0, output: 0, cacheRead: 0, cacheWrite: 0, cacheSavedUsd: 0, approxUsd: 0, usdBy: [0, 0, 0, 0], callsBy: [0, 0, 0, 0] }
  const b = ix.col.basis
  for (let i = 0; i < ix.n; i++) {
    const h = ix.hour[i]!
    if (h < h0 || h >= h1 || !passes(checks, i)) continue
    const usd = ix.nanos[i]! / 1e9
    t.usd += usd
    t.calls += ix.calls[i]!
    t.failed += ix.failed[i]!
    t.repriced += ix.repriced[i]!
    t.input += ix.input[i]!
    t.output += ix.output[i]!
    t.cacheRead += ix.cacheRead[i]!
    t.cacheWrite += ix.cacheWrite[i]!
    t.cacheSavedUsd += ix.cacheSaved[i]! / 1e9
    t.approxUsd += ix.approx[i]! / 1e9
    t.usdBy[b[i]! as 0]! += usd
    t.callsBy[b[i]! as 0]! += ix.calls[i]!
  }
  return t
}

/** The equal-length range just before `r`. */
export const previousPeriod = (r: Range): Range => {
  const [h0, h1] = hours(r)
  return { from: (2 * h0 - h1) * HOUR_MS, to: h0 * HOUR_MS }
}

// ------------------------------------------------------------------------------------------------------------ breakdown

export type BreakdownRow = { id: string; label: string; usd: number; calls: number; tokens: number; failed: number; kind?: SpendOwnerKind }

/** One row per value of `dim` with spend in the range, by USD then calls. */
export function breakdown(ix: Index, r: Range, filters: Filters, dim: Dim): BreakdownRow[] {
  const [h0, h1] = hours(r)
  const checks = compile(ix, filters)
  const g = ix.col[dim]
  const K = ix.dict[dim].ids.length
  const usd = new Float64Array(K)
  const calls = new Float64Array(K)
  const tokens = new Float64Array(K)
  const failed = new Float64Array(K)
  for (let i = 0; i < ix.n; i++) {
    const h = ix.hour[i]!
    if (h < h0 || h >= h1 || !passes(checks, i)) continue
    const v = g[i]!
    usd[v]! += ix.nanos[i]! / 1e9
    calls[v]! += ix.calls[i]!
    tokens[v]! += ix.input[i]! + ix.output[i]!
    failed[v]! += ix.failed[i]!
  }
  const d = ix.dict[dim]
  const rows: BreakdownRow[] = []
  for (let v = 0; v < K; v++) {
    if (!calls[v] && !usd[v]) continue
    rows.push({ id: d.ids[v]!, label: d.labels[v]!, usd: usd[v]!, calls: calls[v]!, tokens: tokens[v]!, failed: failed[v]!, ...(dim === 'owner' && { kind: ix.ownerKind[v] }) })
  }
  return rows.sort((a, b) => b.usd - a.usd || b.calls - a.calls)
}

export type FilterOption = { id: string; label: string; usd: number; calls: number; kind?: SpendOwnerKind }

/**
 * The filter chips' option lists: per dimension, every value with calls in the range under every OTHER active filter
 * (facets), by USD; a selected value with nothing in range stays listed. One pass.
 */
export function options(ix: Index, r: Range, filters: Filters): Record<Dim, FilterOption[]> {
  const [h0, h1] = hours(r)
  const checks = compile(ix, filters)
  const q = checks.filter((c) => c.dim === 'q')
  const cols = DIMS.map((d) => ix.col[d])
  const masks = DIMS.map((d) => checks.find((c) => c.dim === d)?.mask ?? null)
  const U = DIMS.map((d) => new Float64Array(ix.dict[d].ids.length))
  const C = DIMS.map((d) => new Float64Array(ix.dict[d].ids.length))
  const D = DIMS.length
  for (let i = 0; i < ix.n; i++) {
    const h = ix.hour[i]!
    if (h < h0 || h >= h1 || !passes(q, i)) continue
    let failing = -1
    let fails = 0
    for (let k = 0; k < D; k++) {
      const m = masks[k]
      if (m && !m[cols[k]![i]!] && fails++ === 0) failing = k
    }
    if (fails > 1) continue
    const usd = ix.nanos[i]! / 1e9
    const calls = ix.calls[i]!
    for (let k = fails ? failing : 0; k < (fails ? failing + 1 : D); k++) {
      const v = cols[k]![i]!
      U[k]![v]! += usd
      C[k]![v]! += calls
    }
  }
  const out = {} as Record<Dim, FilterOption[]>
  DIMS.forEach((d, k) => {
    const sel = new Set(filters[d] ?? [])
    const rows: FilterOption[] = []
    const dd = ix.dict[d]
    const u = U[k]!
    const c = C[k]!
    for (let v = 0; v < dd.ids.length; v++) {
      if (!c[v] && !u[v] && !sel.has(dd.ids[v]!)) continue
      rows.push({ id: dd.ids[v]!, label: dd.labels[v]!, usd: u[v]!, calls: c[v]!, ...(d === 'owner' && { kind: ix.ownerKind[v] }) })
    }
    out[d] = d === 'basis' ? rows : rows.sort((a, b) => b.usd - a.usd || b.calls - a.calls)
  })
  return out
}

// ----------------------------------------------------------------------------------------------------------------- heat

export type HeatCell = { wd: number; h: number; usd: number; calls: number }

/** The hour-of-day × weekday grid (local time, Monday first): all 168 cells, row-major by weekday. */
export function heat(ix: Index, r: Range, filters: Filters): HeatCell[] {
  const [h0, h1] = hours(r)
  const checks = compile(ix, filters)
  const usd = new Float64Array(168)
  const calls = new Float64Array(168)
  for (let i = 0; i < ix.n; i++) {
    const h = ix.hour[i]!
    if (h < h0 || h >= h1 || !passes(checks, i)) continue
    const k = weekday(ix.day[i]!) * 24 + ix.lhour[i]!
    usd[k]! += ix.nanos[i]! / 1e9
    calls[k]! += ix.calls[i]!
  }
  return Array.from({ length: 168 }, (_, k) => ({ wd: Math.floor(k / 24), h: k % 24, usd: usd[k]!, calls: calls[k]! }))
}

// --------------------------------------------------------------------------------------------------------------- tokens

export type TokenRow = { label: string; tip: string; uncached: number; cacheRead: number; cacheWrite: number; output: number }

/** Input mix and output per bucket; buckets with no tokens are left out (the 100 % area skips them). */
export function tokenSeries(ix: Index, q: Range & { gran: Gran; filters: Filters }): TokenRow[] {
  const ax = axis(ix, q, q.gran)
  const B = ax.buckets.length
  const [h0, h1] = hours(q)
  const checks = compile(ix, q.filters)
  const a = new Float64Array(B * 4)
  for (let i = 0; i < ix.n; i++) {
    const h = ix.hour[i]!
    if (h < h0 || h >= h1 || !passes(checks, i)) continue
    const k = ax.at(i)
    if (k < 0 || k >= B) continue
    a[k * 4]! += Math.max(0, ix.input[i]! - ix.cacheRead[i]! - ix.cacheWrite[i]!)
    a[k * 4 + 1]! += ix.cacheRead[i]!
    a[k * 4 + 2]! += ix.cacheWrite[i]!
    a[k * 4 + 3]! += ix.output[i]!
  }
  const out: TokenRow[] = []
  ax.buckets.forEach((b, k) => {
    if (a[k * 4]! + a[k * 4 + 1]! + a[k * 4 + 2]! + a[k * 4 + 3]! === 0) return
    out.push({ label: b.label, tip: b.tip, uncached: a[k * 4]!, cacheRead: a[k * 4 + 1]!, cacheWrite: a[k * 4 + 2]!, output: a[k * 4 + 3]! })
  })
  return out
}

// ------------------------------------------------------------------------------------------------------------- sessions

export type SessionRow = {
  id: string
  label: string
  deleted: boolean
  owner: string
  ownerId: string
  ownerKind: SpendOwnerKind
  project: string | null
  models: string[]
  calls: number
  tokens: number
  usd: number
  /** The basis carrying most of the row's money (unpriced when it has none). */
  basis: CostBasis
  /** Start of the newest hour with a call (ms). */
  last: number
}

/** Root sessions with calls in the range (each root carries its subagents' calls), by USD. */
export function sessions(ix: Index, r: Range, filters: Filters): SessionRow[] {
  const [h0, h1] = hours(r)
  const checks = compile(ix, filters)
  const R = ix.rootOwner.length
  const usd = new Float64Array(R)
  const calls = new Float64Array(R)
  const tokens = new Float64Array(R)
  const last = new Float64Array(R)
  const by = new Float64Array(R * 4)
  // Per root × model: spend and calls (a flat matrix, not a Set per root), so the row lists its models by spend.
  const M = ix.dict.model.ids.length
  const mUsd = new Float64Array(R * M)
  const mCalls = new Float64Array(R * M)
  for (let i = 0; i < ix.n; i++) {
    const h = ix.hour[i]!
    if (h < h0 || h >= h1 || !passes(checks, i)) continue
    const r0 = ix.root[i]!
    const u = ix.nanos[i]! / 1e9
    usd[r0]! += u
    calls[r0]! += ix.calls[i]!
    tokens[r0]! += ix.tokens[i]!
    by[r0 * 4 + ix.col.basis[i]!]! += u
    if (h > last[r0]!) last[r0] = h
    mUsd[r0 * M + ix.col.model[i]!]! += u
    mCalls[r0 * M + ix.col.model[i]!]! += ix.calls[i]!
  }
  const rows: SessionRow[] = []
  for (let r0 = 0; r0 < R; r0++) {
    if (!calls[r0] && !usd[r0]) continue
    const root = ix.roots[r0]
    const o = ix.rootOwner[r0]!
    let best = 3
    for (let b = 0; b < 3; b++) if (by[r0 * 4 + b]! > 0 && by[r0 * 4 + b]! >= (best === 3 ? 0 : by[r0 * 4 + best]!)) best = b
    rows.push({
      id: root?.id ?? `#${r0}`,
      label: root ? sessionLabel(root) : 'Unknown session',
      deleted: root?.deleted ?? true,
      owner: ix.dict.owner.labels[o]!,
      ownerId: ix.dict.owner.ids[o]!,
      ownerKind: ix.ownerKind[o]!,
      project: root?.project ?? null,
      models: Array.from({ length: M }, (_, m) => m)
        .filter((m) => mCalls[r0 * M + m]! > 0 || mUsd[r0 * M + m]! > 0)
        .sort((a, b) => mUsd[r0 * M + b]! - mUsd[r0 * M + a]! || mCalls[r0 * M + b]! - mCalls[r0 * M + a]!)
        .map((m) => ix.dict.model.labels[m]!),
      calls: calls[r0]!,
      tokens: tokens[r0]!,
      usd: usd[r0]!,
      basis: BASES[best]!,
      last: last[r0]! * HOUR_MS,
    })
  }
  return rows.sort((a, b) => b.usd - a.usd || b.calls - a.calls)
}

// ------------------------------------------------------------------------------------------------------------------ csv

const csvCell = (s: string): string => (/[",\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s)

/** The current buckets × series as CSV: start (ISO), label, one column per series, total, and previous when `compare`. */
export function toCsv(out: SeriesOut, metric: Metric, compare: boolean): string {
  const num = (v: number) => (metric === 'usd' ? v.toFixed(6) : String(Math.round(v)))
  const head = ['bucket_start', 'bucket', ...out.series.map((s) => s.label), 'total', ...(compare ? ['previous'] : [])]
  const lines = [head.map(csvCell).join(',')]
  for (const r of out.rows) lines.push([new Date(r.start).toISOString(), r.tip, ...out.series.map((s) => num(r[s.key] as number)), num(r.total), ...(compare ? [num(r.prev)] : [])].map(csvCell).join(','))
  return `${lines.join('\n')}\n`
}
