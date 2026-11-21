// 208 §8.5: the graph region's client-only preferences (expanded, height, follow, TTLs), persisted like the sidebar
// linger setting. Full screen is session state and never stored.
import { create } from 'zustand'
import { TTL_DEFAULTS, type TtlPrefs } from './ttl'

export const GRAPH_PREFS_KEY = 'attention.graph'
/** `height` null = the viewport default (`defaultGraphHeight`) until the user resizes the region. */
export type GraphPrefs = { expanded: boolean; height: number | null; follow: boolean; ttl: TtlPrefs }
export const GRAPH_PREFS_DEFAULT: GraphPrefs = { expanded: true, height: null, follow: false, ttl: TTL_DEFAULTS }

/** The expanded region's default height: 40 % of the viewport, at least 280 px, at most 60 % of the viewport. */
export function defaultGraphHeight(viewport: number): number {
  return Math.round(Math.min(0.6 * viewport, Math.max(280, 0.4 * viewport)))
}

const clampInt = (v: unknown, lo: number, hi: number, fallback: number): number =>
  typeof v === 'number' && Number.isFinite(v) ? Math.min(hi, Math.max(lo, Math.round(v))) : fallback

/** Parse; garbage or missing parts → defaults; height clamped to [160, 2000]; TTL minutes clamped per kind. */
export function readGraphPrefs(raw: string | null): GraphPrefs {
  let v: unknown = null
  try {
    v = raw === null ? null : JSON.parse(raw)
  } catch {
    v = null
  }
  if (typeof v !== 'object' || v === null) return GRAPH_PREFS_DEFAULT
  const o = v as Record<string, unknown>
  const t = (typeof o.ttl === 'object' && o.ttl !== null ? o.ttl : {}) as Record<string, unknown>
  const d = GRAPH_PREFS_DEFAULT
  return {
    expanded: typeof o.expanded === 'boolean' ? o.expanded : d.expanded,
    height: typeof o.height === 'number' && Number.isFinite(o.height) ? clampInt(o.height, 160, 2000, 160) : null,
    follow: typeof o.follow === 'boolean' ? o.follow : d.follow,
    ttl: {
      child_min: clampInt(t.child_min, 1, 1440, d.ttl.child_min),
      root_min: clampInt(t.root_min, 1, 10080, d.ttl.root_min),
      desk_min: clampInt(t.desk_min, 1, 10080, d.ttl.desk_min),
      edge_min: clampInt(t.edge_min, 1, 240, d.ttl.edge_min),
    },
  }
}

export const useGraphPrefs = create<GraphPrefs & { set: (p: Partial<GraphPrefs>) => void }>()((set, get) => ({
  // `globalThis.` so the module loads under vitest's node environment.
  ...readGraphPrefs(globalThis.localStorage?.getItem(GRAPH_PREFS_KEY) ?? null),
  set(p) {
    const { expanded, height, follow, ttl } = { ...get(), ...p }
    const next = readGraphPrefs(JSON.stringify({ expanded, height, follow, ttl }))
    globalThis.localStorage?.setItem(GRAPH_PREFS_KEY, JSON.stringify(next))
    set(next)
  },
}))
