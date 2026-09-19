import { expect, test } from 'vitest'
import { GRACE_MS, inTree, remaining, shouldLeave, transition } from './private'
import type { SessionSlice } from '@/store/sessions'

const members = new Set(['root', 'child'])

test('only leaving the tree releases the owner (51 §4.3)', () => {
  // Root → child → parent, the same path (a dialog, a `?seq=` inspection): stay.
  expect(shouldLeave('/c/root', '/c/child', members)).toBe(false)
  expect(shouldLeave('/c/child', '/c/root', members)).toBe(false)
  expect(shouldLeave('/c/root', '/c/root', members)).toBe(false)
  // Creation happens on `/`: no route change, no leave.
  expect(shouldLeave('/', '/', members)).toBe(false)
  // Another root, New Chat, a global explorer: leave.
  expect(shouldLeave('/c/root', '/c/other', members)).toBe(true)
  expect(shouldLeave('/c/root', '/', members)).toBe(true)
  expect(shouldLeave('/c/child', '/artifacts', members)).toBe(true)
  expect(shouldLeave('/c/root', '/settings/general', members)).toBe(true)
})

test('membership follows children rows or the parent chain, not just the root', () => {
  const slice = (id: string, parent: string | null, children: string[] = []) =>
    ({ id, parent, children: Object.fromEntries(children.map((c) => [c, {}])) }) as unknown as SessionSlice
  const sessions = {
    root: slice('root', null, ['child']),
    child: slice('child', 'root'),
    grandchild: slice('grandchild', 'child'),
    other: slice('other', null),
  }
  expect(inTree('root', 'root', sessions)).toBe(true)
  expect(inTree('root', 'child', sessions)).toBe(true)
  expect(inTree('root', 'grandchild', sessions)).toBe(true)
  expect(inTree('root', 'other', sessions)).toBe(false)
  expect(inTree('root', 'unknown', sessions)).toBe(false)
})

test('transport edges: first loss starts the window, the next open resumes, nothing else acts', () => {
  expect(transition(undefined, 'reconnecting')).toBe('lost')
  expect(transition(undefined, 'closed')).toBe('lost')
  expect(transition(1000, 'reconnecting')).toBe('none')
  expect(transition(undefined, 'open')).toBe('none')
  expect(transition(1000, 'open')).toBe('resume')
})

test('countdown counts whole seconds down and clamps at the deadline', () => {
  expect(remaining(0, 0)).toBe(30)
  expect(remaining(0, 100)).toBe(30)
  expect(remaining(0, 1000)).toBe(29)
  expect(remaining(0, GRACE_MS - 1)).toBe(1)
  expect(remaining(0, GRACE_MS)).toBe(0)
  expect(remaining(0, GRACE_MS * 2)).toBe(0)
})
