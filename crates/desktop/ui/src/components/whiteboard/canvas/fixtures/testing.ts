// Plain Board builders for the pure-module tests.
import type { Board, Card, Frame, Mark, Stroke } from '../model'

export const mkCard = (id: string, x: number, y: number, w: number, h: number, extra: Partial<Card> = {}): Card => ({
  id, kind: 'sticky', x, y, w, h, z: 1, title: id, versions: [{ source: { t: 'sticky', text: '' }, at: 0, by: 'owner' }], state: 'live', by: 'owner', ...extra,
})

export const mkStroke = (id: string, bb: Stroke['bb'], extra: Partial<Stroke> = {}): Stroke => ({
  id, pts: new Uint8Array(6), n: 1, o: [bb[0], bb[1]], bb, tool: 'pen', colour: 'ink', width: 2, p: 0, at: 0, by: 'owner', ...extra,
})

export const mkFrame = (id: string, x: number, y: number, w: number, h: number): Frame => ({ id, title: id, x, y, w, h, colour: 'neutral', by: 'owner' })

export const mkMark = (id: string, strokes: string[], extra: Partial<Mark> = {}): Mark => ({
  id, strokes, bb: [0, 0, 1, 1], state: 'sent', sent_at: 0, by: 'owner', ...extra,
})

export function mkBoard(o: { cards?: Card[]; ink?: Stroke[]; frames?: Frame[]; marks?: Mark[] }): Board {
  const marks = Object.fromEntries((o.marks ?? []).map((m) => [m.id, m]))
  const markOf: Record<string, string> = {}
  for (const m of o.marks ?? []) for (const s of m.strokes) markOf[s] = m.id
  return {
    meta: { v: 1, created: 0, bg: 'dots' },
    cards: Object.fromEntries((o.cards ?? []).map((c) => [c.id, c])),
    links: {},
    frames: Object.fromEntries((o.frames ?? []).map((f) => [f.id, f])),
    ink: Object.fromEntries((o.ink ?? []).map((s) => [s.id, s])),
    marks, aliases: {}, markOf,
  }
}
