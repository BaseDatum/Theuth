import { expect, test } from 'vitest'
import { flagRuns } from './flag-runs'

test('flagRuns: the running value once known, null while loading, running when the engine cannot answer', () => {
  const flag = (running: boolean) => ({ key: 'computer.enabled' as const, enabled: !running, running, restart_required: true })
  expect(flagRuns({ data: flag(false), isError: false })).toBe(false)
  expect(flagRuns({ data: flag(true), isError: false })).toBe(true)
  expect(flagRuns({ isError: false })).toBeNull()
  expect(flagRuns({ isError: true })).toBe(true)
})
