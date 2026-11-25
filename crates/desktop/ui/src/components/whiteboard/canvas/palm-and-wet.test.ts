// Web-only ink on the iPad: a palm that rests before the Pencil lands must not act as a finger (open the mark menu,
// undo, pan), and the wet stroke draws once per animation frame however many `pointermove`s arrive (iPadOS WebKit sends
// each Pencil sample twice, each with its coalesced and predicted samples). The real canvas, fed pointer events.
import '../../settings/test-dom'
import { act, createElement as h, createRef } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import * as Y from 'yjs'
import { unpack } from '../ink/pack'
import type { CanvasHost, WhiteboardCanvasHandle } from './api'
import Canvas from './Canvas'
import { PALM_MS } from './input'
import { readBoard } from './model'
import { createWhiteboardStore } from './store'

type Sample = { clientX: number; clientY: number; pressure: number; timeStamp: number }
type Extra = { coalesced?: Sample[]; predicted?: Sample[] }

const HOST: CanvasHost = { doc: 'd', blobUrl: async () => null, blobPut: async () => {}, capture: async () => null, describePoint: async () => null, liveBudget: 8 }

let root: Root
let el: HTMLDivElement
let doc: Y.Doc
let clock = 10_000
const handle = createRef<WhiteboardCanvasHandle>()

/** jsdom has no PointerEvent: a MouseEvent carrying the pointer fields of `s`. */
function pointer(type: string, kind: 'pen' | 'touch', id: number, s: Sample, x: Extra = {}) {
  const e = new MouseEvent(type, { bubbles: true, cancelable: true, button: 0, clientX: s.clientX, clientY: s.clientY })
  Object.defineProperties(e, {
    pointerType: { value: kind },
    pointerId: { value: id },
    pressure: { value: s.pressure },
    timeStamp: { value: s.timeStamp },
    ...(x.coalesced ? { getCoalescedEvents: { value: () => x.coalesced } } : {}),
    ...(x.predicted ? { getPredictedEvents: { value: () => x.predicted } } : {}),
  })
  document.querySelector('.react-flow__pane')!.dispatchEvent(e)
}
const at = (clientX: number, clientY: number, timeStamp = clock): Sample => ({ clientX, clientY, pressure: 0.5, timeStamp })

function penStroke(x0: number, y: number) {
  pointer('pointerdown', 'pen', 3, at(x0, y))
  for (let i = 1; i <= 10; i++) pointer('pointermove', 'pen', 3, at(x0 + i * 8, y, clock + i * 8))
  pointer('pointerup', 'pen', 3, at(x0 + 80, y, clock + 90))
}

function twoFingerTap() {
  pointer('pointerdown', 'touch', 20, at(100, 600))
  pointer('pointerdown', 'touch', 21, at(160, 600, clock + 20))
  pointer('pointerup', 'touch', 20, at(100, 600, clock + 120))
  pointer('pointerup', 'touch', 21, at(160, 600, clock + 140))
}

const strokes = () => Object.values(readBoard(doc).ink)
const markMenu = () => [...document.querySelectorAll('button')].find((b) => b.textContent?.includes('Pin mark'))

beforeEach(async () => {
  clock = 10_000
  vi.spyOn(performance, 'now').mockImplementation(() => clock)
  Element.prototype.setPointerCapture ??= () => {}
  el = document.createElement('div')
  document.body.append(el)
  doc = new Y.Doc()
  root = createRoot(el)
  const store = createWhiteboardStore({ mode: 'draw', tool: 'pen' })
  await act(async () => root.render(h(Canvas, { ref: handle, provider: { doc } as never, store, host: HOST })))
})

afterEach(() => {
  act(() => root.unmount())
  el.remove()
  vi.useRealTimers()
  vi.restoreAllMocks()
})

describe('a palm resting before the Pencil', () => {
  it('opens no mark menu, undoes nothing and moves no view while the pen writes; a finger long-press after still does', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] })
    act(() => penStroke(300, 300))
    // The stroke becomes a sent mark (jsdom lays out nothing, so every stroke is "off screen").
    act(() => handle.current!.commitSend(handle.current!.planSend({ offScreen: true }), { to: null }))
    expect(Object.keys(readBoard(doc).marks)).toHaveLength(1)
    clock += 2 * PALM_MS
    const view = handle.current!.view()

    act(() => {
      pointer('pointerdown', 'touch', 30, at(340, 300)) // the palm lands on the mark's ink, before the pen
      pointer('pointerdown', 'pen', 3, at(600, 500, clock + 30))
      for (let i = 1; i <= 20; i++) {
        pointer('pointermove', 'pen', 3, at(600 + i * 6, 500, clock + 30 + i * 8))
        pointer('pointermove', 'touch', 30, at(340 + i / 5, 300 + i / 5, clock + 30 + i * 8)) // the palm shifts, within the long-press slop
      }
    })
    act(() => vi.advanceTimersByTime(700)) // past the long-press, pen still down
    expect(markMenu()).toBeUndefined()
    act(() => {
      pointer('pointerup', 'pen', 3, at(720, 500, clock + 800))
      pointer('pointerup', 'touch', 30, at(360, 320, clock + 810))
    })
    expect(markMenu()).toBeUndefined()
    expect(strokes()).toHaveLength(2)
    expect(handle.current!.view()).toEqual(view)

    // The same press with no pen is a finger: after the palm window it opens the menu (the test above is live).
    clock += 2 * PALM_MS
    act(() => pointer('pointerdown', 'touch', 31, at(340, 300)))
    act(() => vi.advanceTimersByTime(700))
    expect(markMenu()).toBeDefined()
  })

  it('a two-finger tap in the palm window does not undo; one after it does', () => {
    act(() => penStroke(300, 300))
    expect(strokes()).toHaveLength(1)
    clock += PALM_MS / 2
    act(() => twoFingerTap())
    expect(strokes()).toHaveLength(1)
    clock += PALM_MS
    act(() => twoFingerTap())
    expect(strokes()).toHaveLength(0)
  })
})

describe('wet ink', () => {
  it('a 600-sample stroke with doubled moves and predicted samples: at most one wet draw a frame, no predicted point committed, one path', async () => {
    const frames: FrameRequestCallback[] = []
    vi.spyOn(window, 'requestAnimationFrame').mockImplementation((cb) => frames.push(cb))
    vi.spyOn(window, 'cancelAnimationFrame').mockImplementation(() => {})
    const flush = () => {
      const due = frames.splice(0)
      for (const cb of due) cb(clock)
      return due.length
    }
    let draws = 0
    const ctx = { setTransform() {}, clearRect() {}, fill: () => void draws++, globalAlpha: 1, fillStyle: '' }
    vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockImplementation(() => ctx as never)
    vi.stubGlobal('Path2D', class {})
    flush()

    const samples: Sample[] = Array.from({ length: 600 }, (_, i) => ({ clientX: 100 + i, clientY: 300 + 40 * Math.sin(i / 30), pressure: 0.4, timeStamp: 1000 + i / 0.24 }))
    const maxX = 100 + 599
    let frameCount = 0
    act(() => {
      pointer('pointerdown', 'pen', 3, samples[0]!)
      for (let f = 1; f < samples.length; f += 4) {
        const frame = samples.slice(f, f + 4)
        const last = frame.at(-1)!
        const predicted = [{ ...last, clientX: last.clientX + 400, timeStamp: last.timeStamp + 8 }]
        pointer('pointermove', 'pen', 3, last, { coalesced: frame, predicted })
        pointer('pointermove', 'pen', 3, { ...last, pressure: 0.42 }, { coalesced: frame, predicted })
        const before = draws
        frameCount += flush() > 0 ? 1 : 0
        expect(draws - before).toBeLessThanOrEqual(1)
      }
      pointer('pointerup', 'pen', 3, samples.at(-1)!)
    })
    await act(async () => {})
    expect(draws).toBeLessThanOrEqual(frameCount)
    expect(draws).toBeGreaterThan(100) // it did draw, every frame

    const ink = strokes()
    expect(ink).toHaveLength(1)
    const v = handle.current!.view()
    for (const [x] of unpack(ink[0]!.o, ink[0]!.pts)) expect(x).toBeLessThanOrEqual((maxX - v.x) / v.zoom + 0.5)
    expect(el.querySelectorAll('path[data-stroke]')).toHaveLength(1)
  })
})
