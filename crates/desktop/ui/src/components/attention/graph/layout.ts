// 208 §8.3: the Attention graph's tidy tree. Pure: positions depend only on structure (child lists in spawn order), so a
// state change never moves a node; root slots are sticky (D13) so a growing tree pushes its right-hand neighbours only.
import type { GraphNode } from '@/rpc/bindings/method.agent.output'

export type Box = { w: number; h: number; gx: number; gy: number; tree: number }
/** Node box 200 × 64; sibling gap 24; level gap 56 (row pitch 120); gap between trees 80. */
export const BOX: Box = { w: 200, h: 64, gx: 24, gy: 56, tree: 80 }
export type Pos = { x: number; y: number }
export type LayoutCache = Map<string, { pos: Map<string, Pos>; width: number }>

const byStart = (a: GraphNode, b: GraphNode): number => a.started_ms - b.started_ms || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0)

/** parent id → child ids among `nodes` (only parents present in `nodes`), ordered by started_ms asc, then id asc. */
export function childOrder(nodes: readonly GraphNode[]): Map<string, string[]> {
  const present = new Set(nodes.map((n) => n.id))
  const out = new Map<string, string[]>()
  for (const n of [...nodes].sort(byStart)) {
    if (n.parent === null || !present.has(n.parent)) continue
    const list = out.get(n.parent)
    if (list) list.push(n.id)
    else out.set(n.parent, [n.id])
  }
  return out
}

/** Structural key: `id(child,child(grandchild))`, recursive, in child order. */
export function treeSignature(root: string, children: ReadonlyMap<string, readonly string[]>): string {
  const kids = children.get(root)
  if (!kids || kids.length === 0) return root
  return `${root}(${kids.map((k) => treeSignature(k, children)).join(',')})`
}

type Sub = { pos: Map<string, Pos>; left: number[]; right: number[] }

function place(id: string, depth: number, children: ReadonlyMap<string, readonly string[]>, box: Box): Sub {
  const y = depth * (box.h + box.gy)
  const kids = children.get(id) ?? []
  if (kids.length === 0) return { pos: new Map([[id, { x: 0, y }]]), left: [0], right: [box.w] }
  const pos = new Map<string, Pos>()
  const accLeft: number[] = []
  const accRight: number[] = []
  const xs: number[] = []
  kids.forEach((kid, i) => {
    const sub = place(kid, depth + 1, children, box)
    let shift = 0
    if (i > 0) {
      shift = Number.NEGATIVE_INFINITY
      const shared = Math.min(accRight.length, sub.left.length)
      for (let d = 0; d < shared; d++) shift = Math.max(shift, (accRight[d] as number) + box.gx - (sub.left[d] as number))
    }
    for (const [k, p] of sub.pos) pos.set(k, { x: p.x + shift, y: p.y })
    xs.push((sub.pos.get(kid) as Pos).x + shift)
    sub.left.forEach((l, d) => {
      accLeft[d] = accLeft[d] === undefined ? l + shift : Math.min(accLeft[d] as number, l + shift)
    })
    sub.right.forEach((r, d) => {
      accRight[d] = accRight[d] === undefined ? r + shift : Math.max(accRight[d] as number, r + shift)
    })
  })
  const x = ((xs[0] as number) + (xs[xs.length - 1] as number)) / 2
  pos.set(id, { x, y })
  return { pos, left: [x, ...accLeft], right: [x + box.w, ...accRight] }
}

/** One tree, positions of each box's top-left relative to the tree (contour packing; see the contract §8.3). */
export function layoutTree(root: string, children: ReadonlyMap<string, readonly string[]>, box: Box = BOX): { pos: Map<string, Pos>; width: number } {
  const sub = place(root, 0, children, box)
  const min = Math.min(...sub.left)
  const pos = new Map<string, Pos>()
  for (const [k, p] of sub.pos) pos.set(k, { x: p.x - min, y: p.y })
  return { pos, width: Math.max(...sub.right) - min }
}

/** Sticky root slots (208 D13): trees with a previous slot keep their order and never move left of it. */
export function packRoots(trees: readonly { root: string; width: number }[], prev: ReadonlyMap<string, number> | null, gap: number = BOX.tree): Map<string, number> {
  const known = trees
    .map((t, i) => ({ t, i, x: prev?.get(t.root) }))
    .filter((e): e is { t: { root: string; width: number }; i: number; x: number } => e.x !== undefined)
    .sort((a, b) => a.x - b.x || a.i - b.i)
  const fresh = trees.filter((t) => prev?.get(t.root) === undefined)
  const out = new Map<string, number>()
  let cursor = 0
  for (const { t, x: was } of known) {
    const x = Math.max(was, cursor)
    out.set(t.root, x)
    cursor = x + t.width + gap
  }
  for (const t of fresh) {
    out.set(t.root, cursor)
    cursor += t.width + gap
  }
  return out
}

/** A bounded cache (LRU by insertion, `limit` entries). */
export function layoutCache(limit = 256): LayoutCache {
  const cache: LayoutCache = new Map()
  const set = cache.set.bind(cache)
  cache.set = (key, value) => {
    cache.delete(key)
    set(key, value)
    while (cache.size > limit) cache.delete(cache.keys().next().value as string)
    return cache
  }
  return cache
}

/** layoutTree per root (memoised by treeSignature through `cache`) + packRoots; absolute positions and the new slots. */
export function layoutForest(nodes: readonly GraphNode[], prev: ReadonlyMap<string, number> | null, cache?: LayoutCache, box: Box = BOX): { pos: Map<string, Pos>; slots: Map<string, number> } {
  const present = new Set(nodes.map((n) => n.id))
  const roots = nodes.filter((n) => n.parent === null || !present.has(n.parent)).sort(byStart)
  const children = childOrder(nodes)
  const trees = roots.map((r) => {
    const sig = treeSignature(r.id, children)
    let tree = cache?.get(sig)
    if (tree) cache?.set(sig, tree)
    else {
      tree = layoutTree(r.id, children, box)
      cache?.set(sig, tree)
    }
    return { root: r.id, ...tree }
  })
  const slots = packRoots(trees, prev, box.tree)
  const pos = new Map<string, Pos>()
  for (const t of trees) {
    const x0 = slots.get(t.root) as number
    for (const [k, p] of t.pos) pos.set(k, { x: x0 + p.x, y: p.y })
  }
  return { pos, slots }
}
