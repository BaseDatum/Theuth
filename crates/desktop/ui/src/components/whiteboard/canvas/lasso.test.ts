import { describe, expect, it } from 'vitest'
import { pack } from '../ink/pack'
import { mkBoard, mkCard, mkFrame, mkMark, mkStroke } from './fixtures/testing'
import { inside, lassoSelect } from './lasso'

const square = (x: number, y: number, s: number) => [[x, y], [x + s, y], [x + s, y + s], [x, y + s]] as const
const line = (x0: number, y0: number, x1: number, y1: number) => {
  const pts = Array.from({ length: 11 }, (_, i) => [x0 + ((x1 - x0) * i) / 10, y0 + ((y1 - y0) * i) / 10, 0.5, i * 8] as [number, number, number, number])
  return { pts: pack([x0, y0], pts), n: 11, o: [x0, y0] as [number, number] }
}

describe('lasso', () => {
  it('even-odd containment, concave polygons included', () => {
    const u = [[0, 0], [30, 0], [30, 30], [20, 30], [20, 10], [10, 10], [10, 30], [0, 30]] as const
    expect(inside(u, 5, 20)).toBe(true)
    expect(inside(u, 15, 20)).toBe(false) // the notch
    expect(inside(u, 25, 25)).toBe(true)
  })

  it('takes cards and frames by centre, visible strokes by majority, their marks, and links between taken cards', () => {
    const b = mkBoard({
      cards: [mkCard('A', 10, 10, 40, 40), mkCard('B', 60, 10, 40, 40), mkCard('Far', 500, 500, 40, 40)],
      frames: [mkFrame('F', 0, 0, 120, 80)],
      ink: [
        mkStroke('in', [10, 60, 50, 1], line(10, 60, 60, 60)),
        mkStroke('half', [100, 60, 100, 1], line(100, 60, 200, 60)),
        mkStroke('hidden', [10, 60, 50, 1], line(10, 62, 60, 62)),
      ],
      marks: [mkMark('m', ['in']), mkMark('gone', ['hidden'], { state: 'answered' })],
    })
    b.links = { l: { id: 'l', from: 'A', to: 'B', style: 'solid', arrow: 'end', by: 'owner' }, l2: { id: 'l2', from: 'A', to: 'Far', style: 'solid', arrow: 'end', by: 'owner' } }
    const hit = lassoSelect(b, square(0, 0, 130), { showAnswered: false, scrub: {} })
    expect(hit.cards).toEqual(['A', 'B'])
    expect(hit.frames).toEqual(['F'])
    expect(hit.links).toEqual(['l'])
    expect(hit.strokes).toEqual(['in']) // 'half' has under half its points inside; 'hidden' is answered ink
    expect(hit.marks).toEqual(['m'])
  })
})
