import '../settings/test-dom'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { FOLLOW_FRAMES, FOLLOW_IDLE_MS, followRect } from './web-native-frame'

// rAF as a queue the test drains one frame at a time; timers are vitest's fake ones.
let frames: FrameRequestCallback[] = []
const frame = () => {
  const run = frames
  frames = []
  for (const f of run) f(0)
}
let box = { left: 0, top: 0, width: 100, height: 50 }
const el = document.createElement('div')
el.getBoundingClientRect = () => ({ ...box, right: box.left + box.width, bottom: box.top + box.height, x: box.left, y: box.top, toJSON: () => null }) as DOMRect

beforeEach(() => {
  vi.useFakeTimers()
  frames = []
  box = { left: 0, top: 0, width: 100, height: 50 }
  vi.stubGlobal('requestAnimationFrame', (f: FrameRequestCallback) => frames.push(f))
  vi.stubGlobal('cancelAnimationFrame', () => void (frames = []))
})
afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

test('a still placeholder stops asking for frames and polls instead', () => {
  const moved = vi.fn()
  const stop = followRect(el, moved)
  for (let i = 0; i < FOLLOW_FRAMES; i++) frame()
  expect(frames).toHaveLength(0)
  expect(vi.getTimerCount()).toBe(1)
  vi.advanceTimersByTime(FOLLOW_IDLE_MS * 4)
  expect(frames).toHaveLength(0)
  expect(moved).not.toHaveBeenCalled()
  stop()
  expect(vi.getTimerCount()).toBe(0)
})

test('a move while idle is caught by the poll, then followed every frame', () => {
  const moved = vi.fn()
  const stop = followRect(el, moved)
  for (let i = 0; i < FOLLOW_FRAMES; i++) frame()
  box = { ...box, left: 40 }
  vi.advanceTimersByTime(FOLLOW_IDLE_MS)
  expect(moved).toHaveBeenLastCalledWith({ x: 40, y: 0, width: 100, height: 50 })
  expect(frames).toHaveLength(1)
  box = { ...box, left: 80 }
  frame()
  expect(moved).toHaveBeenLastCalledWith({ x: 80, y: 0, width: 100, height: 50 })
  stop()
})

test('a scroll wakes the frame loop at once', () => {
  const moved = vi.fn()
  const stop = followRect(el, moved)
  for (let i = 0; i < FOLLOW_FRAMES; i++) frame()
  window.dispatchEvent(new Event('scroll'))
  expect(frames).toHaveLength(1)
  expect(vi.getTimerCount()).toBe(0)
  box = { ...box, top: 10 }
  frame()
  expect(moved).toHaveBeenCalledTimes(1)
  stop()
  expect(frames).toHaveLength(0)
})
