// Where a node anchored to a card (the reply bubble) goes in screen space: the first side of right, left, below,
// above that stays inside the frame and covers no other card; else the first side inside the frame; else right.

export type Rect = { x: number; y: number; w: number; h: number }

const GAP = 12

const overlaps = (a: Rect, b: Rect) => a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h

/** `frame`: the canvas size; `right`: px kept free at the right edge (the control cluster while it is up). */
export function beside(card: Rect, size: { w: number; h: number }, others: readonly Rect[], frame: { w: number; h: number; right: number }): { x: number; y: number } {
  const sides: Rect[] = [
    { x: card.x + card.w + GAP, y: card.y, ...size },
    { x: card.x - GAP - size.w, y: card.y, ...size },
    { x: card.x, y: card.y + card.h + GAP, ...size },
    { x: card.x, y: card.y - GAP - size.h, ...size },
  ]
  const inside = (r: Rect) => r.x >= 0 && r.y >= 0 && r.x + r.w <= frame.w - frame.right && r.y + r.h <= frame.h
  const pick = sides.find((r) => inside(r) && !others.some((o) => overlaps(r, o))) ?? sides.find(inside) ?? sides[0]!
  return { x: pick.x, y: pick.y }
}
