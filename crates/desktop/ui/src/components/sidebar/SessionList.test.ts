import { expect, test } from 'vitest'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import type { SessionSlice } from '@/store/sessions'
import { runningSubagents } from './SessionItem'
import { buildRows, newChatLabel, type Row, subtrees } from './SessionList'

const s = (id: string, project: string | null, updated_at: string, pinned = false, archived = false): SessionSummary => ({
  id,
  created_at: updated_at,
  updated_at,
  title: id,
  tags: [],
  model: 'm',
  parent: null,
  name: null,
  effort: null,
  agent: null,
  cwd: null,
  pinned,
  archived,
  project,
  retention: 'durable',
  running: false,
  owner: '',
  epoch: 1,
  here: true,
})

const keys = (rows: Row[]) => rows.map((r) => r.key)

test('pinned lead above every project; projects in first-seen order of the newest ordering, Ungrouped last, headers only when a project exists', () => {
  const sessions = [
    s('a', null, '2026-01-03T00:00:00Z'),
    s('b', 'beta', '2026-01-02T00:00:00Z'),
    s('c', 'alpha', '2026-01-01T00:00:00Z', true),
    s('d', 'beta', '2026-01-04T00:00:00Z'),
  ]
  const rows = buildRows(sessions, [])
  expect(keys(rows).slice(0, 2)).toEqual(['h:pinned', 'c'])
  expect(rows.flatMap((r) => ('project' in r ? [r.project] : []))).toEqual(['beta', null])
  expect(keys(rows).filter((k) => !k.startsWith('h:'))).toEqual(['c', 'p:beta', 'd', 'b', 'p:', 'a'])
  expect(keys(buildRows(sessions, [], new Set(['beta'])))).not.toContain('d')
  expect(keys(buildRows([sessions[0]!], [])).some((k) => k.startsWith('p:'))).toBe(false)
  expect(keys(buildRows([sessions[2]!], []))).toEqual(['h:pinned', 'c'])
})

test('a desk session never appears in the chat list: pinned, archived, in a project or ungrouped', () => {
  const desk = (id: string, project: string | null, pinned: boolean, archived = false) => ({ ...s(id, project, '2026-01-05T00:00:00Z', pinned, archived), desk: 'theuth' })
  const sessions = [desk('d1', null, true), desk('d2', 'beta', false), desk('d3', null, false, true), s('a', 'beta', '2026-01-01T00:00:00Z')]
  const rows = buildRows(sessions, ['d1'], new Set(), true)
  expect(keys(rows).filter((k) => !k.startsWith('h:'))).toEqual(['p:beta', 'a'])
  expect(keys(buildRows(sessions.slice(0, 3), [], new Set(), true))).toEqual([])
})

test('archived roots leave the groups for one collapsed Archived header at the bottom; open, they list newest first with their open tree; no orphans', () => {
  const sessions = [
    s('a', null, '2026-01-03T00:00:00Z'),
    s('b', 'beta', '2026-01-02T00:00:00Z', true, true),
    s('c', 'alpha', '2026-01-04T00:00:00Z', false, true),
    { ...s('c1', null, '2026-01-05T00:00:00Z', false, true), parent: 'c' },
    s('d', 'beta', '2026-01-01T00:00:00Z'),
  ]
  const closed = keys(buildRows(sessions, []))
  expect(closed).toEqual(['p:beta', 'h:beta:Older', 'd', 'p:', 'h::Older', 'a', 'p:archived'])
  expect(buildRows(sessions, []).at(-1)).toEqual({ key: 'p:archived', archived: 2, collapsed: true })
  expect(keys(buildRows(sessions, ['c1'], new Set(), true))).toEqual([
    'p:beta', 'h:beta:Older', 'd', 'p:', 'h::Older', 'a', 'p:archived', 'c', 't:c', 'b',
  ])
  expect(keys(buildRows([sessions[0]!], [], new Set(), true))).not.toContain('p:archived')
})

test('project header menu label names the project; the null bucket is the plain new chat', () => {
  expect(newChatLabel('alpha')).toBe('New chat in “alpha”')
  expect(newChatLabel(null)).toBe('New chat (no project)')
})

test('subtrees roll every depth up to the top-level row; a root without subagents has no entry', () => {
  const sessions = [
    s('a', null, '2026-01-01T00:00:00Z'),
    s('b', null, '2026-01-01T00:00:00Z'),
    { ...s('a1', null, '2026-01-01T00:00:00Z'), parent: 'a' },
    { ...s('a11', null, '2026-01-01T00:00:00Z'), parent: 'a1' },
  ]
  const subs = subtrees(sessions)
  expect(subs.get('a')?.map((d) => d.id)).toEqual(['a1', 'a11'])
  expect(subs.has('b')).toBe(false)
  expect(subs.has('a1')).toBe(false)
})

test('running subagents: a live slice counts, so does the list snapshot for a child this client has no slice for; idle ones do not', () => {
  const d = (id: string, running: boolean) => ({ ...s(id, null, '2026-01-01T00:00:00Z'), parent: 'a', running })
  const slice = (running: boolean) => ({ running }) as SessionSlice
  const subs = [d('live', false), d('cold', true), d('idle', false), d('none', false)]
  expect(runningSubagents(subs, { live: slice(true), idle: slice(false) })).toBe(2)
  expect(runningSubagents(subs, { live: slice(false), idle: slice(false) })).toBe(1)
  expect(runningSubagents([], {})).toBe(0)
})
