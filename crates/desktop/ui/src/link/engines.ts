import { useMemo } from 'react'
import { create } from 'zustand'
import { container, isDesktop } from '@/lib/platform'
import type { LinkState } from '@/lib/link-status'
import type { MeshStatus } from "@/rpc/bindings/method.nodes.output"
import { idbGet, idbPut } from './idb'

/**
 * One paired engine (31 §7.3). `id` is the base64url public key this row dials at `/h/<id>`:
 * an engine's link identity for `kind: 'engine'`, the mesh device id for `kind: 'mesh'` (69 §4).
 */
export interface Engine {
  id: string
  name: string
  /** `host[:port]` of the relay. */
  relayHost: string
  /** `wss`/`https` rather than `ws`/`http`. */
  secure: boolean
  /** Unix ms; `0` for a node its parent introduced while offline (32 §8.2): no client until it pairs. */
  pairedAt: number
  lastSeen: number
  order: number
  /** The engine that introduced this one as its node (32 §8.1); listed nested under it. */
  parent?: string
  /** `mesh`: this device is paired to a mesh (its hub answers, whichever engine that is); `engine`: one engine. */
  kind: 'engine' | 'mesh'
  /** `mesh_status.mesh_name` (else the hub's `self_name`), refreshed on every connect. */
  meshName?: string
}

/** A mesh's engines as its hub last reported them: `online` counts the hub itself. */
export type MeshMembers = { online: number; total: number }

/** `refused`: the engine answered the handshake with `CLOSE refused` — it no longer knows this device; pair again. */
export type Presence = 'online' | 'offline' | 'connecting' | 'refused'

/** The desktop's own daemon in the switcher. */
export const LOCAL = 'local'
/** Its label by shell ("This Mac" / "This PC" / "This computer"); `Info.name` replaces it once connected. */
export const LOCAL_NAME = container() === 'macos' ? 'This Mac' : container() === 'windows' ? 'This PC' : 'This computer'
/** The local daemon as an `Engine` row, so its nodes nest under it. */
const LOCAL_ROW: Engine = { id: LOCAL, name: LOCAL_NAME, relayHost: '', secure: false, pairedAt: 0, lastSeen: 0, order: -1, kind: 'engine' }

/** The on-disk record in IndexedDB store `harnesses`, key `state`: the field keeps its pre-rename name so existing installs hydrate. */
type Persisted = { harnesses: Engine[]; active: string }

type EnginesStore = {
  engines: Engine[]
  active: string
  presence: Record<string, Presence>
  /** Per link engine, what its socket last saw (`lib/link-status`); the UI names the failure from it. */
  link: Record<string, LinkState>
  /** Auto-pair with a node failed (32 §8.2): the message, until retried. */
  pairError: Record<string, string>
  /** Engine answers `list_nodes` (32 §8.3); absent until its first connect. */
  nodes: Record<string, boolean>
  /** Per mesh row, from its hub's `mesh_status`; absent until the first connect. */
  members: Record<string, MeshMembers>
  /** IndexedDB read once; `engines` is meaningless before. */
  hydrated: boolean
  add(h: Engine): void
  remove(id: string): void
  rename(id: string, name: string): void
  setParent(id: string, parent: string | undefined): void
  setActive(id: string): void
  reorder(ids: string[]): void
  setPresence(id: string, p: Presence): void
  setLink(id: string, s: LinkState): void
  setPairError(id: string, error: string | undefined): void
  setNodes(id: string, on: boolean): void
  setMesh(id: string, meshName: string | undefined, members: MeshMembers): void
}

/** `{ engine, depth }` rows of the switcher: each child right after its parent, in order; `depth` 0 at the top. */
export type EngineTreeRow = { engine: Engine; depth: number }

/**
 * Parents before their children (32 §8.1): top level by `order`, each engine's
 * children (by `order`) right after it, one level deeper — a node's own nodes
 * included. Orphans (parent not in the list) render flat; a cycle renders once.
 */
export function engineTree(hs: Engine[]): EngineTreeRow[] {
  const sorted = [...hs].sort((a, b) => a.order - b.order)
  const ids = new Set(hs.map((h) => h.id))
  const rows: EngineTreeRow[] = []
  const seen = new Set<string>()
  const visit = (h: Engine, depth: number) => {
    if (seen.has(h.id)) return
    seen.add(h.id)
    rows.push({ engine: h, depth })
    for (const c of sorted) if (c.parent === h.id) visit(c, depth + 1)
  }
  for (const h of sorted) if (!h.parent || !ids.has(h.parent)) visit(h, 0)
  // Members of a cycle have a parent in the list yet no root; list them flat.
  for (const h of sorted) visit(h, 0)
  return rows
}

const byOrder = (hs: Engine[]) => engineTree(hs).map((r) => r.engine)

export const useEngines = create<EnginesStore>()((set) => ({
  engines: [],
  active: LOCAL,
  presence: {},
  link: {},
  pairError: {},
  nodes: {},
  members: {},
  hydrated: false,
  add: (h) => set((s) => ({ engines: byOrder([...s.engines.filter((x) => x.id !== h.id), h]) })),
  remove: (id) =>
    set((s) => {
      const engines = s.engines.filter((x) => x.id !== id)
      const { [id]: _, ...presence } = s.presence
      const { [id]: __, ...pairError } = s.pairError
      const { [id]: ___, ...link } = s.link
      const active = s.active === id ? (hasLocal() ? LOCAL : (engines[0]?.id ?? LOCAL)) : s.active
      return { engines, presence, pairError, link, active }
    }),
  rename: (id, name) => set((s) => ({ engines: s.engines.map((x) => (x.id === id ? { ...x, name } : x)) })),
  setParent: (id, parent) =>
    set((s) =>
      s.engines.some((x) => x.id === id && x.parent !== parent)
        ? { engines: byOrder(s.engines.map((x) => (x.id === id ? { ...x, parent } : x))) }
        : s,
    ),
  setActive: (active) => set({ active }),
  reorder: (ids) =>
    set((s) => ({ engines: byOrder(s.engines.map((x) => ({ ...x, order: ids.indexOf(x.id) < 0 ? ids.length : ids.indexOf(x.id) }))) })),
  setPresence: (id, p) => set((s) => (s.presence[id] === p ? s : { presence: { ...s.presence, [id]: p } })),
  setLink: (id, l) => set((s) => ({ link: { ...s.link, [id]: l } })),
  setPairError: (id, error) =>
    set((s) => {
      if (s.pairError[id] === error) return s
      const { [id]: _, ...rest } = s.pairError
      return { pairError: error === undefined ? rest : { ...rest, [id]: error } }
    }),
  setNodes: (id, on) => set((s) => (s.nodes[id] === on ? s : { nodes: { ...s.nodes, [id]: on } })),
  setMesh: (id, meshName, members) =>
    set((s) => ({
      // A new `engines` array is persisted; only build one when the name moved.
      engines: s.engines.some((x) => x.id === id && x.meshName !== meshName) ? s.engines.map((x) => (x.id === id ? { ...x, meshName } : x)) : s.engines,
      members: { ...s.members, [id]: members },
    })),
}))

/** Fold a mesh row's `mesh_status` in (69 §4): the mesh name (else the hub's own) and `n of m` engines, the hub counted. */
export function applyMeshStatus(id: string, s: MeshStatus): void {
  useEngines.getState().setMesh(id, s.mesh_name ?? s.self_name, { online: 1 + s.peers.filter((p) => p.reachable).length, total: 1 + s.peers.length })
}

/** The switcher's rows: the local daemon first when this container has one, children indented under their parent. */
export function useEngineTree(): EngineTreeRow[] {
  const hs = useEngines((s) => s.engines)
  return useMemo(() => engineTree(hasLocal() ? [LOCAL_ROW, ...hs] : hs), [hs])
}

/** Whether this container has a daemon of its own: a desktop shell (macOS, Linux, Windows), or a dev browser pointed at one. */
export const hasLocal = (): boolean =>
  isDesktop() || (typeof import.meta.env.VITE_THEUTH_URL === 'string' && typeof import.meta.env.VITE_THEUTH_TOKEN === 'string')

/** Nothing to connect to: no local daemon and no paired engine — show the `/link` onboarding. */
export const useNeedsPairing = (): boolean => useEngines((s) => s.hydrated && s.engines.length === 0) && !hasLocal()

let hydrating: Promise<void> | undefined

/** Load the paired engines/`active` from IndexedDB once and persist every later change (on-disk store/field names unchanged). */
export function hydrateEngines(): Promise<void> {
  hydrating ??= (async () => {
    const stored = await idbGet<Persisted>('harnesses', 'state')
    // Rows persisted before 69 have no `kind`: every one of them dialled an engine.
    const engines = byOrder((stored?.harnesses ?? []).map((h) => ({ ...h, kind: h.kind ?? 'engine' })))
    let active = stored?.active ?? LOCAL
    if ((active === LOCAL && !hasLocal()) || (active !== LOCAL && !engines.some((h) => h.id === active))) {
      active = hasLocal() ? LOCAL : (engines[0]?.id ?? LOCAL)
    }
    useEngines.setState({ engines, active, hydrated: true })
    useEngines.subscribe((s, prev) => {
      if (s.engines !== prev.engines || s.active !== prev.active) {
        void idbPut('harnesses', 'state', { harnesses: s.engines, active: s.active } satisfies Persisted)
      }
    })
  })()
  return hydrating
}
