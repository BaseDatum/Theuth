// Design 65 §3.4: `layout(props)` — a structural hash of the `Graph` props, the props translated to an ELK graph
// (groups as compound nodes, `parent` nests them, 123 §4.1), laid out through the kernel's `elkLayout`
// (`lib/diagram/elk.ts`: LRU by hash, in-flight sharing, the worker, the 2 s time-box, label measurement).
import type { ElkExtendedEdge, ElkNode } from 'elkjs/lib/elk-api.js'
import type { GraphEdge } from '@/rpc/bindings/GraphEdge'
import type { GraphGroup } from '@/rpc/bindings/GraphGroup'
import type { GraphNode } from '@/rpc/bindings/GraphNode'
import type { GraphProps } from '@/rpc/bindings/GraphProps'
import { ELK_OPTIONS, elkLayout, flattenElk, fnv, GROUP_OPTIONS, labelSize, type Layout, nodeSize, norm } from './diagram/elk'
import type { Loose } from './diagram/types'

export { FONT_PX, LAYOUT_TIMEOUT_MS, type Layout, type LayoutEdge, type LayoutGroup, type LayoutNode, LayoutTimeout, LINE_PX, LruCache, WRAP, wrapLabel } from './diagram/elk'
export type { Point, Size } from './diagram/types'

/** `GraphProps` as the JSON the model writes: optional keys may be missing, `null` or set. */
export type GraphData = Loose<Omit<GraphProps, 'nodes' | 'edges' | 'groups' | 'legend'>> & {
  nodes: Loose<GraphNode>[]
  edges: Loose<GraphEdge>[]
  groups?: Loose<GraphGroup>[] | null
  legend?: Loose<NonNullable<GraphProps['legend']>[number]>[] | null
}

export function structuralHash(p: GraphData): string {
  const parts = [
    p.direction ?? 'LR',
    ...(p.groups ?? []).map((g) => `g\u0001${g.id}\u0001${norm(g.label)}\u0001${g.parent ?? ''}\u0001${g.kind ?? ''}`),
    ...p.nodes.map((n) => `n\u0001${n.id}\u0001${n.kind ?? ''}\u0001${n.group ?? ''}\u0001${n.state ?? ''}\u0001${norm(n.label)}\u0001${norm(n.sub)}\u0001${norm(n.tag)}`),
    ...p.edges.map((e) => `e\u0001${e.from}\u0001${e.to}\u0001${e.style ?? ''}\u0001${norm(e.label)}\u0001${e.arrow ?? ''}`),
  ]
  return fnv(parts.join('\u0002'))
}

/**
 * The props as an ELK graph: groups are compound nodes (a group with a `parent` nests inside it; an unknown parent
 * or a cycle falls back to the root), every edge sits on the root. A node's `sub`/`tag` add a line to its box.
 */
export function toElk(graph: GraphData): ElkNode {
  const groups = graph.groups ?? []
  const groupIds: Record<string, true> = {}
  for (const g of groups) groupIds[g.id] = true
  const node = (n: GraphData['nodes'][number]): ElkNode => {
    const size = nodeSize(n.label, (n.sub ? 1 : 0) + (n.tag ? 1 : 0))
    return { id: `n:${n.id}`, width: size.w, height: size.h }
  }
  const compound: Record<string, ElkNode> = {}
  for (const g of groups) compound[g.id] = { id: `g:${g.id}`, layoutOptions: GROUP_OPTIONS, children: [] }
  for (const n of graph.nodes) if (n.group && groupIds[n.group]) compound[n.group]!.children!.push(node(n))
  const children: ElkNode[] = []
  for (const g of groups) {
    const parent = g.parent && g.parent !== g.id && groupIds[g.parent] && !descends(g.parent, g.id, groups) ? compound[g.parent] : null
    ;(parent ? parent.children! : children).push(compound[g.id]!)
  }
  for (const n of graph.nodes) if (!(n.group && groupIds[n.group])) children.push(node(n))
  const edges: ElkExtendedEdge[] = graph.edges.map((e, i) => {
    const l = labelSize(e.label)
    return { id: `e:${i}`, sources: [`n:${e.from}`], targets: [`n:${e.to}`], labels: l ? [{ id: `l:${i}`, text: norm(e.label), width: l.w, height: l.h }] : [] }
  })
  return { id: 'root', layoutOptions: { ...ELK_OPTIONS, 'elk.direction': graph.direction === 'TB' ? 'DOWN' : 'RIGHT' }, children, edges }
}

/** True when `ancestor` is reached by walking `parent` links up from `id` (so nesting it under `id` would cycle). */
function descends(ancestor: string, id: string, groups: readonly { id: string; parent?: string | null }[]): boolean {
  const parentOf: Record<string, string> = {}
  for (const g of groups) if (g.parent) parentOf[g.id] = g.parent
  let cur: string | undefined = parentOf[ancestor]
  for (let guard = 0; cur !== undefined && guard < groups.length + 1; guard++) {
    if (cur === id) return true
    cur = parentOf[cur]
  }
  return false
}

/**
 * The layout of `props`: from the cache when its structural hash is known, else measured here and laid out in
 * the worker. Rejects with `LayoutTimeout` after 2 s (a late result still lands in the cache).
 */
export function layout(props: GraphData, hash = structuralHash(props)): Promise<Layout> {
  return elkLayout(hash, () => toElk(props), flattenElk)
}
