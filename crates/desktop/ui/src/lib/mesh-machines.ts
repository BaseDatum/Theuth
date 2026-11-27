// 227: the machines of the mesh as per-machine Settings lists them (the picker, "Set on all machines", "differs on N"):
// the connected engine and every mesh peer (`mesh_status`), cloud nodes marked from `cloud_list`, each with its
// `engine_caps`. This device's own engine first, then the hub, then the rest by name (cloud nodes last).

import { useQueries } from '@tanstack/react-query'
import { useCloudList } from '@/components/cloud/queries'
import type { CloudList, CloudNode } from '@/components/cloud/types'
import { useMesh } from '@/components/tasks/queries'
import { ENGINE_CALL_ERROR } from '@/lib/engine-call'
import { engineCallApi, type WireEngineCaps } from '@/lib/engine-call.types'
import { useApi } from '@/lib/rpc'
import { LOCAL, LOCAL_NAME, useEngines } from '@/link/engines'
import { RpcError } from '@/rpc/client'
import { supports } from '@/rpc/discovery'
import type { MeshStatus } from '@/rpc/bindings/method.nodes.output'
import { useConnection } from '@/store/connection'

/** `local`: the engine of this device; `hub`: the lease holder; `cloud`: a node of `cloud_list`; `peer`: any other. */
export type MachineKind = 'local' | 'hub' | 'peer' | 'cloud'

export type Machine = {
  /** The mesh engine id (`mesh_status.self_id` / `Peer.id`): what `engine_call` takes. */
  id: string
  name: string
  kind: MachineKind
  /** The engine this client is connected to (the picker's default). */
  connected: boolean
  /** Holds the hub lease (a `local` machine may too). */
  hub: boolean
  online: boolean
  /** Its vault is locked: it answers, but nothing that needs a secret works until it is unlocked. */
  sealed: boolean
  /** How the connected engine reaches it: `direct` (LAN), `relay`; `null` for itself or when offline. */
  path: 'direct' | 'relay' | null
  caps?: WireEngineCaps
}

export const machineKeys = {
  caps: (id: string) => ['engine-caps', id] as const,
}

/** 227: whether the connected engine forwards Settings calls (`engine_call`); without it there is no picker. */
export function useEngineCallSupport(): boolean {
  return useConnection((s) => s.discovery.phase === 'ready' && supports(s.discovery, { methods: ['engine_call' as never] }))
}

type Inputs = {
  mesh: MeshStatus | undefined
  cloud?: CloudList
  /** This device runs the connected engine (the desktop's own daemon). */
  local: boolean
  localName?: string
  caps?: Readonly<Record<string, WireEngineCaps | undefined>>
  /** Engines whose `engine_caps` answered "sealed". */
  sealed?: ReadonlySet<string>
}

/** The machines in picker order (see the module comment); `[]` before `mesh_status` answered. */
export function meshMachines({ mesh, cloud, local, localName = LOCAL_NAME, caps = {}, sealed = new Set() }: Inputs): Machine[] {
  if (!mesh) return []
  const holder = mesh.lease.holder
  const nodes = new Map((cloud?.nodes ?? []).flatMap((n): [string, CloudNode][] => (n.peer_id ? [[n.peer_id, n]] : [])))
  const self: Machine = {
    id: mesh.self_id,
    name: local ? localName : mesh.self_name,
    kind: local ? 'local' : holder === mesh.self_id ? 'hub' : nodes.has(mesh.self_id) ? 'cloud' : 'peer',
    connected: true,
    hub: holder === mesh.self_id,
    online: true,
    sealed: sealed.has(mesh.self_id) || nodes.get(mesh.self_id)?.sealed === true,
    path: null,
    caps: caps[mesh.self_id],
  }
  const peers = mesh.peers.map((p): Machine => {
    const node = nodes.get(p.id)
    return {
      id: p.id,
      name: p.name,
      kind: holder === p.id ? 'hub' : node ? 'cloud' : 'peer',
      connected: false,
      hub: holder === p.id,
      online: p.reachable && p.path !== 'offline',
      sealed: sealed.has(p.id) || node?.sealed === true,
      path: !p.reachable || p.path === 'offline' ? null : p.path === 'lan' ? 'direct' : 'relay',
      caps: caps[p.id],
    }
  })
  const rank = (m: Machine) => (m.kind === 'local' ? 0 : m.hub ? 1 : m.kind === 'cloud' ? 3 : 2)
  return [self, ...peers].sort((a, b) => rank(a) - rank(b) || a.name.localeCompare(b.name))
}

/** 227: `meshMachines` of the connected engine, live (mesh and cloud polls, `engine_caps` of each online machine). */
export function useMeshMachines(): Machine[] {
  const api = useApi()
  const client = useConnection((s) => s.client)
  const full = engineCallApi(api, client ? (m, p) => client.call(m, p) : undefined)
  const forwarding = useEngineCallSupport()
  const active = useEngines((s) => s.active)
  const mesh = useMesh().data
  const cloud = useCloudList().data
  const ids = mesh ? [mesh.self_id, ...mesh.peers.filter((p) => p.reachable).map((p) => p.id)] : []
  const answers = useQueries({
    queries: ids.map((id) => ({
      enabled: forwarding,
      queryKey: machineKeys.caps(id),
      queryFn: () => (id === mesh?.self_id ? full.engine_caps() : (full.engine_call(id, 'engine_caps', {}) as Promise<WireEngineCaps>)),
      staleTime: 5 * 60_000,
      retry: false,
    })),
  })
  const caps: Record<string, WireEngineCaps | undefined> = {}
  const sealed = new Set<string>()
  answers.forEach((q, i) => {
    caps[ids[i]!] = q.data
    if (q.error instanceof RpcError && q.error.code === ENGINE_CALL_ERROR.sealed) sealed.add(ids[i]!)
  })
  return meshMachines({ mesh, cloud, local: active === LOCAL, caps, sealed })
}
