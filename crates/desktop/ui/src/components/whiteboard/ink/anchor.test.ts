import { describe, expect, it } from 'vitest'
import type { Board, Card, Stroke } from '../canvas/model'
import { anchorOf, ridersOf, shareInside } from './anchor'
import type { Pt } from './pack'
import { buildStrokes } from './stroke'

const card = (id: string, x: number, y: number, w: number, h: number, z = 1, extra: Partial<Card> = {}): Card => ({
  id, kind: 'sticky', x, y, w, h, z, title: id, versions: [{ source: { t: 'sticky', text: '' }, at: 0, by: 'owner' }], state: 'live', by: 'owner', ...extra,
})
const line = (x0: number, y0: number, x1: number, y1: number, n = 20): Pt[] =>
  Array.from({ length: n + 1 }, (_, i) => [x0 + ((x1 - x0) * i) / n, y0 + ((y1 - y0) * i) / n, 0.5, i * 8])
const stroke = (id: string, bb: Stroke['bb'], extra: Partial<Stroke> = {}): Stroke => ({
  id, pts: new Uint8Array(6), n: 1, o: [bb[0], bb[1]], bb, tool: 'pen', colour: 'ink', width: 2, p: 0, at: 0, by: 'owner', ...extra,
})
const board = (cards: Card[], ink: Stroke[]): Board => ({
  meta: { v: 1, created: 0, bg: 'dots' }, cards: Object.fromEntries(cards.map((c) => [c.id, c])), links: {}, frames: {},
  ink: Object.fromEntries(ink.map((s) => [s.id, s])), marks: {}, aliases: {}, markOf: {},
})

describe('anchoring (§8.6)', () => {
  const a = card('A', 0, 0, 100, 100)

  it('60 % rule: anchored at ≥ 60 % of the length inside, free below', () => {
    expect(shareInside(line(0, 50, 200, 50), [0, 0, 100, 100])).toBeCloseTo(0.5, 5)
    expect(anchorOf(line(40, 50, 140, 50), [a])?.id).toBe('A') // 60 % inside
    expect(anchorOf(line(50, 50, 150, 50), [a])).toBeNull() // 50 %
    expect(anchorOf(line(-200, 50, 50, 50), [a])).toBeNull() // 20 %
  })

  it('top-most by z on a tie; archived cards never anchor', () => {
    const under = card('U', 0, 0, 200, 200, 1)
    const over = card('O', 0, 0, 200, 200, 5)
    expect(anchorOf(line(10, 10, 90, 90), [under, over])?.id).toBe('O')
    expect(anchorOf(line(10, 10, 90, 90), [card('X', 0, 0, 200, 200, 9, { state: 'archived' }), under])?.id).toBe('U')
  })

  it('an anchored stroke is stored card-local with the version and the card size at capture', () => {
    const c = card('A', 300, 200, 100, 100, 1)
    const { strokes, card: hit } = buildStrokes({ pts: line(310, 210, 380, 290), tool: 'pen', colour: 'red', width: 2, hasPressure: false, at: 1000 }, 1, [c])
    expect(hit?.id).toBe('A')
    const s = strokes[0]!
    expect(s.card).toBe('A')
    expect(s.ver).toBe(1)
    expect(s.cs).toEqual([100, 100])
    expect(s.o).toEqual([10, 10])
    expect(s.p).toBe(0)
  })

  it('ride-with-one-card: a free stroke touching exactly one card rides with it', () => {
    const b = board([a, card('B', 500, 0, 100, 100)], [stroke('s1', [90, 40, 30, 10])])
    expect(ridersOf(b, 'A').map((s) => s.id)).toEqual(['s1'])
    expect(ridersOf(b, 'B')).toEqual([])
  })

  it('stay-between-two: a free stroke touching two cards stays', () => {
    const b = board([a, card('B', 150, 0, 100, 100)], [stroke('s1', [90, 40, 70, 10])])
    expect(ridersOf(b, 'A')).toEqual([])
    expect(ridersOf(b, 'B')).toEqual([])
  })

  it('anchored strokes never ride (they move with their card already)', () => {
    const b = board([a], [stroke('s1', [10, 10, 20, 20], { card: 'A', ver: 1, cs: [100, 100] })])
    expect(ridersOf(b, 'A')).toEqual([])
  })
})
