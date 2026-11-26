import { describe, expect, it } from 'vitest'
import * as Y from 'yjs'
import { frameAround, resizeMembership } from './binding'
import { mkBoard, mkCard, mkFrame } from './fixtures/testing'
import { readBoard } from './model'
import { addCard, addFrame, resizeFrame } from './ops'

describe('frame resize and fit (§2.4)', () => {
  const F = mkFrame('F', 0, 0, 400, 300)
  const SMALL = mkFrame('S', 500, 0, 200, 200)
  const board = mkBoard({
    frames: [F, SMALL],
    cards: [
      mkCard('member', 40, 80, 100, 100, { frame: 'F' }),
      mkCard('loose', 450, 50, 100, 100),
      mkCard('half', 380, 200, 100, 100),
      mkCard('nested', 520, 40, 100, 100, { frame: 'S' }),
      mkCard('gone', 60, 60, 50, 50, { state: 'archived' }),
    ],
  })

  it('a resize takes cards now fully inside, not ones half in, archived, or in a smaller frame', () => {
    const r = resizeMembership(board, 'F', [0, 0, 640, 280])
    expect(r.join).toEqual(['loose'])
    expect(r.leave).toEqual([])
  })

  it('a member the frame no longer touches leaves; one it still overlaps stays', () => {
    expect(resizeMembership(board, 'F', [200, 0, 200, 300]).leave).toEqual(['member'])
    expect(resizeMembership(board, 'F', [100, 0, 300, 300]).leave).toEqual([])
  })

  it('a smaller frame dropped over a card of a larger one takes it (innermost wins)', () => {
    const b = mkBoard({ frames: [mkFrame('BIG', 0, 0, 1000, 1000), mkFrame('T', 900, 900, 10, 10)], cards: [mkCard('c', 100, 100, 50, 50, { frame: 'BIG' })] })
    expect(resizeMembership(b, 'T', [90, 90, 100, 100]).join).toEqual(['c'])
  })

  it('fits around cards with the padding and title band wb_frame uses', () => {
    expect(frameAround([[100, 100, 10, 10]])).toEqual([68, 28, 74, 114])
    expect(frameAround([])).toBeNull()
  })

  it('resizeFrame writes the rect and the membership', () => {
    const doc = new Y.Doc()
    const a = addCard(doc, { kind: 'sticky', title: 'a', x: 0, y: 100, w: 100, h: 100 })
    const b = addCard(doc, { kind: 'sticky', title: 'b', x: 900, y: 100, w: 100, h: 100 })
    const f = addFrame(doc, { title: 'F', x: -50, y: 0, w: 300, h: 300, cards: [a] })
    const before = readBoard(doc)
    resizeFrame(doc, f, { x: 600, y: 0, w: 500, h: 300 }, resizeMembership(before, f, [600, 0, 500, 300]))
    const after = readBoard(doc)
    expect(after.frames[f]).toMatchObject({ x: 600, y: 0, w: 500, h: 300 })
    expect(after.cards[a]!.frame).toBeUndefined()
    expect(after.cards[b]!.frame).toBe(f)
  })
})
