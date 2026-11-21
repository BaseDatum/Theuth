// 208 §3.1 / §8.4: what the canvas shows at `now`. Everything has a TTL that starts at the node's last event; subagents'
// is the shortest. Live nodes, nodes with open attention items and their ancestors are pinned; an expired node with a
// visible descendant stays as a ghost so trees stay whole. Pure: the caller passes `now` (a bucketed clock).
import type { GraphEdge, GraphNode } from '@/rpc/bindings/method.agent.output'
import { live } from '@/lib/session-tree'
import type { GraphState } from './reducer'

export type TtlPrefs = { child_min: number; root_min: number; desk_min: number; edge_min: number }
export const TTL_DEFAULTS: TtlPrefs = { child_min: 10, root_min: 240, desk_min: 1440, edge_min: 5 }

const MIN = 60_000
const WEEK_MS = 7 * 1440 * MIN
const GHOST = 0.25

/** child, observed → child_min; root, peer → root_min; desk → desk_min; in ms. */
export function nodeTtlMs(node: GraphNode, ttl: TtlPrefs): number {
  const min = node.kind === 'child' || node.kind === 'observed' ? ttl.child_min : node.kind === 'desk' ? ttl.desk_min : ttl.root_min
  return min * MIN
}

/** live(activity.state.state) || waits.length > 0. */
export function isLiveNode(node: GraphNode): boolean {
  return live(node.activity.state.state) || node.waits.length > 0
}

/** 1 while age < 0.75·ttl; then 1 − 3·(age − 0.75·ttl)/ttl; 0 when age ≥ ttl (expired). */
export function fadeOpacity(ageMs: number, ttlMs: number): number {
  if (ageMs >= ttlMs) return 0
  if (ageMs < 0.75 * ttlMs) return 1
  return 1 - (3 * (ageMs - 0.75 * ttlMs)) / ttlMs
}

/** The session `from` of an `awaiting` node points at; null when it is not awaiting or nothing matches. */
export function awaitTarget(node: GraphNode, nodes: ReadonlyMap<string, GraphNode>): string | null {
  const s = node.activity.state
  if (s.state !== 'awaiting') return null
  const from = s.from
  if (from === 'parent') return node.parent
  if (from.startsWith('session:')) return from.slice('session:'.length)
  const named = (n: GraphNode | undefined): boolean => n !== undefined && (n.name ?? n.title) === from
  const parent = node.parent === null ? undefined : nodes.get(node.parent)
  if (named(parent)) return node.parent
  let sibling: string | null = null
  for (const n of nodes.values()) {
    if (!named(n) || n.id === node.id) continue
    if (n.parent === node.id) return n.id
    if (sibling === null && n.parent !== null && n.parent === node.parent) sibling = n.id
  }
  return sibling
}

export type NodeView = { node: GraphNode; opacity: number; pinned: boolean; ghost: boolean; attention: number; expiresAt: number | null }
export type EdgeView = { edge: GraphEdge; active: boolean; animated: boolean }
export type VisibleGraph = { nodes: Map<string, NodeView>; edges: EdgeView[]; hidden: number }

/** parent → children over `state.nodes`, insertion order. */
function childrenMap(nodes: ReadonlyMap<string, GraphNode>): Map<string, GraphNode[]> {
  const out = new Map<string, GraphNode[]>()
  for (const n of nodes.values()) {
    if (n.parent === null || !nodes.has(n.parent)) continue
    const list = out.get(n.parent)
    if (list) list.push(n)
    else out.set(n.parent, [n])
  }
  return out
}

/** 208 §3.1, contract §8.4: the nodes and edges visible at `now`. */
export function visibleGraph(state: GraphState, attention: ReadonlyMap<string, number>, ttl: TtlPrefs, now: number): VisibleGraph {
  const kids = childrenMap(state.nodes)
  const pinMemo = new Map<string, boolean>()
  const pinned = (n: GraphNode): boolean => {
    const known = pinMemo.get(n.id)
    if (known !== undefined) return known
    pinMemo.set(n.id, false)
    const own = (attention.get(n.id) ?? 0) > 0 || isLiveNode(n)
    const v = own || (kids.get(n.id) ?? []).some(pinned)
    pinMemo.set(n.id, v)
    return v
  }
  const out = new Map<string, NodeView>()
  const visit = (n: GraphNode): boolean => {
    // Children first: a ghost needs to know whether anything below it shows.
    let below = false
    for (const c of kids.get(n.id) ?? []) below = visit(c) || below
    const att = attention.get(n.id) ?? 0
    if (pinned(n)) {
      out.set(n.id, { node: n, opacity: 1, pinned: true, ghost: false, attention: att, expiresAt: null })
      return true
    }
    const ms = nodeTtlMs(n, ttl)
    const opacity = fadeOpacity(now - n.last_event_ms, ms)
    if (opacity > 0) out.set(n.id, { node: n, opacity, pinned: false, ghost: false, attention: att, expiresAt: n.last_event_ms + ms })
    else if (below) out.set(n.id, { node: n, opacity: GHOST, pinned: false, ghost: true, attention: att, expiresAt: n.last_event_ms + ms })
    return opacity > 0 || below
  }
  for (const n of state.nodes.values()) {
    if (n.kind === 'peer') continue
    if (n.parent === null || !state.nodes.has(n.parent)) visit(n)
  }
  // Boundary peers: shown while an edge joins them to a shown session and their own root TTL has not run out.
  for (const n of state.nodes.values()) {
    if (n.kind !== 'peer') continue
    const ms = ttl.root_min * MIN
    const opacity = fadeOpacity(now - n.last_event_ms, ms)
    if (opacity <= 0) continue
    let joined = false
    for (const e of state.edges.values()) {
      const other = e.a === n.id ? e.b : e.b === n.id ? e.a : null
      const v = other === null ? undefined : out.get(other)
      if (v && v.node.kind !== 'peer') {
        joined = true
        break
      }
    }
    if (joined) out.set(n.id, { node: n, opacity, pinned: false, ghost: false, attention: attention.get(n.id) ?? 0, expiresAt: n.last_event_ms + ms })
  }
  const edgeMs = ttl.edge_min * MIN
  const edges: EdgeView[] = []
  for (const e of state.edges.values()) {
    const a = out.get(e.a)
    const b = out.get(e.b)
    if (!a || !b) continue
    const animated = e.open > 0 || awaitTarget(a.node, state.nodes) === e.b || awaitTarget(b.node, state.nodes) === e.a
    edges.push({ edge: e, animated, active: animated || now - e.last_ms < edgeMs })
  }
  return { nodes: out, edges, hidden: state.nodes.size - out.size }
}

/** ms until something visible changes (contract §8.4), clamped to [1000, 60000]. */
export function nextTtlTick(_state: GraphState, view: VisibleGraph, ttl: TtlPrefs, now: number): number {
  // Pinned nodes change only through a delta (which re-renders anyway); ghosts follow their descendants' fades.
  let next = Number.POSITIVE_INFINITY
  for (const v of view.nodes.values()) {
    if (v.pinned || v.ghost) continue
    const ms = v.node.kind === 'peer' ? ttl.root_min * MIN : nodeTtlMs(v.node, ttl)
    if (now - v.node.last_event_ms >= 0.75 * ms) return 5000
    next = Math.min(next, v.node.last_event_ms + 0.75 * ms)
  }
  // An active message edge that is not animated folds at last_ms + edge TTL.
  if (view.edges.some((e) => e.active && !e.animated)) return 5000
  return Math.min(60_000, Math.max(1000, next - now))
}

/** max of the four TTLs in ms, clamped to 7 d: the `window_ms` asked of the engine. */
export function windowMs(ttl: TtlPrefs): number {
  return Math.min(WEEK_MS, Math.max(ttl.child_min, ttl.root_min, ttl.desk_min, ttl.edge_min) * MIN)
}
