// Captured points → committed `Stroke` values (§2.5, §2.6, §8.6): RDP at capture zoom, anchor, card-local
// coordinates, the 4 000-point split, packing.
import type { CapturedStroke } from '../canvas/api'
import { ulid } from '../canvas/ids'
import type { By, Card, Stroke } from '../canvas/model'
import { anchorOf } from './anchor'
import { bboxOf, pack, rdp, RDP_SCREEN_PX, split, type Pt } from './pack'

/** Strokes ready for `ops.addStrokes` (one per part); the anchor card, when any. */
export function buildStrokes(c: CapturedStroke, zoom: number, cards: readonly Card[], by: By = 'owner'): { strokes: Stroke[]; card: Card | null } {
  if (c.pts.length === 0) return { strokes: [], card: null }
  const simple = rdp(c.pts as Pt[], RDP_SCREEN_PX / Math.max(zoom, 1e-3))
  const card = anchorOf(simple, cards)
  const local: Pt[] = card ? simple.map(([x, y, p, t]) => [x - card.x, y - card.y, p, t]) : simple
  const parts = split(local)
  const strokes = parts.map((pts, i): Stroke => {
    const first = pts[0]!
    const o: [number, number] = [first[0], first[1]]
    const bytes = pack(o, c.hasPressure ? pts : pts.map(([x, y, , t]) => [x, y, 0.5, t] as Pt))
    const s: Stroke = {
      id: ulid(c.at + i),
      pts: bytes,
      n: bytes.length / 6,
      o,
      bb: bboxOf(pts, c.width / 2),
      tool: c.tool,
      colour: c.colour,
      width: c.width,
      p: c.hasPressure ? 1 : 0,
      at: c.at + i,
      by,
    }
    if (card) {
      s.card = card.id
      s.ver = card.versions.length
      s.cs = [card.w, card.h]
    }
    return s
  })
  return { strokes, card }
}

/** The Y value of a stroke (the `id` is the map key). */
export function strokeValue(s: Stroke): Record<string, unknown> {
  const v: Record<string, unknown> = {
    pts: s.pts, n: s.n, o: [...s.o], bb: [...s.bb], tool: s.tool, colour: s.colour, width: s.width, p: s.p, at: s.at, by: s.by,
  }
  if (s.card !== undefined) {
    v.card = s.card
    v.ver = s.ver ?? 0
    if (s.cs) v.cs = [...s.cs]
  }
  return v
}
