// §8.6 anchoring: a stroke belongs to the card holding ≥ 60 % of its length (top-most by z on a tie); a free stroke
// whose box touches exactly one card rides with that card when the owner moves it.
import { cardBox, intersects } from '../canvas/geom'
import type { Board, Box, Card, Id, Stroke } from '../canvas/model'
import type { Pt } from './pack'

export const ANCHOR_SHARE = 0.6

/** Length of the segment a→b inside box `r` (Liang–Barsky clip). */
function clippedLength(ax: number, ay: number, bx: number, by: number, r: Box): number {
  const dx = bx - ax
  const dy = by - ay
  let t0 = 0
  let t1 = 1
  const edges: [number, number][] = [
    [-dx, ax - r[0]],
    [dx, r[0] + r[2] - ax],
    [-dy, ay - r[1]],
    [dy, r[1] + r[3] - ay],
  ]
  for (const [p, q] of edges) {
    if (p === 0) {
      if (q < 0) return 0
      continue
    }
    const t = q / p
    if (p < 0) {
      if (t > t1) return 0
      if (t > t0) t0 = t
    } else {
      if (t < t0) return 0
      if (t < t1) t1 = t
    }
  }
  return Math.hypot(dx, dy) * Math.max(0, t1 - t0)
}

/** Share of the polyline's length inside `r`; a dot (zero length) counts as 1 or 0 by containment. */
export function shareInside(pts: readonly Pt[], r: Box): number {
  let total = 0
  let inside = 0
  for (let i = 1; i < pts.length; i++) {
    const [ax, ay] = pts[i - 1]!
    const [bx, by] = pts[i]!
    total += Math.hypot(bx - ax, by - ay)
    inside += clippedLength(ax, ay, bx, by, r)
  }
  if (total === 0) {
    const p = pts[0]
    return p && p[0] >= r[0] && p[0] <= r[0] + r[2] && p[1] >= r[1] && p[1] <= r[1] + r[3] ? 1 : 0
  }
  return inside / total
}

/** The card a stroke (world points) anchors to, or null for a free stroke. Archived cards never anchor. */
export function anchorOf(pts: readonly Pt[], cards: readonly Card[]): Card | null {
  let best: Card | null = null
  for (const c of cards) {
    if (c.state === 'archived') continue
    if (shareInside(pts, cardBox(c)) < ANCHOR_SHARE) continue
    if (!best || c.z > best.z) best = c
  }
  return best
}

/** Cards (not archived) whose box a world box touches. */
export const touching = (bb: Box, cards: readonly Card[]) => cards.filter((c) => c.state !== 'archived' && intersects(bb, cardBox(c)))

/** Free strokes that ride with `card` when it moves: their box touches it and no other card. */
export function ridersOf(board: Board, card: Id): Stroke[] {
  const cards = Object.values(board.cards)
  const out: Stroke[] = []
  for (const s of Object.values(board.ink)) {
    if (s.card) continue
    const t = touching(s.bb, cards)
    if (t.length === 1 && t[0]!.id === card) out.push(s)
  }
  return out
}

/** A free stroke moved by (dx, dy): `o` and `bb` shift, the packed deltas stay. */
export const shifted = (s: Stroke, dx: number, dy: number): Stroke => ({
  ...s,
  o: [s.o[0] + dx, s.o[1] + dy],
  bb: [s.bb[0] + dx, s.bb[1] + dy, s.bb[2], s.bb[3]],
})
