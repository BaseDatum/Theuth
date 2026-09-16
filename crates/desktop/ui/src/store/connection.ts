import { type StoreApi, createStore } from 'zustand'
import type { Api } from '@/rpc/api'
import type { ConnState, RpcClient } from '@/rpc/client'
import type { Info } from '@/rpc/extra'
import { scoped } from './harness-scope'

type ConnectionStore = {
  state: ConnState
  client: RpcClient | null
  api: Api | null
  /** `info()` reply; `null` until the first connection. */
  info: Info | null
  /** Last launch or socket failure shown by the startup screen. */
  error?: string
  setConnState(state: ConnState): void
  setClient(client: RpcClient | null, api: Api | null): void
  setInfo(info: Info | null): void
  setError(error?: string): void
}

export type ConnectionApi = StoreApi<ConnectionStore>

/** One harness's connection (31 §7.4); `useConnection` reads the enclosing harness's. */
export const createConnection = (): ConnectionApi =>
  createStore<ConnectionStore>()((set) => ({
    state: 'connecting',
    client: null,
    api: null,
    info: null,
    setConnState: (state) => set({ state }),
    setClient: (client, api) => set({ client, api }),
    setInfo: (info) => set({ info }),
    setError: (error) => set({ error }),
  }))

export const useConnection = scoped((st) => st.connection)
