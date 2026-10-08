// `createResizeDrag` (138-DIVIDERS): one primary pointer per drag; up, cancel and lost capture all end it, and
// nothing after the end (the leaked-`window`-listener bug on iPad) ever moves a divider again.
import { expect, test, vi } from 'vitest'
import { clamp, clampRatio, createResizeDrag, type DragPointer, keyDelta, RATIO_STEP } from './resize-drag'

const ptr = (over: Partial<DragPointer> = {}): DragPointer => ({ pointerId: 1, isPrimary: true, button: 0, pointerType: 'touch', clientX: 100, clientY: 50, ...over })

function harness() {
  const onStart = vi.fn()
  const onMove = vi.fn()
  const onEnd = vi.fn()
  return { drag: createResizeDrag({ onStart, onMove, onEnd }), onStart, onMove, onEnd }
}

test('starts on a primary pointer only, and for a mouse only on its main button', () => {
  const { drag, onStart } = harness()
  expect(drag.down(ptr({ isPrimary: false }))).toBe(false)
  expect(drag.down(ptr({ pointerType: 'mouse', button: 2 }))).toBe(false)
  expect(drag.down(ptr({ pointerType: 'pen', button: 5 }))).toBe(true) // barrel button: still the primary contact
  expect(onStart).toHaveBeenCalledTimes(1)
  expect(drag.live).toBe(true)
})

test('a second pointer is ignored while a drag is live; only the live pointer moves it', () => {
  const { drag, onMove } = harness()
  drag.down(ptr({ pointerId: 1 }))
  expect(drag.down(ptr({ pointerId: 2 }))).toBe(false)
  drag.move(ptr({ pointerId: 2, clientX: 900 }))
  expect(onMove).not.toHaveBeenCalled()
  drag.move(ptr({ pointerId: 1, clientX: 300, clientY: 60 }))
  expect(onMove).toHaveBeenCalledWith(300, 60)
  drag.end(ptr({ pointerId: 2 })) // another pointer lifting does not end it
  expect(drag.live).toBe(true)
})

test.each(['up', 'cancel', 'lostpointercapture', 'unmount'])('%s ends the drag once, and a later move emits nothing', (how) => {
  const { drag, onMove, onEnd } = harness()
  expect(drag.move(ptr())).toBeUndefined()
  expect(onMove).not.toHaveBeenCalled() // no drag yet
  drag.down(ptr())
  drag.move(ptr({ clientX: 120 }))
  how === 'unmount' ? drag.end() : drag.end(ptr())
  expect(drag.live).toBe(false)
  expect(onEnd).toHaveBeenCalledTimes(1)
  drag.end(ptr())
  expect(onEnd).toHaveBeenCalledTimes(1)
  drag.move(ptr({ clientX: 400 })) // the leaked-listener bug: the next finger must not resize
  expect(onMove).toHaveBeenCalledTimes(1)
  // A fresh press starts a new drag.
  expect(drag.down(ptr({ pointerId: 7 }))).toBe(true)
})

test('ratio and pixel clamps', () => {
  expect(clampRatio(0.05)).toBe(0.15)
  expect(clampRatio(0.95)).toBe(0.85)
  expect(clampRatio(0.5)).toBe(0.5)
  expect(clampRatio(0.85 + RATIO_STEP)).toBe(0.85)
  expect(clampRatio(0.15 - RATIO_STEP)).toBe(0.15)
  expect(clamp(100, 360, 640)).toBe(360)
  expect(clamp(1000, 360, 640)).toBe(640)
  expect(clamp(500, 360, 640)).toBe(500)
})

test('arrow keys follow the line: left/right for a vertical line, up/down for a horizontal one', () => {
  expect(keyDelta('ArrowLeft', 'vertical')).toBe(-1)
  expect(keyDelta('ArrowRight', 'vertical')).toBe(1)
  expect(keyDelta('ArrowUp', 'vertical')).toBe(0)
  expect(keyDelta('ArrowUp', 'horizontal')).toBe(-1)
  expect(keyDelta('ArrowDown', 'horizontal')).toBe(1)
  expect(keyDelta('ArrowRight', 'horizontal')).toBe(0)
  expect(keyDelta('Enter', 'vertical')).toBe(0)
})
