import { type StoreApi, createStore } from 'zustand'
import type { Api } from '@/rpc/bindings/client'
import type { ConnState, RpcClient } from '@/rpc/client'
import { disconnected, type Discovery } from '@/rpc/discovery'
import type { Info } from "@/rpc/bindings/method.rpc_2dcompose.output"
import { scoped } from './engine-scope'

type ConnectionStore = {
  state: ConnState
  client: RpcClient | null
  api: Api | null
  discovery: Discovery
  /** `info()` reply; `null` until the first connection. */
  info: Info | null
  /** Last launch or socket failure shown by the startup screen. */
  error?: string
  /** 176 U3: the socket has been down past `ROUTER_GRACE_MS`; the local engine's router gives way to the startup screen. */
  outage: boolean
  /** 217 §4: when the client's pending backoff redials; `null` when none is pending. */
  retryAt: number | null
  /** Redials since the last open. */
  attempt: number
  /** Cancel the backoff and dial now. */
  retryNow(): void
  setConnState(state: ConnState): void
  setClient(client: RpcClient | null, api: Api | null): void
  setDiscovery(discovery: Discovery): void
  setInfo(info: Info | null): void
  setError(error?: string): void
}

export type ConnectionApi = StoreApi<ConnectionStore>

/** One engine's connection (31 §7.4); `useConnection` reads the enclosing engine's. */
export const createConnection = (): ConnectionApi =>
  createStore<ConnectionStore>()((set, get) => ({
    state: 'connecting',
    client: null,
    api: null,
    discovery: disconnected(),
    info: null,
    outage: false,
    retryAt: null,
    attempt: 0,
    retryNow: () => get().client?.retryNow(),
    // 217 §4: an open socket has no backoff, whatever order the client's two events arrive in.
    setConnState: (state) => set(state === 'open' ? { state, retryAt: null, attempt: 0 } : { state }),
    setClient: (client, api) => set({ client, api }),
    setDiscovery: (discovery) => set({ discovery }),
    setInfo: (info) => set({ info }),
    setError: (error) => set({ error }),
  }))

/** 176 U3: the local engine keeps its router mounted (with a "Reconnecting…" pill) through a drop this long. */
export const ROUTER_GRACE_MS = 10_000

/**
 * Whether the router (else the startup screen) shows: once `info` arrived, while open; a link engine also through a
 * reconnect unless `refused` (iOS kills the socket on every background, 31 §7.3); the local one for `ROUTER_GRACE_MS`.
 */
export const routerMounted = (s: ConnectionStore, local: boolean, refused: boolean): boolean =>
  s.discovery.phase !== 'failed' && s.info !== null && (s.state === 'open' || (local ? !s.outage : !refused))

export type Drops = {
  /** Feed every client state; an `open` that ends a drop returns its gap in ms. */
  next(s: ConnState): number | undefined
  /** The client is gone: cancel the pending outage. */
  stop(): void
}

/**
 * 176 U3: one client's drops as the screen shows them: `outage` after `ROUTER_GRACE_MS`. 217: the status bar's dot and
 * countdown are the only connection signal; nothing toasts.
 */
export function watchDrops(conn: ConnectionApi): Drops {
  let downAt: number | undefined
  let outageTimer: number | undefined
  const stop = () => {
    clearTimeout(outageTimer)
    outageTimer = undefined
  }
  return {
    next(s) {
      if (s === 'reconnecting' && downAt === undefined) {
        downAt = Date.now()
        outageTimer = window.setTimeout(() => conn.setState({ outage: true }), ROUTER_GRACE_MS)
      } else if (s === 'open' && downAt !== undefined) {
        stop()
        conn.setState({ outage: false })
        const gap = Date.now() - downAt
        downAt = undefined
        return gap
      }
      return undefined
    },
    stop,
  }
}

/**
 * Mirror `client`'s state and backoff into `conn` until `signal` aborts; `onState` sees every state the store takes.
 * 217 §4: a window coming back to the foreground re-reads the client's current state (after the client's own
 * foreground handling, which registered first), so nothing it missed while hidden lingers.
 */
export function mirror(conn: ConnectionApi, client: Pick<RpcClient, 'state' | 'retry' | 'onState' | 'onRetry'>, signal: AbortSignal, onState: (s: ConnState) => void = () => {}): void {
  if (signal.aborted) return
  // Guarded as well as unlistened: an earlier abort listener may close the client before this one runs.
  const apply = (s: ConnState) => {
    if (signal.aborted) return
    conn.getState().setConnState(s)
    onState(s)
  }
  const offState = client.onState(apply)
  const offRetry = client.onRetry((r) => {
    if (!signal.aborted) conn.setState({ retryAt: r.at, attempt: r.attempt })
  })
  const resync = () => {
    if (signal.aborted || document.visibilityState !== 'visible') return
    const s = client.state
    if (conn.getState().state !== s) apply(s)
    const r = client.retry
    conn.setState(s === 'open' ? { retryAt: null, attempt: 0 } : { retryAt: r.at, attempt: r.attempt })
  }
  document.addEventListener('visibilitychange', resync)
  window.addEventListener('pageshow', resync)
  signal.addEventListener('abort', () => {
    offState()
    offRetry()
    document.removeEventListener('visibilitychange', resync)
    window.removeEventListener('pageshow', resync)
  })
}

export const useConnection = scoped((st) => st.connection)
