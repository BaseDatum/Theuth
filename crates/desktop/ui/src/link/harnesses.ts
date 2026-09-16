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
  pairedAt: number
  lastSeen: number
  order: number
}

export type Presence = 'online' | 'offline' | 'connecting'

/** The desktop's own daemon in the switcher. */
export const LOCAL = 'local'

type Persisted = { harnesses: Harness[]; active: string }

type HarnessesStore = Persisted & {
  presence: Record<string, Presence>
  /** IndexedDB read once; `harnesses` is meaningless before. */
  hydrated: boolean
  add(h: Harness): void
  remove(id: string): void
  rename(id: string, name: string): void
  setActive(id: string): void
  reorder(ids: string[]): void
  setPresence(id: string, p: Presence): void
}

const byOrder = (hs: Harness[]) => [...hs].sort((a, b) => a.order - b.order)

export const useHarnesses = create<HarnessesStore>()((set) => ({
  harnesses: [],
  active: LOCAL,
  presence: {},
  hydrated: false,
  add: (h) => set((s) => ({ harnesses: byOrder([...s.harnesses.filter((x) => x.id !== h.id), h]) })),
  remove: (id) =>
    set((s) => {
      const harnesses = s.harnesses.filter((x) => x.id !== id)
      const { [id]: _, ...presence } = s.presence
      const active = s.active === id ? (hasLocal() ? LOCAL : (harnesses[0]?.id ?? LOCAL)) : s.active
      return { harnesses, presence, active }
    }),
  rename: (id, name) => set((s) => ({ harnesses: s.harnesses.map((x) => (x.id === id ? { ...x, name } : x)) })),
  setActive: (active) => set({ active }),
  reorder: (ids) =>
    set((s) => ({ harnesses: byOrder(s.harnesses.map((x) => ({ ...x, order: ids.indexOf(x.id) < 0 ? ids.length : ids.indexOf(x.id) }))) })),
  setPresence: (id, p) => set((s) => (s.presence[id] === p ? s : { presence: { ...s.presence, [id]: p } })),
}))

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
