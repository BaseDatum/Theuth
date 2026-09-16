import { useMemo } from 'react'
import { create } from 'zustand'
import { container } from '@/lib/platform'
import { idbGet, idbPut } from './idb'

/** One paired harness (31 §7.3). `id` is the base64url harness public key. */
export interface Harness {
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
  /** The harness that introduced this one as its node (32 §8.1); listed nested under it. */
  parent?: string
}

export type Presence = 'online' | 'offline' | 'connecting'

/** The desktop's own daemon in the switcher. */
export const LOCAL = 'local'

type Persisted = { harnesses: Harness[]; active: string }

type HarnessesStore = Persisted & {
  presence: Record<string, Presence>
  /** Auto-pair with a node failed (32 §8.2): the message, until retried. */
  pairError: Record<string, string>
  /** Harness answers `list_nodes` (32 §8.3); absent until its first connect. */
  nodes: Record<string, boolean>
  /** IndexedDB read once; `harnesses` is meaningless before. */
  hydrated: boolean
  add(h: Harness): void
  remove(id: string): void
  rename(id: string, name: string): void
  setParent(id: string, parent: string | undefined): void
  setActive(id: string): void
  reorder(ids: string[]): void
  setPresence(id: string, p: Presence): void
  setPairError(id: string, error: string | undefined): void
  setNodes(id: string, on: boolean): void
}

/** `{ harness, depth }` rows of the switcher: each child right after its parent, in order. */
export type HarnessTreeRow = { harness: Harness; depth: 0 | 1 }

/** Parents before their children (32 §8.1): top level by `order`, each parent's children (by `order`) right after it; orphans flat. */
export function harnessTree(hs: Harness[]): HarnessTreeRow[] {
  const sorted = [...hs].sort((a, b) => a.order - b.order)
  const ids = new Set(hs.map((h) => h.id))
  const rows: HarnessTreeRow[] = []
  for (const h of sorted) {
    if (h.parent && ids.has(h.parent)) continue
    rows.push({ harness: h, depth: 0 })
    for (const c of sorted) if (c.parent === h.id) rows.push({ harness: c, depth: 1 })
  }
  return rows
}

const byOrder = (hs: Harness[]) => harnessTree(hs).map((r) => r.harness)

export const useHarnesses = create<HarnessesStore>()((set) => ({
  harnesses: [],
  active: LOCAL,
  presence: {},
  pairError: {},
  nodes: {},
  hydrated: false,
  add: (h) => set((s) => ({ harnesses: byOrder([...s.harnesses.filter((x) => x.id !== h.id), h]) })),
  remove: (id) =>
    set((s) => {
      const harnesses = s.harnesses.filter((x) => x.id !== id)
      const { [id]: _, ...presence } = s.presence
      const { [id]: __, ...pairError } = s.pairError
      const active = s.active === id ? (hasLocal() ? LOCAL : (harnesses[0]?.id ?? LOCAL)) : s.active
      return { harnesses, presence, pairError, active }
    }),
  rename: (id, name) => set((s) => ({ harnesses: s.harnesses.map((x) => (x.id === id ? { ...x, name } : x)) })),
  setParent: (id, parent) =>
    set((s) =>
      s.harnesses.some((x) => x.id === id && x.parent !== parent)
        ? { harnesses: byOrder(s.harnesses.map((x) => (x.id === id ? { ...x, parent } : x))) }
        : s,
    ),
  setActive: (active) => set({ active }),
  reorder: (ids) =>
    set((s) => ({ harnesses: byOrder(s.harnesses.map((x) => ({ ...x, order: ids.indexOf(x.id) < 0 ? ids.length : ids.indexOf(x.id) }))) })),
  setPresence: (id, p) => set((s) => (s.presence[id] === p ? s : { presence: { ...s.presence, [id]: p } })),
  setPairError: (id, error) =>
    set((s) => {
      if (s.pairError[id] === error) return s
      const { [id]: _, ...rest } = s.pairError
      return { pairError: error === undefined ? rest : { ...rest, [id]: error } }
    }),
  setNodes: (id, on) => set((s) => (s.nodes[id] === on ? s : { nodes: { ...s.nodes, [id]: on } })),
}))

/** The switcher's rows: children indented one step under their parent. */
export function useHarnessTree(): HarnessTreeRow[] {
  const hs = useHarnesses((s) => s.harnesses)
  return useMemo(() => harnessTree(hs), [hs])
}

/** Whether this container has a daemon of its own: the macOS shell, or a dev browser pointed at one. */
export const hasLocal = (): boolean =>
  container() === 'macos' || (typeof import.meta.env.VITE_WTH_URL === 'string' && typeof import.meta.env.VITE_WTH_TOKEN === 'string')

/** Nothing to connect to: no local daemon and no paired harness — show the `/link` onboarding. */
export const useNeedsPairing = (): boolean => useHarnesses((s) => s.hydrated && s.harnesses.length === 0) && !hasLocal()

let hydrating: Promise<void> | undefined

/** Load `harnesses`/`active` from IndexedDB once and persist every later change. */
export function hydrateHarnesses(): Promise<void> {
  hydrating ??= (async () => {
    const stored = await idbGet<Persisted>('harnesses', 'state')
    const harnesses = byOrder(stored?.harnesses ?? [])
    let active = stored?.active ?? LOCAL
    if ((active === LOCAL && !hasLocal()) || (active !== LOCAL && !harnesses.some((h) => h.id === active))) {
      active = hasLocal() ? LOCAL : (harnesses[0]?.id ?? LOCAL)
    }
    useHarnesses.setState({ harnesses, active, hydrated: true })
    useHarnesses.subscribe((s, prev) => {
      if (s.harnesses !== prev.harnesses || s.active !== prev.active) {
        void idbPut('harnesses', 'state', { harnesses: s.harnesses, active: s.active } satisfies Persisted)
      }
    })
  })()
  return hydrating
}
