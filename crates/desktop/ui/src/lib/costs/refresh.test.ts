import { QueryClient, QueryObserver } from '@tanstack/react-query'
import { afterEach, expect, test, vi } from 'vitest'
import { queryKeys } from '@/lib/rpc'
import { CALLS_KEY, LIMITS_KEY, refreshCosts } from './queries'

afterEach(() => {
  vi.useRealTimers()
})

/** A query the page shows: seeded fresh (staleTime ∞, so only an explicit refetch runs it), with a mounted observer. */
function shown(qc: QueryClient, queryKey: readonly unknown[]) {
  const fn = vi.fn(async () => 'fresh')
  qc.setQueryData(queryKey, 'seed')
  const off = new QueryObserver(qc, { queryKey, queryFn: fn, staleTime: Infinity }).subscribe(() => undefined)
  return { fn, off }
}

test('refresh refetches every shown query despite staleTime, kicks the pollers, then reads the limits once more', async () => {
  vi.useFakeTimers({ toFake: ['setTimeout'] })
  const qc = new QueryClient()
  const limits = shown(qc, LIMITS_KEY)
  const calls = shown(qc, [...CALLS_KEY, { limit: 200 }])
  const logins = shown(qc, queryKeys.logins)
  const providers = shown(qc, queryKeys.providers)
  // Not on screen: no observer, so not refetched.
  const hidden = vi.fn(async () => 'x')
  await qc.fetchQuery({ queryKey: ['costs', 'other'], queryFn: hidden })
  const usage_limits_refresh = vi.fn(async () => null)

  await refreshCosts(qc, { usage_limits_refresh }, true)

  expect(usage_limits_refresh).toHaveBeenCalledWith(null)
  for (const q of [limits, calls, logins, providers]) expect(q.fn).toHaveBeenCalledTimes(1)
  expect(qc.getQueryData(LIMITS_KEY)).toBe('fresh')
  expect(hidden).toHaveBeenCalledTimes(1)
  await vi.advanceTimersByTimeAsync(2_000)
  expect(limits.fn).toHaveBeenCalledTimes(2)
  expect(calls.fn).toHaveBeenCalledTimes(1)
  for (const q of [limits, calls, logins, providers]) q.off()
})

test('without a kick (sim or an engine without the method) nothing is asked of the pollers and no follow-up read runs', async () => {
  vi.useFakeTimers({ toFake: ['setTimeout'] })
  const qc = new QueryClient()
  const limits = shown(qc, LIMITS_KEY)
  const usage_limits_refresh = vi.fn(async () => null)
  await refreshCosts(qc, { usage_limits_refresh }, false)
  await vi.advanceTimersByTimeAsync(5_000)
  expect(usage_limits_refresh).not.toHaveBeenCalled()
  expect(limits.fn).toHaveBeenCalledTimes(1)
  limits.off()
})

test('a failing kick still resolves after the refetches', async () => {
  const qc = new QueryClient()
  const limits = shown(qc, LIMITS_KEY)
  await refreshCosts(qc, { usage_limits_refresh: vi.fn(async () => Promise.reject(new Error('gone'))) }, true)
  expect(limits.fn).toHaveBeenCalledTimes(1)
  limits.off()
})
