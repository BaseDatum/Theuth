import { expect, test } from 'vitest'
import type { BroadcastReceipt } from '@/rpc/bindings/BroadcastReceipt'
import type { BroadcastRow } from '@/rpc/bindings/BroadcastRow'
import type { SessionActivity } from '@/rpc/bindings/SessionActivity'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import type { AgentInfo } from '@/rpc/extra'
import { allRoots, deskRoots, groupRows, outcomeLines, runningRoots, sectionState, splitGroups, summarize, title } from './broadcast'

const activity = (p: Partial<SessionActivity> = {}): SessionActivity => ({
  state: { state: 'idle' },
  last_active: null,
  turn_started_at: null,
  jobs: 0,
  ptys: 0,
  children: 0,
  asks: null,
  live: true,
  ...p,
})

const row = (id: string, parent: string, depth: number, a: Partial<SessionActivity> = {}): AgentInfo => ({
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
})

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

const summary = (id: string, over: Partial<SessionSummary> = {}): SessionSummary => ({
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
  retention: 'durable',
  running: false,
  owner: '',
  epoch: 1,
  here: true,
  ...over,
})

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

// B is the scout's desk, D is theuth's; a1 (A's child) carries a desk id by mistake and C has an empty one.
const deskSessions = [
  summary('B', { desk: 'scout' }),
  summary('a1', { desk: 'stray', parent: 'A' }),
  summary('C', { desk: '' }),
  summary('D', { desk: 'theuth' }),
]

test('"All desks" picks only roots whose session has a desk, theuth first; a desk child is not a desk root', () => {
  expect(deskRoots(rows, deskSessions)).toEqual(['D', 'B'])
  expect(deskRoots(rows, [])).toEqual([])
  expect(deskRoots(rows, undefined)).toEqual([])
})

test('the picker splits groups by the root\'s desk: desks (theuth first), then the rest in order', () => {
  const { desks, rest } = splitGroups(groupRows(rows), deskSessions)
  expect(desks.map((g) => [g.root.id, g.children.map((c) => c.id)])).toEqual([
    ['D', []],
    ['B', ['b1']],
  ])
  expect(rest.map((g) => g.root.id)).toEqual(['A', 'C'])
})

test('a section header is all / some / none by its roots alone; an empty section is none', () => {
  expect(sectionState(new Set(['B', 'D', 'A']), ['D', 'B'])).toBe('all')
  expect(sectionState(new Set(['B', 'A']), ['D', 'B'])).toBe('some')
  expect(sectionState(new Set(['A']), ['D', 'B'])).toBe('none')
  expect(sectionState(new Set(['A']), [])).toBe('none')
})
