import { useQuery } from '@tanstack/react-query'
import { useMemo } from 'react'
import { useApi } from '@/lib/rpc'
import type { PeerSync } from '@/rpc/bindings/method.sessionsync.receiver'
import { useQuerySupport } from '@/rpc/capabilities'

/** React Query key for `sessions_sync()` (memory-only replication status per peer). */
export const peerSyncQueryKey = ['sessions-sync'] as const

/** Poll quickly while a newly joined peer is applying its bootstrap snapshot, slowly when idle. */
export const PEER_SYNC_BOOTSTRAPPING_MS = 3_000
export const PEER_SYNC_IDLE_MS = 30_000

export const BOOTSTRAPPING_LABEL = 'Syncing mesh history…'
export const BOOTSTRAPPING_HINT = 'Finishing initial sync — ready for chats once history lands'

/**
 * Whether `sync` describes a peer still applying its initial bootstrap snapshot of our session log.
 *
 * In `crates/sessionsync/src/sync.rs`:
 * - A freshly connected peer before its first `Hello` has `acked: null` (synthesised by `sync_status`).
 * - Once `Hello` arrives with no cursor for our origin, `acked` becomes `0` and stays `0` while our
 *   bootstrap snapshot is built, transmitted, and applied on the peer; it jumps to `current_to_seq`
 *   (> 0) only after the peer applies the final snapshot terminator frame and sends `Msg::Ack`.
 * - An older-schema peer has `held: true` (we hold our log rather than shipping a snapshot).
 * - An empty mesh (`head === 0`) has no session history to bootstrap.
 */
export function isPeerBootstrapping(sync: PeerSync | undefined): boolean {
  if (!sync) return false
  if (sync.held) return false
  if (sync.head <= 0) return false
  return sync.acked == null || sync.acked === 0
}

/** Index `sessions_sync()` rows by mesh peer id (`EngineId`). */
export function peerSyncById(rows: readonly PeerSync[] | undefined): Map<string, PeerSync> {
  const map = new Map<string, PeerSync>()
  if (!rows) return map
  for (const row of rows) map.set(row.peer, row)
  return map
}

/** Concise user-facing label for a bootstrapping peer. */
export function bootstrappingLabel(_sync?: PeerSync): string {
  return BOOTSTRAPPING_LABEL
}

/**
 * Poll `sessions_sync()` from memory. Degrades to an empty map on older/iOS engines without
 * `sessions_sync`. Polls every 3 s while any peer is bootstrapping and every 30 s otherwise.
 */
export function usePeerSync(): {
  byPeer: Map<string, PeerSync>
  anyBootstrapping: boolean
  data: PeerSync[] | undefined
} {
  const supported = useQuerySupport(['sessions_sync'])
  const api = useApi()
  const { data } = useQuery({
    queryKey: peerSyncQueryKey,
    queryFn: () => (typeof api.sessions_sync === 'function' ? api.sessions_sync() : Promise.resolve([])),
    enabled: supported,
    refetchInterval: (query) => (query.state.data?.some(isPeerBootstrapping) ? PEER_SYNC_BOOTSTRAPPING_MS : PEER_SYNC_IDLE_MS),
  })
  const byPeer = useMemo(() => peerSyncById(data), [data])
  const anyBootstrapping = useMemo(() => data?.some(isPeerBootstrapping) ?? false, [data])
  return { byPeer, anyBootstrapping, data }
}
