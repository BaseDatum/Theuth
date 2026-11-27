// 227: the machines per-machine Settings lists (the picker, "Set on all machines", "differs on N").
// - This device's own engine (`LOCAL`, when the container has one) first, labelled `LOCAL_NAME`, always reached through
//   its own client. A device is in at most one mesh: when its engine is a member of the mesh shown it appears once (as
//   `LOCAL`, not again as a peer); when it is not, it is still listed first, noted "not in this mesh" / "in <mesh>".
// - The mesh shown: the active engine's; with `LOCAL` active, the local engine's own mesh when it has one, else the first
//   paired mesh. Its connected engine is reached through that client; every other member through its `engine_call`.
// - Then the hub, the rest by name, cloud nodes (marked from `cloud_list`) last. Each online machine with its `engine_caps`.

import { type QueryClient, useQueries, useQuery } from '@tanstack/react-query'
import { useStore } from 'zustand'
import { cloudKeys } from '@/components/cloud/queries'
import type { CloudList, CloudNode } from '@/components/cloud/types'
import { ENGINE_CALL_ERROR } from '@/lib/engine-call'
import type { Api } from '@/rpc/bindings/client'
import type { EngineCapsResult } from '@/rpc/bindings/method.agent.output'
import { LOCAL, LOCAL_NAME, hasLocal, useEngines } from '@/link/engines'
import { RpcError } from '@/rpc/client'
import { type Discovery, supports } from '@/rpc/discovery'
import type { MeshStatus } from '@/rpc/bindings/method.nodes.output'
import { scope } from '@/store/engine-scope'

/** `local`: the engine of this device; `hub`: the lease holder; `cloud`: a node of `cloud_list`; `peer`: any other. */
export type MachineKind = 'local' | 'hub' | 'peer' | 'cloud'

export type Machine = {
  /** The mesh engine id (`mesh_status.self_id` / `Peer.id`): what `engine_call` takes. `LOCAL` for this device's engine before its id is known. */
  id: string
  name: string
  kind: MachineKind
  /** The engine the app is showing (the active engine): the picker's default. */
  connected: boolean
  /** The engine scope (`store/engine-scope`) whose client reaches it: `LOCAL` or a paired engine's id. */
  client: string
  /** Reached through that client's `engine_call` (a mesh member other than the engine the client is connected to). */
  forwarded: boolean
  /** Holds the hub lease (a `local` machine may too). */
  hub: boolean
  online: boolean
  /** Its vault is locked: it answers, but nothing that needs a secret works until it is unlocked. */
  sealed: boolean
  /** How the mesh reaches it: `direct` (LAN), `relay`; `null` for an engine reached by its own client or when offline. */
  path: 'direct' | 'relay' | null
  /** This device's engine outside the mesh shown: "not in this mesh" or "in <mesh>". */
  note?: string
  caps?: EngineCapsResult
}

export const machineKeys = {
  caps: (id: string) => ['engine-caps', id] as const,
}

/** This device's engine as `meshMachines` needs it: reachable, and the mesh it is in (from its own `mesh_status`). */
export type LocalEngine = { online: boolean; meshId: string | null; meshName: string | null; name?: string }

export type MachinesInput = {
  /** The mesh shown, from its client's `mesh_status`; `undefined` before it answered (or with no mesh client). */
  mesh: MeshStatus | undefined
  cloud?: CloudList
  /** The engine scope whose client gave `mesh`. */
  meshClient: string
  /** The active engine scope (`useEngines().active`). */
  active: string
  /** This device's engine, `null` when the container has none. */
  local: LocalEngine | null
  /** `false`: the mesh client cannot `engine_call`, so only the engines reached by a client of their own are listed. */
  forwarding?: boolean
  caps?: Readonly<Record<string, EngineCapsResult | undefined>>
  /** Engines whose `engine_caps` answered "sealed". */
  sealed?: ReadonlySet<string>
}

const rank = (m: Machine) => (m.kind === 'local' ? 0 : m.hub ? 1 : m.kind === 'cloud' ? 3 : 2)

/** The machines in picker order (see the module comment). */
export function meshMachines({ mesh, cloud, meshClient, active, local, forwarding = true, caps = {}, sealed = new Set() }: MachinesInput): Machine[] {
  const holder = mesh?.lease.holder ?? null
  const nodes = new Map((cloud?.nodes ?? []).flatMap((n): [string, CloudNode][] => (n.peer_id ? [[n.peer_id, n]] : [])))
  const isSealed = (id: string) => sealed.has(id) || nodes.get(id)?.sealed === true
  // The local engine is a member of the mesh shown: it is the mesh client itself, or its id is the mesh's self or a peer.
  const localId = meshClient === LOCAL ? (mesh?.self_id ?? local?.meshId ?? null) : (local?.meshId ?? null)
  const member = !!mesh && !!localId && (localId === mesh.self_id || mesh.peers.some((p) => p.id === localId))
  const machines: Machine[] = []
  if (local) {
    const id = localId ?? LOCAL
    machines.push({
      id,
      name: LOCAL_NAME,
      kind: 'local',
      connected: active === LOCAL,
      client: LOCAL,
      forwarded: false,
      hub: member && holder === id,
      online: local.online,
      sealed: isSealed(id),
      path: null,
      note: !mesh || member ? undefined : local.meshName ? `in ${local.meshName}` : 'not in this mesh',
      caps: caps[id],
    })
  }
  if (mesh && meshClient !== LOCAL) {
    const self = mesh.self_id
    if (!(member && localId === self))
      machines.push({
        id: self,
        name: mesh.self_name,
        kind: holder === self ? 'hub' : nodes.has(self) ? 'cloud' : 'peer',
        connected: active === meshClient,
        client: meshClient,
        forwarded: false,
        hub: holder === self,
        online: true,
        sealed: isSealed(self),
        path: null,
        caps: caps[self],
      })
  }
  if (mesh && forwarding) {
    for (const p of mesh.peers) {
      // Shown once: this device's engine is the `local` row, reached through its own client.
      if (member && p.id === localId) continue
      const online = p.reachable && p.path !== 'offline'
      machines.push({
        id: p.id,
        name: p.name,
        kind: holder === p.id ? 'hub' : nodes.has(p.id) ? 'cloud' : 'peer',
        connected: false,
        client: meshClient,
        forwarded: true,
        hub: holder === p.id,
        online,
        sealed: isSealed(p.id),
        path: !online ? null : p.path === 'lan' ? 'direct' : 'relay',
        caps: caps[p.id],
      })
    }
  }
  return machines.sort((a, b) => rank(a) - rank(b) || a.name.localeCompare(b.name))
}

/** Whether `discovery` advertises `engine_call` (227); without it a client reaches only its own engine. */
export const forwards = (discovery: Discovery): boolean => discovery.phase === 'ready' && supports(discovery, { methods: ['engine_call' as never] })

/** The live machine list and how to reach them. */
export type MeshMachines = {
  machines: Machine[]
  /** The engine scope whose client shows the mesh (`engine_call_all`, `settings_values`, forwarding). */
  meshClient: string
  /** The mesh client's `Api` and query cache; `null` api before it connected. */
  meshApi: Api | null
  meshQc: QueryClient
  /** This device's engine's `Api` when the container has one and it is connected. */
  localApi: Api | null
}

function useClient(id: string) {
  const st = scope(id)
  const api = useStore(st.connection, (s) => (s.state === 'open' ? s.api : null))
  const discovery = useStore(st.connection, (s) => s.discovery)
  return { api, discovery, qc: st.qc }
}

/** 227: `meshMachines`, live: both clients' `mesh_status`, the cloud list, `engine_caps` of each online machine. */
export function useMeshMachines(): MeshMachines {
  const active = useEngines((s) => s.active)
  const firstMesh = useEngines((s) => s.engines.find((h) => h.kind === 'mesh' && h.pairedAt > 0)?.id ?? null)
  const withLocal = hasLocal()
  const local = useClient(LOCAL)
  const localMesh = useQuery({ enabled: withLocal && !!local.api, queryKey: ['mesh'], queryFn: () => local.api!.mesh_status(), retry: false }, local.qc)
  // LOCAL active: its own mesh when it is in one, else the first paired mesh (one mesh is shown at a time).
  const localInMesh = (localMesh.data?.peers.length ?? 0) > 0
  const meshClient = active !== LOCAL ? active : localInMesh || !firstMesh ? LOCAL : firstMesh
  const viewed = useClient(meshClient)
  const mesh = useQuery({ enabled: !!viewed.api, queryKey: ['mesh'], queryFn: () => viewed.api!.mesh_status(), retry: false }, viewed.qc).data
  const cloud = useQuery(
    { enabled: !!viewed.api && supports(viewed.discovery, { methods: ['cloud_list'] }), queryKey: cloudKeys.list, queryFn: () => viewed.api!.cloud_list(), refetchInterval: 60_000, retry: false },
    viewed.qc,
  ).data
  const localEngine: LocalEngine | null = withLocal
    ? { online: !!local.api, meshId: localMesh.data?.self_id ?? null, meshName: localInMesh ? (localMesh.data!.mesh_name ?? localMesh.data!.self_name) : null }
    : null
  const forwarding = forwards(viewed.discovery)
  const input: MachinesInput = { mesh, cloud, meshClient, active, local: localEngine, forwarding }
  const reachable = meshMachines(input).filter((m) => m.online)
  const answers = useQueries(
    {
      queries: reachable.map((m) => ({
        enabled: forwarding || !m.forwarded,
        queryKey: machineKeys.caps(m.id),
        queryFn: (): Promise<EngineCapsResult> => {
          const api = m.client === LOCAL ? local.api : viewed.api
          if (!api) return Promise.reject(new Error('not connected'))
          return m.forwarded ? (api.engine_call(m.id, 'engine_caps', {}) as Promise<EngineCapsResult>) : api.engine_caps()
        },
        staleTime: 5 * 60_000,
        retry: false,
      })),
    },
    viewed.qc,
  )
  const caps: Record<string, EngineCapsResult | undefined> = {}
  const sealed = new Set<string>()
  answers.forEach((q, i) => {
    const id = reachable[i]!.id
    caps[id] = q.data
    if (q.error instanceof RpcError && q.error.code === ENGINE_CALL_ERROR.sealed) sealed.add(id)
  })
  return { machines: meshMachines({ ...input, caps, sealed }), meshClient, meshApi: viewed.api, meshQc: viewed.qc, localApi: local.api }
}
