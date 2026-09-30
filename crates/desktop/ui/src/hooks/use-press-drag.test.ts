import { expect, test } from 'vitest'
import { edgeScroll } from './use-press-drag'

test('edge auto-scroll: still in the middle, ramps to full speed at either edge, never past it', () => {
  expect(edgeScroll(200, 0, 390)).toBe(0)
  expect(edgeScroll(40, 0, 390)).toBe(0)
  expect(edgeScroll(350, 0, 390)).toBe(0)
  expect(edgeScroll(20, 0, 390)).toBe(-4)
  expect(edgeScroll(370, 0, 390)).toBe(4)
  expect(edgeScroll(0, 0, 390)).toBe(-8)
  expect(edgeScroll(390, 0, 390)).toBe(8)
  // A finger past the scroller's edge (over the sidebar, off the screen) keeps full speed.
  expect(edgeScroll(-30, 0, 390)).toBe(-8)
  expect(edgeScroll(1100, 0, 1024)).toBe(8)
  // The scroller's own left edge, not the viewport's: the desktop board sits right of the sidebar.
  expect(edgeScroll(270, 256, 1024)).toBe(-8 * (26 / 40))
})
