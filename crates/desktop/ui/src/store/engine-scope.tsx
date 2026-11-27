// Per-engine state (31 §7.4): every event-fed store and the React Query cache
// are created per engine and held here. `EngineProvider` puts one engine's
// set in context; the store hooks (`useSessions`, `useConnection`, …) read from
// it, and their static `getState`/`setState`/`subscribe` address the active
// engine — components are only ever mounted under the active provider.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { type ReactNode, createContext, useContext } from 'react'
import { type StoreApi, useStore } from 'zustand'
import { type SearchApi, createSearchState } from '@/lib/search'
import { useEngines } from '@/link/engines'
import { RpcError } from '@/rpc/client'
import { UnsupportedCapability } from '@/rpc/discovery'
import {
  type ApprovalsApi,
  type AsksApi,
  type SecretRequestsApi,
  createApprovals,
  createAsks,
  createSecretRequests,
} from './approvals'
import { type ConnectionApi, createConnection } from './connection'
import { type ElementFormsApi, createElementForms } from './element-forms'
import { type SessionsApi, createSessions } from './sessions'

/** One engine's stores and query cache. */
export type Stores = {
  id: string
  sessions: SessionsApi
  approvals: ApprovalsApi
  asks: AsksApi
  secretRequests: SecretRequestsApi
  /** 209 §4.10: the form cards of `browser_show_element`. */
  elementForms: ElementFormsApi
  connection: ConnectionApi
  qc: QueryClient
  /** Transient search state: remembered categories, last view, origin of a search-opened screen (49 §7). */
  search: SearchApi
  /** The negotiated topic/legacy feed unsubscribe, while this engine is active. */
  attached: Promise<() => Promise<void>> | null
}

const scopes = new Map<string, Stores>()

/** The stores for `id` (`'local'` or an engine id), created on first use. */
export function scope(id: string): Stores {
  let st = scopes.get(id)
  if (!st) {
    st = {
      id,
      sessions: createSessions(),
      approvals: createApprovals(),
      asks: createAsks(),
      secretRequests: createSecretRequests(),
      elementForms: createElementForms(),
      connection: createConnection(),
      search: createSearchState(),
      qc: new QueryClient({ defaultOptions: { queries: { staleTime: 30_000, retry: (count, error) => !(error instanceof UnsupportedCapability || error instanceof RpcError && error.code === -32601) && count < 1 } } }),
      attached: null,
    }
    scopes.set(id, st)
  }
  return st
}

/** Forget a removed engine's state; its client must already be closed. */
export function dropScope(id: string): void {
  scopes.get(id)?.qc.clear()
  scopes.delete(id)
}

/** The active engine's stores (what the static store methods address). */
export function active(): Stores {
  return scope(useEngines.getState().active)
}

const EngineContext = createContext<Stores | null>(null)

/** The enclosing engine's stores; outside a provider, the active engine's. */
export function useScope(): Stores {
  return useContext(EngineContext) ?? active()
}

/** Mount with `key={engine}` so a switch remounts the tree with the other engine's stores and query cache. */
export function EngineProvider({ engine, children }: { engine: string; children: ReactNode }) {
  return <StoresProvider stores={scope(engine)}>{children}</StoresProvider>
}

/** `stores` in context. 227: a Settings tab aimed at another machine of the mesh reads that machine's connection and query cache. */
export function StoresProvider({ stores, children }: { stores: Stores; children: ReactNode }) {
  return (
    <EngineContext value={stores}>
      <QueryClientProvider client={stores.qc}>{children}</QueryClientProvider>
    </EngineContext>
  )
}

/** A `create()`-shaped hook whose store is picked from the enclosing (or active) engine's `Stores`. */
export type ScopedStore<T> = {
  (): T
  <U>(selector: (state: T) => U): U
} & StoreApi<T>

/** Bind a hook + static store API to `pick(scope)` — the seam that keeps component imports unchanged. */
export function scoped<T>(pick: (st: Stores) => StoreApi<T>): ScopedStore<T> {
  const hook = <U,>(selector: (s: T) => U = (s) => s as unknown as U) => useStore(pick(useScope()), selector)
  // Live getters: `useSessions.getState()` always addresses the active engine's store. (Store modules
  // call `scoped` while this module may still be evaluating, so nothing here reads a module-level `const`.)
  for (const k of ['getState', 'getInitialState', 'setState', 'subscribe'] as const) {
    Object.defineProperty(hook, k, { get: () => pick(active())[k] })
  }
  // The hook and the static API are one callable object, as zustand's `create()` returns.
  return hook as unknown as ScopedStore<T>
}
