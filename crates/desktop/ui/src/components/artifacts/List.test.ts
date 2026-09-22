import { expect, test } from 'vitest'
import type { Artifact } from '@/rpc/extra'
import { group, nameMatch } from './List'

const art = (over: Partial<Artifact>): Artifact => ({
  id: 'abc',
  session: 's1',
  call_id: 'c1',
  name: 'report.md',
  title: 'Report',
  kind: 'markdown',
  mime: 'text/markdown',
  bytes: 10,
  version: 1,
  path: '/d/artifacts/abc.md',
  source: null,
  preview: null,
  pages: null,
  origin: null,
  created_at: 0,
  ...over,
})

const ALL = { q: '', kind: null, session: null }

test('nameMatch is a case-insensitive substring of the name or the title', () => {
  const a = art({ name: 'q3-report.md', title: 'Quarterly' })
  expect(nameMatch(a, ' REPORT ')).toBe(true)
  expect(nameMatch(a, 'quart')).toBe(true)
  expect(nameMatch(a, 'annual')).toBe(false)
  expect(nameMatch(a, '  ')).toBe(true)
})

test('two versions of one lineage are one stack whose head is the newest, whatever order they arrive in', () => {
  const v1 = art({ call_id: 'c1', version: 1, created_at: 10 })
  const v2 = art({ call_id: 'edit:2-def', id: 'def', version: 2, created_at: 20 })
  const other = art({ call_id: 'c3', name: 'data.csv', kind: 'csv', session: 's2' })
  const groups = group([v1, other, v2], ALL)
  expect(groups.map((g) => g.session)).toEqual(['s1', 's2'])
  expect(groups[0]?.stacks).toHaveLength(1)
  expect(groups[0]?.stacks[0]).toEqual({ head: v2, older: [v1] })
  expect(groups[1]?.stacks[0]).toEqual({ head: other, older: [] })
})

test('the same name in another session is its own lineage; filters apply per version', () => {
  const a = art({ call_id: 'c1', session: 's1' })
  const b = art({ call_id: 'c2', session: 's2' })
  expect(group([a, b], ALL).map((g) => g.stacks[0]?.head.call_id)).toEqual(['c1', 'c2'])
  expect(group([a, b], { ...ALL, session: 's2' }).map((g) => g.session)).toEqual(['s2'])
  expect(group([a, b], { ...ALL, kind: 'pdf' })).toEqual([])
})
