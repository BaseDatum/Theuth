// Select mode's finger lasso (design §4.2): a free polygon over cards and ink. Pure.
import { lookOf } from '../ink/life'
import { unpack } from '../ink/pack'
import { scaleOf, shownCards } from './geom'
import type { Board, Id } from './model'

export type Poly = readonly (readonly [number, number])[]

/** Even-odd point in polygon. */
export function inside(poly: Poly, x: number, y: number): boolean {
  let hit = false
  for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
    const [xi, yi] = poly[i]!
    const [xj, yj] = poly[j]!
    if (yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) hit = !hit
  }
  return hit
}

export type LassoHit = { cards: Id[]; frames: Id[]; links: Id[]; marks: Id[]; strokes: Id[] }

/**
 * What a lasso (world polygon) takes: cards and frames whose centre it encloses, visible strokes with at least half
 * their points inside, the marks of those strokes, and links whose both cards are taken.
 */
export function lassoSelect(board: Board, poly: Poly, view: { showAnswered: boolean; scrub: Record<Id, number> }): LassoHit {
  if (poly.length < 3) return { cards: [], frames: [], links: [], marks: [], strokes: [] }
  const cards = shownCards(board).filter((c) => inside(poly, c.x + c.w / 2, c.y + c.h / 2)).map((c) => c.id)
  const frames = Object.values(board.frames).filter((f) => inside(poly, f.x + f.w / 2, f.y + f.h / 2)).map((f) => f.id)
  const strokes: Id[] = []
  for (const s of Object.values(board.ink)) {
    if (lookOf(board, s, view).opacity === 0) continue
    let ox = 0
    let oy = 0
    let sx = 1
    let sy = 1
    if (s.card) {
      const c = board.cards[s.card]
      if (!c) continue
      ;[sx, sy] = scaleOf(s, c)
      ox = c.x
      oy = c.y
    }
    const pts = unpack(s.o, s.pts)
    const step = Math.max(1, Math.floor(pts.length / 24))
    let n = 0
    let k = 0
    for (let i = 0; i < pts.length; i += step) {
      n++
      if (inside(poly, ox + pts[i]![0] * sx, oy + pts[i]![1] * sy)) k++
    }
    if (n > 0 && k * 2 >= n) strokes.push(s.id)
  }
  const taken = new Set(cards)
  const links = Object.values(board.links).filter((l) => taken.has(l.from) && taken.has(l.to)).map((l) => l.id)
  const marks = [...new Set(strokes.map((s) => board.markOf[s]).filter((m): m is Id => m !== undefined))]
  return { cards, frames, links, marks, strokes }
}
