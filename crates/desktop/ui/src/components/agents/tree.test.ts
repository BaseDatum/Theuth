import { expect, test } from 'vitest'
import type { AgentSummary } from '@/rpc/bindings/AgentSummary'
import { type Node, budgetUsed, tree } from './tree'

const s = (name: string, reports_to: string, extra: Partial<AgentSummary> = {}): AgentSummary => ({
  file: { name, title: name, reports_to, model: null, effort: null, tools: null, mcp: [], budget: { type: 'none' }, capabilities: null, paused: false, proposed_by: null, body: 'x' },
  status: 'idle', desk: null, open_tasks: 0, spent_month: 0, ...extra,
})

const names = (n: Node): unknown => [n.name, n.children.map(names)]

test('tree nests by reports_to under theuth, sorted', () => {
  const t = tree([s('writer', 'ops'), s('ops', 'theuth'), s('research', 'theuth')])
  expect(names(t)).toEqual(['theuth', [['ops', [['writer', []]]], ['research', []]]])
})

test('unknown manager and cycles fall back to the root', () => {
  const t = tree([s('a', 'ghost'), s('b', 'c'), s('c', 'b')])
  expect(t.children.map((c) => c.name)).toEqual(['a', 'b', 'c'])
  expect(t.children.every((c) => c.children.length === 0)).toBe(true)
})

test('the built-in theuth on the roster is the root, never its own child', () => {
  const t = tree([s('theuth', ''), s('ops', 'theuth')])
  expect(names(t)).toEqual(['theuth', [['ops', []]]])
  expect(t.agent?.file.name).toBe('theuth')
})

test('budget fraction is capped and absent for none', () => {
  expect(budgetUsed(s('a', 'theuth'))).toBeNull()
  expect(budgetUsed(s('a', 'theuth', { spent_month: 20, file: { ...s('a', 'theuth').file, budget: { type: 'monthly', amount: 40, currency: 'USD' } } }))).toBe(0.5)
  expect(budgetUsed(s('a', 'theuth', { spent_month: 99, file: { ...s('a', 'theuth').file, budget: { type: 'monthly', amount: 40, currency: 'USD' } } }))).toBe(1)
})
