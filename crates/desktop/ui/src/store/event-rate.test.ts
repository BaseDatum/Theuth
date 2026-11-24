import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { BUCKETS, formatRate, rate, useEventRate } from './event-rate'

beforeEach(() => {
  vi.useFakeTimers()
  vi.setSystemTime(1_000_000)
  useEventRate.getState().reset()
})
afterEach(() => vi.useRealTimers())

const st = () => useEventRate.getState()

test('formatRate', () => {
  const table: [number, string][] = [
    [0, '0'],
    [38, '38'],
    [999, '999'],
    [999.6, '1k'],
    [1000, '1k'],
    [1049, '1k'],
    [1499, '1.5k'],
    [9940, '9.9k'],
    [9950, '10k'],
    [15000, '15k'],
    [123456, '123k'],
  ]
  for (const [n, want] of table) expect([n, formatRate(n)]).toEqual([n, want])
})

test('ticks fill the open second; rolling closes it, silent seconds count as zero', () => {
  for (let i = 0; i < 40; i++) st().tick()
  st().tick(10)
  expect(st().buckets.at(-1)).toBe(0)
  vi.advanceTimersByTime(1_000)
  st().roll()
  expect(st().buckets.slice(-1)).toEqual([50])
  vi.advanceTimersByTime(500)
  st().tick(3)
  vi.advanceTimersByTime(2_500)
  st().roll()
  expect(st().buckets.slice(-4)).toEqual([50, 3, 0, 0])
  expect(st().buckets).toHaveLength(BUCKETS)
})

test('a clock that steps back neither reopens nor loses a second', () => {
  st().tick(5)
  vi.setSystemTime(998_000)
  st().tick(2)
  vi.setSystemTime(1_001_000)
  st().roll()
  expect(st().buckets.at(-1)).toBe(7)
})

test('rate is the mean of the last two complete seconds', () => {
  expect(rate([0, 0, 10, 30])).toBe(20)
  expect(rate([])).toBe(0)
})

test('a long silence clears the ring; reset starts over', () => {
  st().tick(5)
  vi.advanceTimersByTime(100_000)
  st().roll()
  expect(st().buckets).toEqual(Array(BUCKETS).fill(0))
  st().tick(7)
  st().reset()
  st().roll()
  vi.advanceTimersByTime(1_000)
  st().roll()
  expect(st().buckets.at(-1)).toBe(0)
})
