import { receiveComponent } from '@/rpc/bindings/receive'
import { QueryClient, QueryObserver } from '@tanstack/react-query'
import { expect, test, vi } from 'vitest'
import type { EndlessProgress } from "@/rpc/bindings/event.agent_2eendless_2dprogress.output"
import type { SessionMemory } from "@/rpc/bindings/method.agent.output"
import type { Stalled } from "@/rpc/bindings/method.agent.output"
import { dispatch } from '@/store/sessions'
import { applyEndlessProgress, onContextChanged, queryKeys } from './rpc'

const memory = { mode: 'endless', view: [], settling: true, unbuilt: 3, units: 9, model: 'ollama/qwen3:8b', view_bytes: 128_000, backfill: null, stalled: null } as unknown as SessionMemory
const failing: Stalled = { failures: 2, error: 'timed out', next_retry_at: '2026-11-04T22:00:40Z', model: 'ollama/qwen3:8b', blocking: true }
const progress = (stalled: Stalled | null): EndlessProgress => (receiveComponent("event.agent_2eendless_2dprogress.output.EndlessProgress", { type: 'EndlessProgress', session: 's1', built: 6, total: 9, settling: true, error: null, backfill: null, stalled }))

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

test('EndlessProgress with no cached memory writes nothing: the panel fetches the whole memory when it opens', () => {
  const qc = new QueryClient()
  applyEndlessProgress(qc, progress(failing))
  expect(qc.getQueryData(queryKeys.sessionMemory('s1'))).toBeUndefined()
  expect(qc.getQueryCache().find({ queryKey: queryKeys.sessionMemory('s1') })?.state.data).toBeUndefined()
})

test('183 W-7: a replicated ContextChanged refetches the session list; any other reason only searches', () => {
  const search = vi.fn()
  const sessions = vi.fn()
  const off = onContextChanged(search, sessions)
  dispatch([{ type: 'ContextChanged', session: 's1', revision: '2', reason: 'context_edit' }])
  expect(search).toHaveBeenCalledTimes(1)
  expect(sessions).not.toHaveBeenCalled()
  dispatch([{ type: 'ContextChanged', session: 's1', revision: '3', reason: 'replicated' }])
  expect(search).toHaveBeenCalledTimes(2)
  expect(sessions).toHaveBeenCalledTimes(1)
  off()
  dispatch([{ type: 'ContextChanged', session: 's1', revision: '4', reason: 'replicated' }])
  expect(sessions).toHaveBeenCalledTimes(1)
})
