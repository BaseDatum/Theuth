import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { createRenderQueue } from './render-queue'

beforeEach(() => vi.useFakeTimers())
afterEach(() => vi.useRealTimers())

test('50 requests over 400 ms coalesce to 3–6 renders and the last request is the last rendered', async () => {
  const rendered: number[] = []
  const q = createRenderQueue<number>(async (n) => {
    rendered.push(n)
  }, 80)
  for (let i = 1; i <= 50; i++) {
    q.request(i)
    await vi.advanceTimersByTimeAsync(8)
  }
  await vi.advanceTimersByTimeAsync(200)
  expect(rendered.length).toBeGreaterThanOrEqual(3)
  expect(rendered.length).toBeLessThanOrEqual(6)
  expect(rendered.at(-1)).toBe(50)
})

test('a superseded render sees isCurrent() false; dispose drops the trailing request', async () => {
  const seen: Array<[number, boolean]> = []
  let release!: () => void
  const q = createRenderQueue<number>(async (n, isCurrent) => {
    if (n === 1) await new Promise<void>((r) => (release = r))
    seen.push([n, isCurrent()])
  }, 80)
  q.request(1)
  await vi.advanceTimersByTimeAsync(0)
  q.request(2)
  release()
  await vi.advanceTimersByTimeAsync(0)
  expect(seen).toEqual([[1, false]])
  q.request(3)
  q.dispose()
  await vi.advanceTimersByTimeAsync(500)
  expect(seen).toEqual([[1, false]])
})
