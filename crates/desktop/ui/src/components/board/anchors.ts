// Design 71 §2.3: the one anchor model of a board. Prose `code:` spans, diagram units (`ref`), `Excerpt`s and their
// gutter rows all resolve to `Anchor`s; exactly one anchor is active (hover < pinned < tour step); every element whose
// anchor overlaps the active one lights up. One store per open board (`BoardReader` owns it, `AnchorContext` hands it
// down); nothing here is persisted — anchors are the reference itself, never renderer-invented ids.
import { createContext, useContext, useEffect, useId, useRef } from 'react'
import { useStore } from 'zustand'
import { createStore, type StoreApi } from 'zustand/vanilla'
import type { Anchor } from '@/components/chat/surface/anchor'

export type { Anchor }

/** What an anchored element is, for step 5's tour ordering and the pane's "which excerpt covers this". */
export type ElementKind = 'span' | 'unit' | 'excerpt' | 'row'

export type Registered = { anchor: Anchor; kind: ElementKind; blockId: string }

/**
 * One stop of a tour (M6): the anchor it lights (`null` for an "explanation" stop — a unit without a `ref`), what the
 * feed shows, and the stage — the diagram block (with the unit to recolour) or the excerpt block the stop sits on.
 */
export type TourStop = {
  anchor: Anchor | null
  kind: ElementKind
  label: string
  detail: string | null
  stage: { blockId: string; unit: string | null } | null
}

/** `hover` after 120 ms on a fine pointer or on focus; `pinned` on click / Enter; `tour` from a Tour button / `T` / ⌘↵. */
export type AnchorState = {
  hover: Anchor | null
  pinned: { anchor: Anchor; by: string; claim: string | null } | null
  /** `blockId` is the diagram toured (`null` = the whole board, §2.6 "Tour this board"); persisted as `{blockId, index}`. */
  tour: { stops: TourStop[]; index: number; by: string; blockId: string | null } | null
  registry: Map<string, Registered>
}

export type AnchorStore = AnchorState & {
  setHover(a: Anchor | null): void
  pin(anchor: Anchor, by: string, claim?: string | null): void
  unpin(): void
  openTour(stops: TourStop[], index: number, by: string, blockId: string | null): void
  /** Clamped to the stops; a no-op while no tour is open. */
  stepTour(index: number): void
  closeTour(): void
  register(id: string, entry: Registered): () => void
}

/** The rule that makes three things one: same path and overlapping ranges; a bare path covers the whole file. */
export function overlaps(a: Anchor, b: Anchor): boolean {
  if (a.kind === 'block' || b.kind === 'block') return a.kind === 'block' && b.kind === 'block' && a.id === b.id
  if (a.path !== b.path) return false
  if (!a.lines || !b.lines) return true
  return a.lines[0] <= b.lines[1] && b.lines[0] <= a.lines[1]
}

/** `outer` is the code window for `inner`: same file and `inner`'s lines sit inside `outer`'s (a bare `inner` never does). */
export function contains(outer: Anchor, inner: Anchor): boolean {
  if (outer.kind !== 'code' || inner.kind !== 'code' || outer.path !== inner.path) return false
  if (!outer.lines) return true
  if (!inner.lines) return false
  return outer.lines[0] <= inner.lines[0] && inner.lines[1] <= outer.lines[1]
}

/** The lines of `a` that fall inside `range`, or `null` when none do (a bare path lights the frame, not rows). */
export function intersect(a: Anchor, range: [number, number]): [number, number] | null {
  if (a.kind !== 'code' || !a.lines) return null
  const lo = Math.max(a.lines[0], range[0])
  const hi = Math.min(a.lines[1], range[1])
  return lo <= hi ? [lo, hi] : null
}

/** Back to the grammar: `path#L<a>-L<b>`, `path#L<a>`, `path`, `block:<id>`. */
export function formatAnchor(a: Anchor): string {
  if (a.kind === 'block') return `block:${a.id}`
  if (!a.lines) return a.path
  const [lo, hi] = a.lines
  return lo === hi ? `${a.path}#L${lo}` : `${a.path}#L${lo}-L${hi}`
}

/** M4: exactly one — the tour step, else the pin, else the hover. */
export function activeOf(s: AnchorState): Anchor | null {
  return s.tour ? (s.tour.stops[s.tour.index]?.anchor ?? null) : (s.pinned?.anchor ?? s.hover)
}

export function createAnchorStore(): StoreApi<AnchorStore> {
  return createStore<AnchorStore>()((set, get) => ({
    hover: null,
    pinned: null,
    tour: null,
    registry: new Map(),
    setHover: (hover) => set({ hover }),
    pin: (anchor, by, claim = null) => set({ pinned: { anchor, by, claim } }),
    unpin: () => set({ pinned: null }),
    openTour: (stops, index, by, blockId) => {
      if (stops.length > 0) set({ tour: { stops, index: Math.min(Math.max(0, index), stops.length - 1), by, blockId } })
    },
    stepTour: (index) => {
      const t = get().tour
      if (t) set({ tour: { ...t, index: Math.min(Math.max(0, index), t.stops.length - 1) } })
    },
    closeTour: () => set({ tour: null }),
    register(id, entry) {
      get().registry.set(id, entry)
      return () => void get().registry.delete(id)
    },
  }))
}

export const AnchorContext = createContext<StoreApi<AnchorStore> | null>(null)

export function useAnchorStore(): StoreApi<AnchorStore> {
  const s = useContext(AnchorContext)
  if (!s) throw new Error('useAnchorStore outside a BoardReader')
  return s
}

/** The active anchor, re-rendering the caller only when it changes. */
export function useActiveAnchor(): Anchor | null {
  return useStore(useAnchorStore(), activeOf)
}

/** §0 Q2: hover lights after 120 ms on a fine pointer; focus lights at once. */
export const HOVER_MS = 120

/**
 * Wire one anchored element: registers it, lights it when the active anchor overlaps its own, and returns the
 * handlers for hover / focus / click plus the `data-anchor-*` attributes. `claim` is what the pane shows above the
 * code window (the sentence or unit label that pinned). `coarse`: no hover (tap = pin, §2.10).
 */
export function useAnchored(
  entry: Registered,
  { claim, coarse = false }: { claim?: string | null; coarse?: boolean } = {},
): { id: string; active: boolean; pinned: boolean; props: Record<string, unknown> } {
  const store = useAnchorStore()
  const id = useId()
  const timer = useRef<number | undefined>(undefined)
  const latest = useRef(entry)
  latest.current = entry
  const key = formatAnchor(entry.anchor)
  useEffect(() => store.getState().register(id, latest.current), [store, id, key, entry.kind, entry.blockId])
  const active = useStore(store, (s) => {
    const a = activeOf(s)
    return a !== null && overlaps(a, entry.anchor)
  })
  const pinned = useStore(store, (s) => s.pinned?.by === id)
  const clear = () => {
    clearTimeout(timer.current)
    const h = store.getState().hover
    if (h && formatAnchor(h) === key) store.getState().setHover(null)
  }
  return {
    id,
    active,
    pinned,
    props: {
      'data-anchor': key,
      'data-anchor-id': id,
      'data-anchor-active': active || undefined,
      'data-anchor-pinned': pinned || undefined,
      onMouseEnter: coarse
        ? undefined
        : () => {
            clearTimeout(timer.current)
            timer.current = window.setTimeout(() => store.getState().setHover(latest.current.anchor), HOVER_MS)
          },
      onMouseLeave: coarse ? undefined : clear,
      onFocus: () => store.getState().setHover(latest.current.anchor),
      onBlur: clear,
      onClick: (e: { preventDefault(): void; stopPropagation(): void }) => {
        e.preventDefault()
        e.stopPropagation()
        store.getState().pin(latest.current.anchor, id, claim ?? null)
      },
      onKeyDown: (e: { key: string; preventDefault(): void; currentTarget: EventTarget }) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault()
          store.getState().pin(latest.current.anchor, id, claim ?? null)
        }
      },
    },
  }
}
