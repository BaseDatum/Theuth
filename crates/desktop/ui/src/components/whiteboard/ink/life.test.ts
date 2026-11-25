import { describe, expect, it } from 'vitest'
import { mkBoard, mkCard, mkMark, mkStroke } from '../canvas/fixtures/testing'
import { lifeOf } from '../canvas/model'
import { lookOf } from './life'

const V2 = [
  { source: { t: 'sticky' as const, text: '' }, at: 0, by: 'owner' as const },
  { source: { t: 'sticky' as const, text: '' }, at: 1, by: 'owner' as const },
]

describe('ink life (§2.7)', () => {
  const b = mkBoard({
    ink: [mkStroke('fresh', [0, 0, 1, 1]), mkStroke('sent', [0, 0, 1, 1]), mkStroke('ans', [0, 0, 1, 1]), mkStroke('pin', [0, 0, 1, 1])],
    marks: [mkMark('m1', ['sent']), mkMark('m2', ['ans'], { state: 'answered' }), mkMark('m3', ['pin'], { state: 'pinned' })],
  })
  const view = { showAnswered: false, scrub: {} }

  it('derives life from marks', () => {
    expect(['fresh', 'sent', 'ans', 'pin'].map((id) => lifeOf(b, id))).toEqual(['fresh', 'sent', 'answered', 'pinned'])
  })

  it('fresh 100 %, sent 30 %, answered hidden, pinned 100 % with the annotation style', () => {
    expect(lookOf(b, b.ink.fresh!, view)).toEqual({ opacity: 1, pinned: false })
    expect(lookOf(b, b.ink.sent!, view)).toEqual({ opacity: 0.3, pinned: false })
    expect(lookOf(b, b.ink.ans!, view)).toEqual({ opacity: 0, pinned: false })
    expect(lookOf(b, b.ink.pin!, view)).toEqual({ opacity: 1, pinned: true })
  })

  it('Show answered marks brings answered ink back at 30 %', () => {
    expect(lookOf(b, b.ink.ans!, { showAnswered: true, scrub: {} }).opacity).toBe(0.3)
  })

  it('answered ink disappears with its version and returns at 30 % when scrubbed to it', () => {
    const C = mkCard('C', 0, 0, 100, 100, { versions: V2 })
    const bb = mkBoard({
      cards: [C],
      ink: [mkStroke('old', [1, 1, 1, 1], { card: 'C', ver: 1 }), mkStroke('pinOld', [1, 1, 1, 1], { card: 'C', ver: 1 })],
      marks: [mkMark('m', ['old'], { state: 'answered', card: 'C', ver: 1 }), mkMark('p', ['pinOld'], { state: 'pinned', card: 'C', ver: 1 })],
    })
    expect(lookOf(bb, bb.ink.old!, view).opacity).toBe(0)
    expect(lookOf(bb, bb.ink.old!, { showAnswered: false, scrub: { C: 1 } }).opacity).toBe(0.3)
    expect(lookOf(bb, bb.ink.pinOld!, view)).toEqual({ opacity: 1, pinned: true })
  })

  it('ink on an archived card is not drawn', () => {
    const bb = mkBoard({ cards: [mkCard('C', 0, 0, 10, 10, { state: 'archived' })], ink: [mkStroke('s', [0, 0, 1, 1], { card: 'C', ver: 1 })] })
    expect(lookOf(bb, bb.ink.s!, view).opacity).toBe(0)
  })

  it('ink drawn on a placeholder (ver 0) stays when it fills as v1', () => {
    const bb = mkBoard({ cards: [mkCard('C', 0, 0, 10, 10)], ink: [mkStroke('s', [0, 0, 1, 1], { card: 'C', ver: 0 })] })
    expect(lookOf(bb, bb.ink.s!, view).opacity).toBe(1)
  })
})
