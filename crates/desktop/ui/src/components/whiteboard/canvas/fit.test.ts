import { describe, expect, it } from 'vitest'
import { clearSpot, FIT_MAX, FIT_MIN, fitSize } from './fit'
import { mkBoard, mkCard } from './fixtures/testing'

describe('size to content', () => {
  it('a body that fits the max is shown whole at full size, under its title bar', () => {
    expect(fitSize({ w: 700, h: 500 }, 30)).toEqual({ w: 700, h: 530, scale: 1 })
  })

  it('never goes under the minimum', () => {
    expect(fitSize({ w: 80, h: 40 }, 30)).toEqual({ w: FIT_MIN.w, h: FIT_MIN.h, scale: 1 })
  })

  it('content past the max is scaled down whole, keeping its aspect (no crop)', () => {
    const tall = fitSize({ w: 800, h: 4000 }, 30)
    expect(tall.h).toBe(FIT_MAX.h)
    expect(tall.scale).toBeCloseTo((FIT_MAX.h - 30) / 4000, 2)
    expect(tall.w).toBe(Math.max(FIT_MIN.w, Math.round(800 * tall.scale)))
    const wide = fitSize({ w: 3200, h: 600 }, 0)
    expect(wide).toEqual({ w: FIT_MAX.w, h: 300, scale: 0.5 })
  })

  it('a grown card stays put when it covers nothing', () => {
    const b = mkBoard({ cards: [mkCard('a', 0, 0, 300, 200), mkCard('far', 2000, 0, 100, 100)] })
    expect(clearSpot(b, 'a', [0, 0, 900, 600])).toEqual({ x: 0, y: 0 })
  })

  it('a grown card that would cover a neighbour moves to the nearer clear spot, right or down', () => {
    const b = mkBoard({ cards: [mkCard('a', 0, 0, 300, 200), mkCard('right', 400, 0, 300, 100)] })
    // Down past `right` (to y 124) is nearer than right past it (to x 724).
    expect(clearSpot(b, 'a', [0, 0, 600, 300])).toEqual({ x: 0, y: 124 })
    const c = mkBoard({ cards: [mkCard('a', 0, 0, 300, 200), mkCard('below', 0, 260, 120, 900)] })
    expect(clearSpot(c, 'a', [0, 0, 300, 500])).toEqual({ x: 144, y: 0 })
  })

  it('archived cards are not in the way', () => {
    const b = mkBoard({ cards: [mkCard('a', 0, 0, 300, 200), mkCard('gone', 320, 0, 300, 200, { state: 'archived' })] })
    expect(clearSpot(b, 'a', [0, 0, 700, 300])).toEqual({ x: 0, y: 0 })
  })
})
