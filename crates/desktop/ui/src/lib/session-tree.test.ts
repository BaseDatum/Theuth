import { Fragment, createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import { Branch } from '@/components/sidebar/AgentTree'
import { groupRows } from '@/components/chat/broadcast'
import { buildRows } from '@/components/sidebar/SessionList'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import type { SessionActivity } from '@/rpc/bindings/SessionActivity'
import type { AgentInfo } from '@/rpc/extra'
import { STRIP_CAP, TREE_CAP, cappedTree, childrenOf, live, rankSummary, stripRows, withAncestors, rootOf, subtrees } from './session-tree'

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

const row = (id: string, state: AgentInfo['state']['state'], parent = 'root', extra: Partial<AgentInfo> = {}): AgentInfo => ({
  id,
  name: id,
  parent,
  depth: parent === 'root' ? 1 : 2,
  agent: null,
  model: 'm',
  state: { state } as AgentInfo['state'],
  started_at: null,
  finished_at: null,
  last: '',
  ...extra,
})
const day = (n: number) => `2026-01-${String(n).padStart(2, '0')}T00:00:00Z`
const at = (id: string, updated: number, created = 1) => ({ ...s(id, 'root'), updated_at: day(updated), created_at: day(created) })

test('rank: live rows (running, awaiting, approval) beat a newer idle one; idle rows go newest activity first; created_at breaks ties', () => {
  const sessions = [at('idle-new', 9), at('run-old', 1), at('wait', 2), at('appr', 3), at('idle-old', 4), at('tie-a', 5, 1), at('tie-b', 5, 2)]
  const rows = [row('idle-new', 'idle'), row('run-old', 'running'), row('wait', 'awaiting'), row('appr', 'approval'), row('idle-old', 'idle'), row('tie-a', 'idle'), row('tie-b', 'idle')]
  expect(cappedTree(rows, sessions, true).shown.map((r) => r.id)).toEqual(['appr', 'wait', 'run-old', 'idle-new', 'tie-b', 'tie-a', 'idle-old'])
})

test('rank: activity is the newest of activity.last_active, the list updated_at and the live started_at/finished_at', () => {
  const sessions = [at('a', 2), at('b', 3), at('c', 1)]
  const activity: SessionActivity = { state: { state: 'idle' }, last_active: day(8), turn_started_at: null, jobs: 0, ptys: 0, children: 0, asks: null, live: false }
  const rows = [row('a', 'idle', 'root', { activity }), row('b', 'idle'), row('c', 'idle', 'root', { finished_at: day(5) }), row('d', 'idle', 'root', { started_at: day(4) })]
  // a: last_active 8 beats updated_at 2; c: finished_at 5 beats updated_at 1; d is unknown to the list, ranks by started_at.
  expect(cappedTree(rows, sessions, true).shown.map((r) => r.id)).toEqual(['a', 'c', 'd', 'b'])
  expect(rankSummary({ ...at('x', 3, 2), running: true })).toEqual({ live: true, at: Date.parse(day(3)), created: Date.parse(day(2)) })
})

test('cap: five shown and hidden = the rest; a kept row (the open chat) outside the five is appended; expanded shows all', () => {
  const rows = Array.from({ length: 7 }, (_, i) => row(`f${i + 1}`, 'idle'))
  const sessions = rows.map((r, i) => at(r.id, i + 1))
  const folded = cappedTree(rows, sessions, false)
  expect(folded.shown.map((r) => r.id)).toEqual(['f7', 'f6', 'f5', 'f4', 'f3'])
  expect(folded.hidden).toBe(2)
  const kept = cappedTree(rows, sessions, false, new Set(['f1']))
  expect(kept.shown.map((r) => r.id)).toEqual(['f7', 'f6', 'f5', 'f4', 'f3', 'f1'])
  expect(kept.hidden).toBe(1)
  expect(cappedTree(rows, sessions, true, new Set(['f1']))).toEqual({ shown: [...rows].reverse(), hidden: 0 })
  expect(cappedTree(rows.slice(0, TREE_CAP), sessions, false).hidden).toBe(0)
})

test('childrenOf keys rows by parent; withAncestors adds every ancestor found in the subtree', () => {
  const rows = [row('a', 'idle'), row('a1', 'idle', 'a'), row('a11', 'idle', 'a1'), row('b', 'idle')]
  const by = childrenOf(rows)
  expect([...by.keys()]).toEqual(['root', 'a', 'a1'])
  expect(by.get('root')?.map((r) => r.id)).toEqual(['a', 'b'])
  const byId = Object.fromEntries(rows.map((r) => [r.id, r]))
  expect([...withAncestors(byId, ['a11'])]).toEqual(['a11', 'a1', 'a'])
  expect([...withAncestors(byId, ['b', 'a1'])]).toEqual(['b', 'a1', 'a'])
  expect(withAncestors(byId, []).size).toBe(0)
})

test('a Branch renders five rows and "Show more (2)" for a parent with seven children; the open chat and its parent stay visible', () => {
  const rows = [...Array.from({ length: 7 }, (_, i) => row(`f${i + 1}`, 'idle')), row('g', 'idle', 'f1')]
  const sessions = rows.map((r, i) => at(r.id, i + 1))
  const html = (keep?: Set<string>) =>
    renderToStaticMarkup(
      createElement(Branch, {
        parent: 'root',
        byParent: childrenOf(rows),
        sessions,
        keep,
        unit: 12,
        moreClass: 'more',
        render: (r, sub) => createElement(Fragment, null, createElement('a', { 'data-row': r.id }), sub),
      }),
    )
  const ids = (h: string) => [...h.matchAll(/data-row="([^"]+)"/g)].map((m) => m[1])
  const folded = html()
  expect(ids(folded)).toEqual(['f7', 'f6', 'f5', 'f4', 'f3'])
  expect(folded).toContain('>Show more (2)</button>')
  // The open chat is `g`, a grandchild: `f1` (its parent) is appended past the cap and `g` shows under it.
  const kept = html(new Set(['g', 'f1']))
  expect(ids(kept)).toEqual(['f7', 'f6', 'f5', 'f4', 'f3', 'f1', 'g'])
  expect(kept).toContain('>Show more (1)</button>')
})

test('93 stripRows: live rows only, in spawn order (started_at, unknown last, then name), 24 dots and a count of the rest', () => {
  const children = Object.fromEntries(
    [
      row('late', 'running', 'root', { started_at: day(3) }),
      row('done', 'idle', 'root', { started_at: day(1) }),
      row('early', 'awaiting', 'root', { started_at: day(2), state: { state: 'awaiting', from: 'x' } }),
      row('b-unknown', 'approval', 'kid', { state: { state: 'approval', tool: 'bash' } }),
      row('a-unknown', 'running', 'kid'),
      row('failed', 'error', 'root', { started_at: day(1) }),
    ].map((r) => [r.id, r]),
  )
  expect(stripRows(children).shown.map((r) => r.id)).toEqual(['early', 'late', 'a-unknown', 'b-unknown'])
  expect(stripRows(children).more).toBe(0)
  expect(stripRows({})).toEqual({ shown: [], more: 0 })
  const many = Object.fromEntries(Array.from({ length: 30 }, (_, i) => [`r${i}`, row(`r${i}`, 'running', 'root', { started_at: day(1 + (i % 28)) })]))
  const { shown, more } = stripRows(many)
  expect([shown.length, more]).toEqual([STRIP_CAP, 6])
  expect(shown[0]?.id).toBe('r0')
  expect(stripRows(many, 3)).toMatchObject({ more: 27 })
})

test('125 §8.5: an advisor child is never a strip dot, a tree row, a subtree member or a broadcast target', () => {
  const adv = row('advisor', 'running', 'root', { kind: 'advisor' })
  const worker = row('worker', 'running')
  expect(stripRows({ worker, adv }).shown.map((r) => r.id)).toEqual(['worker'])
  expect(childrenOf([worker, adv]).get('root')?.map((r) => r.id)).toEqual(['worker'])
  const subs = subtrees([s('root'), { ...s('worker', 'root') }, { ...s('advisor', 'root'), kind: 'advisor' }])
  expect(subs.get('root')?.map((d) => d.id)).toEqual(['worker'])
  const root = { ...row('root', 'running', 'none'), depth: 0, parent: '' }
  expect(groupRows([root, worker, adv]).map((g) => [g.root.id, g.children.map((c) => c.id)])).toEqual([['root', ['worker']]])
  expect(buildRows([s('root'), { ...s('advisor-root'), kind: 'advisor' }], []).map((r) => r.key).filter((k) => !k.startsWith('h:'))).toEqual(['root'])
})
