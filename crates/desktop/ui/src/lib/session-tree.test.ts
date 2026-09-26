import { expect, test } from 'vitest'
import { live } from '@/components/sidebar/AgentTree'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import { rootOf, subtrees } from './session-tree'

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
