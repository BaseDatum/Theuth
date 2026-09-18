// Per-harness state (31 §7.4): every event-fed store and the React Query cache
// are created per harness and held here. `HarnessProvider` puts one harness's
// set in context; the store hooks (`useSessions`, `useConnection`, …) read from
// it, and their static `getState`/`setState`/`subscribe` address the active
// harness — components are only ever mounted under the active provider.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { type ReactNode, createContext, useContext } from 'react'
import { type StoreApi, useStore } from 'zustand'
import { type SearchApi, createSearchState } from '@/lib/search'
import { useHarnesses } from '@/link/harnesses'
import {
  type ApprovalsApi,
  type AsksApi,
  type SecretRequestsApi,
  createApprovals,
  createAsks,
  createSecretRequests,
} from './approvals'
import { type ConnectionApi, createConnection } from './connection'
import { type SessionsApi, createSessions } from './sessions'

/** One harness's stores and query cache. */
export type Stores = {
  id: string
  sessions: SessionsApi
  approvals: ApprovalsApi
  asks: AsksApi
  secretRequests: SecretRequestsApi
  connection: ConnectionApi
  qc: QueryClient
  /** Transient search state: remembered categories, last view, origin of a search-opened screen (49 §7). */
  search: SearchApi
  /** The live `subscribe_events` unsubscribe, while this harness is the active one. */
  attached: Promise<() => Promise<void>> | null
}

const scopes = new Map<string, Stores>()

/** The stores for `id` (`'local'` or a harness id), created on first use. */
export function scope(id: string): Stores {
  let st = scopes.get(id)
  if (!st) {
    st = {
      id,
      sessions: createSessions(),
      approvals: createApprovals(),
      asks: createAsks(),
      secretRequests: createSecretRequests(),
      connection: createConnection(),
      search: createSearchState(),
      qc: new QueryClient({ defaultOptions: { queries: { staleTime: 30_000, retry: 1 } } }),
      attached: null,
    }
    scopes.set(id, st)
  }
  return st
}

/** Forget a removed harness's state; its client must already be closed. */
export function dropScope(id: string): void {
  scopes.get(id)?.qc.clear()
  scopes.delete(id)
}

/** The active harness's stores (what the static store methods address). */
export function active(): Stores {
  return scope(useHarnesses.getState().active)
}

const HarnessContext = createContext<Stores | null>(null)

/** The enclosing harness's stores; outside a provider, the active harness's. */
export function useScope(): Stores {
  return useContext(HarnessContext) ?? active()
}

/** Mount with `key={harness}` so a switch remounts the tree with the other harness's stores and query cache. */
export function HarnessProvider({ harness, children }: { harness: string; children: ReactNode }) {
  const st = scope(harness)
  return (
    <HarnessContext value={st}>
      <QueryClientProvider client={st.qc}>{children}</QueryClientProvider>
    </HarnessContext>
  )
}

/** A `create()`-shaped hook whose store is picked from the enclosing (or active) harness's `Stores`. */
export type ScopedStore<T> = {
  (): T
  <U>(selector: (state: T) => U): U
} & StoreApi<T>

/** Bind a hook + static store API to `pick(scope)` — the seam that keeps component imports unchanged. */
export function scoped<T>(pick: (st: Stores) => StoreApi<T>): ScopedStore<T> {
  const hook = <U,>(selector: (s: T) => U = (s) => s as unknown as U) => useStore(pick(useScope()), selector)
  // Live getters: `useSessions.getState()` always addresses the active harness's store. (Store modules
  // call `scoped` while this module may still be evaluating, so nothing here reads a module-level `const`.)
  for (const k of ['getState', 'getInitialState', 'setState', 'subscribe'] as const) {
    Object.defineProperty(hook, k, { get: () => pick(active())[k] })
  }
  // The hook and the static API are one callable object, as zustand's `create()` returns.
  return hook as unknown as ScopedStore<T>
}
