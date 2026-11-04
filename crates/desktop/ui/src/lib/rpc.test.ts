import { QueryClient, QueryObserver } from '@tanstack/react-query'
import { expect, test, vi } from 'vitest'
import type { EndlessProgress } from '@/rpc/bindings/EndlessProgress'
import type { SessionMemory } from '@/rpc/bindings/SessionMemory'
import type { Stalled } from '@/rpc/bindings/Stalled'
import { applyEndlessProgress, queryKeys } from './rpc'

const memory = { mode: 'endless', view: [], settling: true, unbuilt: 3, units: 9, model: 'ollama/qwen3:8b', view_bytes: 128_000, backfill: null, stalled: null } as unknown as SessionMemory
const failing: Stalled = { failures: 2, error: 'timed out', next_retry_at: '2026-11-04T22:00:40Z', model: 'ollama/qwen3:8b', blocking: true }
const progress = (stalled: Stalled | null): EndlessProgress => ({ type: 'EndlessProgress', session: 's1', built: 6, total: 9, settling: true, error: null, backfill: null, stalled })

test('EndlessProgress writes `stalled` into the cached memory before the refetch starts, and `stalled: null` clears it', () => {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const key = queryKeys.sessionMemory('s1')
  qc.setQueryData(key, memory)
  // The refetch never settles here: whatever the cache holds when it starts is the event's write alone.
  const seen: (Stalled | null | undefined)[] = []
  const queryFn = vi.fn(() => {
    seen.push(qc.getQueryData<SessionMemory>(key)?.stalled)
    return new Promise<SessionMemory>(() => {})
  })
  const off = new QueryObserver(qc, { queryKey: key, queryFn }).subscribe(() => {})

  applyEndlessProgress(qc, progress(failing))
  expect(qc.getQueryData<SessionMemory>(key)?.stalled).toEqual(failing)
  expect(seen).toEqual([failing])

  applyEndlessProgress(qc, progress(null))
  expect(qc.getQueryData<SessionMemory>(key)?.stalled).toBeNull()
  expect(qc.getQueryData<SessionMemory>(key)?.unbuilt).toBe(3) // the rest waits for the refetch
  expect(seen).toEqual([failing, null])
  off()
})
