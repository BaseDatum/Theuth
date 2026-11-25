// 218 hover preview: a Pencil hovering (WebKit: `pointerType: 'pen'` moves with no button down) shows where the mark
// lands, at the right canvas position whatever the zoom and pan, in the tool's colour and width; pen-down hides it and
// the stroke starts at the pen-down, never at a hover point; leaving, 200 ms of silence, or Hand hide it.
import '../settings/test-dom'
import { act, createElement as h, createRef } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import * as Y from 'yjs'
import type { CanvasHost, WhiteboardCanvasHandle } from './canvas/api'
import Canvas from './canvas/Canvas'
import { HOVER_IDLE_MS } from './canvas/hover'
import { readBoard } from './canvas/model'
import { createWhiteboardStore, type WbStore } from './canvas/store'

const HOST: CanvasHost = { doc: 'd', blobUrl: async () => null, blobPut: async () => {}, capture: async () => null, describePoint: async () => null, liveBudget: 8 }

let root: Root
let el: HTMLDivElement
let doc: Y.Doc
let store: WbStore
const canvas = createRef<WhiteboardCanvasHandle>()

beforeEach(async () => {
  Element.prototype.setPointerCapture ??= () => {}
  el = document.createElement('div')
  document.body.append(el)
  doc = new Y.Doc()
  store = createWhiteboardStore({ mode: 'draw', tool: 'pen', colour: 'red', width: 2.8 })
  root = createRoot(el)
  await act(async () => root.render(h(Canvas, { ref: canvas, provider: { doc } as never, store, host: HOST })))
})

afterEach(() => {
  act(() => root.unmount())
  el.remove()
  document.body.innerHTML = ''
})

const pane = () => document.querySelector('.react-flow__pane')!
const dot = () => document.querySelector<HTMLDivElement>('[data-hover-preview]')!
async function frames(n = 2) {
  for (let i = 0; i < n; i++) {
    const { promise, resolve } = Promise.withResolvers<number>()
    requestAnimationFrame(resolve)
    await act(async () => void (await promise))
  }
}
function pointer(type: string, kind: string, x: number, y: number, buttons = 0) {
  const e = new MouseEvent(type, { bubbles: true, cancelable: true, button: 0, buttons, clientX: x, clientY: y })
  Object.defineProperties(e, { pointerType: { value: kind }, pointerId: { value: kind === 'mouse' ? 1 : 9 }, pressure: { value: buttons ? 0.4 : 0 } })
  pane().dispatchEvent(e)
}
const hover = async (kind: string, x: number, y: number) => {
  await act(async () => pointer('pointermove', kind, x, y))
  await frames()
}
const world = () => dot().dataset.world?.split(',').map(Number)

describe('Pencil hover preview', () => {
  it('shows at the canvas point under the tip, sized by the tool and the zoom, after a pan and zoom', async () => {
    await act(async () => canvas.current!.setView({ x: 100, y: 50, zoom: 2 }))
    await frames()
    expect(canvas.current!.view()).toEqual({ x: 100, y: 50, zoom: 2 })
    await hover('pen', 300, 250)
    expect(dot().hidden).toBe(false)
    expect(world()).toEqual([100, 100]) // (300 − 100) / 2, (250 − 50) / 2
    expect(dot().style.width).toBe('5.6px') // the pen's 2.8 at 2×
    expect(dot().style.transform).toBe('translate(297.2px, 247.2px)')
    // The marker draws three widths wide, translucent.
    await act(async () => store.getState().set({ tool: 'marker', width: 6 }))
    await frames()
    expect(dot().style.width).toBe('36px')
    expect(Number(dot().style.opacity)).toBeLessThan(0.5)
  })

  it('pen-down hides it, and the stroke holds no hover point', async () => {
    await hover('pen', 900, 600)
    expect(dot().hidden).toBe(false)
    await act(async () => {
      pointer('pointerdown', 'pen', 100, 100, 1)
      for (let i = 1; i <= 10; i++) pointer('pointermove', 'pen', 100 + i * 8, 100, 1)
    })
    expect(dot().hidden).toBe(true)
    await act(async () => pointer('pointerup', 'pen', 180, 100))
    await frames(3)
    const ink = Object.values(readBoard(doc).ink)
    expect(ink).toHaveLength(1)
    const [x0, y0, w, hgt] = ink[0]!.bb
    expect(x0 + w).toBeLessThan(400)
    expect(y0 + hgt).toBeLessThan(300)
  })

  it('leaving the canvas, or 200 ms without a hover move, hides it', async () => {
    await hover('pen', 300, 250)
    await act(async () => el.querySelector('.wb-canvas')!.dispatchEvent(new MouseEvent('pointerleave')))
    expect(dot().hidden).toBe(true)
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] }) // the idle clock; animation frames stay real
    try {
      await hover('pen', 320, 250)
      expect(dot().hidden).toBe(false)
      act(() => vi.advanceTimersByTime(HOVER_IDLE_MS - 1))
      expect(dot().hidden).toBe(false)
      act(() => vi.advanceTimersByTime(1))
    } finally {
      vi.useRealTimers()
    }
    expect(dot().hidden).toBe(true)
  })

  it('Hand shows no ink preview; the eraser shows its circle', async () => {
    await act(async () => store.getState().set({ mode: 'hand' }))
    await hover('pen', 300, 250)
    expect(dot().hidden).toBe(true)
    await act(async () => store.getState().set({ mode: 'erase' }))
    await hover('pen', 300, 250)
    expect(dot().hidden).toBe(false)
    expect(dot().style.boxShadow).toContain('inset')
    expect(dot().style.background).toBe('transparent')
  })

  it('the mouse previews with an ink tool and not without one', async () => {
    await hover('mouse', 300, 250)
    expect(dot().hidden).toBe(false)
    await act(async () => store.getState().set({ mode: 'idle' }))
    await frames()
    expect(dot().hidden).toBe(true)
  })
})
