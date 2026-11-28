// Remote node self-update query (`update_status`) and action (`update_apply`) via `engine_call`.
// Shared by Settings › Network › Machines and Settings › Network › Machines › Cloud nodes so
// both lists deduplicate `update_status` queries for the same online peer and never block rendering.

import { useQueries, useQueryClient } from '@tanstack/react-query'
import { useCallback, useMemo, useState } from 'react'
import { errorText } from '@/components/settings/shared'
import { keys as meshKeys } from '@/components/tasks/queries'
import { notify } from '@/lib/notify'
import { useApi } from '@/lib/rpc'
import { useQuerySupport } from '@/rpc/capabilities'
import { cloudKeys } from './events'

export type PeerUpdateStatus = {
  version: string
  applied: boolean
}

export const peerUpdateKey = (peerId: string) => ['peer-update-status', peerId] as const

/** Validate the wire shape of `update_status` (`{ version, applied } | null`). */
export function parseUpdateStatus(raw: unknown): PeerUpdateStatus | null {
  if (!raw || typeof raw !== 'object') return null
  const rec = raw as { version?: unknown; applied?: unknown }
  if (typeof rec.version !== 'string' || !rec.version.trim()) return null
  return { version: rec.version.trim(), applied: Boolean(rec.applied) }
}

const stripV = (v: string) => v.trim().replace(/^v/i, '')

/** True when `status` represents a newer release than `currentVersion` (or `currentVersion` is unknown). */
export function isNewerUpdate(status: PeerUpdateStatus | null | undefined, currentVersion?: string | null): status is PeerUpdateStatus {
  if (!status || !status.version) return false
  if (currentVersion && stripV(status.version) === stripV(currentVersion)) return false
  return true
}

/**
 * Query `engine_call(peerId, 'update_status', {})` only for the provided online peer ids.
 * Runs asynchronously in the background without blocking the machine or cloud-node list.
 */
export function usePeerUpdates(onlinePeerIds: readonly string[]): ReadonlyMap<string, PeerUpdateStatus | null> {
  const canEngineCall = useQuerySupport(['engine_call'])
  const api = useApi()
  const uniqueIds = useMemo(() => [...new Set(onlinePeerIds.filter(Boolean))], [onlinePeerIds])
  const enabled = canEngineCall && typeof api.engine_call === 'function'

  const results = useQueries({
    queries: uniqueIds.map((peerId) => ({
      queryKey: peerUpdateKey(peerId),
      enabled,
      retry: false,
      staleTime: 60_000,
      queryFn: async (): Promise<PeerUpdateStatus | null> => parseUpdateStatus(await api.engine_call(peerId, 'update_status', {})),
    })),
  })

  return useMemo(() => {
    const map = new Map<string, PeerUpdateStatus | null>()
    uniqueIds.forEach((peerId, i) => {
      const data = results[i]?.data
      map.set(peerId, data ?? null)
    })
    return map
  }, [uniqueIds, results])
}

/**
 * Per-peer `engine_call(peerId, 'update_apply', {})` runner that tracks in-flight state and
 * keeps any RPC failure error per row so the caller can render it inline.
 */
export function usePeerUpdateApply() {
  const api = useApi()
  const qc = useQueryClient()
  const [pending, setPending] = useState<ReadonlySet<string>>(() => new Set())
  const [errors, setErrors] = useState<Record<string, string>>({})

  const apply = useCallback(
    (peerId: string, name: string) => {
      if (typeof api.engine_call !== 'function') {
        setErrors((prev) => ({ ...prev, [peerId]: 'Remote update is not available on this engine' }))
        return
      }
      setPending((prev) => {
        const next = new Set(prev)
        next.add(peerId)
        return next
      })
      setErrors((prev) => {
        if (!(peerId in prev)) return prev
        const next = { ...prev }
        delete next[peerId]
        return next
      })
      void Promise.resolve()
        .then(() => api.engine_call(peerId, 'update_apply', {}))
        .then(
          () => {
            notify(`Updating ${name}`)
            void qc.invalidateQueries({ queryKey: peerUpdateKey(peerId) })
            void qc.invalidateQueries({ queryKey: meshKeys.mesh })
            void qc.invalidateQueries({ queryKey: cloudKeys.list })
          },
          (e: unknown) => {
            setErrors((prev) => ({ ...prev, [peerId]: errorText(e) }))
          },
        )
        .finally(() => {
          setPending((prev) => {
            if (!prev.has(peerId)) return prev
            const next = new Set(prev)
            next.delete(peerId)
            return next
          })
        })
    },
    [api, qc],
  )

  return {
    apply,
    isPending: (peerId: string) => pending.has(peerId),
    errorOf: (peerId: string) => errors[peerId] ?? null,
  }
}
