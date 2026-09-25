import { expect, test } from 'vitest'
import { withOwner } from '@/lib/rpc'
import { RpcError } from '@/rpc/client'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import { engineLabel, lastSeenText, ownerOfflineText, ownerUnreachable } from './owner'

const KEY = 'abcdef0123456789'.repeat(4)

test('engine label prefers the mesh name, falls back to the short id', () => {
  expect(engineLabel('')).toBe('?')
  expect(engineLabel(KEY)).toBe('abcdef01')
  expect(engineLabel(KEY, [{ id: KEY, name: 'desk' }])).toBe('desk')
  expect(engineLabel(KEY, [{ id: 'other', name: 'desk' }])).toBe('abcdef01')
})

test('OwnerUnreachable is read from the refusal data in both forms, nothing else', () => {
  expect(ownerUnreachable(new Error('x'))).toBeNull()
  expect(ownerUnreachable(new RpcError(-32000, 'owner unreachable'))).toBeNull()
  expect(ownerUnreachable(new RpcError(-32000, 'x', { other: 1 }))).toBeNull()
  expect(ownerUnreachable(new RpcError(-32000, 'x', { OwnerUnreachable: { owner: KEY, last_seen: null } }))).toEqual({
    owner: KEY,
    lastSeen: null,
  })
  expect(ownerUnreachable(new RpcError(-32000, 'x', { kind: 'OwnerUnreachable', owner: KEY, last_seen: 1700000000000 }))).toEqual({
    owner: KEY,
    lastSeen: 1700000000000,
  })
})

test('last seen renders as an age or never; the sentence names the engine twice', () => {
  const now = Date.parse('2026-09-25T12:00:00Z')
  expect(lastSeenText(null)).toBe('never')
  expect(lastSeenText('2026-09-25T11:57:00Z', now)).toBe('3m ago')
  expect(lastSeenText(now - 5000, now)).toBe('5s ago')
  expect(lastSeenText('garbage', now)).toBe('garbage')
  expect(ownerOfflineText('desk', '3m ago')).toBe(
    'Owned by desk, last seen 3m ago. Continue here now? (turns not yet shipped from desk stay there)',
  )
})

test('withOwner patches only the named row and leaves a foreign list alone', () => {
  const row = { id: 'a', owner: 'x', epoch: 1, here: false } as SessionSummary
  const rows = [row, { ...row, id: 'b' }]
  expect(withOwner(undefined, { session: 'a', owner: 'me', epoch: 2, here: true })).toBeUndefined()
  expect(withOwner(rows, { session: 'zz', owner: 'me', epoch: 2, here: true })).toBe(rows)
  const next = withOwner(rows, { session: 'a', owner: 'me', epoch: 2, here: true })!
  expect(next[0]).toMatchObject({ id: 'a', owner: 'me', epoch: 2, here: true })
  expect(next[1]).toBe(rows[1])
})
