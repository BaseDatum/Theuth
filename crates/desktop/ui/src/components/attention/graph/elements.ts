// 208 §8.6: what the canvas draws, as plain data — positions (tidy tree, sticky root slots), folds (D17), highlight
// dimming, tree edges and message edges with their handles. Pure, so the React Flow wrapper stays thin.
import type { GraphNode } from '@/rpc/bindings/method.agent.output'
import { BOX, type LayoutCache, layoutForest, type Pos } from './layout'
import type { GraphState } from './reducer'
import { pathToRoot } from './selectors'
import type { NodeView, VisibleGraph } from './ttl'

/** A parent folds its children into one chip when its visible subtree (itself excluded) has this many nodes, none pinned. */
export const FOLD_MIN = 8
/** Zoom buckets (contract §8.6): ≥ 0.75 full, ≥ 0.4 compact, else dot. */
export function lodOf(zoom: number): 'full' | 'compact' | 'dot' {
  return zoom >= 0.75 ? 'full' : zoom >= 0.4 ? 'compact' : 'dot'
}

/** parent id → the visible descendants hidden under its fold chip (expanded parents excluded). */
export function folds(view: VisibleGraph, expanded: ReadonlySet<string>): Map<string, string[]> {
  const kids = new Map<string, NodeView[]>()
  for (const v of view.nodes.values()) {
    const p = v.node.parent
    if (p === null || !view.nodes.has(p)) continue
    const list = kids.get(p)
    if (list) list.push(v)
    else kids.set(p, [v])
  }
  const below = (id: string): NodeView[] => (kids.get(id) ?? []).flatMap((k) => [k, ...below(k.node.id)])
  const out = new Map<string, string[]>()
  const visit = (id: string) => {
    const sub = below(id)
    if (!expanded.has(id) && sub.length >= FOLD_MIN && sub.every((v) => !v.pinned)) {
      out.set(id, sub.map((v) => v.node.id))
      return
    }
    for (const k of kids.get(id) ?? []) visit(k.node.id)
  }
  for (const v of view.nodes.values()) if (v.node.parent === null || !view.nodes.has(v.node.parent)) visit(v.node.id)
  return out
}

export type CanvasNode =
  | { type: 'session'; id: string; pos: Pos; view: NodeView; highlighted: boolean; dim: boolean }
  | { type: 'fold'; id: string; pos: Pos; parent: string; count: number; dim: boolean }
export type CanvasEdge =
  | { type: 'tree'; id: string; source: string; target: string; onPath: boolean; dim: boolean }
  | { type: 'message'; id: string; source: string; target: string; sourceHandle: 'r' | 'b'; targetHandle: 'l' | 't'; count: number; active: boolean; animated: boolean; dim: boolean }

export type Elements = { nodes: CanvasNode[]; edges: CanvasEdge[]; slots: Map<string, number>; pos: Map<string, Pos> }

/** Builds the canvas: `prev` slots keep roots in place (null re-packs, as Fit does). */
export function canvasElements(state: GraphState, view: VisibleGraph, highlight: string | null, expandedFolds: ReadonlySet<string>, prev: ReadonlyMap<string, number> | null, cache?: LayoutCache): Elements {
  const folded = folds(view, expandedFolds)
  const hidden = new Set([...folded.values()].flat())
  const laid: GraphNode[] = []
  for (const v of view.nodes.values()) if (!hidden.has(v.node.id)) laid.push(v.node)
  for (const [parent, ids] of folded) {
    const p = (view.nodes.get(parent) as NodeView).node
    laid.push({ ...p, id: `fold:${parent}`, parent, started_ms: Number.MAX_SAFE_INTEGER, title: `+${ids.length}` })
  }
  const { pos, slots } = layoutForest(laid, prev, cache)
  const path = new Set(highlight ? pathToRoot(state.nodes, highlight) : [])
  const lit = path.size > 0
  const nodes: CanvasNode[] = []
  for (const n of laid) {
    const at = pos.get(n.id) as Pos
    if (n.id.startsWith('fold:') && n.parent !== null) {
      nodes.push({ type: 'fold', id: n.id, pos: at, parent: n.parent, count: (folded.get(n.parent) ?? []).length, dim: lit })
      continue
    }
    nodes.push({ type: 'session', id: n.id, pos: at, view: view.nodes.get(n.id) as NodeView, highlighted: path.has(n.id), dim: lit && !path.has(n.id) })
  }
  const edges: CanvasEdge[] = []
  for (const n of laid) {
    if (n.parent === null || !pos.has(n.parent)) continue
    const onPath = path.has(n.id) && path.has(n.parent)
    edges.push({ type: 'tree', id: `t:${n.id}`, source: n.parent, target: n.id, onPath, dim: lit && !onPath })
  }
  for (const e of view.edges) {
    const a = pos.get(e.edge.a)
    const b = pos.get(e.edge.b)
    if (!a || !b) continue
    // Side handles when the ends sit apart horizontally, bottom → top when one is above the other.
    const [left, right] = a.x <= b.x ? [e.edge.a, e.edge.b] : [e.edge.b, e.edge.a]
    const apart = Math.abs(a.x - b.x) >= BOX.w
    const [upper, lower] = a.y <= b.y ? [e.edge.a, e.edge.b] : [e.edge.b, e.edge.a]
    const onPath = path.has(e.edge.a) && path.has(e.edge.b)
    edges.push({
      type: 'message',
      id: e.edge.id,
      source: apart ? left : upper,
      target: apart ? right : lower,
      sourceHandle: apart ? 'r' : 'b',
      targetHandle: apart ? 'l' : 't',
      count: e.edge.ab + e.edge.ba,
      active: e.active,
      animated: e.animated,
      dim: lit && !onPath,
    })
  }
  return { nodes, edges, slots, pos }
}
