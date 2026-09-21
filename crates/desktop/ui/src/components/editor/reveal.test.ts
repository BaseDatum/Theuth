import { expect, test } from 'vitest'
import { MAX_REVEAL_MS, MIN_CHARS_PER_S, attribute, attributionLabel, revealDuration, revealedChars } from './reveal'

test('reveal rate is max(24 chars/s, len / 1.5 s): short text at 24/s, long text capped at 1.5 s', () => {
  expect(revealDuration(12)).toBe(500)
  expect(revealDuration(24)).toBe(1000)
  expect(revealDuration(36)).toBe(MAX_REVEAL_MS)
  expect(revealDuration(3000)).toBe(MAX_REVEAL_MS)
  expect(revealDuration(0)).toBe(0)
  // At the cap the effective rate exceeds the floor.
  expect(3000 / (revealDuration(3000) / 1000)).toBeGreaterThan(MIN_CHARS_PER_S)
})

test('revealed characters grow monotonically from 0 to len and clamp', () => {
  const len = 480
  let last = 0
  for (let t = 0; t <= 2000; t += 50) {
    const n = revealedChars(len, t)
    expect(n).toBeGreaterThanOrEqual(last)
    expect(n).toBeLessThanOrEqual(len)
    last = n
  }
  expect(revealedChars(len, 0)).toBe(0)
  expect(revealedChars(len, 750)).toBe(240)
  expect(revealedChars(len, MAX_REVEAL_MS)).toBe(len)
  expect(revealedChars(len, 10_000)).toBe(len)
  expect(revealedChars(0, 100)).toBe(0)
  expect(revealedChars(10, -5)).toBe(0)
})

test('attribution: agent blocks from Authored, owner edits override, empty events are no-ops', () => {
  const a = attribute({}, { who: 'agent', blocks: ['b1', 'b2'], at: 1000, session: 's' })
  expect(a).toEqual({ b1: { who: 'agent', at: 1000, session: 's' }, b2: { who: 'agent', at: 1000, session: 's' } })
  const b = attribute(a, { who: 'owner', blocks: ['b2'], at: 2000 })
  expect(b.b1).toEqual({ who: 'agent', at: 1000, session: 's' })
  expect(b.b2).toEqual({ who: 'owner', at: 2000 })
  expect(attribute(b, { who: 'agent', blocks: [], at: 3000 })).toBe(b)
  // Later agent edit of the same block wins over the owner's.
  expect(attribute(b, { who: 'agent', blocks: ['b2'], at: 4000 }).b2).toEqual({ who: 'agent', at: 4000, session: undefined })
})

test('attribution label names theuth and the time', () => {
  const at = new Date(2026, 8, 21, 14, 2).getTime()
  expect(attributionLabel({ who: 'agent', at })).toMatch(/^theuth · \d{1,2}:02/)
  expect(attributionLabel({ who: 'owner', at })).toMatch(/^you · /)
})
