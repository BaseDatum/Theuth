// Design 65 §3.4: `layout(props)` — a structural hash of the props, an LRU of finished layouts (an unchanged graph
// re-renders without touching the worker), label measurement with the app font on a canvas (never `getBBox`), the
// props translated to an ELK graph and laid out in ELK's own worker, and a 2 s time-box after which the caller draws
// the adjacency list instead.
import ELK, { type ELK as ElkApi, type ElkExtendedEdge, type ElkNode } from 'elkjs/lib/elk-api.js'
import type { GraphEdge } from '@/rpc/bindings/GraphEdge'
import type { GraphGroup } from '@/rpc/bindings/GraphGroup'
import type { GraphNode } from '@/rpc/bindings/GraphNode'
import type { GraphProps } from '@/rpc/bindings/GraphProps'
import { diagramTokens } from './diagram-theme'

export interface Size {
  w: number
  h: number
}
export interface Point {
  x: number
  y: number
}
export interface LayoutNode extends Point, Size {
  id: string
}
export interface LayoutGroup extends Point, Size {
  id: string
}
export interface LayoutEdge {
  /** Index into `GraphData.edges`. */
  index: number
  sections: { start: Point; bends: Point[]; end: Point }[]
  label?: Point & Size
}
/** Boxes and routes in root coordinates (children flattened), plus the total size. */
export interface Layout {
  nodes: LayoutNode[]
  edges: LayoutEdge[]
  groups: LayoutGroup[]
  width: number
  height: number
}

/** ts-rs spells `Option<T>` as a required `T | null`; on the wire the key is simply absent. */
type Loose<T> = { [K in keyof T as null extends T[K] ? never : K]: T[K] } & { [K in keyof T as null extends T[K] ? K : never]?: T[K] | undefined }
/** `GraphProps` as the JSON the model writes: optional keys may be missing, `null` or set. */
export type GraphData = Loose<Omit<GraphProps, 'nodes' | 'edges' | 'groups'>> & {
  nodes: Loose<GraphNode>[]
  edges: Loose<GraphEdge>[]
  groups?: Loose<GraphGroup>[] | null
}

/** Characters per label line before wrapping. */
export const WRAP = 24
export const FONT_PX = 13
export const LINE_PX = 17
const PAD_X = 12
const PAD_Y = 8
const MIN: Size = { w: 64, h: 32 }
export const LAYOUT_TIMEOUT_MS = 2000

/** Whitespace-normalised: the hash and the drawing ignore label spacing the model happened to emit. */
function norm(s: string | null | undefined): string {
  return (s ?? '').replace(/\s+/g, ' ').trim()
}

export function structuralHash(p: GraphData): string {
  const parts = [
    p.direction ?? 'LR',
    ...(p.groups ?? []).map((g) => `g\u0001${g.id}\u0001${norm(g.label)}`),
    ...p.nodes.map((n) => `n\u0001${n.id}\u0001${n.kind ?? ''}\u0001${n.group ?? ''}\u0001${n.state ?? ''}\u0001${norm(n.label)}`),
    ...p.edges.map((e) => `e\u0001${e.from}\u0001${e.to}\u0001${e.style ?? ''}\u0001${norm(e.label)}`),
  ]
  const s = parts.join('\u0002')
  let h = 0x811c9dc5
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 0x01000193)
  return (h >>> 0).toString(16).padStart(8, '0')
}

/** Word-wrap `text` at `WRAP` characters; a longer word is broken by character. */
export function wrapLabel(text: string): string[] {
  const out: string[] = []
  let line = ''
  for (const word of norm(text).split(' ')) {
    if (line && line.length + 1 + word.length > WRAP) {
      out.push(line)
      line = ''
    }
    if (word.length <= WRAP) {
      line = line ? `${line} ${word}` : word
      continue
    }
    for (const ch of word) {
      if (line.length >= WRAP) {
        out.push(line)
        line = ''
      }
      line += ch
    }
  }
  out.push(line)
  return out
}

/** Least-recently-used map with a fixed capacity; `get` refreshes. */
export class LruCache<V> {
  readonly #map = new Map<string, V>()
  constructor(readonly max: number) {}
  get size(): number {
    return this.#map.size
  }
  get(key: string): V | undefined {
    const v = this.#map.get(key)
    if (v === undefined) return undefined
    this.#map.delete(key)
    this.#map.set(key, v)
    return v
  }
  set(key: string, value: V): void {
    this.#map.delete(key)
    this.#map.set(key, value)
    if (this.#map.size > this.max) this.#map.delete(this.#map.keys().next().value!)
  }
  has(key: string): boolean {
    return this.#map.has(key)
  }
}

export class LayoutTimeout extends Error {
  constructor() {
    super(`layout took longer than ${LAYOUT_TIMEOUT_MS} ms`)
  }
}

const cache = new LruCache<Layout>(64)

let ctx: CanvasRenderingContext2D | null = null
/** Text width in the app font at `FONT_PX`. */
function textWidth(text: string): number {
  if (!ctx) {
    ctx = document.createElement('canvas').getContext('2d')
    if (!ctx) return text.length * FONT_PX * 0.6
    ctx.font = `${FONT_PX}px ${diagramTokens().font}`
  }
  return ctx.measureText(text).width
}

/** Node boxes from wrapped labels; edge-label boxes for labelled edges. */
function measure(p: GraphData): { sizes: Record<string, Size>; labels: Record<number, Size> } {
  const sizes: Record<string, Size> = {}
  for (const n of p.nodes) {
    const lines = wrapLabel(n.label)
    const w = Math.max(...lines.map(textWidth)) + 2 * PAD_X
    const h = lines.length * LINE_PX + 2 * PAD_Y
    sizes[n.id] = { w: Math.max(MIN.w, Math.ceil(w)), h: Math.max(MIN.h, Math.ceil(h)) }
  }
  const labels: Record<number, Size> = {}
  p.edges.forEach((e, i) => {
    if (norm(e.label)) labels[i] = { w: Math.ceil(textWidth(norm(e.label)) + 8), h: LINE_PX + 2 }
  })
  return { sizes, labels }
}

/** ELK options of 65 §3.4. */
const OPTIONS = {
  'elk.algorithm': 'layered',
  'elk.edgeRouting': 'ORTHOGONAL',
  'elk.layered.considerModelOrder.strategy': 'NODES_AND_EDGES',
  'elk.hierarchyHandling': 'INCLUDE_CHILDREN',
  'elk.spacing.nodeNode': '24',
  'elk.layered.spacing.nodeNodeBetweenLayers': '40',
  'elk.spacing.edgeLabel': '4',
}
const GROUP_OPTIONS = { 'elk.padding': '[top=28,left=12,bottom=12,right=12]' }

/** The props as an ELK graph: groups are compound nodes, every edge sits on the root. */
function toElk(graph: GraphData, sizes: Record<string, Size>, labels: Record<number, Size>): ElkNode {
  const groups = graph.groups ?? []
  const groupIds: Record<string, true> = {}
  for (const g of groups) groupIds[g.id] = true
  const node = (id: string): ElkNode => ({ id: `n:${id}`, width: sizes[id]?.w ?? MIN.w, height: sizes[id]?.h ?? MIN.h })
  const children: ElkNode[] = groups.map((g) => ({
    id: `g:${g.id}`,
    layoutOptions: GROUP_OPTIONS,
    children: graph.nodes.filter((n) => n.group === g.id).map((n) => node(n.id)),
  }))
  for (const n of graph.nodes) if (!(n.group && groupIds[n.group])) children.push(node(n.id))
  const edges: ElkExtendedEdge[] = graph.edges.map((e, i) => ({
    id: `e:${i}`,
    sources: [`n:${e.from}`],
    targets: [`n:${e.to}`],
    labels: labels[i] ? [{ id: `l:${i}`, text: norm(e.label), width: labels[i].w, height: labels[i].h }] : [],
  }))
  return { id: 'root', layoutOptions: { ...OPTIONS, 'elk.direction': graph.direction === 'TB' ? 'DOWN' : 'RIGHT' }, children, edges }
}

/**
 * ELK's result flattened. Child coordinates are relative to their parent. Edge coordinates are relative to the
 * edge's container, which ELK makes the lowest common ancestor of its endpoints: the group when both ends sit in
 * one group, the root otherwise (groups nest one level, 65 §0).
 */
function fromElk(root: ElkNode): Layout {
  const out: Layout = { nodes: [], edges: [], groups: [], width: root.width ?? 0, height: root.height ?? 0 }
  /** Absolute origin of the group each node sits in; absent for root-level nodes. */
  const groupOf: Record<string, Point> = {}
  const visit = (n: ElkNode, ox: number, oy: number, group?: Point) => {
    const x = ox + (n.x ?? 0)
    const y = oy + (n.y ?? 0)
    const box = { id: n.id.slice(2), x, y, w: n.width ?? 0, h: n.height ?? 0 }
    if (n.id.startsWith('g:')) {
      out.groups.push(box)
      for (const c of n.children ?? []) visit(c, x, y, { x, y })
    } else {
      out.nodes.push(box)
      if (group) groupOf[n.id] = group
    }
  }
  for (const c of root.children ?? []) visit(c, 0, 0)
  for (const e of root.edges ?? []) {
    const a = groupOf[e.sources[0]!]
    const b = groupOf[e.targets[0]!]
    const o = a && b && a.x === b.x && a.y === b.y ? a : { x: 0, y: 0 }
    const shift = (p: Point): Point => ({ x: p.x + o.x, y: p.y + o.y })
    const l = e.labels?.[0]
    out.edges.push({
      index: Number(e.id.slice(2)),
      sections: (e.sections ?? []).map((s) => ({ start: shift(s.startPoint), bends: (s.bendPoints ?? []).map(shift), end: shift(s.endPoint) })),
      ...(l && l.x !== undefined && l.y !== undefined ? { label: { ...shift({ x: l.x, y: l.y }), w: l.width ?? 0, h: l.height ?? 0 } } : {}),
    })
  }
  return out
}

let elk: ElkApi | null = null
/** Callers waiting per hash; several cards with the same graph share one layout. */
const pending = new Map<string, Promise<Layout>>()

/** Lay `graph` out in ELK's worker (created on first use); concurrent requests for one hash share the call. */
function run(hash: string, graph: GraphData): Promise<Layout> {
  const inflight = pending.get(hash)
  if (inflight) return inflight
  elk ??= new ELK({ workerFactory: () => new Worker(new URL('./graph-layout.worker.ts', import.meta.url), { type: 'module' }) })
  const { sizes, labels } = measure(graph)
  const p = elk
    .layout(toElk(graph, sizes, labels))
    .then((root) => {
      const l = fromElk(root)
      cache.set(hash, l)
      return l
    })
    .finally(() => pending.delete(hash))
  pending.set(hash, p)
  return p
}

let fontsReady: Promise<unknown> | null = null

/**
 * The layout of `props`: from the cache when its structural hash is known, else measured here and laid out in
 * the worker. Rejects with `LayoutTimeout` after 2 s (a late result still lands in the cache).
 */
export async function layout(props: GraphData, hash = structuralHash(props)): Promise<Layout> {
  const hit = cache.get(hash)
  if (hit) return hit
  await (fontsReady ??= document.fonts.ready)
  const again = cache.get(hash)
  if (again) return again
  const { promise, reject } = Promise.withResolvers<Layout>()
  const timer = setTimeout(() => reject(new LayoutTimeout()), LAYOUT_TIMEOUT_MS)
  try {
    return await Promise.race([run(hash, props), promise])
  } finally {
    clearTimeout(timer)
  }
}
