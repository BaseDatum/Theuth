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

/** Largest TTL in minutes per kind: child 1 d, root and desk 7 d, edge 4 h. */
export const TTL_MAX: TtlPrefs = { child_min: 1440, root_min: 10080, desk_min: 10080, edge_min: 240 }

/** Review U9: a stored TTL counts only when finite, positive and within its bound (rounded, at least 1); else the default. */
const ttlMin = (v: unknown, hi: number, fallback: number): number =>
  typeof v === 'number' && Number.isFinite(v) && v > 0 && v <= hi ? Math.max(1, Math.round(v)) : fallback

/** Parse; garbage or missing parts → defaults; height clamped to [160, 2000]; a TTL outside (0, TTL_MAX] → its default. */
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
      child_min: ttlMin(t.child_min, TTL_MAX.child_min, d.ttl.child_min),
      root_min: ttlMin(t.root_min, TTL_MAX.root_min, d.ttl.root_min),
      desk_min: ttlMin(t.desk_min, TTL_MAX.desk_min, d.ttl.desk_min),
      edge_min: ttlMin(t.edge_min, TTL_MAX.edge_min, d.ttl.edge_min),
    },
  }
}

export const useGraphPrefs = create<GraphPrefs & { set: (p: Partial<GraphPrefs>) => void }>()((set, get) => ({
  // `globalThis.` so the module loads under vitest's node environment.
  ...readGraphPrefs(globalThis.localStorage?.getItem(GRAPH_PREFS_KEY) ?? null),
  set(p) {
    const { expanded, height, follow, ttl } = { ...get(), ...p }
    // An in-app edit past a bound is clamped to it (the popover's input), not reset like garbage from storage.
    const capped = {
      child_min: Math.min(ttl.child_min, TTL_MAX.child_min),
      root_min: Math.min(ttl.root_min, TTL_MAX.root_min),
      desk_min: Math.min(ttl.desk_min, TTL_MAX.desk_min),
      edge_min: Math.min(ttl.edge_min, TTL_MAX.edge_min),
    }
    const next = readGraphPrefs(JSON.stringify({ expanded, height, follow, ttl: capped }))
    globalThis.localStorage?.setItem(GRAPH_PREFS_KEY, JSON.stringify(next))
    set(next)
  },
}))
