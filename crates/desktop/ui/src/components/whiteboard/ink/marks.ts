// §8.6 marks: single-link clustering of fresh strokes — same anchor (card, version), box gap ≤ 48 world px, time gap
// ≤ 4 s. Pure; `planSend` (canvas/send.ts) turns clusters into marks and images.
import { gap, intersects, strokeWorldBox, union } from '../canvas/geom'
import { lifeOf, type Board, type Box, type Id, type Stroke } from '../canvas/model'

export const LINK_GAP = 48
export const LINK_MS = 4000

export type Cluster = { strokes: Id[]; card?: Id; ver?: number; bb: Box /* stroke space */; world: Box; at: number }

const keyOf = (s: Stroke) => (s.card ? `${s.card}#${s.ver ?? 0}` : '')

/** Clusters the given strokes (any life); strokes whose card is gone are skipped. Ordered by first stroke time. */
export function cluster(board: Board, strokes: readonly Stroke[]): Cluster[] {
  const items = strokes
    .map((s) => ({ s, w: strokeWorldBox(s, board.cards) }))
    .filter((x): x is { s: Stroke; w: Box } => x.w !== null)
    .sort((a, b) => a.s.at - b.s.at || (a.s.id < b.s.id ? -1 : 1))
  const parent = items.map((_, i) => i)
  const find = (i: number): number => {
    while (parent[i] !== i) {
      parent[i] = parent[parent[i]!]!
      i = parent[i]!
    }
    return i
  }
  for (let i = 0; i < items.length; i++) {
    for (let j = i + 1; j < items.length; j++) {
      const a = items[i]!
      const b = items[j]!
      if (keyOf(a.s) !== keyOf(b.s)) continue
      if (Math.abs(a.s.at - b.s.at) > LINK_MS) continue
      if (gap(a.w, b.w) > LINK_GAP) continue
      parent[find(j)] = find(i)
    }
  }
  const groups = new Map<number, { s: Stroke; w: Box }[]>()
  items.forEach((x, i) => {
    const r = find(i)
    const g = groups.get(r)
    if (g) g.push(x)
    else groups.set(r, [x])
  })
  return [...groups.values()].map((g) => {
    let bb: Box | null = null
    let world: Box | null = null
    for (const x of g) {
      bb = union(bb, x.s.bb)
      world = union(world, x.w)
    }
    const first = g[0]!.s
    const c: Cluster = { strokes: g.map((x) => x.s.id), bb: bb!, world: world!, at: first.at }
    if (first.card) {
      c.card = first.card
      c.ver = first.ver ?? 0
    }
    return c
  })
}

/** Fresh strokes split by the view (world box): in view when their world box intersects it. */
export function freshStrokes(board: Board, view: Box): { inView: Stroke[]; offScreen: Stroke[] } {
  const inView: Stroke[] = []
  const offScreen: Stroke[] = []
  for (const s of Object.values(board.ink)) {
    if (lifeOf(board, s.id) !== 'fresh') continue
    if (s.card) {
      // Ink on a version the card no longer shows stays with that version (it is not on screen to send).
      const card = board.cards[s.card]
      const ver = s.ver ?? 0
      if (!card || (ver !== card.versions.length && !(ver === 0 && card.versions.length <= 1))) continue
    }
    const w = strokeWorldBox(s, board.cards)
    if (!w) continue
    ;(intersects(w, view) ? inView : offScreen).push(s)
  }
  return { inView, offScreen }
}
