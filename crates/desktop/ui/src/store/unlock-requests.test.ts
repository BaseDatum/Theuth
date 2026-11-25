import { afterEach, expect, test, vi } from 'vitest'
import { UNLOCK_TTL_MS, useUnlockRequests } from './unlock-requests'

const req = (id: string) => ({ id, conn: 'personal', backend: 'bw', account: 'me@example.org', reason: 'bw/personal/2f1c#password' })
const ids = () => useUnlockRequests.getState().pending.map((p) => p.id)

afterEach(() => {
  useUnlockRequests.getState().clear()
  vi.useRealTimers()
})

test('221 §9.4: the engine snapshot replaces every card a missed Resolved left behind, and a switch clears them', () => {
  const s = useUnlockRequests.getState()
  // The event as `rpc.ts` passes it, `type` included.
  const event = { type: 'ExternalUnlockRequested' as const, ...req('stale') }
  s.add(event)
  s.add(req('live'))
  s.add(req('live'))
  expect(ids()).toEqual(['stale', 'live'])
  s.replace([req('live'), req('new')])
  expect(ids()).toEqual(['live', 'new'])
  // Wire fields beyond the view never ride along.
  expect(Object.keys(useUnlockRequests.getState().pending[0]!).sort()).toEqual(['account', 'backend', 'conn', 'id', 'reason'])
  s.clear()
  expect(ids()).toEqual([])
})

test('221 §9.4: a card whose Resolved never arrives drops after the TTL; a reseed restarts it', () => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] })
  const s = useUnlockRequests.getState()
  s.add(req('a'))
  vi.advanceTimersByTime(UNLOCK_TTL_MS - 1000)
  s.replace([req('a')])
  vi.advanceTimersByTime(2000)
  expect(ids()).toEqual(['a'])
  vi.advanceTimersByTime(UNLOCK_TTL_MS)
  expect(ids()).toEqual([])
})
