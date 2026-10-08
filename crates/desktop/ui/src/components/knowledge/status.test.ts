import { expect, test } from 'vitest'
import { changedByDreaming, statusBadge } from './status'

const tidy = { status: 'tidy', superseded_by: null, verified: null, source: 'harvest' } as const

test('badge precedence: superseded > status > verified date > none', () => {
  expect(statusBadge(tidy)).toBeNull()
  expect(statusBadge({ ...tidy, status: 'fresh' })).toBe('fresh')
  expect(statusBadge({ ...tidy, status: 'archived' })).toBe('archived')
  expect(statusBadge({ ...tidy, status: 'stale', verified: '2026-10-01T03:00:00Z' })).toBe('stale')
  expect(statusBadge({ ...tidy, verified: '2026-10-01T03:00:00Z' })).toBe('verified 2026-10-01')
  expect(statusBadge({ ...tidy, status: 'archived', superseded_by: 'facts/newer' })).toBe('superseded')
})

test('changed by dreaming: source or any non-tidy status', () => {
  expect(changedByDreaming(tidy)).toBe(false)
  expect(changedByDreaming({ ...tidy, source: 'dreaming' })).toBe(true)
  expect(changedByDreaming({ ...tidy, status: 'fresh' })).toBe(true)
  expect(changedByDreaming({ ...tidy, status: 'merged', source: null })).toBe(true)
})
