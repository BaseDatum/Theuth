import { useUnlockRequests } from '@/store/unlock-requests'
import { UnlockRequestCard } from './UnlockCard'

/**
 * 221 §9.4: the unlock cards agents' calls are waiting on, above every route (the request belongs to no chat: a
 * connection is engine-wide). Answering one, declining it, or its `ExternalUnlockResolved` from elsewhere drops it.
 */
export function UnlockHost() {
  const pending = useUnlockRequests((s) => s.pending)
  if (pending.length === 0) return null
  return (
    <div className="fixed right-4 bottom-12 z-50 flex w-96 max-w-[calc(100vw-2rem)] flex-col gap-2" role="region" aria-label="Unlock requests">
      {pending.map((r) => (
        <UnlockRequestCard key={r.id} request={r} onDone={() => useUnlockRequests.getState().resolve(r.id)} />
      ))}
    </div>
  )
}
