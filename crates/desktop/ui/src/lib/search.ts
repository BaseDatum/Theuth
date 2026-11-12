// Search's shared frontend contract (49 §2, §5, §7): how anything opens the one
// root-mounted dialog, and the transient per-engine state a search-opened
// destination reads to offer Back to results. No corpora live here; results are
// cached per engine by TanStack Query under `queryKeys.search`.

import { type StoreApi, createStore } from 'zustand'
import type { SearchCategory } from '@/rpc/bindings/method.rpc_2dcompose.input'
import type { SearchGroup } from "@/rpc/bindings/method.rpc_2dcompose.output"
import { scoped } from '@/store/engine-scope'

/** Display and wire order of the six categories; also the canonical order of a selection. */
export const CATEGORIES: readonly SearchCategory[] = [
  'sessions',
  'artifacts',
  'knowledge',
  'knowledge_history',
  'notes',
  'meetings',
  'sources',
  'notebooks',
]

export const CATEGORY_LABEL: Record<SearchCategory, string> = {
  sessions: 'Sessions',
  artifacts: 'Artifacts',
  knowledge: 'Knowledge',
  knowledge_history: 'Knowledge history',
  notes: 'Notes',
  meetings: 'Meetings',
  sources: 'Sources',
  notebooks: 'Notebooks',
}

/** Per-group page sizes Show more steps through (49 §2.3); the last is the backend cap. */
export const LIMITS = [10, 30, 50] as const
export const MAX_QUERY_CHARS = 1024

export type Limits = Record<SearchCategory, number>
export const DEFAULT_LIMITS: Limits = {
  sessions: 10,
  artifacts: 10,
  knowledge: 10,
  knowledge_history: 10,
  notes: 10,
  meetings: 10,
  sources: 10,
  notebooks: 10,
}

/** Dedupe into fixed order, dropping unknown values — the request identity the backend also computes. */
export function canonical(categories: readonly string[]): SearchCategory[] {
  return CATEGORIES.filter((c) => categories.includes(c))
}

/**
 * Selected groups still building at the base limit (49 §4.5): the only ones a poll re-asks, so ready, failed and
 * unavailable groups are never re-matched. A category with its own Show more page polls that page instead.
 */
export function indexing(
  categories: readonly SearchCategory[],
  groups: readonly SearchGroup[] | undefined,
  limits: Limits,
): SearchCategory[] {
  if (!groups) return []
  return categories.filter(
    (c) => limits[c] === LIMITS[0] && groups.find((g) => g.category === c)?.page.state === 'indexing',
  )
}

/** A poll reply replaces the groups it re-asked; every other group keeps the page it already had. */
export function patch(groups: readonly SearchGroup[], fresh: readonly SearchGroup[]): SearchGroup[] {
  return groups.map((g) => fresh.find((f) => f.category === g.category) ?? g)
}

/** Where a result opens: the stable source identity, never a row index. */
export type SearchDestination =
  | { category: 'sessions'; session: string; seq: number | null }
  | { category: 'artifacts'; callId: string }
  | { category: 'knowledge'; id: string }
  | { category: 'knowledge_history'; id: string; commit: string; snapshot: string | null }
  | { category: 'notes'; id: string; block: string }
  | { category: 'meetings'; id: string }
  | { category: 'sources'; id: string; block: string }
  | { category: 'notebooks'; id: string; block: string }

/** The search a destination was opened from; the query stays out of the URL. */
export type SearchOrigin = { query: string; destination: SearchDestination }

/** What `openSearch` asks of the dialog. */
export type SearchRequest = {
  /** Prefill and run this query (`/search <query>`). */
  query?: string
  /** One-open selection override (`/sessions`); not remembered. */
  categories?: SearchCategory[]
  /** Back to results: restore the last view instead of starting blank. */
  restore?: boolean
}

/** Everything the dialog showed at activation, so Back to results can put it back. */
export type SearchView = {
  query: string
  categories: SearchCategory[]
  limits: Limits
  /** Displayed groups (base plus any Show more pages), used to reseed the query cache. */
  groups: SearchGroup[]
  /** Fetched-at of `groups`, so the reseeded cache keeps honest staleness. */
  at: number
  /** cmdk value of the active row. */
  selected: string | null
  scroll: number
}

export type SearchState = {
  /** Remembered selection (49 §2.1): any subset, first open all four. */
  categories: SearchCategory[]
  view: SearchView | null
  origin: SearchOrigin | null
  /** Bumped per `restore` request so a restore while already open still applies. */
  restoreGeneration: number
  setCategories(categories: readonly SearchCategory[]): void
  /** A result was activated: remember the view and where it went. */
  leave(view: SearchView, origin: SearchOrigin): void
  restored(): void
}

export type SearchApi = StoreApi<SearchState>

/** One engine's search state (31 §7.4); `useSearchState` reads the enclosing engine's. */
export const createSearchState = (): SearchApi =>
  createStore<SearchState>()((set) => ({
    categories: [...CATEGORIES],
    view: null,
    origin: null,
    restoreGeneration: 0,
    setCategories: (categories) => set({ categories: canonical(categories) }),
    leave: (view, origin) => set({ view, origin }),
    restored: () => set((s) => ({ restoreGeneration: s.restoreGeneration + 1 })),
  }))

export const useSearchState = scoped((st) => st.search)

/** The search the current screen was opened from, if any; `null` after an ordinary navigation. */
export function useSearchOrigin(): SearchOrigin | null {
  return useSearchState((s) => s.origin)
}

/** Open the one search instance (root listens). Plain `theuth:search` events from hotkeys/triggers mean `{}`. */
export function openSearch(options: SearchRequest = {}): void {
  window.dispatchEvent(new CustomEvent<SearchRequest>('theuth:search', { detail: options }))
}

/** Back to results: reopen search exactly as it was at activation. */
export function backToSearch(): void {
  openSearch({ restore: true })
}

/** Whether `origin` opened exactly `destination` (immutable key compare). */
export function isSearchDestination(origin: SearchOrigin | null, destination: SearchDestination): boolean {
  const o = origin?.destination
  if (!o || o.category !== destination.category) return false
  switch (destination.category) {
    case 'sessions':
      return o.category === 'sessions' && o.session === destination.session && o.seq === destination.seq
    case 'artifacts':
      return o.category === 'artifacts' && o.callId === destination.callId
    case 'knowledge':
      return o.category === 'knowledge' && o.id === destination.id
    case 'knowledge_history':
      return (
        o.category === 'knowledge_history' &&
        o.id === destination.id &&
        o.commit === destination.commit &&
        o.snapshot === destination.snapshot
      )
    case 'notes':
    case 'sources':
    case 'notebooks':
      return o.category === destination.category && o.id === destination.id && o.block === destination.block
    case 'meetings':
      return o.category === 'meetings' && o.id === destination.id
  }
}
