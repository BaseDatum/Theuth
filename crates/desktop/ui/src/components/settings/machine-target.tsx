// 227: which machine Settings edits. The pick lives per window (this JS context) and defaults to the active engine.
// A `machine` tab aimed elsewhere renders inside `MachineScope`: an engine with a client of its own (this device's
// engine, the mesh's engine while this device's is active) under that client's stores; any other mesh member under a
// connection store whose `api` forwards every call through the mesh client's `engine_call`, with its own query cache,
// refetched every 5 s and invalidated after every write.

import { MutationCache, QueryClient, useQuery } from '@tanstack/react-query'
import { createContext, type ReactNode, useContext, useEffect, useMemo } from 'react'
import { create } from 'zustand'
import type { SetOnAllRequest } from '@/components/settings/copy/SetOnAllSheet'
import type { SettingsTab } from '@/components/settings/types'
import { forEngine, isEngineCallError } from '@/lib/engine-call'
import { type Machine, useMeshMachines } from '@/lib/mesh-machines'
import type { Api } from '@/rpc/bindings/client'
import { type ConnectionApi, connectionOfApi, createConnection } from '@/store/connection'
import { type Stores, StoresProvider, scope } from '@/store/engine-scope'

/** While a remote machine's tab is open, its queries refetch this often. */
export const REMOTE_REFETCH_MS = 5_000

/** The picked machine of this window; `null` is the connected engine. */
export const usePickedMachine = create<{ picked: string | null; pick(id: string | null): void }>()((set) => ({
  picked: null,
  pick: (picked) => set({ picked }),
}))

export type SettingsMachines = {
  /** Every machine (`useMeshMachines`): this device's engine, the mesh shown's engine, and the members its client can `engine_call`. */
  machines: Machine[]
  /** The machine `machine` tabs edit: the pick when it is still listed, else the active engine. */
  target: Machine | null
  /** The active engine's machine (the picker's default). */
  connected: Machine | null
  /** The mesh client's own `Api` (never forwarded), with the 227 methods: `engine_call_all`, `settings_values`, forwarding. */
  api: Api | null
  /** This device's engine's `Api`, when the container has one: its setters and getters are called on it directly. */
  local: Api | null
  /** The engine scope whose client shows the mesh. */
  meshClient: string
  /** The mesh client's query cache (a remote tab's own is not where "differs on N" lives). */
  qc: QueryClient
  /** The open tab, for its `copyable` and `needs`. */
  tab: SettingsTab | null
  /** Open the "Set on all machines…" sheet. */
  open(request: SetOnAllRequest): void
}

const MachinesContext = createContext<SettingsMachines | null>(null)

export const SettingsMachinesProvider = MachinesContext.Provider

/** The Settings page's machines; `null` outside it (a settings pane rendered elsewhere has no picker). */
export function useSettingsMachines(): SettingsMachines | null {
  return useContext(MachinesContext)
}

/** The machines, the window's pick, the target: the Settings page's `SettingsMachines`. */
export function useSettingsMachinesValue(tab: SettingsTab | null, open: (request: SetOnAllRequest) => void): SettingsMachines {
  const { machines, meshClient, meshApi, meshQc, localApi } = useMeshMachines()
  const picked = usePickedMachine((s) => s.picked)
  const connected = machines.find((m) => m.connected) ?? null
  const target = machines.find((m) => m.id === picked) ?? connected
  return { machines, target, connected, api: meshApi, local: localApi, meshClient, qc: meshQc, tab, open }
}

/** Whether `machine` is not the connected engine (its tab reads through `engine_call`). */
export const isRemote = (machine: Machine | null): machine is Machine => !!machine && !machine.connected

const remote = new WeakMap<Api, Map<string, Stores>>()

/**
 * The stores of `base`'s engine aimed at `target`: the same event stores, a connection mirroring `base`'s (state,
 * discovery) whose `api` forwards to `target`, and a query cache of its own. Cached per connected `api` and target.
 */
export function remoteStores(base: Stores, target: string, self: string, api: Api): Stores {
  let byTarget = remote.get(api)
  if (!byTarget) remote.set(api, (byTarget = new Map()))
  const hit = byTarget.get(target)
  if (hit) return hit
  const connection = createConnection()
  const forwarded = forEngine(api, target, self)
  connectionOfApi.set(forwarded, connection)
  mirror(base.connection, connection, forwarded)
  const qc: QueryClient = new QueryClient({
    defaultOptions: { queries: { staleTime: 0, refetchInterval: REMOTE_REFETCH_MS, retry: false } },
    // "After writes invalidate that target's queries": whatever a setter touched there, the tab re-reads it.
    mutationCache: new MutationCache({ onSuccess: () => void qc.invalidateQueries() }),
  })
  const stores: Stores = { ...base, id: `${base.id}@${target}`, connection, qc }
  byTarget.set(target, stores)
  return stores
}

/** Copy `from`'s connection state into `to`, keeping `to`'s forwarded `api` and its own `info`. */
function mirror(from: ConnectionApi, to: ConnectionApi, api: Api): void {
  const copy = () => {
    const { state, client, discovery } = from.getState()
    to.setState({ state, client, discovery, api })
  }
  copy()
  from.subscribe(copy)
}

/** Why a remote machine's tab cannot load, from its `info()` failure. */
export function unreachableText(e: unknown, name: string): string {
  if (isEngineCallError(e, 'offline')) return `${name} is offline. Its settings show when it reconnects.`
  if (isEngineCallError(e, 'sealed')) return `${name}'s vault is locked. Unlock it to edit its settings.`
  if (isEngineCallError(e, 'notForwardable')) return `${name}'s settings cannot be edited from here.`
  return `${name} did not answer: ${e instanceof Error ? e.message : String(e)}`
}

/**
 * The machine a tab edits: the target for a `machine` tab; `null` (the connected engine, no forwarding) for any other
 * scope, and for a `connectedOnly` tab whose target only `engine_call` reaches (its methods are not forwardable).
 */
export const aimedAt = (tab: Pick<SettingsTab, 'scope' | 'connectedOnly'>, target: Machine | null): Machine | null =>
  tab.scope !== 'machine' || (tab.connectedOnly && target?.forwarded) ? null : target

/**
 * `children` aimed at `machine`: as is for the active engine; under its own client's stores for an engine with a
 * client of its own (this device's, or the mesh's while this device's is active); else in its remote scope, through
 * the mesh client's `engine_call`, once its `info()` answered.
 */
export function MachineScope({ machine, children }: { machine: Machine | null; children: ReactNode }) {
  const machines = useSettingsMachines()
  if (!machines || !machine || machine.connected) return children
  if (!machine.forwarded) return <StoresProvider stores={scope(machine.client)}>{children}</StoresProvider>
  const self = machines.machines.find((m) => m.client === machines.meshClient && !m.forwarded)
  if (!self || !machines.api) return <p role="status" data-machine-unreachable="" className="py-4 text-sm text-fg-tertiary">{machine.name} cannot be reached from here.</p>
  return (
    <RemoteScope machine={machine} self={self.id} api={machines.api}>
      {children}
    </RemoteScope>
  )
}

function RemoteScope({ machine, self, api, children }: { machine: Machine; self: string; api: Api; children: ReactNode }) {
  const base = scope(machine.client)
  const stores = useMemo(() => remoteStores(base, machine.id, self, api), [base, machine.id, self, api])
  const forwarded = stores.connection.getState().api!
  const info = useQuery({ queryKey: ['info'], queryFn: () => forwarded.info() }, stores.qc)
  useEffect(() => {
    if (info.data) stores.connection.getState().setInfo(info.data)
  }, [info.data, stores])
  if (info.error && !info.data)
    return (
      <p role="status" data-machine-unreachable="" className="rounded-[10px] border border-border-subtle bg-layer-2 px-3 py-2 text-sm text-fg-secondary">
        {unreachableText(info.error, machine.name)}
      </p>
    )
  if (!info.data) return <p role="status" className="py-4 text-sm text-fg-tertiary">Loading {machine.name}…</p>
  return <StoresProvider stores={stores}>{children}</StoresProvider>
}
