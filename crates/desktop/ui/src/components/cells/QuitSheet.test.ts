import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import type { CellRun } from '@/rpc/bindings/CellRun'
import { KILL_AFTER_MS } from './Cell'
import { POLL_MS, confirmQuit, stopRuns, usePreQuit, waitForRuns } from './QuitSheet'
import { run } from './test-fixtures'

beforeEach(() => vi.useFakeTimers())
afterEach(() => {
  vi.useRealTimers()
  usePreQuit.setState({ live: null, api: null, answer: null })
})

const live = run({ state: 'running', exit: null, secs: null, ended_ms: null, started_ms: Date.now() - 134_000 })
/** `cell_live` answering each of `reads` in turn, then `[]`; `cell_stop` recorded. */
function engine(reads: CellRun[][]) {
  const stops: [string, boolean][] = []
  return {
    stops,
    api: {
      cell_live: async () => reads.shift() ?? [],
      cell_stop: async (id: string, kill: boolean) => {
        stops.push([id, kill])
        return null
      },
    },
  }
}

test('no live run: the quit goes ahead at once, no sheet (today unchanged)', async () => {
  expect(await confirmQuit(engine([[]]).api)).toBe(true)
  expect(await confirmQuit(null)).toBe(true)
  expect(usePreQuit.getState().live).toBeNull()
})

test('a live run opens the sheet; Cancel keeps the app; a second quit while it is up is refused', async () => {
  const pending = confirmQuit(engine([[live]]).api)
  await vi.advanceTimersByTimeAsync(0)
  expect(usePreQuit.getState().live).toEqual([live])
  expect(await confirmQuit(engine([[live]]).api)).toBe(false)
  usePreQuit.getState().answer?.(false)
  expect(await pending).toBe(false)
})

test('Wait polls cell_live and proceeds by itself when the last run ends', async () => {
  const e = engine([[live], [live]])
  const seen: number[] = []
  let done: boolean | null = null
  void waitForRuns(e.api, (l) => seen.push(l.length), () => false).then((d) => (done = d))
  await vi.advanceTimersByTimeAsync(POLL_MS * 2)
  expect(done).toBeNull()
  await vi.advanceTimersByTimeAsync(POLL_MS)
  expect(seen).toEqual([1, 1, 0])
  expect(done).toBe(true)
})

test('Wait stops polling once cancelled', async () => {
  let off = false
  let done: boolean | null = null
  void waitForRuns(engine([[live], [live], [live]]).api, () => undefined, () => off).then((d) => (done = d))
  await vi.advanceTimersByTimeAsync(POLL_MS)
  off = true
  await vi.advanceTimersByTimeAsync(POLL_MS)
  expect(done).toBe(false)
})

test('Stop it and quit: Stop, then Kill what is still live 5 s later', async () => {
  const e = engine([[live]])
  let finished = false
  void stopRuns(e.api, [live]).then(() => (finished = true))
  await vi.advanceTimersByTimeAsync(0)
  expect(e.stops).toEqual([[live.id, false]])
  await vi.advanceTimersByTimeAsync(KILL_AFTER_MS - 1)
  expect(finished).toBe(false)
  await vi.advanceTimersByTimeAsync(1)
  expect(e.stops).toEqual([
    [live.id, false],
    [live.id, true],
  ])
  expect(finished).toBe(true)
})
