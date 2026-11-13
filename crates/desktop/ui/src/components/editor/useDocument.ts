import { useQuery } from '@tanstack/react-query'
import { useEffect, useState, useSyncExternalStore } from 'react'
import { type DocStatus, type EngineProvider, acquire, release } from '@/lib/doc/provider'
import { useApi } from '@/lib/rpc'
import type { Binding } from "@/rpc/bindings/method.agent.output"
import type { DocId } from "@/rpc/bindings/method.agent.output"
import { useConnection } from '@/store/connection'
import { useScope } from '@/store/engine-scope'

const CONNECTING: DocStatus = { kind: 'connecting' }
const noop = () => () => {}

/**
 * Resolve `target` — a binding (created on demand for knowledge, 59 §7.1) or a known `{ doc }` id (a note) — and
 * hold one shared provider for it while mounted (§6.5). `provider` is `null` until resolved; `error` when the engine refused.
 */
export type LiveDocument = {
  provider: EngineProvider | null
  status: DocStatus
  error: Error | null
}

export type DocTarget = Binding | { doc: DocId }

export function useDocument(target: DocTarget): LiveDocument {
  const api = useApi()
  const client = useConnection((s) => s.client)
  const engine = useScope().id
  const known = typeof target === 'object' && 'doc' in target ? target.doc : null
  const resolved = useQuery({
    queryKey: ['doc', target],
    queryFn: () => api.doc_resolve(target as Binding),
    staleTime: Number.POSITIVE_INFINITY,
    enabled: known === null,
  })
  const id = known ?? resolved.data ?? null
  const [provider, setProvider] = useState<EngineProvider | null>(null)
  useEffect(() => {
    if (!id || !client) return
    const p = acquire(client, api, id, engine)
    setProvider(p)
    return () => {
      setProvider(null)
      release(p)
    }
  }, [id, client, api, engine])
  const status = useSyncExternalStore(provider ? (cb) => provider.onStatus(cb) : noop, () => provider?.status ?? CONNECTING)
  const error = resolved.error ?? (resolved.isSuccess && id === null ? new Error('no document for this binding') : null)
  return { provider, status, error }
}
