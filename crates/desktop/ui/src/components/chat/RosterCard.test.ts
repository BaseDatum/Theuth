import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type { ChildCost } from '@/rpc/bindings/ChildCost'
import type { ObservedMeta } from '@/rpc/bindings/ObservedMeta'
import type { AgentInfo } from '@/rpc/extra'
import { RosterRows, rosterCost, rosterFooter, rosterRows } from './RosterCard'

vi.mock('@tanstack/react-router', () => ({
  Link: ({ children, params, ...rest }: { children: unknown; params: { session: string }; title?: string }) => createElement('a', { href: `/c/${params.session}`, title: rest.title }, children as never),
}))

const meta = (over: Partial<ObservedMeta> = {}): ObservedMeta => ({
  source: 'omp_task',
  external: 'x',
  state: 'running',
  can_cancel: false,
  cost_rule: 'counted',
  tool_call: 'toolu_1',
  detail: null,
  ...over,
})
const row = (id: string, observed: ObservedMeta, extra: Partial<AgentInfo> = {}): AgentInfo => ({
  id,
  name: id,
  parent: 'harness',
  depth: 1,
  agent: null,
  model: 'm',
  state: { state: 'running' },
  started_at: null,
  finished_at: null,
  last: '',
  kind: 'observed',
  observed,
  ...extra,
})
const money = (n: string): ChildCost['total'] => ({
  returned: [{ currency: 'USD', nanos: n }],
  estimated_for_unavailable: [],
  calls: 1,
  dispatched: 1,
  unavailable: 0,
  unresolved: 0,
  tokens: { input: 0, output: 0, cache_read: 0, cache_write: 0, unknown_calls: 0 },
})

test('rosterRows picks the children of one spawn call in the harness’s order; the footer counts their states', () => {
  const b = row('ReadB', meta({ state: 'completed', detail: { index: 1 } }))
  const a = row('ReadA', meta({ detail: { index: 0 } }))
  const other = row('Other', meta({ tool_call: 'toolu_2' }))
  const native = row('native', meta(), { observed: undefined, kind: 'chat' })
  const rows = rosterRows({ b, a, other, native }, 'toolu_1')
  expect(rows.map((r) => r.id)).toEqual(['ReadA', 'ReadB'])
  expect(rosterFooter(rows)).toBe('2 subagents · 1 running · 1 done')
  expect(rosterFooter([row('x', meta({ state: 'failed' }))])).toBe('1 subagent · 1 failed')
  expect(rosterRows({ b }, 'toolu_9')).toEqual([])
})

test('the cost cell follows the rule: counted → the meter’s money (else the harness’s), inside_parent → greyed, unreported → —', () => {
  const metered: ChildCost = { session: 'ReadA', name: 'ReadA', deleted: false, total: money('121280250'), scope: 'observed' }
  expect(rosterCost(row('ReadA', meta({ detail: { cost: 0.11 } })), metered)).toMatchObject({ text: '$0.121', grey: false })
  expect(rosterCost(row('ReadA', meta({ detail: { cost: 0.11 } })), undefined)).toMatchObject({ text: '$0.110', grey: false })
  expect(rosterCost(row('ReadA', meta({ cost_rule: 'inside_parent', detail: { cost: 0.05 } })), undefined)).toMatchObject({ text: '$0.050', grey: true, title: expect.stringMatching(/inside parent/) })
  expect(rosterCost(row('oc', meta({ source: 'opencode_ext', cost_rule: 'unreported' })), undefined)).toMatchObject({ text: '—', grey: true })
  expect(rosterCost(row('rfd', meta({ source: 'rfd', cost_rule: 'display_only' })), undefined)).toMatchObject({ text: '—', title: expect.stringMatching(/not counted/) })
})

test('RosterRows renders one link per child with agent, activity, tools, tokens, cost and the Q9 progress bar when present', () => {
  const rows = [
    row('ReadA', meta({ detail: { index: 0, agent: 'scout', lastIntent: 'Reading first line', toolCount: 2, tokens: 9118, cost: 0.12128025, completionPercent: 40 } })),
    row('ReadB', meta({ state: 'completed', description: 'Read b.txt', detail: { index: 1 } })),
  ]
  const html = renderToStaticMarkup(createElement(RosterRows, { rows }))
  expect([...html.matchAll(/href="([^"]+)"/g)].map((m) => m[1])).toEqual(['/c/ReadA', '/c/ReadB'])
  expect(html).toContain('scout · Reading first line')
  expect(html).toContain('2 tools · 9.1k tok')
  expect(html).toContain('$0.121')
  expect(html).toContain('role="progressbar"')
  expect(html).toContain('aria-valuenow="40"')
  // The finished row: a check, its description, no progress bar of its own.
  expect(html).toContain('Read b.txt')
  expect(html).toContain('finished')
  expect(html.match(/role="progressbar"/g)).toHaveLength(1)
})
