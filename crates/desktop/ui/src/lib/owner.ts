import { create } from 'zustand'
import { age } from '@/components/tasks/helpers'
import { useMesh } from '@/components/tasks/queries'
import type { Peer } from '@/rpc/bindings/Peer'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import { notOwner } from '@/lib/front-door'
import { RpcError } from '@/rpc/client'
import type { Stores } from '@/store/engine-scope'
import { on } from '@/store/sessions'

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
 * on the engine we talk to; retryable (code `-32012`). Read from the refusal's `data` (`{"SessionDraining": {…}}`),
 * never from the code alone (`-32011` beside it is `NotHub`). `null` for every other error.
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

/** 183 §4.4 (D2): a routed call the owner could not take, parked behind the OwnerOfflineCard. `retry` is the call
 * itself; absent, the card resends the composer's draft. Nothing runs until Continue here. */
export type ParkedCall = OwnerUnreachable & { retry?: () => Promise<unknown> }

/** One parked call per session: the composer of that chat shows its card (the newest refusal wins). */
export const useOwnerOffline = create<{ by: Record<string, ParkedCall>; park(session: string, call: ParkedCall): void; clear(session: string): void }>()((set) => ({
  by: {},
  park: (session, call) => set((s) => ({ by: { ...s.by, [session]: call } })),
  clear: (session) =>
    set((s) => {
      const { [session]: _, ...by } = s.by
      return { by }
    }),
}))

/**
 * 183 §4.4: every routed action (a ⋯ setter, stop, an approval answer…) goes through here. A `-32010` parks the call
 * behind the OwnerOfflineCard and resolves `undefined`; every other error is the caller's.
 */
export async function routed<T>(session: string, call: () => Promise<T>): Promise<T | undefined> {
  try {
    return await call()
  } catch (e) {
    if (!parkIfOffline(session, e, call)) throw e
    return undefined
  }
}

/** 183 §4.4: park `retry` behind the OwnerOfflineCard when `e` is `OwnerUnreachable` (for the callers that cannot go
 * through `routed`: a command line, the composer's flush); `false` for every other error, which stays the caller's. */
export function parkIfOffline(session: string, e: unknown, retry: () => Promise<unknown>): boolean {
  const offline = ownerUnreachable(e)
  if (!offline) return false
  useOwnerOffline.getState().park(session, { ...offline, retry })
  return true
}

/**
 * 183 §13 row 2: a forwarded call reached an engine that no longer owns the row (`-32013 NotOwner`); the replicated
 * owner change follows. `retry` runs once, on the next `SessionOwnerChanged` for `session`; `giveUp` if none comes in `ms`.
 */
export function retryAfterOwnerChange(session: string, retry: () => void, giveUp: () => void, ms = 10_000): void {
  const off = on('SessionOwnerChanged', (e) => {
    if (e.session !== session) return
    off()
    clearTimeout(timer)
    retry()
  })
  const timer = setTimeout(() => {
    off()
    giveUp()
  }, ms)
}

/** The remote sessions `peer` owns, out of a `list_sessions` page. */
export const ownedBy = (rows: readonly Pick<SessionSummary, 'id' | 'owner' | 'here'>[] | undefined, peer: string): Set<string> =>
  new Set(rows?.filter((s) => !s.here && s.owner === peer).map((s) => s.id))

/** 183 §5.1: a session known to be owned elsewhere (`here === false`) — its `TurnFinished` settles the replicated rows; an
 * unknown row or a local one does not (a local turn's own `ContextChanged` does that already). */
export const ownedElsewhere = (rows: readonly Pick<SessionSummary, 'id' | 'here'>[] | undefined, session: string): boolean =>
  rows?.find((r) => r.id === session)?.here === false

/** Which of `ids` are rows owned elsewhere (what leaving the focus set unfollows). */
export const remoteAmong = (rows: readonly Pick<SessionSummary, 'id' | 'here'>[] | undefined, ids: readonly string[]): Set<string> =>
  new Set(ids.filter((id) => rows?.some((r) => r.id === id && !r.here)))

/**
 * 183 §4.4 / §12.1 (R1): a relay that ended publishes no `*Resolved`, so a remote session's relayed approvals and asks
 * go with it (its owner disconnected, the user left the chat, the owner changed); the next `Watch` sends them again
 * if they still wait. Shared by the peer, owner-change and unfocus paths.
 */
export function withdrawRelayed(st: Pick<Stores, 'approvals' | 'asks'>, sessions: ReadonlySet<string>): void {
  if (sessions.size === 0) return
  st.approvals.getState().withdraw(sessions)
  st.asks.getState().withdraw(sessions)
}

/**
 * `SessionOwnerChanged`: a real owner change (a move, epoch > 1) withdraws the session's relayed cards — they are the
 * suspended turn's, and the new owner asks again on resume. 183 §5.6: epoch 1 is the first landing of a chat created on
 * another engine (its row reaching this one); its owner did not change and its follow stream goes on, so a card it
 * relayed still waits there and stays.
 */
export function onOwnerChanged(st: Pick<Stores, 'approvals' | 'asks'>, e: { session: string; epoch: number }): void {
  if (e.epoch > 1) withdrawRelayed(st, new Set([e.session]))
}

/**
 * 183 §13 row 2: a send refused `-32013 NotOwner` keeps its draft and goes once more after the next `SessionOwnerChanged`
 * of that session (`retried` is the caller's one-shot flag, reset by its next success). `true` when this took the error;
 * `false` (the caller shows it) for any other error or a second refusal.
 */
export function retryNotOwner(e: unknown, session: string, retried: { current: boolean }, resend: () => void, fail: () => void): boolean {
  if (!notOwner(e) || retried.current) return false
  retried.current = true
  retryAfterOwnerChange(session, resend, fail)
  return true
}

/**
 * 183 R2: the follow stream to `session`'s owner ended or stalled, so its owner's `TurnFinished` may never come. Ask the
 * open chat to settle from the replicated rows (the route's `reload` listener; other chats settle when shown, the slice
 * being stale). Call it after the slice has seen `FollowState{live: false}`, so the settle's freshness is the new one.
 */
export function settleAfterFollowLoss(session: string): void {
  window.dispatchEvent(new CustomEvent('theuth:reload-session', { detail: { session, settled: true } }))
}

/** A peer connected or disconnected (`MeshEvent.peer`): on disconnect its remote sessions' cards go, their follow stream is
 * down (an open reply goes stale and settles from the rows, R2), and on connect a parked call no longer keeps the chip offline. */
export function onPeerChange(st: Pick<Stores, 'approvals' | 'asks' | 'sessions'>, rows: readonly SessionSummary[] | undefined, e: { type: 'connected' | 'disconnected'; peer: string }): void {
  const owned = ownedBy(rows, e.peer)
  if (e.type === 'connected') {
    for (const id of owned) useOwnerOffline.getState().clear(id)
    return
  }
  withdrawRelayed(st, owned)
  const known = [...owned].filter((id) => st.sessions.getState().sessions[id] !== undefined)
  st.sessions.getState().applyEvents(known.map((session) => ({ type: 'FollowState', session, live: false })))
  for (const id of known) settleAfterFollowLoss(id)
}

/** 183 §12.1: the owner chip's state — `live` only while the follow stream is up (`FollowState`), `connected` while
 * the owner is merely connected (not followed live, or an old owner), `offline` when it is not or a routed call just
 * said so, `undefined` for an owner the roster does not know. */
export function ownerState(owner: string, peers: readonly Pick<Peer, 'id' | 'reachable'>[] | undefined, parked: boolean, live = false): 'live' | 'connected' | 'offline' | undefined {
  if (parked) return 'offline'
  const peer = peers?.find((p) => p.id === owner)
  return peer === undefined ? undefined : !peer.reachable ? 'offline' : live ? 'live' : 'connected'
}
