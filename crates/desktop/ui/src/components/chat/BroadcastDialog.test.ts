import { expect, test } from 'vitest'
import type { BroadcastReceipt } from '@/rpc/bindings/BroadcastReceipt'
import type { BroadcastRow } from '@/rpc/bindings/BroadcastRow'
import type { SessionActivity } from '@/rpc/bindings/SessionActivity'
import type { AgentInfo } from '@/rpc/extra'
import { allRoots, groupRows, outcomeLines, runningRoots, summarize } from './broadcast'

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

test('outcome lines put failures first and fall back to the short id when the title is withheld', () => {
  const lines = outcomeLines(
    receipt([
      receiptRow('A', {}, { outcome: 'started' }),
      receiptRow('nameless', {}, { outcome: 'failed', error: 'unknown session' }),
      receiptRow('B', {}, { outcome: 'queued', input_id: 'i1', delivery: 'next_turn' }),
      receiptRow('C', {}, { outcome: 'skipped', reason: 'remote node' }),
    ]),
  )
  expect(lines).toEqual([
    'nameless — failed: unknown session',
    'T A — started',
    'T B — queued (next turn)',
    'T C — skipped: remote node',
  ])
})
