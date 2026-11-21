// 208 §10.2: graph ↔ list joins and the strip/node words.
import { expect, test } from 'vitest'
import { ATTENTION_FIXTURE } from '@/components/settings/attention-fixtures'
import type { GraphNode } from '@/rpc/bindings/method.agent.output'
import type { AttentionItem } from '@/rpc/bindings/method.roster.output'
import { GRAPH_FIXTURE, GRAPH_FIXTURE_ATTENTION as ATT, GRAPH_FIXTURE_NOW as T0 } from './fixture'
import { fromSnapshot } from './reducer'
import { attentionBySession, includeSessions, nodeLabel, pathToRoot, statusText, stripCounts, stripText, subtreeIds } from './selectors'
import { TTL_DEFAULTS, visibleGraph } from './ttl'

const S = fromSnapshot(GRAPH_FIXTURE)
const node = (id: string): GraphNode => S.nodes.get(id) as GraphNode
const item = (session: string | null, expires_ms: number | null = null, i = 0): AttentionItem => ({ ...(ATTENTION_FIXTURE[i] as AttentionItem), session, expires_ms } as AttentionItem)

test('pathToRoot and subtreeIds', () => {
  expect(pathToRoot(S.nodes, 'r1.builder.linter')).toEqual(['r1.builder.linter', 'r1.builder', 'r1'])
  expect(pathToRoot(S.nodes, 'nope')).toEqual([])
  expect([...subtreeIds(S.nodes, 'r3')].sort()).toEqual(['r3', 'r3.audit', 'r3.audit.diff', 'r3.audit.grep', 'r3.rotate', 'r3.sweep'])
  expect(subtreeIds(S.nodes, 'nope').size).toBe(0)
})

test('stripCounts at T0 and its text', () => {
  const c = stripCounts(visibleGraph(S, ATT, TTL_DEFAULTS, T0))
  expect(c).toEqual({ needYou: 4, working: 11, finished: 14 })
  expect(stripText(c)).toBe('4 need you · 11 working · 14 finished')
  expect(stripText({ needYou: 0, working: 0, finished: 0 })).toBe('0 need you · 0 working · 0 finished')
})

test('attentionBySession skips expired items and items without a session; includeSessions unique, sorted, capped', () => {
  const now = 1000
  const items = [item('b'), item('a'), item('b'), item(null), item('gone', now), item('later', now + 1)]
  expect(Object.fromEntries(attentionBySession(items, now))).toEqual({ b: 2, a: 1, later: 1 })
  expect(includeSessions(items, now)).toEqual(['a', 'b', 'later'])
  const many = Array.from({ length: 250 }, (_, i) => item(`s${String(i).padStart(3, '0')}`))
  const inc = includeSessions(many, now)
  expect(inc).toHaveLength(200)
  expect(inc[0]).toBe('s000')
  expect(inc[199]).toBe('s199')
})

test('nodeLabel: titles for roots, desks and peers; names for children', () => {
  expect(nodeLabel(node('r1'))).toBe('Ship 2.5 release')
  expect(nodeLabel(node('r3'))).toBe('ops desk')
  expect(nodeLabel(node('peer:alice'))).toBe('alice')
  expect(nodeLabel({ ...node('r1.claude'), title: 'Some title' })).toBe('impl')
})

test('statusText for each state, wait and outcome', () => {
  expect(statusText(node('r1.builder'))).toBe('approval: bash')
  expect(statusText(node('r1.builder.linter'))).toBe('awaiting builder')
  expect(statusText(node('r3.sweep'))).toBe('waiting on you')
  expect(statusText(node('r1.builder.tests'))).toBe('waiting on a job')
  expect(statusText(node('r5.migrator'))).toBe('waiting on children')
  expect(statusText(node('r1'))).toBe('running')
  expect(statusText(node('r1.codex'))).toBe('error')
  expect(statusText(node('r1.docs'))).toBe('finished')
  expect(statusText(node('r1.reviewer'))).toBe('failed')
  expect(statusText(node('r2.bisect'))).toBe('cancelled')
  expect(statusText(node('r2.verify'))).toBe('timed out')
  expect(statusText(node('peer:alice'))).toBe('idle')
})
