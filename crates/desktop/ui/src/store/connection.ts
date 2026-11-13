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
  setConnState(state: ConnState): void
  setClient(client: RpcClient | null, api: Api | null): void
  setDiscovery(discovery: Discovery): void
  setInfo(info: Info | null): void
  setError(error?: string): void
}

export type ConnectionApi = StoreApi<ConnectionStore>

/** One engine's connection (31 §7.4); `useConnection` reads the enclosing engine's. */
export const createConnection = (): ConnectionApi =>
  createStore<ConnectionStore>()((set) => ({
    state: 'connecting',
    client: null,
    api: null,
    discovery: disconnected(),
    info: null,
    outage: false,
    setConnState: (state) => set({ state }),
    setClient: (client, api) => set({ client, api }),
    setDiscovery: (discovery) => set({ discovery }),
    setInfo: (info) => set({ info }),
    setError: (error) => set({ error }),
  }))

/** 176 U3: a drop shorter than this shows no toast. */
export const TOAST_GRACE_MS = 3_000
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
  /** The client is gone: cancel the pending toast and outage. */
  stop(): void
}

/**
 * 176 U3: one client's drops as the screen shows them. "Connection lost, reconnecting…" only once a drop has lasted
 * `TOAST_GRACE_MS` (`toast` answers whether it showed one), "Reconnected" only after that toast; `outage` after
 * `ROUTER_GRACE_MS`.
 */
export function watchDrops(conn: ConnectionApi, toast: (text: string) => boolean): Drops {
  let downAt: number | undefined
  let toasted = false
  let toastTimer: number | undefined
  let outageTimer: number | undefined
  const stop = () => {
    clearTimeout(toastTimer)
    clearTimeout(outageTimer)
    toastTimer = outageTimer = undefined
  }
  return {
    next(s) {
      if (s === 'reconnecting' && downAt === undefined) {
        downAt = Date.now()
        toastTimer = window.setTimeout(() => (toasted = toast('Connection lost, reconnecting…')), TOAST_GRACE_MS)
        outageTimer = window.setTimeout(() => conn.setState({ outage: true }), ROUTER_GRACE_MS)
      } else if (s === 'open' && downAt !== undefined) {
        stop()
        conn.setState({ outage: false })
        if (toasted) toast('Reconnected')
        toasted = false
        const gap = Date.now() - downAt
        downAt = undefined
        return gap
      }
      return undefined
    },
    stop,
  }
}

export const useConnection = scoped((st) => st.connection)
