import '@/components/settings/test-dom'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { cancelSecretClear, copySecret } from './secret-clipboard'

let board = ''
beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] })
  vi.stubGlobal('navigator', {
    userAgent: 'test',
    clipboard: {
      writeText: vi.fn(async (t: string) => void (board = t)),
      readText: vi.fn(async () => board),
    },
  })
})
afterEach(() => {
  cancelSecretClear()
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

test('221 §9.1: a copied secret is cleared after 60 s while it is still on the clipboard', async () => {
  await copySecret('s3cret', 60_000)
  await vi.advanceTimersByTimeAsync(60_000)
  expect(board).toBe('')
})

test('221 §9.1: what the user copied since is never cleared', async () => {
  await copySecret('s3cret', 60_000)
  board = 'their own text'
  await vi.advanceTimersByTimeAsync(60_000)
  expect(board).toBe('their own text')
})

test('221 §9.1: a second copy replaces the first one’s clear instead of cutting its own time short', async () => {
  await copySecret('first', 60_000)
  await vi.advanceTimersByTimeAsync(30_000)
  await copySecret('second', 60_000)
  await vi.advanceTimersByTimeAsync(30_000)
  expect(board).toBe('second')
  await vi.advanceTimersByTimeAsync(30_000)
  expect(board).toBe('')
})
