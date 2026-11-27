import { useEffect, useState } from 'react'
import { useConnection } from '@/store/connection'
import { isUnsupported, type UnsealRequestView, vaultApi } from './api'
import { type BeforeApprove, UnsealPrompt } from './UnsealPrompt'

/** 225: `vault_unseal_requests` has no event; the contract asks for a poll every ~3 s. */
export const UNSEAL_POLL_MS = 3_000

/**
 * 225: the sealed nodes asking this device to unlock them, above every route, top right (the secrets unlock cards hold
 * the bottom right). Polls the connected engine until it says it has no vault methods; an engine switch or a new
 * discovery (the engine restarted, maybe upgraded) starts over.
 * A card goes once answered; an id the next poll still returns stays hidden until the engine drops it.
 */
export function UnsealHost({ beforeApprove }: { beforeApprove?: BeforeApprove }) {
  const client = useConnection((s) => s.client)
  const discovery = useConnection((s) => s.discovery)
  const [requests, setRequests] = useState<UnsealRequestView[]>([])
  const [answered, setAnswered] = useState<ReadonlySet<string>>(new Set())

  useEffect(() => {
    setRequests([])
    setAnswered(new Set())
    if (!client) return
    const api = vaultApi(client)
    let stopped = false
    let timer: ReturnType<typeof setTimeout> | undefined
    const poll = async () => {
      try {
        const next = await api.unsealRequests()
        if (stopped) return
        setRequests(next)
        // Forget answered ids the engine no longer lists, so the set never grows.
        setAnswered((done) => (done.size === 0 ? done : new Set(next.map((r) => r.id).filter((id) => done.has(id)))))
      } catch (e) {
        if (isUnsupported(e)) return
        // Not connected right now, or the engine is busy: ask again next round.
      }
      if (!stopped) timer = setTimeout(() => void poll(), UNSEAL_POLL_MS)
    }
    void poll()
    return () => {
      stopped = true
      clearTimeout(timer)
    }
  }, [client, discovery])

  const shown = requests.filter((r) => !answered.has(r.id))
  if (shown.length === 0) return null
  return (
    <div
      className="fixed top-[calc(var(--safe-top)+3rem)] right-4 z-50 flex max-h-[calc(50vh)] w-96 max-w-[calc(100vw-2rem)] flex-col gap-2 overflow-y-auto"
      role="region"
      aria-label="Vault unlock requests"
    >
      {shown.map((r) => (
        <UnsealPrompt key={r.id} request={r} beforeApprove={beforeApprove} onDone={(id) => setAnswered((done) => new Set(done).add(id))} />
      ))}
    </div>
  )
}
