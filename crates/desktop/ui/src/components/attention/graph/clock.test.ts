// 208 review U1: the region ages nodes in engine time, whatever the local clock says.
import { afterEach, expect, test } from 'vitest'
import { graphNow, noteEngineTime, setGraphClock } from './clock'

afterEach(() => setGraphClock(null))

test('graphNow follows the engine clock measured at the last snapshot or delta', () => {
  let local = 1_000_000
  setGraphClock(() => local)
  expect(graphNow()).toBe(1_000_000)
  // The engine is 5 minutes ahead.
  noteEngineTime(local + 300_000)
  local += 10_000
  expect(graphNow()).toBe(1_310_000)
  // A later reading replaces the skew (engine now 2 minutes behind).
  noteEngineTime(local - 120_000)
  expect(graphNow()).toBe(local - 120_000)
  // Garbage is ignored.
  noteEngineTime(Number.NaN)
  noteEngineTime(0)
  expect(graphNow()).toBe(local - 120_000)
})
