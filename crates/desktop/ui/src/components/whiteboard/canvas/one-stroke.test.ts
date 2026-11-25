// One pen-down → pen-up is one stroke with one continuous outline (owner report, iPad + Pencil: curves broke every few cm
// into pieces, each ending flat and the next starting with a round cap). The real canvas, fed the way iPadOS WebKit
// delivers a Pencil: every sample's `pointermove` comes twice (the second when its estimated force arrives), each with
// the frame's `getCoalescedEvents()`, pressure changing throughout.
import '../../settings/test-dom'
import { getStrokePoints } from 'perfect-freehand'
import { act, createElement as h, createRef } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import * as Y from 'yjs'
import { unpack, type Pt } from '../ink/pack'
import { freehandOptions } from '../ink/render'
import type { CanvasHost, WhiteboardCanvasHandle } from './api'
import Canvas from './Canvas'
import { readBoard, type Stroke } from './model'
import { createWhiteboardStore, type WbStore } from './store'

type Sample = { clientX: number; clientY: number; pressure: number; timeStamp: number }

const HOST: CanvasHost = { doc: 'd', blobUrl: async () => null, blobPut: async () => {}, capture: async () => null, describePoint: async () => null, liveBudget: 8 }

let root: Root
let el: HTMLDivElement
let doc: Y.Doc
let store: WbStore

/** jsdom has no PointerEvent: a MouseEvent carrying the pointer fields (and the event clock) of `s`. */
function pointer(type: string, s: Sample, coalesced?: Sample[]) {
  const e = new MouseEvent(type, { bubbles: true, cancelable: true, button: 0, clientX: s.clientX, clientY: s.clientY })
  Object.defineProperties(e, {
    pointerType: { value: 'pen' },
    pointerId: { value: 3 },
    pressure: { value: s.pressure },
    timeStamp: { value: s.timeStamp },
    ...(coalesced ? { getCoalescedEvents: { value: () => coalesced } } : {}),
  })
  document.querySelector('.react-flow__pane')!.dispatchEvent(e)
}

/** Sharp corners perfect-freehand finds in a stroke's points: at each it closes the outline with a cap and starts anew. */
function corners(s: Stroke): number {
  const pts = unpack(s.o, s.pts).map(([x, y, p]) => [x, y, p])
  const sp = getStrokePoints(pts, freehandOptions(s.tool, s.width, s.p === 1))
  const total = sp.at(-1)!.runningLength
  let n = 0
  let prev = sp[0]!.vector
  let cornered = false
  for (let i = 0; i < sp.length; i++) {
    const last = i === sp.length - 1
    if (!last && total - sp[i]!.runningLength < 3) continue
    const v = sp[i]!.vector
    const next = (last ? sp[i]! : sp[i + 1]!).vector
    const back = v[0]! * prev[0]! + v[1]! * prev[1]! < 0 && !cornered
    const ahead = !last && v[0]! * next[0]! + v[1]! * next[1]! < 0
    if (back || ahead) {
      n++
      cornered = ahead
      continue
    }
    cornered = false
    prev = v
  }
  return n
}

beforeEach(async () => {
  Element.prototype.setPointerCapture ??= () => {}
  el = document.createElement('div')
  document.body.append(el)
  doc = new Y.Doc()
  store = createWhiteboardStore({ mode: 'draw', tool: 'pen' })
  root = createRoot(el)
  await act(async () => root.render(h(Canvas, { ref: createRef<WhiteboardCanvasHandle>(), provider: { doc } as never, store, host: HOST })))
})

afterEach(() => {
  act(() => root.unmount())
  el.remove()
})

describe('one pen-down → pen-up is one stroke (iPad Pencil, pointer path)', () => {
  it('a long cursive curve with doubled moves, coalesced samples and changing pressure: one stroke, one path, no breaks', async () => {
    // 2.5 s at 240 Hz, 4 samples a 60 Hz frame; slow at the turns, as a hand writes.
    const samples: Sample[] = []
    let s = 0
    for (let i = 0; i < 600; i++) {
      s += (40 + 260 * Math.abs(Math.sin(s * 3))) / 240 / 150
      samples.push({ clientX: 200 + s * 150 + 40 * Math.sin(s * 6), clientY: 300 + 50 * Math.cos(s * 6), pressure: 0.35 + 0.3 * Math.sin(i / 40) ** 2, timeStamp: 1000 + i / 0.24 })
    }
    act(() => {
      pointer('pointerdown', samples[0]!)
      for (let f = 1; f < samples.length; f += 4) {
        const frame = samples.slice(f, f + 4)
        const at = frame.at(-1)!
        pointer('pointermove', at, frame)
        // The same sample again once its estimated force arrives: same place and time, the frame's samples again.
        pointer('pointermove', { ...at, pressure: at.pressure + 0.02 }, frame.map((x) => ({ ...x, pressure: x.pressure + 0.02 })))
      }
      pointer('pointerup', samples.at(-1)!)
    })
    await act(async () => {})

    const ink = Object.values(readBoard(doc).ink)
    expect(ink).toHaveLength(1)
    const stroke = ink[0]!
    const pts: Pt[] = unpack(stroke.o, stroke.pts)
    for (let i = 1; i < pts.length; i++) expect(pts[i]![3]).toBeGreaterThanOrEqual(pts[i - 1]![3])
    expect(corners(stroke)).toBe(0)

    const paths = el.querySelectorAll('path[data-stroke]')
    expect(paths).toHaveLength(1)
    const d = paths[0]!.getAttribute('d')!
    expect(d.match(/M/g)).toHaveLength(1)
    expect(d.match(/Z/g)).toHaveLength(1)
  })
})
