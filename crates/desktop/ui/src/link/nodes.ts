// Auto-pair with a parent's nodes (32 §8.2): after an engine connects, every node
// it lists becomes an engine of this device — `invite_node` on the parent hands
// out the node's pairing string and the ordinary 31 §7.5 paste path does the rest.
// Offline nodes get a placeholder row (no client) and pair on `NodeOnline`; a
// failed pairing is retried once with a fresh invite, then shown as
// `pairing failed · retry` under the node.

import { useQuery } from '@tanstack/react-query'
import { deviceName } from '@/lib/platform'
import { useApi } from '@/lib/rpc'
import type { Api } from '@/rpc/bindings/client'
import type { Node } from "@/rpc/bindings/method.nodes.output"
import type { NodeEvent } from "@/rpc/bindings/event.nodes_2eevents.output"
import { RpcError } from '@/rpc/client'
import { useConnection } from '@/store/connection'
import { scope } from '@/store/engine-scope'
import { on } from '@/store/sessions'
import { type Engine, useEngines } from './engines'
import { pair } from './pairing'

export type NodesApi = Pick<Api, 'list_nodes' | 'invite_node'>
/** The pairing function (`pair` from `pairing.ts`); a test seam. */
export type PairFn = (key: string, deviceName: string) => Promise<Engine>

/** Node ids waiting for the parent's `NodeOnline{true}` before the invite. */
const pending = new Set<string>()
/** Invites in flight: the parent's `NodeAdded` and the phone's `add_node` reply both start one. */
const inflight = new Set<string>()

/** A row for a node this device has not paired with yet: `pairedAt: 0`, so `App` opens no client. */
function placeholder(node: Pick<Node, 'id' | 'name' | 'relay'>, parent: string, engines: Engine[]): Engine {
  const relay = new URL(node.relay)
  return {
    id: node.id,
    name: node.name,
    relayHost: relay.host,
    secure: relay.protocol === 'wss:' || relay.protocol === 'https:',
    pairedAt: 0,
    lastSeen: 0,
    order: engines.reduce((m, x) => Math.max(m, x.order + 1), 0),
    parent,
    kind: 'engine',
  }
}

/**
 * After `parentId` connected and `info` resolved: list its nodes and adopt each.
 * An engine without the `nodes` plugin answers -32601 → remembered as such, nothing
 * else; any other failure (the connection dropped) is retried on the next `open`.
 */
export async function autoPair(parentId: string, api: NodesApi, pairFn: PairFn = pair): Promise<void> {
  let nodes: Node[]
  try {
    nodes = await api.list_nodes()
  } catch (e) {
    if (e instanceof RpcError && e.code === -32601) useEngines.getState().setNodes(parentId, false)
    return
  }
  useEngines.getState().setNodes(parentId, true)
  await Promise.all(nodes.map((n) => adopt(parentId, api, n, pairFn)))
}

/**
 * One node of `parentId` as an engine of this device: already paired (a scan earlier,
 * or an earlier adopt) → only `parent` is set; offline → placeholder row until
 * `NodeOnline`; online → invite and pair. Revoked nodes cannot invite and are skipped.
 */
export async function adopt(parentId: string, api: NodesApi, node: Node, pairFn: PairFn = pair): Promise<void> {
  const s = useEngines.getState()
  const known = s.engines.find((h) => h.id === node.id)
  if (known && known.pairedAt > 0) {
    s.setParent(node.id, parentId)
    return
  }
  if (node.revoked) return
  if (!node.online) {
    if (!known) s.add(placeholder(node, parentId, s.engines))
    s.setPresence(node.id, 'offline')
    pending.add(node.id)
    return
  }
  await invite(parentId, api, node, pairFn)
}

/** `invite_node` → `pair` with the returned key; a failure gets one fresh invite after 1–5 s (32 §4.4), then `pairError`. */
async function invite(parentId: string, api: NodesApi, node: Pick<Node, 'id' | 'name' | 'relay'>, pairFn: PairFn): Promise<void> {
  if (inflight.has(node.id)) return
  inflight.add(node.id)
  pending.delete(node.id)
  const s = useEngines.getState()
  try {
    try {
      await pairFn((await api.invite_node(node.id)).key, deviceName())
    } catch {
      const { promise, resolve } = Promise.withResolvers<void>()
      setTimeout(resolve, 1000 + Math.random() * 4000)
      await promise
      await pairFn((await api.invite_node(node.id)).key, deviceName())
    }
    s.setParent(node.id, parentId)
    s.setPairError(node.id, undefined)
  } catch (e) {
    // A row to hang the `pairing failed · retry` state on.
    const { engines } = useEngines.getState()
    if (!engines.some((h) => h.id === node.id)) s.add(placeholder(node, parentId, engines))
    s.setPairError(node.id, e instanceof Error ? e.message : String(e))
  } finally {
    inflight.delete(node.id)
  }
}

/** The parent's `Node.relay` form of a stored row, for a re-invite. */
const relayUrl = (h: Engine) => `${h.secure ? 'wss' : 'ws'}://${h.relayHost}`

/** The switcher's `retry` on a failed row: a fresh invite through the node's parent. */
export function retryPair(id: string): void {
  const h = useEngines.getState().engines.find((x) => x.id === id)
  const api = h?.parent ? scope(h.parent).connection.getState().api : null
  if (!h?.parent || !api) return
  useEngines.getState().setPairError(id, undefined)
  void invite(h.parent, api, { id: h.id, name: h.name, relay: relayUrl(h) }, pair)
}

/** A `NodeEvent` from `parentId`'s stream (32 §8.2). */
export function onNodeEvent(parentId: string, api: NodesApi, e: NodeEvent, pairFn: PairFn = pair): void {
  const s = useEngines.getState()
  switch (e.type) {
    case 'NodeAdded':
      void adopt(parentId, api, e.node, pairFn)
      return
    case 'NodeOnline': {
      const h = s.engines.find((x) => x.id === e.id)
      if (e.online && h && pending.has(e.id)) void invite(parentId, api, { id: h.id, name: h.name, relay: relayUrl(h) }, pairFn)
      return
    }
    case 'NodeRemoved':
    case 'NodeRevoked': {
      pending.delete(e.id)
      const h = s.engines.find((x) => x.id === e.id)
      if (!h) return
      // Never paired: nothing of ours to keep. Paired: the device paired itself; only the introduction goes.
      if (h.pairedAt === 0) s.remove(e.id)
      else s.setParent(e.id, undefined)
      s.setPairError(e.id, undefined)
    }
  }
}

/** Route the active engine's `NodeEvent`s (the only stream subscribed, 31 §7.3) into `onNodeEvent`. Returns the unsubscribe. */
export function watchNodes(): () => void {
  const handle = (e: NodeEvent) => {
    const api = useConnection.getState().api
    if (api) onNodeEvent(useEngines.getState().active, api, e)
  }
  const offs = (['NodeAdded', 'NodeOnline', 'NodeRemoved', 'NodeRevoked'] as const).map((t) => on(t, handle))
  return () => {
    for (const off of offs) off()
  }
}

/** The active engine answers `list_nodes` (32 §8.3): the Nodes tab and the phone's "as a node of" choice. */
export const useHasNodes = (): boolean => useEngines((s) => s.nodes[s.active] === true)

/** React Query key of the active engine's `list_nodes`; `Nodes.tsx` invalidates it on every `NodeEvent`. */
export const NODES_KEY = ['nodes'] as const

/** The active engine's nodes; disabled (no request) where `list_nodes` is not answered. */
export function useNodes() {
  const api = useApi()
  return useQuery({ queryKey: NODES_KEY, queryFn: () => api.list_nodes(), enabled: useHasNodes(), retry: false })
}
