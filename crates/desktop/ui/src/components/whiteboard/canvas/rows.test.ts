import { describe, expect, it } from 'vitest'
import { mkBoard, mkCard, mkFrame } from './fixtures/testing'
import { rowsOf } from './rows'

describe('rows (§3.2)', () => {
  it("the §3.2 example's rows: f1 row 1 = c1 c2 c3; loose row 1 = c4 c5", () => {
    const f1 = mkFrame('f1', 36, 48, 610, 424)
    const b = mkBoard({
      frames: [f1],
      cards: [
        mkCard('c3', 450, 88, 180, 390, { frame: 'f1', kind: 'page' }),
        mkCard('c1', 50, 80, 180, 390, { frame: 'f1', kind: 'image' }),
        mkCard('c2', 250, 100, 180, 390, { frame: 'f1', kind: 'page', state: 'dimmed' }),
        mkCard('c5', 300, 480, 360, 260, { kind: 'diagram' }),
        mkCard('c4', 50, 494, 220, 150),
        mkCard('gone', 900, 900, 10, 10, { state: 'archived' }),
      ],
    })
    const r = rowsOf(b)
    expect(r.frames.map((f) => [f.frame.id, f.rows])).toEqual([['f1', [['c1', 'c2', 'c3']]]])
    expect(r.loose).toEqual([['c4', 'c5']])
  })

  it('a card whose centre falls below the row head starts a new row; frames in reading order', () => {
    const b = mkBoard({
      frames: [mkFrame('low', 0, 1000, 500, 500), mkFrame('right', 600, 0, 500, 500), mkFrame('left', 0, 0, 500, 500)],
      cards: [
        mkCard('a', 0, 0, 100, 100, { frame: 'left' }),
        mkCard('b', 200, 40, 100, 100, { frame: 'left' }), // centre 90 ≤ 100: same row
        mkCard('c', 100, 60, 100, 100, { frame: 'left' }), // centre 110 > 100: next row
        mkCard('orphan', 0, 0, 10, 10, { frame: 'missing' }),
      ],
    })
    const r = rowsOf(b)
    expect(r.frames.map((f) => f.frame.id)).toEqual(['left', 'right', 'low'])
    expect(r.frames[0]!.rows).toEqual([['a', 'b'], ['c']])
    expect(r.loose).toEqual([['orphan']])
  })
})
