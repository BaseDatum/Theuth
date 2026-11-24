import { beforeEach, expect, test } from 'vitest'
import { BUCKETS, formatRate, rate, useEventRate } from './event-rate'

beforeEach(() => useEventRate.getState().reset())

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
  const t0 = 1_000_000
  st().roll(t0)
  for (let i = 0; i < 40; i++) st().tick()
  st().tick(10)
  expect(st().buckets.at(-1)).toBe(0)
  st().roll(t0 + 1_000)
  expect(st().buckets.slice(-1)).toEqual([50])
  st().roll(t0 + 1_500)
  st().roll(t0 + 4_000)
  expect(st().buckets.slice(-4)).toEqual([50, 0, 0, 0])
  expect(st().buckets).toHaveLength(BUCKETS)
})

test('rate is the mean of the last two complete seconds', () => {
  expect(rate([0, 0, 10, 30])).toBe(20)
  expect(rate([])).toBe(0)
})

test('a long silence clears the ring; reset starts over', () => {
  st().roll(0)
  st().tick(5)
  st().roll(1_000)
  st().roll(100_000)
  expect(st().buckets).toEqual(Array(BUCKETS).fill(0))
  st().tick(7)
  st().reset()
  st().roll(200_000)
  st().roll(201_000)
  expect(st().buckets.at(-1)).toBe(0)
})
