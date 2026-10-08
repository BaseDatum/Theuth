import { age } from '@/components/tasks/helpers'
import { useMesh } from '@/components/tasks/queries'
import { RpcError } from '@/rpc/client'

/** Owner chip text (68 §5): the engine's mesh name when a peer carries it, else the first 8 chars of its id; `?` when empty. */
export function engineLabel(owner: string, peers: readonly { id: string; name: string }[] = []): string {
  if (!owner) return '?'
  return peers.find((p) => p.id === owner)?.name || owner.slice(0, 8)
}

/** `engineLabel` against the live mesh roster. */
export function useEngineLabel(owner: string): string {
  const { data } = useMesh()
  return engineLabel(owner, data?.peers)
}

export type OwnerUnreachable = { owner: string; lastSeen: string | number | null }

/**
 * `OwnerUnreachable{owner, last_seen}` (68 §5) out of a failed call: the refusal's `data` in serde's external form
 * (`{"OwnerUnreachable": {…}}`) or flat and tagged `kind`/`type`. `null` for every other error.
 */
export function ownerUnreachable(e: unknown): OwnerUnreachable | null {
  if (!(e instanceof RpcError) || typeof e.data !== 'object' || e.data === null) return null
  const d = e.data as Record<string, unknown>
  const tagged = d.kind === 'OwnerUnreachable' || d.type === 'OwnerUnreachable'
  const o = (d.OwnerUnreachable ?? (tagged ? d : null)) as Record<string, unknown> | null
  if (!o || typeof o.owner !== 'string') return null
  const seen = o.last_seen
  return { owner: o.owner, lastSeen: typeof seen === 'string' || typeof seen === 'number' ? seen : null }
}

export type SessionDraining = { session: string; reason: string; nextOwner: string | null }

/**
 * 139 §6.1 (D16): `SessionDraining{session, reason, next_owner}` out of a failed send — the session is mid-release
 * on the engine we talk to; retryable. Read from the refusal's `data` (`{"SessionDraining": {…}}`), never from the
 * code alone: `-32011` is also `NotHub`. `null` for every other error.
 */
export function sessionDraining(e: unknown): SessionDraining | null {
  if (!(e instanceof RpcError) || typeof e.data !== 'object' || e.data === null) return null
  const d = (e.data as Record<string, unknown>).SessionDraining as Record<string, unknown> | undefined
  if (!d || typeof d.session !== 'string' || typeof d.reason !== 'string') return null
  return { session: d.session, reason: d.reason, nextOwner: typeof d.next_owner === 'string' ? d.next_owner : null }
}

/** `3m ago` / `never` for a `last_seen` that is RFC 3339 text or ms since the epoch. */
export function lastSeenText(v: string | number | null, now = Date.now()): string {
  if (v === null) return 'never'
  const ms = typeof v === 'number' ? v : Date.parse(v)
  return Number.isFinite(ms) ? `${age(ms, now)} ago` : String(v)
}

/** The owner-offline card's sentence (68 §5). */
export const ownerOfflineText = (engine: string, lastSeen: string): string =>
  `Owned by ${engine}, last seen ${lastSeen}. Continue here now? (turns not yet shipped from ${engine} stay there)`

/** 139 §9.3: the card's second sentence when `Loaded.resume` shows the owner mid-turn (D5: the take resumes it). */
export const ownerMidTurnText = (engine: string): string =>
  `${engine} was mid-turn. Continue here and resume — ${engine} may still be working; whatever it did after the last synced step will be discarded.`
