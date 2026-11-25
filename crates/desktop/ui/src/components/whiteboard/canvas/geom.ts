// Box helpers shared by binding, anchoring, marks and placement. World units throughout.
import type { Board, Box, Card, Stroke } from './model'

export const right = (b: Box) => b[0] + b[2]
export const bottom = (b: Box) => b[1] + b[3]
export const cardBox = (c: Pick<Card, 'x' | 'y' | 'w' | 'h'>): Box => [c.x, c.y, c.w, c.h]

export const intersects = (a: Box, b: Box) => a[0] <= right(b) && b[0] <= right(a) && a[1] <= bottom(b) && b[1] <= bottom(a)
export const contains = (outer: Box, inner: Box) =>
  inner[0] >= outer[0] && inner[1] >= outer[1] && right(inner) <= right(outer) && bottom(inner) <= bottom(outer)
export const containsPoint = (b: Box, x: number, y: number) => x >= b[0] && x <= right(b) && y >= b[1] && y <= bottom(b)

export function union(a: Box | null, b: Box): Box {
  if (!a) return [...b] as Box
  const x = Math.min(a[0], b[0])
  const y = Math.min(a[1], b[1])
  return [x, y, Math.max(right(a), right(b)) - x, Math.max(bottom(a), bottom(b)) - y]
}

/** Gap between two boxes (0 when they touch or overlap). */
export function gap(a: Box, b: Box): number {
  const dx = Math.max(0, b[0] - right(a), a[0] - right(b))
  const dy = Math.max(0, b[1] - bottom(a), a[1] - bottom(b))
  return Math.hypot(dx, dy)
}

export const pad = (b: Box, p: number): Box => [b[0] - p, b[1] - p, b[2] + 2 * p, b[3] + 2 * p]

/** At least `w × h`, grown around its centre. */
export function atLeast(b: Box, w: number, h: number): Box {
  const nw = Math.max(b[2], w)
  const nh = Math.max(b[3], h)
  return [b[0] - (nw - b[2]) / 2, b[1] - (nh - b[3]) / 2, nw, nh]
}

/** Card-local (at capture size) → world, for an anchored stroke's box; a free stroke's box is world already. */
export function strokeWorldBox(stroke: Stroke, cards: Board['cards']): Box | null {
  if (!stroke.card) return stroke.bb
  const card = cards[stroke.card]
  if (!card || card.state === 'archived') return null
  const [sx, sy] = scaleOf(stroke, card)
  return [card.x + stroke.bb[0] * sx, card.y + stroke.bb[1] * sy, stroke.bb[2] * sx, stroke.bb[3] * sy]
}

/** Render scale of an anchored stroke (§2.5): card size now ÷ card size at capture. */
export function scaleOf(stroke: Stroke, card: Pick<Card, 'w' | 'h'>): [number, number] {
  const cs = stroke.cs
  return cs && cs[0] > 0 && cs[1] > 0 ? [card.w / cs[0], card.h / cs[1]] : [1, 1]
}

/** Cards that are drawn on the canvas (not archived). */
export const shownCards = (board: Board) => Object.values(board.cards).filter((c) => c.state !== 'archived')
