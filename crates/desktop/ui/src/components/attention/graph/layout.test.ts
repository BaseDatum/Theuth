// 208 §10.2: the tidy tree — contract §9.1 exactly.
import { expect, test } from 'vitest'
import type { GraphNode } from '@/rpc/bindings/method.agent.output'
import { GRAPH_FIXTURE } from './fixture'
import { childOrder, layoutCache, layoutForest, layoutTree, packRoots, treeSignature } from './layout'

const base = GRAPH_FIXTURE.nodes[0] as GraphNode
const n = (id: string, parent: string | null, started: number): GraphNode => ({ ...base, id, parent, root: parent ? 'x' : id, started_ms: started })

const caseA = [n('R1', null, 1), n('A', 'R1', 2), n('B', 'R1', 3), n('A1', 'A', 4), n('A2', 'A', 5), n('R2', null, 6)]
const xy = (pos: Map<string, { x: number; y: number }>) => Object.fromEntries([...pos].map(([k, p]) => [k, [p.x, p.y]]))

test('case A: two levels plus a separate leaf root, compact packing', () => {
  const kids = childOrder(caseA)
  const r1 = layoutTree('R1', kids)
  expect(r1.width).toBe(536)
  expect(xy(r1.pos)).toEqual({ R1: [224, 0], A: [112, 120], B: [336, 120], A1: [0, 240], A2: [224, 240] })
  const forest = layoutForest(caseA, null)
  expect(xy(forest.pos)).toEqual({ R1: [224, 0], A: [112, 120], B: [336, 120], A1: [0, 240], A2: [224, 240], R2: [616, 0] })
  expect(Object.fromEntries(forest.slots)).toEqual({ R1: 0, R2: 616 })
})

test('case B: a deeper right subtree tucks under the leaf on its left (contour)', () => {
  const nodes = [n('P', null, 1), n('C1', 'P', 2), n('C2', 'P', 3), n('D1', 'C2', 4), n('D2', 'C2', 5)]
  const t = layoutTree('P', childOrder(nodes))
  expect(t.width).toBe(536)
  expect(xy(t.pos)).toEqual({ P: [112, 0], C1: [0, 120], C2: [224, 120], D1: [112, 240], D2: [336, 240] })
})

test('sticky packing: kept slots, growth pushes right, gone trees leave gaps, new trees append, null re-packs', () => {
  const prev = new Map([['R1', 0], ['R2', 616]])
  expect(Object.fromEntries(packRoots([{ root: 'R1', width: 536 }, { root: 'R2', width: 200 }], prev))).toEqual({ R1: 0, R2: 616 })
  expect(Object.fromEntries(packRoots([{ root: 'R1', width: 600 }, { root: 'R2', width: 200 }], prev))).toEqual({ R1: 0, R2: 680 })
  expect(Object.fromEntries(packRoots([{ root: 'R2', width: 200 }], prev))).toEqual({ R2: 616 })
  expect(Object.fromEntries(packRoots([{ root: 'R2', width: 200 }, { root: 'R3', width: 200 }], prev))).toEqual({ R2: 616, R3: 896 })
  expect(Object.fromEntries(packRoots([{ root: 'R2', width: 200 }, { root: 'R3', width: 200 }], null))).toEqual({ R2: 0, R3: 280 })
})

test('deterministic: child order by started_ms then id, whatever the input order', () => {
  const tie = [n('R', null, 1), n('b', 'R', 5), n('a', 'R', 5), n('c', 'R', 2)]
  expect(childOrder(tie).get('R')).toEqual(['c', 'a', 'b'])
  const once = layoutForest(tie, null)
  const again = layoutForest([...tie].reverse(), null)
  expect(xy(again.pos)).toEqual(xy(once.pos))
  const fixture = layoutForest(GRAPH_FIXTURE.nodes, null)
  expect(xy(layoutForest([...GRAPH_FIXTURE.nodes].reverse(), null).pos)).toEqual(xy(fixture.pos))
})

test('treeSignature changes on structure only; layoutForest reuses the cached tree for an unchanged signature', () => {
  const kids = childOrder(caseA)
  expect(treeSignature('R1', kids)).toBe('R1(A(A1,A2),B)')
  const restated = caseA.map((x) => ({ ...x, activity: { ...x.activity, state: { state: 'running' as const } }, last_event_ms: 9 }))
  expect(treeSignature('R1', childOrder(restated))).toBe('R1(A(A1,A2),B)')
  expect(treeSignature('R1', childOrder([...caseA, n('B1', 'B', 9)]))).toBe('R1(A(A1,A2),B(B1))')
  const cache = layoutCache(8)
  layoutForest(caseA, null, cache)
  const cached = cache.get('R1(A(A1,A2),B)')
  layoutForest(restated, null, cache)
  expect(cache.get('R1(A(A1,A2),B)')).toBe(cached)
  const bounded = layoutCache(2)
  for (const k of ['a', 'b', 'c']) bounded.set(k, { pos: new Map(), width: 0 })
  expect([...bounded.keys()]).toEqual(['b', 'c'])
})

test('state-only change moves nothing; a new root lands at the right with sticky slots', () => {
  const first = layoutForest(GRAPH_FIXTURE.nodes, null)
  const restated = GRAPH_FIXTURE.nodes.map((x) => ({ ...x, outcome: null }))
  expect(xy(layoutForest(restated, first.slots).pos)).toEqual(xy(first.pos))
  const grown = layoutForest([...GRAPH_FIXTURE.nodes, n('zz', null, GRAPH_FIXTURE_LAST)], first.slots)
  const maxX = Math.max(...[...first.pos.values()].map((p) => p.x))
  expect((grown.pos.get('zz') as { x: number }).x).toBeGreaterThan(maxX)
})
const GRAPH_FIXTURE_LAST = Math.max(...GRAPH_FIXTURE.nodes.map((x) => x.started_ms)) + 1
