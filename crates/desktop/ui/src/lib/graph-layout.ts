// Design 65 §3.4 / 123 §5.2(a): `layout(props)` — a structural hash of the `Graph` props, the props translated to an
// ELK graph (groups as compound nodes, `parent` nests them, 123 §4.1), laid out through the kernel's `elkLayout`
// (`lib/diagram/elk.ts`: LRU by hash, in-flight sharing, the worker, the 2 s time-box, label measurement). The
// `useElkLayout` / `useDiagramTokens` hooks at the end are what every ELK-family `<NameView>` (Graph, StateMachine,
// Entities, Swimlane, GraphDelta, Tree nodes|org) shares: the render queue, the last good layout, the time-out.
import type { ElkExtendedEdge, ElkNode } from 'elkjs/lib/elk-api.js'
import { useEffect, useRef, useState } from 'react'
import type { GraphEdge } from '@/rpc/bindings/GraphEdge'
import type { GraphGroup } from '@/rpc/bindings/GraphGroup'
import type { GraphNode } from '@/rpc/bindings/GraphNode'
import type { GraphProps } from '@/rpc/bindings/GraphProps'
import { ELK_OPTIONS, elkLayout, elkPeek, flattenElk, fnv, GROUP_OPTIONS, labelSize, type Layout, LayoutTimeout, nodeSize, norm } from './diagram/elk'
import type { Loose } from './diagram/types'
import { createRenderQueue, type RenderQueue } from './render-queue'

export { flattenElk, FONT_PX, LAYOUT_TIMEOUT_MS, type Layout, type LayoutEdge, type LayoutGroup, type LayoutNode, LayoutTimeout, LINE_PX, LruCache, nodeSize, WRAP, wrapLabel } from './diagram/elk'
export type { Point, Size } from './diagram/types'

/** `GraphProps` as the JSON the model writes: optional keys may be missing, `null` or set. */
export type GraphData = Loose<Omit<GraphProps, 'nodes' | 'edges' | 'groups' | 'legend'>> & {
  nodes: Loose<GraphNode>[]
  edges: Loose<GraphEdge>[]
  groups?: Loose<GraphGroup>[] | null
  legend?: Loose<NonNullable<GraphProps['legend']>[number]>[] | null
}

/** Width an inline icon adds to a node box (16 px glyph + gap). */
export const ICON_W = 22

/** §5.4 #3/#4: edges never share a stroke, labels sit 8 px off it; layered on top of the kernel's options. */
export const EDGE_OPTIONS = {
  'elk.spacing.edgeEdge': '12',
  'elk.layered.spacing.edgeEdgeBetweenLayers': '12',
  'elk.spacing.edgeLabel': '8',
  'elk.layered.mergeEdges': 'false',
  'elk.spacing.portPort': '12',
}

export function structuralHash(p: GraphData): string {
  const parts = [
    p.direction ?? 'LR',
    ...(p.groups ?? []).map((g) => `g\u0001${g.id}\u0001${norm(g.label)}\u0001${g.parent ?? ''}\u0001${g.kind ?? ''}`),
    ...p.nodes.map((n) => `n\u0001${n.id}\u0001${n.kind ?? ''}\u0001${n.group ?? ''}\u0001${n.state ?? ''}\u0001${norm(n.label)}\u0001${norm(n.sub)}\u0001${norm(n.tag)}\u0001${n.icon ?? ''}`),
    ...p.edges.map((e) => `e\u0001${e.from}\u0001${e.to}\u0001${e.style ?? ''}\u0001${norm(e.label)}\u0001${e.arrow ?? ''}`),
  ]
  return fnv(parts.join('\u0002'))
}

/** The box of a graph node: wrapped label plus a line each for `sub` and `tag`, wider by the icon. */
export function graphNodeSize(n: Pick<GraphData['nodes'][number], 'label' | 'sub' | 'tag' | 'icon'>): { w: number; h: number } {
  const size = nodeSize(n.label, (n.sub ? 1 : 0) + (n.tag ? 1 : 0))
  return n.icon ? { w: size.w + ICON_W, h: size.h } : size
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
    const size = graphNodeSize(n)
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
  return { id: 'root', layoutOptions: { ...ELK_OPTIONS, ...EDGE_OPTIONS, 'elk.direction': graph.direction === 'TB' ? 'DOWN' : 'RIGHT' }, children, edges }
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

/** The diagram tokens for the current theme (re-read when `dark` flips), or a snapshot frame's palette. */
export { useThemeTokens as useDiagramTokens } from './snapshot-mode'

/** What an ELK-family view shows: the layout for `hash`, or the time-out (the §4.4 text stands in). */
export type Shown<T> = { hash: string; layout: T } | { hash: string; timeout: true }

type Req<T> = { hash: string; build: () => ElkNode; map: (root: ElkNode) => T }

/**
 * Layout through the render queue (coalesced while streaming, the last good layout stays on screen until the next
 * lands), `LayoutTimeout` → `{ timeout }` — never a blank card. `build`/`map` are read when the request runs;
 * `enabled: false` (a layout the component computes itself) requests nothing. 135 R2.3: a hash the kernel's LRU
 * already holds is shown on the first render, no request.
 */
export function useElkLayout<T>(hash: string, build: () => ElkNode, map: (root: ElkNode) => T, enabled = true): Shown<T> | null {
  const [shown, setShown] = useState<Shown<T> | null>(() => {
    const layout = enabled ? elkPeek<T>(hash) : undefined
    return layout === undefined ? null : { hash, layout }
  })
  const queue = useRef<RenderQueue<Req<T>> | null>(null)
  useEffect(() => {
    const q = createRenderQueue<Req<T>>(async (req, isCurrent) => {
      try {
        const l = await elkLayout(req.hash, req.build, req.map)
        if (isCurrent()) setShown({ hash: req.hash, layout: l })
      } catch (e) {
        if (isCurrent() && e instanceof LayoutTimeout) setShown({ hash: req.hash, timeout: true })
      }
    })
    queue.current = q
    return () => q.dispose()
  }, [])
  const latest = useRef({ build, map })
  latest.current = { build, map }
  const have = shown !== null && 'layout' in shown && shown.hash === hash
  useEffect(() => {
    if (enabled && !have) queue.current?.request({ hash, build: () => latest.current.build(), map: (r) => latest.current.map(r) })
  }, [hash, enabled, have])
  return shown
}

/**
 * A deterministic stand-in for the worker: `root`'s children on a diagonal grid (groups around their members),
 * every edge one orthogonal section between its ends' centres (an elbow at the target's x), labels before the
 * longest run's midpoint (where `edgeLabelAt` puts them without ELK). Component tests render the pure `<NameSvg>`
 * with it, so snapshots never depend on ELK.
 */
export function fixedLayout(root: ElkNode): Layout {
  const out: Layout = { nodes: [], edges: [], groups: [], width: 0, height: 0 }
  const at: Record<string, { x: number; y: number; w: number; h: number }> = {}
  let i = 0
  const visit = (n: ElkNode, depth: number) => {
    if (n.id.startsWith('g:')) {
      const start = i
      for (const c of n.children ?? []) visit(c, depth + 1)
      const members = out.nodes.slice(start)
      const x = Math.min(...members.map((m) => m.x)) - 12 - 8 * depth
      const y = Math.min(...members.map((m) => m.y)) - 28 - 8 * depth
      const box = { id: n.id.slice(2), x, y, w: Math.max(...members.map((m) => m.x + m.w)) + 12 + 8 * depth - x, h: Math.max(...members.map((m) => m.y + m.h)) + 12 + 8 * depth - y }
      out.groups.unshift(box)
      return
    }
    const box = { id: n.id.slice(2), x: 40 + i * 160, y: 40 + i * 80, w: n.width ?? 64, h: n.height ?? 32 }
    for (const p of n.ports ?? []) at[p.id] = { x: box.x + (p.x ?? 0), y: box.y + (p.y ?? 0), w: 0, h: 0 }
    at[n.id] = box
    out.nodes.push(box)
    i++
  }
  // Partitioned children (Swimlane) are placed partition by partition, as ELK's partitioning would band them.
  const part = (n: ElkNode) => Number(n.layoutOptions?.['org.eclipse.elk.partitioning.partition'] ?? 0)
  for (const c of [...(root.children ?? [])].sort((a, b) => part(a) - part(b))) visit(c, 0)
  const all = (n: ElkNode): ElkExtendedEdge[] => [...(n.edges ?? []), ...(n.children ?? []).flatMap(all)]
  for (const e of all(root)) {
    const a = at[e.sources[0] ?? '']
    const b = at[e.targets[0] ?? '']
    if (!a || !b) continue
    const start = { x: a.x + a.w, y: a.y + a.h / 2 }
    const end = { x: b.x + b.w / 2, y: b.y }
    const self = e.sources[0] === e.targets[0]
    const bends = self ? [{ x: start.x + 24, y: start.y }, { x: start.x + 24, y: a.y - 16 }, { x: end.x, y: a.y - 16 }] : [{ x: end.x, y: start.y }]
    const l = e.labels?.[0]
    const pts = [start, ...bends, end]
    let run = { a: start, b: end, len: -1 }
    for (let k = 1; k < pts.length; k++) {
      const len = Math.hypot(pts[k]!.x - pts[k - 1]!.x, pts[k]!.y - pts[k - 1]!.y)
      if (len > run.len) run = { a: pts[k - 1]!, b: pts[k]!, len }
    }
    const mid = { x: (run.a.x + run.b.x) / 2, y: (run.a.y + run.b.y) / 2 }
    const w = l?.width ?? 40
    const h = l?.height ?? 18
    const label = Math.abs(run.b.x - run.a.x) >= Math.abs(run.b.y - run.a.y) ? { x: mid.x - w / 2, y: mid.y - 8 - h, w, h } : { x: mid.x - 8 - w, y: mid.y - h / 2, w, h }
    out.edges.push({ index: Number(e.id.slice(2)), sections: [{ start, bends, end }], ...(l ? { label } : {}) })
  }
  out.width = Math.max(0, ...out.nodes.map((n) => n.x + n.w)) + 40
  out.height = Math.max(0, ...out.nodes.map((n) => n.y + n.h)) + 40
  return out
}
