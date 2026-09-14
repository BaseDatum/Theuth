import { create } from 'zustand'
import type { Api } from '@/rpc/api'
import type { ConnState, RpcClient } from '@/rpc/client'
import type { Info } from '@/rpc/extra'

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

export const useConnection = create<ConnectionStore>()((set) => ({
  state: 'connecting',
  client: null,
  api: null,
  info: null,
  setConnState: (state) => set({ state }),
  setClient: (client, api) => set({ client, api }),
  setInfo: (info) => set({ info }),
  setError: (error) => set({ error }),
}))
