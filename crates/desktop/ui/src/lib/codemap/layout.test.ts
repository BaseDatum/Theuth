import { expect, test } from 'vitest'
import change from '@/components/codemap/__fixtures__/change.json'
import fit from '@/components/codemap/__fixtures__/fit.json'
import { bandsLayout, columnsLayout, nodeSize, placeSlot, type Placed, type Slots } from './layout'
import type { ChangeLens, FitLens } from './types'

const FIT = fit as FitLens
const CHANGE = change as unknown as ChangeLens
const pos = (p: Placed): Record<string, [number, number]> => Object.fromEntries(p.layout.nodes.map((n) => [n.id, [n.x, n.y]]))

test('placeSlot: existing ids keep their slot; new ids take the first free index per layer', () => {
  const slots: Slots = new Map()
  expect(placeSlot(slots, 'a', 0, 10, 10)).toMatchObject({ layer: 0, index: 0 })
  expect(placeSlot(slots, 'b', 0, 10, 10)).toMatchObject({ layer: 0, index: 1 })
  expect(placeSlot(slots, 'c', 2, 10, 10)).toMatchObject({ layer: 2, index: 0 })
  expect(placeSlot(slots, 'a', 0, 99, 99)).toMatchObject({ layer: 0, index: 0, w: 10 })
  expect(placeSlot(slots, 'd', 0)).toMatchObject({ layer: 0, index: 2 })
})

test('columnsLayout over the fixture: 3 columns, grouped file nodes, bundles last, dashed edges, badges', () => {
  const W = 800
  const p = columnsLayout(FIT, new Map(), W, { stale: new Set([FIT.callees[0]!.moniker, 'rust:crates/tools/src/lib.rs']) })
  expect(p.graph.nodes).toHaveLength(20)
  expect(p.graph.nodes.map((n) => n.id)).toContain('rust:crates/tools/src/lib.rs')
  expect(p.graph.nodes.find((n) => n.id === 'rust:crates/tools/src/lib.rs')?.label).toBe('lib.rs ×2')
  expect(p.graph.nodes.find((n) => n.id === FIT.focus.moniker)).toMatchObject({ label: 'ƒ ToolRegistry::start', state: 'active' })
  expect(p.badges[FIT.focus.moniker]).toBe('impacted · ?3')
  const test = FIT.callers.find((c) => c.is_test)!
  expect(p.graph.nodes.find((n) => n.id === test.moniker)?.state).toBe('done')
  expect(p.badges[test.moniker]).toBe('test')
  expect(p.hatch).toEqual(['rust:crates/tools/src/lib.rs'])
  expect(p.badges['rust:crates/tools/src/lib.rs']).toBe('stale')
  const b0 = p.graph.nodes.find((n) => n.id === 'bundle:0')!
  expect(b0).toMatchObject({ label: '+14 callers', kind: 'pill', state: 'muted' })
  expect(p.graph.nodes.find((n) => n.id === 'bundle:2')?.label).toBe('+1 callees')
  const at = pos(p)
  const callerYs = FIT.callers.map((c) => at[c.moniker]![1])
  expect(callerYs).toEqual([...callerYs].sort((a, b) => a - b))
  expect(at['bundle:0']![1]).toBeGreaterThan(Math.max(...callerYs))
  for (const c of FIT.callers) expect(at[c.moniker]![0]).toBe(0)
  const focus = p.layout.nodes.find((n) => n.id === FIT.focus.moniker)!
  expect(focus.x).toBe((W - focus.w) / 2)
  expect(focus.y).toBe((p.layout.height - focus.h) / 2)
  const callee = p.layout.nodes.find((n) => n.id === 'rust:crates/tools/src/lib.rs')!
  expect(callee.x).toBe(W - callee.w)
  expect(p.graph.edges.filter((e) => e.style === 'dashed')).toHaveLength(7)
  expect(p.graph.edges.every((e) => p.graph.nodes.some((n) => n.id === e.from) && p.graph.nodes.some((n) => n.id === e.to))).toBe(true)
  const lib = p.graph.edges.find((e) => e.to === 'rust:crates/tools/src/lib.rs')!
  expect(lib.label).toBe('×4')
  const le = p.layout.edges[p.graph.edges.indexOf(lib)]!
  const [s] = le.sections
  expect(s!.bends).toEqual([{ x: (s!.start.x + s!.end.x) / 2, y: s!.start.y }, { x: (s!.start.x + s!.end.x) / 2, y: s!.end.y }])
  expect(le.label).toBeDefined()
})

test('columnsLayout limit folds the excess into the bundle counts', () => {
  const p = columnsLayout(FIT, new Map(), 600, { limit: 10 })
  expect(p.graph.nodes.filter((n) => !n.id.startsWith('bundle:')).length).toBeLessThanOrEqual(10)
  const more = Number(/\+(\d+)/.exec(p.graph.nodes.find((n) => n.id === 'bundle:0')!.label)![1]) + Number(/\+(\d+)/.exec(p.graph.nodes.find((n) => n.id === 'bundle:2')!.label)![1])
  expect(more).toBe(14 + 1 + 19 - 9)
})

test('stability: a reordering refetch moves nothing; a new caller takes the first free index; a removed node leaves a gap', () => {
  const slots: Slots = new Map()
  const first = pos(columnsLayout(FIT, slots, 800, { limit: 40 }))
  const shuffled = { ...FIT, callers: [...FIT.callers].reverse().sort((a, b) => a.name.localeCompare(b.name)), callees: [...FIT.callees].reverse() }
  const second = pos(columnsLayout(shuffled, slots, 800, { limit: 40 }))
  expect(second).toEqual(first)
  const fresh = { ...FIT.callers[0]!, moniker: 'rust:crates/x/src/new.rs#brand_new(function)', path: 'crates/x/src/new.rs', name: 'brand_new' }
  const grown = { ...FIT, callers: [fresh, ...FIT.callers] }
  const third = pos(columnsLayout(grown, slots, 800, { limit: 40 }))
  expect(slots.get(fresh.moniker)).toMatchObject({ layer: 0, index: 12 })
  for (const c of FIT.callers) expect(third[c.moniker]).toEqual(first[c.moniker])
  expect(third[fresh.moniker]![1]).toBeGreaterThan(Math.max(...FIT.callers.map((c) => first[c.moniker]![1])))
  const gone = FIT.callers[3]!
  const removed = { ...FIT, callers: FIT.callers.filter((c) => c !== gone) }
  const fourth = pos(columnsLayout(removed, slots, 800, { limit: 40 }))
  expect(fourth[gone.moniker]).toBeUndefined()
  for (const c of removed.callers) expect(fourth[c.moniker]).toEqual(first[c.moniker])
  expect(slots.get(gone.moniker)).toMatchObject({ layer: 0, index: 3 })
})

test('nodeSize: wraps at WRAP and never below the minimum box', () => {
  expect(nodeSize('ƒ f')).toEqual({ w: 64, h: 33 })
  const one = nodeSize('a'.repeat(24))
  const two = nodeSize(`${'a'.repeat(24)} ${'b'.repeat(10)}`)
  expect(two.h).toBe(one.h + 17)
  expect(two.w).toBe(one.w)
})

test('bandsLayout: four groups with counts, states + badges, stale hatch, glyph prefixes, no edges', () => {
  const p = bandsLayout(CHANGE, new Map(), 700)
  expect(p.graph.groups?.map((g) => g.label)).toEqual(['changed (3)', 'direct (12)', 'transitive (4)', 'tests (8)'])
  expect(p.graph.edges).toEqual([])
  expect(p.layout.edges).toEqual([])
  expect(p.graph.nodes).toHaveLength(3 + 12 + 4 + 8)
  expect(p.graph.nodes.find((n) => n.id === 'crates/codeindex/src/rank.rs')).toMatchObject({ label: '+ src/rank.rs', state: 'danger', group: 'changed', kind: 'pill' })
  expect(p.graph.nodes.find((n) => n.id === 'crates/codeindex/src/index.rs')?.label).toBe('~ src/index.rs')
  expect(p.badges['crates/codeindex/src/rank.rs']).toBe('changed')
  expect(p.graph.nodes.find((n) => n.id === 'crates/rpc/src/api.rs')).toMatchObject({ state: 'active', group: 'direct' })
  expect(p.badges['crates/rpc/src/api.rs']).toBe('impacted')
  expect(p.graph.nodes.find((n) => n.id === 'crates/cli/src/main.rs')?.state).toBe('muted')
  expect(p.badges['crates/cli/src/main.rs']).toBe('transitive')
  expect(p.graph.nodes.find((n) => n.id === 'crates/codeindex/tests/index.rs')).toMatchObject({ state: 'done', label: '✓ tests/index.rs' })
  expect(p.graph.nodes.find((n) => n.id === 'crates/codeindex/tests/fixture.rs')?.label).toBe('◇ fixture.rs')
  expect(p.badges['crates/codeindex/tests/index.rs']).toBe('test')
  expect(p.hatch).toEqual(['crates/codeindex/src/lens.rs'])
  expect(p.badges['crates/codeindex/src/lens.rs']).toBe('impacted · stale')
  // wraps at width: every pill inside the band box, rows stacked, groups stacked
  const box = Object.fromEntries(p.layout.groups.map((g) => [g.id, g]))
  for (const n of p.layout.nodes) {
    const g = box[p.graph.nodes.find((x) => x.id === n.id)!.group!]!
    expect(n.x + n.w).toBeLessThanOrEqual(700)
    expect(n.y).toBeGreaterThanOrEqual(g.y)
    expect(n.y + n.h).toBeLessThanOrEqual(g.y + g.h)
  }
  const direct = p.layout.nodes.filter((n) => CHANGE.direct.some((f) => f.path === n.id))
  expect(new Set(direct.map((n) => n.y)).size).toBeGreaterThan(1)
  expect(p.layout.groups[1]!.y).toBeGreaterThan(p.layout.groups[0]!.y + p.layout.groups[0]!.h)
  expect(p.layout.height).toBe(p.layout.groups[3]!.y + p.layout.groups[3]!.h)
})

test('bandsLayout caps: beyond a cap a `+N in <dir>` bundle carries the count; slots keep pills in place', () => {
  const slots: Slots = new Map()
  const p = bandsLayout(CHANGE, slots, 700, { caps: [12, 5, 16, 16] })
  const bundle = p.graph.nodes.find((n) => n.id === 'bundle:1')!
  expect(bundle).toMatchObject({ label: '+7 in crates/codeindex', state: 'muted', group: 'direct' })
  expect(p.graph.nodes.filter((n) => n.group === 'direct')).toHaveLength(6)
  const withMore = { ...CHANGE, more: [2, 0, 0, 0] as [number, number, number, number] }
  expect(bandsLayout(withMore, new Map(), 700).graph.nodes.find((n) => n.id === 'bundle:0')?.label).toBe('+2 in crates/codeindex')
  expect(bandsLayout(withMore, new Map(), 700).graph.groups?.[0]?.label).toBe('changed (5)')
  const before = Object.fromEntries(p.layout.nodes.map((n) => [n.id, [n.x, n.y]]))
  const reordered = { ...CHANGE, direct: [...CHANGE.direct.slice(0, 5).reverse(), ...CHANGE.direct.slice(5)] }
  const q = bandsLayout(reordered, slots, 700, { caps: [12, 5, 16, 16] })
  expect(Object.fromEntries(q.layout.nodes.map((n) => [n.id, [n.x, n.y]]))).toEqual(before)
  const added = { ...CHANGE, direct: [{ ...CHANGE.direct[11]!, path: 'crates/new/src/file.rs' }, ...CHANGE.direct] }
  const r = bandsLayout(added, slots, 700, { caps: [12, 5, 16, 16] })
  expect(slots.get('crates/new/src/file.rs')).toMatchObject({ layer: 1, index: 5 })
  for (const f of CHANGE.direct.slice(0, 4)) expect(r.layout.nodes.find((n) => n.id === f.path)).toMatchObject({ x: before[f.path]![0], y: before[f.path]![1] })
  expect(r.graph.nodes.find((n) => n.id === 'bundle:1')?.label).toBe('+8 in crates/codeindex')
})
