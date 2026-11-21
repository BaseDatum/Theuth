// 208 review U4: the stream's lifecycle decisions.
import { expect, test } from 'vitest'
import { isStale, retryDelay, streamRuns } from './lifecycle'

test('the stream runs whenever supported, with a source, and visible — collapsed or not', () => {
  expect(streamRuns({ supported: true, source: true, visible: true })).toBe(true)
  expect(streamRuns({ supported: true, source: true, visible: false })).toBe(false)
  expect(streamRuns({ supported: false, source: true, visible: true })).toBe(false)
  expect(streamRuns({ supported: true, source: false, visible: true })).toBe(false)
})

test('retries back off 5 s, 15 s, then 60 s for every later failure', () => {
  expect([1, 2, 3, 4, 10].map(retryDelay)).toEqual([5_000, 15_000, 60_000, 60_000, 60_000])
  expect(retryDelay(0)).toBe(5_000)
})

test('stale only when a failed call leaves an earlier state on screen', () => {
  expect(isStale(true, 'boom')).toBe(true)
  expect(isStale(false, 'boom')).toBe(false)
  expect(isStale(true, null)).toBe(false)
})
