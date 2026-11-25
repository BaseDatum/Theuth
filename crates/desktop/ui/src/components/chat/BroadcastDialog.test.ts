import { receiveComponent } from '@/rpc/bindings/receive'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import { queryKeys } from '@/lib/rpc'
import { useConnection } from '@/store/connection'
import type { BroadcastReceipt } from "@/rpc/bindings/method.agent.output"
import type { BroadcastRow } from "@/rpc/bindings/method.agent.output"
import type { SessionActivity } from "@/rpc/bindings/method.agent.output"
import type { SessionSummary } from "@/rpc/bindings/method.agent.output"
import type { AgentInfo } from '@/store/ui-events'
import type { AgentSummary } from '@/rpc/bindings/method.roster.output'
import { deskList, deskSessions } from '@/lib/desks'
import { activeDescendants, allRoots, deskRoots, groupRows, outcomeLines, runningRoots, sectionState, splitGroups, summarize, title } from './broadcast'
import { Form } from './BroadcastDialog'

vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => ({}) }))

const activity = (p: Partial<SessionActivity> = {}): SessionActivity => (receiveComponent("method.agent.output.SessionActivity", {
  state: { state: 'idle' },
  last_active: null,
  turn_started_at: null,
  jobs: 0,
  ptys: 0,
  children: 0,
  asks: null,
  live: true,
  ...p,
}))

const row = (id: string, parent: string, depth: number, a: Partial<SessionActivity> = {}): AgentInfo => (receiveComponent("method.agent.output.AgentInfo", {
  id,
  name: id,
  parent,
  depth,
  agent: null,
  model: 'm',
  state: a.state ?? { state: 'idle' },
  started_at: null,
  finished_at: null,
  last: '',
  activity: activity(a),
}))

// Roots: A runs, B idle with a PTY, C idle and jobless, D awaiting. Children: a1 under A (running), a11 under a1, b1 under B.
const rows = [
  row('A', 'A', 0, { state: { state: 'running' } }),
  row('a1', 'A', 1, { state: { state: 'running' } }),
  row('a11', 'a1', 2),
  row('B', 'B', 0, { ptys: 1 }),
  row('b1', 'B', 1),
  row('C', 'C', 0),
  row('D', 'D', 0, { state: { state: 'awaiting', from: 'x' } }),
]

test('"Running roots" ticks busy roots and roots with a running PTY, never children or idle roots', () => {
  expect(runningRoots(rows)).toEqual(['A', 'B', 'D'])
})

test('"Running roots" ticks an idle root whose subagent works, at any depth; idle/finished subagents and non-roots never', () => {
  const tree = [
    row('R', 'R', 0, { state: { state: 'running' } }), // running itself
    row('P', 'P', 0), // idle, child running
    row('p1', 'P', 1, { state: { state: 'running' } }),
    row('p11', 'p1', 2, { state: { state: 'running' } }),
    row('G', 'G', 0), // idle, grandchild owns a PTY under an idle child
    row('g1', 'G', 1),
    row('g11', 'g1', 2, { ptys: 1 }),
    row('Q', 'Q', 0), // idle, children idle or finished
    row('q1', 'Q', 1),
    row('q2', 'Q', 1, { state: { state: 'cancelled' } }),
    row('S', 'S', 0), // idle, grandchild approval-blocked
    row('s1', 'S', 1, { state: { state: 'cancelled' } }),
    row('s11', 's1', 2, { state: { state: 'approval', tool: 't' } }),
    row('o1', 'gone', 1, { state: { state: 'running' } }), // orphan subagent: not a root, never ticked
  ]
  expect(runningRoots(tree)).toEqual(['R', 'P', 'G', 'S'])
  expect([...activeDescendants(tree)]).toEqual([
    ['P', 2],
    ['G', 1],
    ['S', 1],
  ])
})

test('"All roots" ticks every root, never children', () => {
  expect(allRoots(rows)).toEqual(['A', 'B', 'C', 'D'])
})

test('rows group under their root through any depth; an orphan stands alone', () => {
  const groups = groupRows([...rows, row('z1', 'Z', 1)])
  expect(groups.map((g) => [g.root.id, g.children.map((c) => c.id)])).toEqual([
    ['A', ['a1', 'a11']],
    ['B', ['b1']],
    ['C', []],
    ['D', []],
    ['z1', []],
  ])
})

const receiptRow = (session: string, a: Partial<SessionActivity>, outcome: BroadcastRow['outcome']): BroadcastRow => ({
  session,
  title: session === 'nameless' ? '' : `T ${session}`,
  parent: null,
  before: activity(a),
  outcome,
})

const receipt = (rows: BroadcastRow[]): BroadcastReceipt => ({ id: '0', at: '2026-09-22T00:00:00Z', rows })

test('dry-run label counts busy by state only: a PTY-owning idle root starts now', () => {
  const r = receipt([
    receiptRow('A', { state: { state: 'running' } }, { outcome: 'skipped', reason: 'dry run' }),
    receiptRow('B', { ptys: 1 }, { outcome: 'skipped', reason: 'dry run' }),
    receiptRow('D', { state: { state: 'approval', tool: 't' } }, { outcome: 'skipped', reason: 'dry run' }),
  ])
  expect(summarize(r)).toBe('3 sessions · 2 busy → queued, 1 idle → start now')
  expect(summarize(receipt([r.rows[0]!]))).toBe('1 session · 1 busy → queued, 0 idle → start now')
})

const summary = (id: string, over: Partial<SessionSummary> = {}): SessionSummary => (receiveComponent("method.agent.output.SessionSummary", {
  id,
  created_at: new Date(2026, 8, 18, 14, 5).toISOString(),
  updated_at: new Date(2026, 8, 19).toISOString(),
  title: '',
  tags: [],
  model: 'm',
  parent: null,
  name: null,
  effort: null,
  agent: null,
  cwd: null,
  pinned: false,
  archived: false,
  project: null,
  notebook: null,
  retention: 'durable',
  running: false,
  owner: '',
  epoch: 1,
  memory: 'epochs',
  here: true,
  ...over,
}))

const ID = '0192a4c1-7e3b-7c0e-9c1f-2b6d1e8f4a10'

test('a recipient is named by the session list: title, else the engine-seeded `Untitled chat · date`; a subagent by its peer name; never the id', () => {
  expect(title(ID, [summary(ID, { title: 'Fix relay reconnect loop' })])).toBe('Fix relay reconnect loop')
  expect(title(ID, [summary(ID)])).toBe('Untitled chat · Sep 18')
  expect(title(ID, [summary(ID, { title: 'Ignored', name: 'Scout', parent: 'p' })])).toBe('Scout')
  // The list does not know it yet: the caller's own label, else the generic name.
  expect(title(ID, undefined, 'Scout')).toBe('Scout')
  expect(title(ID, [])).toBe('Untitled chat')
  for (const sessions of [[summary(ID)], undefined]) expect(title(ID, sessions)).not.toContain(ID.slice(0, 8))
})

test('outcome lines put failures first and name rows like the picker: the list, else the receipt title, else `Untitled chat`', () => {
  const lines = outcomeLines(
    receipt([
      receiptRow('A', {}, { outcome: 'started' }),
      receiptRow('nameless', {}, { outcome: 'failed', error: 'unknown session' }),
      receiptRow('B', {}, { outcome: 'queued', input_id: 'i1', delivery: 'next_turn' }),
      receiptRow('C', {}, { outcome: 'skipped', reason: 'remote node' }),
    ]),
    [summary('A', { title: 'Listed A' })],
  )
  expect(lines).toEqual([
    'Untitled chat — failed: unknown session',
    'Listed A — started',
    'T B — queued (next turn)',
    'T C — skipped: remote node',
  ])
})

// The roster: scout's current desk is B, theuth's is D; maya has no desk yet; ghost is only proposed. Every session
// that ever was a desk keeps `SessionSummary.desk`: C is theuth's earlier desk, a1 carries one by mistake, and E (a
// root added below) is the desk of seed-1, an agent gone from the roster. Only the roster's current desks count.
const agent = (name: string, desk: string | null, status = 'idle') => ({ desk, status, file: { name } }) as unknown as AgentSummary
const ROSTER = [agent('scout', 'B'), agent('theuth', 'D', 'working'), agent('maya', null), agent('ghost', 'G', 'proposed')]
const DESKS = deskSessions(deskList(ROSTER))
const deskSummaries = [
  summary('B', { desk: 'scout' }),
  summary('a1', { desk: 'stray', parent: 'A' }),
  summary('C', { desk: 'theuth' }),
  summary('D', { desk: 'theuth' }),
  summary('E', { desk: 'seed-1' }),
]
const withStale = [...rows, row('E', 'E', 0), row('G', 'G', 0)]

test('the desks are the sidebar\'s: one per active roster agent, at its current desk session, theuth first', () => {
  expect(deskList(ROSTER).map((d) => [d.name, d.id])).toEqual([['theuth', 'D'], ['scout', 'B'], ['maya', null]])
  expect([...DESKS]).toEqual([['D', 'theuth'], ['B', 'scout']])
})

test('"All desks" picks only current desks: an agent\'s older desk, a removed agent\'s desk and a proposed one are not', () => {
  expect(deskRoots(withStale, DESKS)).toEqual(['D', 'B'])
  expect(deskRoots(withStale, new Map())).toEqual([])
})

test('the picker splits groups by the roster\'s desks (the sidebar\'s order); stale desk sessions list under Sessions', () => {
  const { desks, rest } = splitGroups(groupRows(withStale), DESKS)
  expect(desks.map((g) => [g.root.id, g.children.map((c) => c.id)])).toEqual([
    ['D', []],
    ['B', ['b1']],
  ])
  expect(rest.map((g) => g.root.id)).toEqual(['A', 'C', 'E', 'G'])
})

test('the desks preset: the Desks section and "Send to N" count the roster\'s desks only, one row per agent', () => {
  useConnection.setState({ discovery: { ...useConnection.getState().discovery, phase: 'legacy' } })
  const qc = new QueryClient()
  qc.setQueryData(['activity'], withStale)
  qc.setQueryData(queryKeys.sessions, deskSummaries)
  qc.setQueryData(queryKeys.roster, ROSTER)
  const html = renderToStaticMarkup(createElement(QueryClientProvider, { client: qc }, createElement(Form, { req: { text: 'hi', preset: 'desks' }, onDone: () => {} })))
  expect(html.match(/>Send to \d+ sessions?</g)).toEqual(['>Send to 2 sessions<'])
  const desks = html.slice(html.indexOf('aria-label="Desks"'), html.lastIndexOf('<input', html.indexOf('aria-label="All sessions"')))
  // Two desk rows and scout's subagent b1 under its desk.
  expect(desks.match(/type="checkbox"/g)).toHaveLength(3)
  expect(desks.match(/>(theuth|scout|seed-1|b1)</g)).toEqual(['>theuth<', '>scout<', '>b1<'])
  // The Desks header reads 2 of 2; the Sessions header 0 of its roots (A, C, E, G).
  const headers = [...html.matchAll(/tabular-nums text-fg-tertiary">(.*?)<\/span>/g)].map((m) => m[1]!.replace(/<!-- -->/g, ''))
  expect(headers.filter((t) => / of /.test(t))).toEqual(['2 of 2', '0 of 4'])
})

test('a section header is all / some / none by its roots alone; an empty section is none', () => {
  expect(sectionState(new Set(['B', 'D', 'A']), ['D', 'B'])).toBe('all')
  expect(sectionState(new Set(['B', 'A']), ['D', 'B'])).toBe('some')
  expect(sectionState(new Set(['A']), ['D', 'B'])).toBe('none')
  expect(sectionState(new Set(['A']), [])).toBe('none')
})

test('broadcast-top: message, one Send button and the preview line render above the first target row', () => {
  useConnection.setState({ discovery: { ...useConnection.getState().discovery, phase: 'legacy' } })
  const qc = new QueryClient()
  qc.setQueryData(['activity'], rows)
  qc.setQueryData(queryKeys.sessions, deskSummaries)
  qc.setQueryData(queryKeys.roster, ROSTER)
  const html = renderToStaticMarkup(createElement(QueryClientProvider, { client: qc }, createElement(Form, { req: { text: 'hi' }, onDone: () => {} })))
  const firstRow = html.indexOf('type="checkbox"')
  expect(firstRow).toBeGreaterThan(0)
  expect(html.match(/>Send to \d+ sessions?</g)).toEqual(['>Send to 3 sessions<'])
  // The untouched "running" preset ticks A, B and D; the dry run has not answered yet.
  for (const part of ['<textarea', '>Send to 3 sessions<', '>Previewing…<']) expect(html.indexOf(part), part).toBeLessThan(firstRow)
  expect(html.indexOf('<textarea')).toBeLessThan(html.indexOf('>Send to'))
  expect(html.indexOf('>Send to')).toBeLessThan(html.indexOf('>Previewing…<'))
})
