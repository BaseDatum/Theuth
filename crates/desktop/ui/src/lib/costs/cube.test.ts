import { defaultParseSearch, defaultStringifySearch } from '@tanstack/react-router'
import { expect, test } from 'vitest'
import type { CostCells } from '@/rpc/bindings/CostCells'
import type { CostCube } from '@/rpc/bindings/CostCube'
import type { CostRootEntry } from '@/rpc/bindings/CostRootEntry'
import { costsFixture } from '@/dev/costs-fixture'
import { breakdown, emptyCube, heat, HOUR_MS, index, merge, options, previousPeriod, series, sessions, toCsv, tokenSeries, totals } from './cube'
import { granBlock, resolveRange, validateCostsSearch } from './search'

type Cell = { hour: number; root?: number; model?: number; provider?: number; kind?: number; via?: number; basis?: number; usd?: number; calls?: number; failed?: number; input?: number; output?: number; read?: number; write?: number }

const h = (iso: string) => Date.parse(iso) / HOUR_MS
const ROOTS: CostRootEntry[] = [
  { ix: 0, root: { id: 'r0', title: 'Alpha chat', owner_kind: 'user', owner: 'you', project: 'atlas', deleted: false, created_at_ms: Date.parse('2026-10-01T09:00:00Z') } },
  { ix: 1, root: { id: 'r1', title: 'Desk', owner_kind: 'agent', owner: 'builder', project: null, deleted: false, created_at_ms: Date.parse('2026-10-01T09:00:00Z') } },
  { ix: 2, root: { id: 'r2', title: '', owner_kind: 'routine', owner: 'nightly', project: 'atlas', deleted: true, created_at_ms: Date.parse('2026-10-03T09:00:00Z') } },
]

function cube(cells: Cell[], over: Partial<CostCube> = {}): CostCube {
  const c: CostCells = { hour: [], root: [], model: [], provider: [], kind: [], via: [], basis: [], nanos: [], calls: [], failed: [], repriced: [], input: [], output: [], cache_read: [], cache_write: [], cache_saved_nanos: [], approx_nanos: [] }
  for (const x of cells) {
    c.hour.push(x.hour)
    c.root.push(x.root ?? 0)
    c.model.push(x.model ?? 0)
    c.provider.push(x.provider ?? 0)
    c.kind.push(x.kind ?? 0)
    c.via.push(x.via ?? 0)
    c.basis.push(x.basis ?? 2)
    c.nanos.push(Math.round((x.usd ?? 1) * 1e9))
    c.calls.push(x.calls ?? 1)
    c.failed.push(x.failed ?? 0)
    c.repriced.push(0)
    c.input.push(x.input ?? 0)
    c.output.push(x.output ?? 0)
    c.cache_read.push(x.read ?? 0)
    c.cache_write.push(x.write ?? 0)
    c.cache_saved_nanos.push(0)
    c.approx_nanos.push(0)
  }
  return {
    ...emptyCube(),
    epoch: 'e1',
    rev: 1,
    providers: ['anthropic', 'openai'],
    kinds: ['round', 'endless'],
    vias: ['theuth', 'omp'],
    models: [
      { id: 'anthropic/claude-haiku-4-5', name: 'claude-haiku-4-5', provider: 'anthropic' },
      { id: 'anthropic/claude-haiku-4-5-20251001', name: 'claude-haiku-4-5', provider: 'anthropic' },
      { id: 'openai/gpt-6', name: 'gpt-6', provider: 'openai' },
    ],
    roots: ROOTS,
    cells: c,
    ...over,
  }
}

/** One $1 call in each UTC hour of `[from, to)`. */
const hourly = (from: string, to: string): Cell[] => Array.from({ length: h(to) - h(from) }, (_, i) => ({ hour: h(from) + i }))
const range = (from: string, to: string) => ({ from: Date.parse(from), to: Date.parse(to) })

test('merge: full replaces, a same-epoch delta replaces its hours and upserts roots, another epoch asks for a full fetch, a stale rev is ignored', () => {
  const held = cube([{ hour: 100, usd: 1 }, { hour: 101, usd: 2 }, { hour: 101, root: 1, usd: 3 }, { hour: 102, usd: 4 }])
  expect(merge(held, { ...cube([{ hour: 5 }]), full: true })!.cells.hour).toEqual([5])
  const delta: CostCube = {
    ...cube([{ hour: 101, usd: 10 }, { hour: 103, root: 3, usd: 20 }]),
    rev: 2,
    full: false,
    hours: [101, 103],
    providers: ['anthropic', 'openai', 'xai'],
    roots: [
      { ix: 1, root: { ...ROOTS[1]!.root, title: 'Desk renamed' } },
      { ix: 3, root: { id: 'r3', title: 'New', owner_kind: 'user', owner: 'you', project: null, deleted: false, created_at_ms: 0 } },
    ],
  }
  const m = merge(held, delta)!
  expect(m.rev).toBe(2)
  expect(m.full).toBe(true)
  expect(m.cells.hour).toEqual([100, 102, 101, 103])
  expect(m.cells.nanos).toEqual([1e9, 4e9, 10e9, 20e9])
  expect(m.roots.map((r) => [r.ix, r.root.title])).toEqual([[0, 'Alpha chat'], [1, 'Desk renamed'], [2, ''], [3, 'New']])
  expect(m.providers).toEqual(['anthropic', 'openai', 'xai'])
  for (const col of Object.values(m.cells)) expect(col).toHaveLength(4)
  expect(merge(held, { ...delta, epoch: 'e2' })).toBeNull()
  expect(merge(undefined, delta)).toBeNull()
  expect(merge(m, { ...delta, rev: 2 })).toBe(m)
})

test('local days across the autumn DST change: London and New York days have 25 hours, ISO weeks start Monday, months by local date', () => {
  const cells = hourly('2026-10-23T00:00:00Z', '2026-11-04T00:00:00Z')
  const lon = index(cube(cells), 'Europe/London')
  const days = series(lon, { ...range('2026-10-24T00:00:00+01:00', '2026-10-27T00:00:00Z'), gran: 'day', groupBy: 'model', metric: 'calls', filters: {} })
  expect(days.rows.map((r) => [r.label, r.total])).toEqual([['24 Oct', 24], ['25 Oct', 25], ['26 Oct', 24]])
  const ny = index(cube(cells), 'America/New_York')
  const nyDays = series(ny, { ...range('2026-10-31T04:00:00Z', '2026-11-03T05:00:00Z'), gran: 'day', groupBy: 'model', metric: 'calls', filters: {} })
  expect(nyDays.rows.map((r) => [r.label, r.total])).toEqual([['31 Oct', 24], ['1 Nov', 25], ['2 Nov', 24]])
  // Sunday 1 Nov is the last day of the ISO week of Mon 26 Oct; Monday 2 Nov starts the next.
  const weeks = series(ny, { ...range('2026-10-26T04:00:00Z', '2026-11-03T05:00:00Z'), gran: 'week', groupBy: 'model', metric: 'calls', filters: {} })
  expect(weeks.rows.map((r) => [r.label, r.total])).toEqual([['26 Oct', 7 * 24 + 1], ['2 Nov', 24]])
  // 31 Oct 23:00 New York is 1 Nov 03:00 UTC: October.
  const months = series(ny, { ...range('2026-10-23T04:00:00Z', '2026-11-04T00:00:00Z'), gran: 'month', groupBy: 'model', metric: 'calls', filters: {} })
  expect(months.rows.map((r) => [r.label, r.total])).toEqual([['Oct 2026', 9 * 24], ['Nov 2026', 3 * 24 - 4]])
})

test('spring DST change: the London and New York local days have 23 hours, and hour buckets keep local labels', () => {
  const lon = index(cube(hourly('2026-03-28T00:00:00Z', '2026-03-31T00:00:00Z')), 'Europe/London')
  const d = series(lon, { ...range('2026-03-28T00:00:00Z', '2026-03-30T23:00:00Z'), gran: 'day', groupBy: 'model', metric: 'calls', filters: {} })
  expect(d.rows.map((r) => r.total)).toEqual([24, 23, 24]) // 30 Mar runs 23:00Z → 23:00Z
  const hours = series(lon, { ...range('2026-03-29T00:00:00Z', '2026-03-29T03:00:00Z'), gran: 'hour', groupBy: 'model', metric: 'calls', filters: {} })
  expect(hours.rows.map((r) => r.label)).toEqual(['00:00', '02:00', '03:00'])
  const ny = index(cube(hourly('2026-03-07T05:00:00Z', '2026-03-10T04:00:00Z')), 'America/New_York')
  expect(series(ny, { ...range('2026-03-07T05:00:00Z', '2026-03-10T04:00:00Z'), gran: 'day', groupBy: 'model', metric: 'calls', filters: {} }).rows.map((r) => r.total)).toEqual([24, 23, 24])
})

test('filters: model aliases share a name, owner via the root, title search, and facets ignore their own dimension', () => {
  const ix = index(cube([{ hour: 10, model: 0, usd: 1 }, { hour: 10, model: 1, usd: 2 }, { hour: 10, model: 2, provider: 1, root: 1, usd: 4 }, { hour: 11, root: 2, usd: 8 }]), 'UTC')
  const r = { from: 0, to: 20 * HOUR_MS }
  expect(totals(ix, r, { model: ['claude-haiku-4-5'] }).usd).toBe(11)
  expect(totals(ix, r, { owner: ['agent:builder'] }).usd).toBe(4)
  expect(totals(ix, r, { project: ['atlas'] }).usd).toBe(11)
  expect(totals(ix, r, { project: [''] }).usd).toBe(4)
  expect(totals(ix, r, { q: 'alpha' }).usd).toBe(3)
  expect(totals(ix, r, { q: 'deleted session' }).usd).toBe(8)
  expect(totals(ix, r, { model: ['gpt-6'], provider: ['anthropic'] }).usd).toBe(0)
  const o = options(ix, r, { provider: ['openai'] })
  expect(o.provider.map((x) => [x.id, x.usd])).toEqual([['anthropic', 11], ['openai', 4]])
  expect(o.model.map((x) => [x.id, x.usd])).toEqual([['gpt-6', 4]])
  expect(o.owner.map((x) => [x.id, x.kind])).toEqual([['agent:builder', 'agent']])
})

test('top 7 + other: ten groups fold the three smallest into other; the stack sums to the total', () => {
  const models = Array.from({ length: 10 }, (_, i) => ({ id: `m${i}`, name: `m${i}`, provider: 'anthropic' }))
  const ix = index(cube(models.map((_, i) => ({ hour: 10, model: i, usd: i + 1 })), { models }), 'UTC')
  const s = series(ix, { from: 10 * HOUR_MS, to: 11 * HOUR_MS, gran: 'hour', groupBy: 'model', metric: 'usd', filters: {} })
  expect(s.series.map((x) => x.label)).toEqual(['m9', 'm8', 'm7', 'm6', 'm5', 'm4', 'm3', 'other'])
  expect(s.series.at(-1)!.color).toBe('var(--chart-6)')
  expect(s.rows[0]!.other).toBe(1 + 2 + 3)
  expect(s.rows[0]!.total).toBe(55)
  expect(s.total).toBe(55)
  expect(new Set(s.series.map((x) => x.color)).size).toBe(8)
})

test('previous period: the dashed line is the equal run of buckets before, the KPI window the equal hours before', () => {
  const ix = index(cube([{ hour: h('2026-10-01T05:00:00Z'), usd: 3 }, { hour: h('2026-10-03T05:00:00Z'), usd: 5 }, { hour: h('2026-10-04T05:00:00Z'), usd: 7 }, { hour: h('2026-10-06T05:00:00Z'), usd: 11 }]), 'UTC')
  const r = range('2026-10-04T00:00:00Z', '2026-10-07T00:00:00Z')
  const s = series(ix, { ...r, gran: 'day', groupBy: 'model', metric: 'usd', filters: {} })
  expect(s.rows.map((x) => [x.label, x.total, x.prev])).toEqual([['4 Oct', 7, 3], ['5 Oct', 0, 0], ['6 Oct', 11, 5]])
  expect(s.prevTotal).toBe(8)
  expect(previousPeriod(r)).toEqual(range('2026-10-01T00:00:00Z', '2026-10-04T00:00:00Z'))
  expect(totals(ix, previousPeriod(r), {}).usd).toBe(8)
})

test('totals per basis, breakdowns, heat grid, token mix and session rows', () => {
  const t = h('2026-10-05T14:00:00Z') // a Monday
  const ix = index(
    cube([
      { hour: t, basis: 0, usd: 1, calls: 2, input: 1000, read: 600, write: 100, output: 50 },
      { hour: t, basis: 1, usd: 2, root: 1, via: 1, model: 2, provider: 1 },
      { hour: t + 24, basis: 2, usd: 4, calls: 3, failed: 1 },
      { hour: t + 25, basis: 3, usd: 0, calls: 5, failed: 5, root: 2 },
    ]),
    'Europe/Paris',
  )
  const r = range('2026-10-05T00:00:00Z', '2026-10-08T00:00:00Z')
  const tt = totals(ix, r, {})
  expect(tt.usdBy).toEqual([1, 2, 4, 0])
  expect(tt.callsBy).toEqual([2, 1, 3, 5])
  expect([tt.calls, tt.failed, tt.input, tt.cacheRead, tt.output]).toEqual([11, 6, 1000, 600, 50])
  expect(breakdown(ix, r, {}, 'via').map((x) => [x.id, x.usd, x.calls])).toEqual([['theuth', 5, 10], ['omp', 2, 1]])
  expect(breakdown(ix, r, {}, 'owner').map((x) => [x.label, x.kind])).toEqual([['you', 'user'], ['builder', 'agent'], ['nightly', 'routine']])
  const g = heat(ix, r, {})
  expect(g).toHaveLength(168)
  expect(g.find((c) => c.wd === 0 && c.h === 16)!.usd).toBe(3) // 14:00 UTC = 16:00 Paris (CEST), Monday
  expect(g.find((c) => c.wd === 1 && c.h === 17)!.calls).toBe(5)
  const tok = tokenSeries(ix, { ...r, gran: 'day', filters: {} })
  expect(tok).toEqual([{ label: '5 Oct', tip: 'Mon 5 Oct 2026', uncached: 300, cacheRead: 600, cacheWrite: 100, output: 50 }])
  const rows = sessions(ix, r, {})
  expect(rows.map((x) => [x.id, x.usd, x.basis, x.models.join()])).toEqual([['r0', 5, 'estimated', 'claude-haiku-4-5'], ['r1', 2, 'reported', 'gpt-6'], ['r2', 0, 'unpriced', 'claude-haiku-4-5']])
  expect(rows[2]!.label).toMatch(/^Deleted session · 3 Oct$/)
  expect(rows[0]!.last).toBe((t + 24) * HOUR_MS)
})

test('CSV: one line per bucket, a column per series, total and previous; labels quoted when needed', () => {
  const ix = index(cube([{ hour: h('2026-10-04T05:00:00Z'), usd: 1.5 }, { hour: h('2026-10-04T06:00:00Z'), model: 2, provider: 1, usd: 2 }]), 'UTC')
  const s = series(ix, { ...range('2026-10-04T00:00:00Z', '2026-10-06T00:00:00Z'), gran: 'day', groupBy: 'model', metric: 'usd', filters: {} })
  expect(toCsv(s, 'usd', true).split('\n')).toEqual([
    'bucket_start,bucket,gpt-6,claude-haiku-4-5,total,previous',
    '2026-10-04T00:00:00.000Z,Sun 4 Oct 2026,2.000000,1.500000,3.500000,0.000000',
    '2026-10-05T00:00:00.000Z,Mon 5 Oct 2026,0.000000,0.000000,0.000000,0.000000',
    '',
  ])
})

test('URL search: validate drops junk and defaults, survives the router round trip, resolves presets in the zone', () => {
  const s = validateCostsSearch({ range: '7d', gran: 'day', by: 'owner', metric: 'calls', compare: false, model: ['a', 'a', 'b'], provider: 'openai', kind: 42, q: 'alpha', junk: 1 })
  expect(s).toEqual({ range: '7d', gran: 'day', by: 'owner', metric: 'calls', compare: false, model: ['a', 'b'], provider: ['openai'], q: 'alpha' })
  expect(validateCostsSearch(defaultParseSearch(defaultStringifySearch(s)))).toEqual(s)
  const custom = { range: 'custom' as const, from: 1_790_000_000_000, to: 1_790_086_400_000 }
  expect(validateCostsSearch(defaultParseSearch(defaultStringifySearch(custom)))).toEqual(custom)
  expect(validateCostsSearch({ range: '30d', by: 'model', metric: 'usd', compare: true, range2: 'x' })).toEqual({})
  expect(validateCostsSearch({ range: 'custom', from: 5, to: 1 })).toEqual({})
  const now = Date.parse('2026-11-02T14:25:00Z')
  expect(resolveRange({ range: 'today' }, now, 'America/New_York', null)).toEqual({ from: Date.parse('2026-11-02T05:00:00Z'), to: now })
  expect(resolveRange({ range: 'mtd' }, now, 'Europe/London', null).from).toBe(Date.parse('2026-11-01T00:00:00Z'))
  expect(resolveRange({}, now, 'Europe/London', null).from).toBe(Date.parse('2026-10-04T00:00:00+01:00'))
  expect(granBlock('UTC', resolveRange({ range: '30d' }, now, 'UTC', null), 'hour')).toMatch(/more than 400/)
  expect(granBlock('UTC', resolveRange({ range: 'today' }, now, 'UTC', null), 'day')).toMatch(/one bucket/)
  expect(granBlock('UTC', resolveRange({ range: '90d' }, now, 'UTC', null), 'day')).toBeNull()
})

test('perf bound: 100 k cells — index once, then a full page query (series, totals ×2, 7 breakdowns, options, heat, tokens, sessions) under 50 ms', () => {
  const base = costsFixture(Date.parse('2026-11-02T14:00:00Z'))
  const c = base.cells
  const n = 100_000
  const big: CostCells = Object.fromEntries(Object.entries(c).map(([k, col]) => [k, Array.from({ length: n }, (_, i) => (k === 'hour' ? (col as number[])[i % col.length]! - 24 * 56 * Math.floor(i / col.length) : (col as number[])[i % col.length]!))])) as CostCells
  const ix = index({ ...base, cells: big }, 'Europe/London')
  expect(ix.n).toBe(n)
  const r = { from: (ix.minHour - 1) * HOUR_MS, to: (ix.maxHour + 1) * HOUR_MS }
  const run = () => {
    series(ix, { ...r, gran: 'week', groupBy: 'model', metric: 'usd', filters: { kind: ['round', 'endless'] } })
    totals(ix, r, {})
    totals(ix, previousPeriod(r), {})
    for (const d of ['model', 'provider', 'owner', 'kind', 'via', 'basis', 'project'] as const) breakdown(ix, r, {}, d)
    options(ix, r, { kind: ['round'] })
    heat(ix, r, {})
    tokenSeries(ix, { ...r, gran: 'week', filters: {} })
    sessions(ix, r, {})
  }
  run()
  const t0 = performance.now()
  run()
  expect(performance.now() - t0).toBeLessThan(50)
})
