import { expect, test } from 'vitest'
import { live } from '@/components/sidebar/AgentTree'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import type { AgentInfo } from '@/rpc/extra'
import { TREE_CAP, cappedTree, rootOf, subtrees } from './session-tree'

const s = (id: string, parent: string | null = null, desk?: string): SessionSummary => ({
  id,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
  title: id,
  tags: [],
  model: 'm',
  parent,
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
  ...(desk === undefined ? {} : { desk }),
})

test('subtrees roll every depth up to the top-level row; a root without subagents has no entry', () => {
  const sessions = [s('a'), s('b'), s('a1', 'a'), s('a11', 'a1')]
  const subs = subtrees(sessions)
  expect(subs.get('a')?.map((d) => d.id)).toEqual(['a1', 'a11'])
  expect(subs.has('b')).toBe(false)
  expect(subs.has('a1')).toBe(false)
})

test('a subagent whose ancestor is a desk rolls up to the desk, at any depth, and the desk is its root', () => {
  const sessions = [s('desk', null, 'theuth'), s('work', 'desk'), s('helper', 'work'), s('chat'), s('chat1', 'chat')]
  const subs = subtrees(sessions)
  expect(subs.get('desk')?.map((d) => d.id)).toEqual(['work', 'helper'])
  expect(subs.get('chat')?.map((d) => d.id)).toEqual(['chat1'])
  expect(rootOf(sessions, 'helper')).toBe('desk')
  expect(rootOf(sessions, 'desk')).toBe('desk')
  expect(rootOf(sessions, 'unknown')).toBe('unknown')
})

test('live subagent rows are the ones still working or waiting on someone; finished states are not', () => {
  expect((['running', 'awaiting', 'approval'] as const).map(live)).toEqual([true, true, true])
  expect((['idle', 'cancelled', 'error'] as const).map(live)).toEqual([false, false, false])
})

test('a capped tree shows running rows first, then newest by updated_at, five at a time; expanded shows all and hidden is what Show more reveals', () => {
  const row = (id: string, state: AgentInfo['state']['state'], started_at: string | null = null): AgentInfo => ({
    id,
    name: id,
    parent: 'desk',
    depth: 1,
    agent: null,
    model: 'm',
    state: { state } as AgentInfo['state'],
    started_at,
    finished_at: null,
    last: '',
  })
  const at = (id: string, day: number) => ({ ...s(id, 'desk'), updated_at: `2026-01-${String(day).padStart(2, '0')}T00:00:00Z` })
  // Seven rows: two running (old and new), five finished with distinct days, one of them unknown to the list.
  const rows = [row('f3', 'idle'), row('r-old', 'running'), row('f1', 'error'), row('f5', 'cancelled'), row('r-new', 'running'), row('f2', 'idle'), row('f4', 'idle', '2026-01-04T12:00:00Z')]
  const sessions = [at('f1', 1), at('f2', 2), at('f3', 3), at('f5', 5), at('r-old', 1), at('r-new', 9)]
  const folded = cappedTree(rows, sessions, false)
  expect(folded.shown.map((r) => r.id)).toEqual(['r-new', 'r-old', 'f5', 'f4', 'f3'])
  expect(folded.hidden).toBe(2)
  const open = cappedTree(rows, sessions, true)
  expect(open.shown.map((r) => r.id)).toEqual(['r-new', 'r-old', 'f5', 'f4', 'f3', 'f2', 'f1'])
  expect(open.hidden).toBe(0)
  // At or under the cap nothing is hidden and the order still holds.
  expect(cappedTree(rows.slice(0, TREE_CAP), sessions, false)).toEqual({ shown: [rows[4], rows[1], rows[3], rows[0], rows[2]], hidden: 0 })
})
