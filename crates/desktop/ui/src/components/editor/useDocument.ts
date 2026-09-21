import { useQuery } from '@tanstack/react-query'
import { useEffect, useState, useSyncExternalStore } from 'react'
import { type DocStatus, type HarnessProvider, acquire, release } from '@/lib/doc/provider'
import { useApi } from '@/lib/rpc'
import type { Binding } from '@/rpc/bindings/Binding'
import { useConnection } from '@/store/connection'

const CONNECTING: DocStatus = { kind: 'connecting' }
const noop = () => () => {}

/**
 * Resolve `binding` to its document (created on demand for knowledge, 59 §7.1) and hold one shared
 * provider for it while mounted (§6.5). `provider` is `null` until resolved; `error` when the harness refused.
 */
export function useDocument(binding: Binding): {
  provider: HarnessProvider | null
  status: DocStatus
  error: Error | null
} {
  const api = useApi()
  const client = useConnection((s) => s.client)
  const resolved = useQuery({
    queryKey: ['doc', binding],
    queryFn: () => api.doc_resolve(binding),
    staleTime: Number.POSITIVE_INFINITY,
  })
  const id = resolved.data ?? null
  const [provider, setProvider] = useState<HarnessProvider | null>(null)
  useEffect(() => {
    if (!id || !client) return
    const p = acquire(client, api, id)
    setProvider(p)
    return () => {
      setProvider(null)
      release(p)
    }
  }, [id, client, api])
  const status = useSyncExternalStore(provider ? (cb) => provider.onStatus(cb) : noop, () => provider?.status ?? CONNECTING)
  const error = resolved.error ?? (resolved.isSuccess && id === null ? new Error('no document for this binding') : null)
  return { provider, status, error }
}
