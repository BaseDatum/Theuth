import { describe, expect, it } from 'vitest'
import { canScroll, scrollTarget, wheelRoute, type Scroller } from './scroll'

/** A 300 px tall region of 900 px content, scrolled to `top`; no horizontal overflow. */
const table = (top: number): Scroller => ({ top, left: 0, sh: 900, ch: 300, sw: 400, cw: 400, x: false, y: true })
const wide: Scroller = { top: 0, left: 0, sh: 200, ch: 200, sw: 1200, cw: 400, x: true, y: false }

describe('scrolling inside a card', () => {
  it('a card that is not entered never scrolls: the wheel pans or zooms the board', () => {
    expect(scrollTarget(false, [table(100)], 0, 40)).toBe(-1)
  })

  it('an entered card scrolls the region under the pointer that can move that way', () => {
    expect(scrollTarget(true, [table(0)], 0, 40)).toBe(0)
    expect(scrollTarget(true, [table(300)], 0, -40)).toBe(0)
  })

  it('at the scroll edge the gesture falls through (to an outer region, else the board)', () => {
    expect(scrollTarget(true, [table(600)], 0, 40)).toBe(-1)
    expect(scrollTarget(true, [table(0)], 0, -40)).toBe(-1)
    expect(scrollTarget(true, [table(600), table(0)], 0, 40)).toBe(1)
  })

  it('only along an axis the region scrolls on', () => {
    expect(canScroll(wide, 0, 40)).toBe(false)
    expect(canScroll(wide, 40, 0)).toBe(true)
    expect(scrollTarget(true, [wide], 0, 40)).toBe(-1)
    expect(scrollTarget(true, [table(0)], 40, 0)).toBe(-1)
  })

  it('an inner pan/zoom diagram zooms only inside an entered card, and then the board does not', () => {
    const over = (entered: boolean, pinch = false) => wheelRoute(entered, { zoomSurface: true, pinch, chain: [], dx: 0, dy: 40 })
    expect(over(false)).toBe('board')
    expect(over(false, true)).toBe('board')
    expect(over(true)).toBe('inner')
    expect(over(true, true)).toBe('inner')
  })

  it('elsewhere in an entered card: a region that can scroll takes the wheel, a pinch zooms the board', () => {
    expect(wheelRoute(true, { zoomSurface: false, pinch: false, chain: [table(0)], dx: 0, dy: 40 })).toBe('inner')
    expect(wheelRoute(true, { zoomSurface: false, pinch: false, chain: [table(600)], dx: 0, dy: 40 })).toBe('board')
    expect(wheelRoute(true, { zoomSurface: false, pinch: true, chain: [table(0)], dx: 0, dy: 40 })).toBe('board')
    expect(wheelRoute(false, { zoomSurface: false, pinch: false, chain: [table(0)], dx: 0, dy: 40 })).toBe('board')
  })
})
