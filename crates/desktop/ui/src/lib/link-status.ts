// Names the failure of one relay device socket (31 §4.2 PRESENCE/CLOSE, 69 decision 6) from what the
// client can observe. Pure: `LinkTransport` folds events in with `reduce`, the UI asks `failure()` at
// render time so the parked/backoff counters tick without more events.

import { backoff } from '@/rpc/client'

/** Parked (PRESENCE 0) this long before the client says the engine is not there. */
export const PARKED_AFTER_MS = 10_000

export type LinkEvent =
  /** A new socket is being opened. */
  | { type: 'connecting'; at: number }
  /** The relay said whether the engine has a live uplink. */
  | { type: 'presence'; online: boolean; at: number }
  /** A relay `CLOSE` frame; `handshake` = our IK message was out (the engine itself answered). */
  | { type: 'close'; reason: number; at: number; handshake: boolean }
  /** The socket ended; `opened` false = the upgrade never completed. `reachable`: the relay answered HTTP just now. */
  | { type: 'lost'; at: number; opened: boolean; reachable: boolean }
  /** Session handshake finished. */
  | { type: 'ready'; at: number }

export type FailureKind =
  | 'engine-offline'
  | 'stale-pairing'
  | 'engine-full'
  | 'revoked'
  | 'engine-gone'
  | 'hub-moved'
  | 'queue-full'
  | 'idle'
  | 'rate-limited'
  | 'unreachable'

export type LinkState = {
  phase: 'idle' | 'connecting' | 'parked' | 'online' | 'ready' | 'lost'
  /** When PRESENCE 0 arrived on the current socket. */
  parkedAt: number | null
  /** A failure that outlives the socket it happened on; cleared by a finished handshake. */
  sticky: FailureKind | null
  lastClose: { reason: number; at: number } | null
  /** Sockets opened since the last finished handshake. */
  attempts: number
  /** When the `RpcClient` redials, from its backoff. */
  retryAt: number | null
}

export const INITIAL: LinkState = { phase: 'idle', parkedAt: null, sticky: null, lastClose: null, attempts: 0, retryAt: null }

/** Relay `reason` byte → failure. Reason 3 means three things; only the socket's history tells them apart. */
function closeKind(reason: number, parked: boolean, handshake: boolean): FailureKind | null {
  switch (reason) {
    case 1:
      return 'queue-full'
    case 2:
      return 'idle'
    case 3:
      // ponytail: heuristic. The engine answers our handshake → it does not know this device. Refused
      // while parked (PRESENCE 0 seen) → nothing registered under this id: the pairing is stale. Refused
      // straight after connect → the conn cap. An engine that came back full after we parked reads as stale.
      return handshake ? 'revoked' : parked ? 'stale-pairing' : 'engine-full'
    case 4:
      return 'engine-gone'
    case 5:
      return 'hub-moved'
    default:
      return null
  }
}

export function reduce(s: LinkState, e: LinkEvent): LinkState {
  switch (e.type) {
    case 'connecting':
      return { ...s, phase: 'connecting', parkedAt: null, attempts: s.attempts + 1, retryAt: null }
    case 'presence':
      return e.online ? { ...s, phase: 'online', parkedAt: null } : { ...s, phase: 'parked', parkedAt: s.parkedAt ?? e.at }
    case 'close': {
      const kind = closeKind(e.reason, s.parkedAt !== null, e.handshake)
      return { ...s, sticky: kind ?? s.sticky, lastClose: { reason: e.reason, at: e.at } }
    }
    case 'lost': {
      const sticky = e.opened ? s.sticky : e.reachable ? 'rate-limited' : 'unreachable'
      // `attempts` counts this socket already; the client's backoff index is the sockets before it.
      return { ...s, phase: 'lost', parkedAt: null, sticky, retryAt: e.at + backoff(Math.max(0, s.attempts - 1)) }
    }
    case 'ready':
      return { ...s, phase: 'ready', parkedAt: null, sticky: null, attempts: 0, retryAt: null }
  }
}

export type LinkFailure = {
  kind: FailureKind
  title: string
  detail: string
  /** Re-pair this entry (pairing screen, same name). */
  action?: 'repair'
}

const secs = (ms: number) => `${Math.max(0, Math.ceil(ms / 1000))}s`

/** What to tell the user at `now`, or `null` while nothing is wrong (or not wrong for long enough). */
export function failure(s: LinkState, now: number, engine: { name: string; relayHost: string }): LinkFailure | null {
  const parked = s.phase === 'parked' && s.parkedAt !== null && now - s.parkedAt >= PARKED_AFTER_MS
  // Stale pairing and revocation need the user; they win over a parked socket.
  const terminal = s.sticky === 'stale-pairing' || s.sticky === 'revoked'
  const kind = s.sticky !== null && (terminal || !parked) ? s.sticky : parked ? 'engine-offline' : null
  if (kind === null) return null
  const retry = s.retryAt === null ? '' : s.retryAt > now ? ` Retrying in ${secs(s.retryAt - now)}.` : ' Retrying…'
  switch (kind) {
    case 'engine-offline':
      return {
        kind,
        title: `${engine.name} is not connected to the relay`,
        detail: `Waiting… (${secs(now - (s.parkedAt ?? now))}). If it is running, this pairing may be stale — re-pair this device.`,
        action: 'repair',
      }
    case 'stale-pairing':
      return {
        kind,
        title: 'This pairing no longer matches an engine on the relay',
        detail: 'The engine may have been upgraded or re-identified. Re-pair this device.',
        action: 'repair',
      }
    case 'engine-full':
      return { kind, title: `${engine.name} is serving its maximum number of devices`, detail: `Waiting for a slot.${retry}` }
    case 'revoked':
      return {
        kind,
        title: `${engine.name} refused this device`,
        detail: 'It no longer knows this device (revoked, or its device list was reset). Re-pair this device.',
        action: 'repair',
      }
    case 'engine-gone':
      return { kind, title: `${engine.name} went away`, detail: `Waiting for it to return.${retry}` }
    case 'hub-moved':
      return { kind, title: 'The mesh hub moved', detail: `Reconnecting.${retry}` }
    case 'queue-full':
      return { kind, title: 'Disconnected: the relay queue for this device overflowed', detail: `Reconnecting.${retry}` }
    case 'idle':
      return { kind, title: 'Disconnected after inactivity', detail: `Reconnecting.${retry}` }
    case 'rate-limited':
      return { kind, title: 'The relay is rate-limiting this network', detail: retry.trim() || 'Retrying.' }
    case 'unreachable':
      return { kind, title: `Cannot reach ${engine.relayHost} from this network`, detail: `A proxy may be blocking it.${retry}` }
  }
}

/** Plain-text diagnostics for the "Copy diagnostics" button. */
export function diagnostics(s: LinkState, now: number, engine: { id: string; name: string; relayHost: string; secure: boolean }): string {
  const f = failure(s, now, engine)
  const reason = ['normal', 'queue full', 'idle', 'refused', 'engine gone', 'hub moved']
  return [
    `relay: ${engine.secure ? 'wss' : 'ws'}://${engine.relayHost}/h/${engine.id.slice(0, 8)}…`,
    `engine: ${engine.name} (${engine.id.slice(0, 8)})`,
    `state: ${s.phase}${f ? ` · ${f.kind}` : ''}`,
    `last close: ${s.lastClose ? `${s.lastClose.reason} ${reason[s.lastClose.reason] ?? '?'} at ${new Date(s.lastClose.at).toISOString()}` : 'none'}`,
    `parked: ${s.parkedAt === null ? 'no' : secs(now - s.parkedAt)}`,
    `attempts: ${s.attempts}`,
    `now: ${new Date(now).toISOString()}`,
  ].join('\n')
}
