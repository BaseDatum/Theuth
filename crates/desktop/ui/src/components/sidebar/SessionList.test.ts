import { expect, test } from 'vitest'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import { buildRows, type Row } from './SessionList'

const s = (id: string, project: string | null, updated_at: string, pinned = false): SessionSummary => ({
  id,
  created_at: updated_at,
  updated_at,
  title: id,
  model: 'm',
  parent: null,
  name: null,
  effort: null,
  agent: null,
  cwd: null,
  pinned,
  project,
  retention: 'durable',
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
