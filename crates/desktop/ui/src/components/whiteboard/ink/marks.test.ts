import { describe, expect, it } from 'vitest'
import { mkBoard, mkCard, mkMark, mkStroke } from '../canvas/fixtures/testing'
import { cluster, freshStrokes } from './marks'

const VIEW: [number, number, number, number] = [0, 0, 1000, 1000]

describe('marks: single-link clustering (§8.6)', () => {
  it('48 px box gap links, 49 px does not', () => {
    const b = mkBoard({ ink: [mkStroke('a', [0, 0, 10, 10]), mkStroke('b', [58, 0, 10, 10]), mkStroke('c', [117, 0, 10, 10])] })
    const cs = cluster(b, Object.values(b.ink))
    expect(cs.map((c) => c.strokes)).toEqual([['a', 'b'], ['c']])
  })

  it('single link chains: a—b—c in one mark though a and c are far apart', () => {
    const b = mkBoard({ ink: [mkStroke('a', [0, 0, 10, 10]), mkStroke('b', [50, 0, 10, 10]), mkStroke('c', [100, 0, 10, 10])] })
    expect(cluster(b, Object.values(b.ink)).map((c) => c.strokes)).toEqual([['a', 'b', 'c']])
  })

  it('4 s time gap links, more does not', () => {
    const b = mkBoard({ ink: [mkStroke('a', [0, 0, 10, 10], { at: 0 }), mkStroke('b', [5, 5, 10, 10], { at: 4000 }), mkStroke('c', [5, 5, 10, 10], { at: 8001 })] })
    expect(cluster(b, Object.values(b.ink)).map((c) => c.strokes)).toEqual([['a', 'b'], ['c']])
  })

  it('anchors never mix: same place, different card or version → different marks', () => {
    const A = mkCard('A', 0, 0, 200, 200)
    const B = mkCard('B', 0, 0, 200, 200, { versions: [{ source: { t: 'sticky', text: '' }, at: 0, by: 'owner' }, { source: { t: 'sticky', text: '' }, at: 1, by: 'owner' }] })
    const b = mkBoard({
      cards: [A, B],
      ink: [
        mkStroke('a1', [10, 10, 10, 10], { card: 'A', ver: 1, cs: [200, 200] }),
        mkStroke('a2', [15, 10, 10, 10], { card: 'A', ver: 1, cs: [200, 200] }),
        mkStroke('b1', [10, 10, 10, 10], { card: 'B', ver: 2, cs: [200, 200] }),
        mkStroke('b0', [10, 10, 10, 10], { card: 'B', ver: 1, cs: [200, 200] }),
        mkStroke('f', [10, 10, 10, 10]),
      ],
    })
    const cs = cluster(b, Object.values(b.ink))
    expect(cs.map((c) => [c.card ?? null, c.ver ?? null, c.strokes])).toEqual(
      expect.arrayContaining([['A', 1, ['a1', 'a2']], ['B', 2, ['b1']], ['B', 1, ['b0']], [null, null, ['f']]]),
    )
    expect(cs).toHaveLength(4)
  })

  it('off-screen excluded: only fresh strokes in view are taken; sent, answered and pinned ink never', () => {
    const b = mkBoard({
      ink: [mkStroke('in', [10, 10, 10, 10]), mkStroke('out', [5000, 10, 10, 10]), mkStroke('sent', [20, 20, 10, 10])],
      marks: [mkMark('m', ['sent'])],
    })
    const f = freshStrokes(b, VIEW)
    expect(f.inView.map((s) => s.id)).toEqual(['in'])
    expect(f.offScreen.map((s) => s.id)).toEqual(['out'])
  })

  it('ink on a version the card no longer shows is not offered', () => {
    const C = mkCard('C', 0, 0, 200, 200, { versions: [{ source: { t: 'sticky', text: '' }, at: 0, by: 'owner' }, { source: { t: 'sticky', text: '' }, at: 1, by: 'owner' }] })
    const b = mkBoard({ cards: [C], ink: [mkStroke('old', [10, 10, 10, 10], { card: 'C', ver: 1, cs: [200, 200] })] })
    expect(freshStrokes(b, VIEW).inView).toEqual([])
  })

  it('a mark box is the union of its strokes, in their space; world box follows the card', () => {
    const C = mkCard('C', 100, 100, 200, 200)
    const b = mkBoard({ cards: [C], ink: [mkStroke('a', [10, 10, 10, 10], { card: 'C', ver: 1, cs: [100, 100] }), mkStroke('b', [30, 10, 10, 20], { card: 'C', ver: 1, cs: [100, 100] })] })
    const [c] = cluster(b, Object.values(b.ink))
    expect(c!.bb).toEqual([10, 10, 30, 20])
    expect(c!.world).toEqual([120, 120, 60, 40]) // scaled ×2 (card grew from 100 to 200)
  })
})
