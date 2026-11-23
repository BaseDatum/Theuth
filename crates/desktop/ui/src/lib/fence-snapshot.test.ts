import { expect, test } from 'vitest'
import { IDLE, MAX_FRAME, MAX_PNG_BYTES, QUIET_MS, SETTLE_MS, admit, fitPng, frameFor, pngSize, release, scales, settleAt } from './fence-snapshot'

test('two render at a time, the rest wait in arrival order and move up as slots free', () => {
  let r = admit(IDLE, 'a')
  expect(r.start).toEqual(['a'])
  r = admit(r.queue, 'b')
  expect(r.start).toEqual(['b'])
  r = admit(r.queue, 'c')
  r = admit(r.queue, 'd')
  expect(r.start).toEqual([])
  expect(r.queue).toEqual({ running: ['a', 'b'], waiting: ['c', 'd'] })
  // A repeated request changes nothing.
  expect(admit(r.queue, 'c')).toEqual({ queue: r.queue, start: [] })
  // A waiting request resolved elsewhere leaves without starting anyone.
  r = release(r.queue, 'd')
  expect(r).toEqual({ queue: { running: ['a', 'b'], waiting: ['c'] }, start: [] })
  r = release(r.queue, 'a')
  expect(r).toEqual({ queue: { running: ['b', 'c'], waiting: [] }, start: ['c'] })
  // Releasing an unknown id is harmless.
  expect(release(r.queue, 'zz').start).toEqual([])
})

test('draws 300 ms after the last change once ready, never past 8 s after mount; pending waits the full 8 s', () => {
  expect(settleAt(1000, 1000, false)).toBe(1000 + SETTLE_MS)
  expect(settleAt(1000, 2500, true)).toBe(2500 + QUIET_MS)
  // A layout that keeps changing runs into the settle budget.
  expect(settleAt(1000, 1000 + SETTLE_MS, true)).toBe(1000 + SETTLE_MS)
  // Changes while pending do not move the deadline.
  expect(settleAt(1000, 5000, false)).toBe(1000 + SETTLE_MS)
})

test('content that fits keeps the requested width; wider content widens to its scroll width, clipping only past 1600 px', () => {
  expect(frameFor(560, 480)).toEqual({ width: 560, clipped: false })
  expect(frameFor(560, 560)).toEqual({ width: 560, clipped: false })
  // A Kanban whose third column scrolls: the whole scroll width, rounded up to whole px.
  expect(frameFor(560, 812.4)).toEqual({ width: 813, clipped: false })
  expect(frameFor(560, MAX_FRAME)).toEqual({ width: MAX_FRAME, clipped: false })
  expect(frameFor(560, MAX_FRAME + 1)).toEqual({ width: MAX_FRAME, clipped: true })
  // A request already wider than the cap is never narrowed.
  expect(frameFor(2000, 2400)).toEqual({ width: 2000, clipped: true })
  expect(frameFor(2000, 1900)).toEqual({ width: 2000, clipped: false })
})

test('the scale ladder starts at the requested scale (clamped 1–3) and steps down through 1.5 and 1', () => {
  expect(scales(2)).toEqual([2, 1.5, 1])
  expect(scales(3)).toEqual([3, 1.5, 1])
  expect(scales(9)).toEqual([3, 1.5, 1])
  expect(scales(1.5)).toEqual([1.5, 1])
  expect(scales(1)).toEqual([1])
  expect(scales(0)).toEqual([1])
})

test('fitPng lowers the scale for sides over 8000 px and for PNGs over 4 MB, and fails when scale 1 still does not fit', async () => {
  const tried: number[] = []
  const draw = (bytes: (s: number) => number) => async (s: number) => {
    tried.push(s)
    return new Uint8Array(bytes(s))
  }
  // 4500 px tall: 9000 at 2×, 6750 at 1.5× — the side cap skips 2 without drawing.
  const tall = await fitPng(560, 4500, 2, draw(() => 10))
  expect('png' in tall).toBe(true)
  expect(tried).toEqual([1.5])
  tried.length = 0
  // Too many bytes at 2× and 1.5×, fine at 1×.
  const heavy = await fitPng(560, 400, 2, draw((s) => (s > 1 ? MAX_PNG_BYTES + 1 : MAX_PNG_BYTES)))
  expect('png' in heavy && heavy.png.byteLength).toBe(MAX_PNG_BYTES)
  expect(tried).toEqual([2, 1.5, 1])
  tried.length = 0
  const huge = await fitPng(560, 9000, 2, draw(() => 10))
  expect(huge).toEqual({ reason: expect.stringContaining('exceeds 8000 px') })
  expect(tried).toEqual([])
  const fat = await fitPng(560, 400, 1, draw(() => MAX_PNG_BYTES + 1))
  expect(fat).toEqual({ reason: expect.stringContaining('bytes exceeds') })
})

test('pngSize reads IHDR', () => {
  const png = new Uint8Array(24)
  png.set([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, 0x49, 0x48, 0x44, 0x52])
  new DataView(png.buffer).setUint32(16, 1120)
  new DataView(png.buffer).setUint32(20, 70000)
  expect(pngSize(png)).toEqual({ width: 1120, height: 70000 })
})
