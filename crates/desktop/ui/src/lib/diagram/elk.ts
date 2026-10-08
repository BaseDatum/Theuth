// Design 123 §5.1 — `graph-layout.ts` generalised: the ELK worker, the structural-hash LRU, the in-flight sharing,
// the 2 s time-box and the label measurer, for any component that builds an `ElkNode` (Graph, StateMachine,
// Entities, Swimlane, GraphDelta, Tree nodes|org). `flattenElk` turns ELK's nested result into root coordinates.
// W1 adds ports / partitions / markers on top of this file additively; the exports below are FROZEN (README.md).
import ELK, { type ELK as ElkApi, type ElkExtendedEdge, type ElkNode } from 'elkjs/lib/elk-api.js'
import { diagramTokens } from '../diagram-theme'
import type { Point, Size } from './types'

export interface LayoutNode extends Point, Size {
  id: string
}
export interface LayoutGroup extends Point, Size {
  id: string
}
export interface LayoutEdge {
  /** Index into the component's edge array. */
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

/** Characters per label line before wrapping. */
export const WRAP = 24
export const FONT_PX = 13
export const LINE_PX = 17
export const PAD_X = 12
export const PAD_Y = 8
export const MIN_SIZE: Size = { w: 64, h: 32 }
export const LAYOUT_TIMEOUT_MS = 2000

/** ELK options of 65 §3.4: layered, orthogonal, model order, compound groups. */
export const ELK_OPTIONS = {
  'elk.algorithm': 'layered',
  'elk.edgeRouting': 'ORTHOGONAL',
  'elk.layered.considerModelOrder.strategy': 'NODES_AND_EDGES',
  'elk.hierarchyHandling': 'INCLUDE_CHILDREN',
  'elk.spacing.nodeNode': '24',
  'elk.layered.spacing.nodeNodeBetweenLayers': '40',
  'elk.spacing.edgeLabel': '4',
}
/** Padding of a compound (group) node: a title band on top. */
export const GROUP_OPTIONS = { 'elk.padding': '[top=28,left=12,bottom=12,right=12]' }

/** Whitespace-normalised: the hash and the drawing ignore label spacing the model happened to emit. */
export function norm(s: string | null | undefined): string {
  return (s ?? '').replace(/\s+/g, ' ').trim()
}

/** FNV-1a of `s` as 8 hex digits — the structural-hash primitive. */
export function fnv(s: string): string {
  let h = 0x811c9dc5
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 0x01000193)
  return (h >>> 0).toString(16).padStart(8, '0')
}

/** Word-wrap `text` at `WRAP` characters; a longer word is broken by character. */
export function wrapLabel(text: string): string[] {
  const words = norm(text).split(' ')
  const lines: string[] = []
  let line = ''
  for (const w of words) {
    if (!line) line = w
    else if (line.length + 1 + w.length <= WRAP) line += ` ${w}`
    else {
      lines.push(line)
      line = w
    }
    while (line.length > WRAP) {
      lines.push(line.slice(0, WRAP))
      line = line.slice(WRAP)
    }
  }
  if (line) lines.push(line)
  return lines.length ? lines : ['']
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

let ctx: CanvasRenderingContext2D | null = null
/** Text width in the app font at `px` (canvas `measureText`, never `getBBox`); `0.6 em` per character without a DOM (tests). */
export function textWidth(text: string, px = FONT_PX): number {
  if (!ctx) {
    if (typeof document === 'undefined') return text.length * px * 0.6
    ctx = document.createElement('canvas').getContext('2d')
    if (!ctx) return text.length * px * 0.6
  }
  ctx.font = `${px}px ${diagramTokens().font}`
  return ctx.measureText(text).width
}

/** A node box for `label`: wrapped lines plus padding, never under `MIN_SIZE`; `extraLines` adds `sub`/`tag` rows. */
export function nodeSize(label: string, extraLines = 0): Size {
  const lines = wrapLabel(label)
  const w = Math.max(...lines.map((l) => textWidth(l))) + 2 * PAD_X
  const h = (lines.length + extraLines) * LINE_PX + 2 * PAD_Y
  return { w: Math.max(MIN_SIZE.w, Math.ceil(w)), h: Math.max(MIN_SIZE.h, Math.ceil(h)) }
}

/** An edge-label box for `text` (one line, small font), or `undefined` for an empty label. */
export function labelSize(text: string | null | undefined): Size | undefined {
  const t = norm(text)
  return t ? { w: Math.ceil(textWidth(t, FONT_PX - 1) + 8), h: LINE_PX + 2 } : undefined
}

/**
 * ELK's result flattened to root coordinates. Child coordinates are relative to their parent; an edge's are relative
 * to its `container` (the lowest common ancestor of its ends) — ELK names it when it is not the root.
 * Ids: `n:<id>` nodes, `g:<id>` groups (any depth), `e:<index>` edges, `l:<index>` labels.
 */
export function flattenElk(root: ElkNode): Layout {
  const out: Layout = { nodes: [], edges: [], groups: [], width: root.width ?? 0, height: root.height ?? 0 }
  const origin: Record<string, Point> = { [root.id]: { x: 0, y: 0 } }
  const visit = (n: ElkNode, ox: number, oy: number) => {
    const x = ox + (n.x ?? 0)
    const y = oy + (n.y ?? 0)
    origin[n.id] = { x, y }
    const box = { id: n.id.slice(2), x, y, w: n.width ?? 0, h: n.height ?? 0 }
    if (n.id.startsWith('g:')) {
      out.groups.push(box)
      for (const c of n.children ?? []) visit(c, x, y)
    } else out.nodes.push(box)
  }
  for (const c of root.children ?? []) visit(c, 0, 0)
  const edges = (n: ElkNode): ElkExtendedEdge[] => [...(n.edges ?? []), ...(n.children ?? []).flatMap(edges)]
  for (const e of edges(root)) {
    const o = origin[(e as { container?: string }).container ?? root.id] ?? { x: 0, y: 0 }
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

const cache = new LruCache<unknown>(64)
let elk: ElkApi | null = null
/** Callers waiting per hash; several cards with the same diagram share one layout. */
const pending = new Map<string, Promise<unknown>>()
let fontsReady: Promise<unknown> | null = null

/** 135 R2.3: the cached layout for `hash` now, no promise — a remounted diagram paints it on its first frame. */
export function elkPeek<T>(hash: string): T | undefined {
  return cache.get(hash) as T | undefined
}

/**
 * The layout keyed by `hash`: from the LRU when known, else `build()` (called after `document.fonts.ready`, so
 * `textWidth` measures the real font) is laid out in ELK's worker and `map`ped. Concurrent requests for one hash
 * share the call. Rejects with `LayoutTimeout` after 2 s (a late result still lands in the cache).
 */
export async function elkLayout<T>(hash: string, build: () => ElkNode, map: (root: ElkNode) => T): Promise<T> {
  const hit = cache.get(hash)
  if (hit !== undefined) return hit as T
  await (fontsReady ??= document.fonts.ready)
  const again = cache.get(hash)
  if (again !== undefined) return again as T
  const { promise, reject } = Promise.withResolvers<T>()
  const timer = setTimeout(() => reject(new LayoutTimeout()), LAYOUT_TIMEOUT_MS)
  try {
    return await Promise.race([run(hash, build, map), promise])
  } finally {
    clearTimeout(timer)
  }
}

function run<T>(hash: string, build: () => ElkNode, map: (root: ElkNode) => T): Promise<T> {
  const inflight = pending.get(hash)
  if (inflight) return inflight as Promise<T>
  elk ??= new ELK({ workerFactory: () => new Worker(new URL('../graph-layout.worker.ts', import.meta.url), { type: 'module' }) })
  const p = elk
    .layout(build())
    .then((root) => {
      const l = map(root)
      cache.set(hash, l)
      return l
    })
    .finally(() => pending.delete(hash))
  pending.set(hash, p)
  return p
}
