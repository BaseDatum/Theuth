// 208 §10.2: TTL fading, pinning, ghosts and edge views — contract §9.2 / §9.3 exactly.
import { expect, test } from 'vitest'
import type { GraphNode } from '@/rpc/bindings/method.agent.output'
import { GRAPH_FIXTURE, GRAPH_FIXTURE_ATTENTION as ATT, GRAPH_FIXTURE_NOW as T0 } from './fixture'
import { fromSnapshot, type GraphState } from './reducer'
import { awaitTarget, fadeOpacity, nextTtlTick, TTL_DEFAULTS, visibleGraph, windowMs } from './ttl'

const MIN = 60_000
const S = fromSnapshot(GRAPH_FIXTURE)
const T1 = T0 + 4 * MIN
const T2 = T0 + 10 * MIN
type Want = 'pin' | 'ghost' | '-' | number
// node ids → [T0, T1, T2]
const TABLE: [string[], Want, Want, Want][] = [
  [['r1', 'r1.builder', 'r1.builder.linter', 'r1.builder.tests', 'r1.claude', 'r1.claude.explore'], 'pin', 'pin', 'pin'],
  [['r1.docs'], 1, '-', '-'],
  [['r1.reviewer'], 0.85, '-', '-'],
  [['r1.scout'], '-', '-', '-'],
  [['r1.claude.plan'], 0.55, '-', '-'],
  [['r1.codex'], 1, 0.85, '-'],
  [['r2'], 1, 1, 1],
  [['r2.bisect', 'r2.repro.min'], '-', '-', '-'],
  [['r2.repro'], 0.4, '-', '-'],
  [['r2.verify'], 1, '-', '-'],
  [['r2.omp'], 1, 'ghost', '-'],
  [['r2.omp.task1'], 1, 0.55, '-'],
  [['r2.omp.task2'], 1, 1, '-'],
  [['r3', 'r3.sweep', 'r3.audit', 'r3.audit.grep', 'r3.audit.diff'], 'pin', 'pin', 'pin'],
  [['r3.rotate'], '-', '-', '-'],
  [['r4'], 0.375, 0.325, '-'],
  [['r4.summarize', 'r4.index'], '-', '-', '-'],
  [['r5', 'r5.migrator', 'r5.migrator.check'], 'pin', 'pin', 'pin'],
  [['r5.old'], '-', '-', '-'],
  [['r6', 'r6.render'], 'pin', 'pin', 'pin'],
  [['r6.fetch'], '-', '-', '-'],
  [['r7'], 'pin', 'pin', 'pin'],
  [['r7.reply'], 0.55, '-', '-'],
  [['r8'], '-', '-', '-'],
  [['peer:alice'], 1, 1, 1],
  [['peer:mini-ops'], 1, 1, 1],
]

test('the §9.2 table covers every fixture node once', () => {
  expect(TABLE.flatMap(([ids]) => ids).sort()).toEqual(GRAPH_FIXTURE.nodes.map((n) => n.id).sort())
})

test.each([
  ['T0', T0, 1],
  ['T1', T1, 2],
  ['T2', T2, 3],
] as const)('§9.2 at %s', (_name, now, col) => {
  const view = visibleGraph(S, ATT, TTL_DEFAULTS, now)
  for (const row of TABLE) {
    const want = row[col]
    for (const id of row[0]) {
      const v = view.nodes.get(id)
      if (want === '-') expect(v, id).toBeUndefined()
      else if (want === 'pin') expect([id, v?.pinned, v?.opacity]).toEqual([id, true, 1])
      else if (want === 'ghost') expect([id, v?.ghost, v?.opacity]).toEqual([id, true, 0.25])
      else {
        expect(v, id).toBeDefined()
        expect(v?.pinned, id).toBe(false)
        expect(v?.opacity, id).toBeCloseTo(want, 3)
      }
    }
  }
})

test('T0: 31 visible, 9 hidden, r1.docs expires at NOW + 4 min, pinning reasons', () => {
  const view = visibleGraph(S, ATT, TTL_DEFAULTS, T0)
  expect(view.nodes.size).toBe(31)
  expect(view.hidden).toBe(9)
  expect(view.nodes.get('r1.docs')?.expiresAt).toBe(T0 + 4 * MIN)
  expect(view.nodes.get('r1.builder')?.attention).toBe(1)
  // r3 and r7 by attention: without it r7 is a plain desk at age 50 min (desk TTL 24 h) and r3 keeps only its live child.
  const bare = visibleGraph(S, new Map(), TTL_DEFAULTS, T0)
  expect(bare.nodes.get('r7')?.pinned).toBe(false)
  expect(bare.nodes.get('r3')?.pinned).toBe(true)
  expect(bare.nodes.get('r3.audit')?.pinned).toBe(true)
  // A live grandchild pins its expired grandparent: r5 (age 250 min > 240) through r5.migrator.
  expect(bare.nodes.get('r5')?.pinned).toBe(true)
})

test('fadeOpacity at 0, 0.75·ttl, 0.875·ttl, ttl', () => {
  expect(fadeOpacity(0, 100)).toBe(1)
  expect(fadeOpacity(75, 100)).toBe(1)
  expect(fadeOpacity(87.5, 100)).toBeCloseTo(0.625, 6)
  expect(fadeOpacity(100, 100)).toBe(0)
})

test('ghost: an expired parent stays at 0.25 while a child shows, hidden when none does', () => {
  const view = visibleGraph(S, ATT, TTL_DEFAULTS, T1)
  expect(view.nodes.get('r2.omp')).toMatchObject({ ghost: true, opacity: 0.25, pinned: false })
  expect(visibleGraph(S, ATT, TTL_DEFAULTS, T2).nodes.has('r2.omp')).toBe(false)
})

test('a peer shows only while an edge joins it to a shown session', () => {
  const nodes = new Map(S.nodes)
  nodes.delete('r7.reply')
  // Without r7 (no attention, last event 50 min ago on a 24 h desk TTL it is still shown) alice stays; drop r7 entirely:
  nodes.delete('r7')
  const lone: GraphState = { ...S, nodes }
  expect(visibleGraph(lone, ATT, TTL_DEFAULTS, T0).nodes.has('peer:alice')).toBe(false)
  expect(visibleGraph(S, ATT, TTL_DEFAULTS, T0).nodes.has('peer:alice')).toBe(true)
  // Its own root TTL runs out: 240 min after its last exchange.
  expect(visibleGraph(S, ATT, TTL_DEFAULTS, T0 + 200 * MIN).nodes.has('peer:alice')).toBe(false)
})

test('§9.3 edges at T0', () => {
  const view = visibleGraph(S, ATT, TTL_DEFAULTS, T0)
  const by = new Map(view.edges.map((e) => [e.edge.id, e]))
  const want = (ids: string[], active: boolean, animated: boolean) => {
    for (const id of ids) expect([id, by.get(id)?.active, by.get(id)?.animated]).toEqual([id, active, animated])
  }
  want(['r1.builder|r1.builder.linter', 'r3.audit|r3.audit.diff', 'r5.migrator|r5.migrator.check'], true, true)
  want(['r1|r1.builder', 'r1.builder|r1.builder.tests', 'r1.claude|r1.codex'], true, false)
  want(['r1|r1.docs', 'r1|r1.reviewer', 'r1.builder|r1.reviewer', 'r1|r2', 'r2|r2.repro', 'r2|r2.verify', 'r3|r3.sweep', 'r3|r6', 'peer:alice|r7', 'peer:mini-ops|r3'], false, false)
  expect(by.has('r1|r8')).toBe(false)
  expect(by.has('r4|r4.summarize')).toBe(false)
  expect(view.edges).toHaveLength(16)
})

test('awaitTarget: parent by name, "parent", session: id, a child, a sibling, unknown → null', () => {
  const nodes = S.nodes
  expect(awaitTarget(nodes.get('r1.builder.linter') as GraphNode, nodes)).toBe('r1.builder')
  expect(awaitTarget(nodes.get('r3.audit.diff') as GraphNode, nodes)).toBe('r3.audit')
  const with_ = (from: string, id = 'r1.builder.linter'): GraphNode => ({ ...(nodes.get(id) as GraphNode), activity: { ...(nodes.get(id) as GraphNode).activity, state: { state: 'awaiting', from } } })
  expect(awaitTarget(with_('parent'), nodes)).toBe('r1.builder')
  expect(awaitTarget(with_('session:r9'), nodes)).toBe('r9')
  expect(awaitTarget(with_('tests'), nodes)).toBe('r1.builder.tests')
  expect(awaitTarget(with_('linter', 'r1.builder'), nodes)).toBe('r1.builder.linter')
  expect(awaitTarget(with_('nobody'), nodes)).toBeNull()
  expect(awaitTarget(nodes.get('r1') as GraphNode, nodes)).toBeNull()
})

test('nextTtlTick: 5 s while something fades, else the next fade start, within [1 s, 60 s]', () => {
  const view = visibleGraph(S, ATT, TTL_DEFAULTS, T0)
  expect(nextTtlTick(S, view, TTL_DEFAULTS, T0)).toBe(5000)
  // Only pinned nodes and no edges: nothing to wait for → 60 s.
  const pinnedOnly: GraphState = { ...S, nodes: new Map([['r1', S.nodes.get('r1') as GraphNode]]), edges: new Map() }
  expect(nextTtlTick(pinnedOnly, visibleGraph(pinnedOnly, ATT, TTL_DEFAULTS, T0), TTL_DEFAULTS, T0)).toBe(60_000)
  // One fresh child: its fade starts 7.5 min after its last event → clamped to 60 s; 1 s before → 1 s.
  const fresh = { ...(S.nodes.get('r1.docs') as GraphNode), parent: null, last_event_ms: T0 }
  const one: GraphState = { ...S, nodes: new Map([[fresh.id, fresh]]), edges: new Map() }
  expect(nextTtlTick(one, visibleGraph(one, ATT, TTL_DEFAULTS, T0), TTL_DEFAULTS, T0)).toBe(60_000)
  const late = T0 + 7.5 * MIN - 500
  expect(nextTtlTick(one, visibleGraph(one, ATT, TTL_DEFAULTS, late), TTL_DEFAULTS, late)).toBe(1000)
})

test('windowMs: the largest TTL, at most 7 days', () => {
  expect(windowMs(TTL_DEFAULTS)).toBe(1440 * MIN)
  expect(windowMs({ ...TTL_DEFAULTS, desk_min: 20_000 })).toBe(7 * 1440 * MIN)
})
