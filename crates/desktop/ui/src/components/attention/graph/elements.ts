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
  | { type: 'message'; id: string; source: string; target: string; sourceHandle: 'r' | 'ts'; targetHandle: 'l' | 't' | 'rt'; fwd: number; back: number; count: number; active: boolean; animated: boolean; dim: boolean }

/** The flow-space box the canvas draws in: every node, plus the room message arcs take above their row. */
export type Extent = { x: number; y: number; w: number; h: number }

/** Height of the control points of a same-row message arc over `dx` (the arc's top is ¾ of it). */
export function arcRise(dx: number): number {
  return Math.min(150, Math.max(44, Math.abs(dx) * 0.16))
}

export type Elements = { nodes: CanvasNode[]; edges: CanvasEdge[]; slots: Map<string, number>; pos: Map<string, Pos>; extent: Extent }

/** Screen room kept around the drawing by the opening viewport (px). */
export const VIEW_PAD = { x: 32, y: 28 }
/** The opening viewport. The whole drawing when it fits at a readable zoom; otherwise the rows are fitted to the height
 * at no less than the full level of detail (0.75) where the height allows (never under 0.4), left-aligned so the first
 * tree is whole and the rest pans in. Centred on each axis that fits. `floor` overrides the lowest zoom (Fit: 0.1, the
 * whole drawing). */
export function initialViewport(e: Extent, width: number, height: number, floor?: number): { x: number; y: number; zoom: number } {
  if (e.w <= 0 || e.h <= 0 || width <= 0 || height <= 0) return { x: VIEW_PAD.x, y: VIEW_PAD.y, zoom: 1 }
  const byH = (height - 2 * VIEW_PAD.y) / e.h
  const byW = (width - 2 * VIEW_PAD.x) / e.w
  const zoom = Math.max(floor ?? Math.min(0.75, Math.max(0.4, byH)), Math.min(1, byH, byW))
  const x = e.w * zoom <= width - 2 * VIEW_PAD.x ? (width - e.w * zoom) / 2 - e.x * zoom : VIEW_PAD.x - e.x * zoom
  const y = e.h * zoom <= height - 2 * VIEW_PAD.y ? (height - e.h * zoom) / 2 - e.y * zoom : VIEW_PAD.y - e.y * zoom
  return { x, y, zoom }
}

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
  let x0 = Number.POSITIVE_INFINITY
  let y0 = Number.POSITIVE_INFINITY
  let x1 = Number.NEGATIVE_INFINITY
  let y1 = Number.NEGATIVE_INFINITY
  for (const p of pos.values()) {
    x0 = Math.min(x0, p.x)
    y0 = Math.min(y0, p.y)
    x1 = Math.max(x1, p.x + BOX.w)
    y1 = Math.max(y1, p.y + BOX.h)
  }
  for (const e of view.edges) {
    const a = pos.get(e.edge.a)
    const b = pos.get(e.edge.b)
    if (!a || !b) continue
    // Same row with room for a card between them: an arc over the row (top → top), never through the cards between.
    // Otherwise side handles when the ends sit apart horizontally; one above the other (a parent and its child, whose
    // spawn line runs between them) right side to right side, so the message curve bulges clear of the spawn line.
    const [left, right] = a.x <= b.x ? [e.edge.a, e.edge.b] : [e.edge.b, e.edge.a]
    const dx = Math.abs(a.x - b.x)
    const apart = dx >= BOX.w
    const arc = a.y === b.y && dx >= 2 * BOX.w
    const [upper, lower] = a.y <= b.y ? [e.edge.a, e.edge.b] : [e.edge.b, e.edge.a]
    const onPath = path.has(e.edge.a) && path.has(e.edge.b)
    const source = apart ? left : upper
    if (arc) y0 = Math.min(y0, a.y - 0.75 * arcRise(dx))
    edges.push({
      type: 'message',
      id: e.edge.id,
      source,
      target: apart ? right : lower,
      sourceHandle: arc ? 'ts' : 'r',
      targetHandle: arc ? 't' : apart ? 'l' : 'rt',
      fwd: source === e.edge.a ? e.edge.ab : e.edge.ba,
      back: source === e.edge.a ? e.edge.ba : e.edge.ab,
      count: e.edge.ab + e.edge.ba,
      active: e.active,
      animated: e.animated,
      dim: lit && !onPath,
    })
  }
  const extent = pos.size === 0 ? { x: 0, y: 0, w: 0, h: 0 } : { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
  return { nodes, edges, slots, pos, extent }
}
