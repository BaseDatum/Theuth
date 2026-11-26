// The ink tool latches through every stroke (owner report on v2.30.2: the first stroke after a pick drew, and its pen-up
// dropped the tool). The real sequence through the real canvas, cluster and timer: pick in the popover, then stroke
// after stroke. Ink is web-only: on the iPad the Pencil is a `pointerType: 'pen'` pointer (its pen-down hides the
// cluster); on the desktop the mouse inks and the cursor follows the tool. Only another tool, Select or Esc changes it.
import '../settings/test-dom'
import { act, createElement as h, createRef } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { useStore } from 'zustand'
import * as Y from 'yjs'
import { Cluster } from './Cluster'
import type { CanvasHost, WhiteboardCanvasHandle } from './canvas/api'
import Canvas from './canvas/Canvas'
import { readBoard } from './canvas/model'
import { createWhiteboardStore, type WbStore } from './canvas/store'
import { type ClusterTimer, clusterTimer } from './cluster-timer'

const HOST: CanvasHost = { doc: 'd', blobUrl: async () => null, blobPut: async () => {}, capture: async () => null, describePoint: async () => null, liveBudget: 8 }

let root: Root
let el: HTMLDivElement
let doc: Y.Doc
let store: WbStore
let timer: ClusterTimer
const canvas = createRef<WhiteboardCanvasHandle>()

function Shell({ s }: { s: WbStore }) {
  const open = useStore(s, (v) => v.clusterOpen)
  return h(
    'div',
    null,
    h(Canvas, {
      ref: canvas, provider: { doc } as never, store: s, host: HOST,
      onCanvasTap: () => timer.tap(), onPenActivity: (down: boolean) => down && timer.pen(),
    }),
    open &&
      h(Cluster, {
        store: s, onActivity: () => timer.activity(), onAdd() {}, onFit() {}, onOutline() {}, onUndo() {}, onRedo() {},
        canUndo: false, canRedo: false, more: null, onHold: timer.hold,
        addable: { sticky: true, note: true, artifact: true, excerpt: true, diff: true, terminal: true, web: true, image: true, view: true, clipboard: true },
      }),
  )
}

async function mount() {
  Element.prototype.setPointerCapture ??= () => {}
  el = document.createElement('div')
  document.body.append(el)
  doc = new Y.Doc()
  store = createWhiteboardStore()
  timer = clusterTimer({ onChange: (open) => store.getState().set({ clusterOpen: open }) })
  store.subscribe((s, p) => s.penDown && !p.penDown && timer.pen()) // the shell (WhiteboardView)
  root = createRoot(el)
  await act(async () => root.render(h(Shell, { s: store })))
}

afterEach(() => {
  act(() => root.unmount())
  timer.dispose()
  el.remove()
  document.body.innerHTML = ''
})

const pane = () => document.querySelector('.react-flow__pane')!
const button = (label: string) => document.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)
const inkTools = () => document.querySelector<HTMLButtonElement>('button[aria-label^="Ink tools"]')
/** A committed stroke paints after two animation frames: wait those out. */
async function settle() {
  for (let i = 0; i < 3; i++) {
    const { promise, resolve } = Promise.withResolvers<number>()
    requestAnimationFrame(resolve)
    await act(async () => void (await promise))
  }
}

function pointer(target: Element, type: string, kind: string, x = 0, y = 0, extra: Record<string, unknown> = {}) {
  const e = new MouseEvent(type, { bubbles: true, cancelable: true, button: 0, clientX: x, clientY: y })
  Object.defineProperties(e, { pointerType: { value: kind }, pointerId: { value: kind === 'mouse' ? 1 : 9 }, pressure: { value: 0.4 }, ...Object.fromEntries(Object.entries(extra).map(([k, v]) => [k, { value: v }])) })
  return target.dispatchEvent(e)
}

/** A tap (finger, mouse or Pencil) on an element: down, up, click. */
async function tap(target: Element, kind: string) {
  await act(async () => {
    pointer(target, 'pointerdown', kind)
    pointer(target, 'pointerup', kind)
    target.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }))
  })
}

/** A finger (iPad) or mouse (desktop) tap on empty canvas raises the cluster; then the ink tools popover opens. */
async function openInkTools(kind: 'touch' | 'mouse') {
  await tap(pane(), kind)
  expect(store.getState().clusterOpen).toBe(true)
  await tap(inkTools()!, kind)
  expect(button('Pen')).not.toBeNull()
}

/** A Pencil stroke on the canvas (iPad): pointer events with `pointerType: 'pen'`, as WebKit delivers them. */
async function pencilStroke(x0: number) {
  await act(async () => {
    pointer(pane(), 'pointerdown', 'pen', x0, 100)
    for (let i = 1; i <= 20; i++) pointer(pane(), 'pointermove', 'pen', x0 + i * 4, 100 + Math.sin(i / 3) * 20)
    pointer(pane(), 'pointerup', 'pen', x0 + 80, 100)
  })
  await settle()
}

/** A mouse stroke on the canvas (desktop), with the click the browser sends after the mouse-up. */
async function mouseStroke(x0: number) {
  await act(async () => {
    pointer(pane(), 'pointerdown', 'mouse', x0, 200)
    for (let i = 1; i <= 20; i++) pointer(pane(), 'pointermove', 'mouse', x0 + i * 4, 200 + Math.cos(i / 3) * 20, { buttons: 1 })
    pointer(pane(), 'pointerup', 'mouse', x0 + 80, 200)
    pane().dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }))
  })
  await settle()
}

const strokes = () => Object.values(readBoard(doc).ink).sort((a, b) => a.at - b.at)

describe('iPad + Pencil: the picked tool draws every stroke', () => {
  beforeEach(() => mount())

  it('pick Marker + red with the Pencil → stroke, stroke, stroke, stroke: each one stroke with the marker in red', async () => {
    await openInkTools('touch')
    await tap(button('Marker')!, 'pen')
    await tap(document.querySelector('button[aria-label="red"]')!, 'pen')
    await settle()
    expect(store.getState()).toMatchObject({ mode: 'draw', tool: 'marker', colour: 'red' })

    for (let n = 1; n <= 4; n++) {
      await pencilStroke(n * 120)
      expect(strokes()).toHaveLength(n)
      expect(strokes().at(-1)).toMatchObject({ tool: 'marker', colour: 'red' })
      // The first stroke's pen-down hid the cluster; pen-up changes nothing.
      expect(store.getState()).toMatchObject({ mode: 'draw', tool: 'marker', colour: 'red', penDown: false, clusterOpen: false })
    }
    expect(el.querySelectorAll('path[data-stroke]')).toHaveLength(4)
  })

  it('picking the armed pen again keeps it (a repeated or re-fired pick never switches it off)', async () => {
    await openInkTools('touch')
    await tap(button('Pen')!, 'pen')
    await tap(button('Pen')!, 'pen')
    expect(store.getState()).toMatchObject({ mode: 'draw', tool: 'pen' })
    await tap(button('Eraser')!, 'pen')
    await tap(button('Eraser')!, 'pen')
    expect(store.getState().mode).toBe('erase')
  })

  it('the cluster auto-hiding and the popover closing do not change the tool', async () => {
    await openInkTools('touch')
    await tap(button('Fine liner')!, 'pen')
    await tap(inkTools()!, 'touch') // the popover closes; the hide timer starts again
    expect(button('Fine liner')).toBeNull()
    act(() => timer.hide()) // what the 4 s auto-hide does (cluster-timer's own tests time it)
    expect(store.getState().clusterOpen).toBe(false)
    expect(store.getState()).toMatchObject({ mode: 'draw', tool: 'monoline' })
    await pencilStroke(100)
    expect(strokes().at(-1)).toMatchObject({ tool: 'monoline' })
  })

  it('the tool comes down only by a pick of another tool, Hand, Select, or Esc', async () => {
    await openInkTools('touch')
    await tap(button('Pen')!, 'pen')
    await pencilStroke(100)
    await pencilStroke(220)
    expect(store.getState().mode).toBe('draw')
    await openInkTools('touch')
    await tap(button('Eraser')!, 'touch')
    expect(store.getState().mode).toBe('erase')
    await tap(button('Fine liner')!, 'touch')
    expect(store.getState()).toMatchObject({ mode: 'draw', tool: 'monoline' })
    await tap(button('Select')!, 'touch')
    expect(store.getState().mode).toBe('select')
    // Select closed the popover (a press outside it): open it again from the cluster's ink tools button.
    await tap(inkTools()!, 'touch')
    await tap(button('Pen')!, 'touch')
    expect(store.getState().mode).toBe('draw')
    await act(async () => {
      document.body.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
    })
    // Esc puts the ink tool down as Hand does: the Pencil navigates.
    expect(store.getState().mode).toBe('hand')
  })
})

/** A drag on the canvas: whether its pointer-down reached React Flow (not prevented), as a pan or drag needs. */
async function drag(kind: 'pen' | 'mouse', x0: number): Promise<boolean> {
  const down = { passed: false }
  await act(async () => {
    down.passed = pointer(pane(), 'pointerdown', kind, x0, 300)
    for (let i = 1; i <= 20; i++) pointer(pane(), 'pointermove', kind, x0 + i * 4, 300, { buttons: 1 })
    pointer(pane(), 'pointerup', kind, x0 + 80, 300)
  })
  await settle()
  return down.passed
}

/** The same Hand round trip with the Pencil (iPad) or the mouse (desktop): pick pen → strokes draw → pick Hand → a
 *  drag reaches React Flow (pan / select) and draws nothing → pick pen again → it draws. */
describe.each([
  { name: 'iPad + Pencil', kind: 'pen' as const, opener: 'touch' as const, stroke: pencilStroke },
  { name: 'desktop + mouse', kind: 'mouse' as const, opener: 'mouse' as const, stroke: mouseStroke },
])('$name: Hand puts the ink tool down', ({ kind, opener, stroke }) => {
  beforeEach(() => mount())

  /** The ink panel, from wherever the last step left it: the cluster raised as a canvas tap raises it (a mouse tap on
   *  the canvas would ink in draw mode), then its ink tools button. */
  async function inkPanel() {
    if (!store.getState().clusterOpen) act(() => timer.show())
    if (!button('Pen')) await tap(inkTools()!, kind)
    expect(button('Hand')).not.toBeNull()
  }

  it('pick Pen → draws; pick Hand → a drag pans or selects and makes no stroke; pick Pen → draws again', async () => {
    await openInkTools(opener)
    await tap(button('Pen')!, kind)
    await stroke(100)
    await stroke(220)
    expect(strokes()).toHaveLength(2)

    await inkPanel()
    await tap(button('Hand')!, kind)
    expect(store.getState().mode).toBe('hand')
    expect(button('Hand')!.getAttribute('aria-pressed')).toBe('true')
    expect(inkTools()!.getAttribute('aria-label')).toBe('Ink tools: Hand')
    expect(await drag(kind, 340)).toBe(true) // not taken by the ink layer: React Flow gets it
    expect(await drag(kind, 460)).toBe(true)
    expect(strokes()).toHaveLength(2)
    expect(store.getState().mode).toBe('hand') // latched through the drags

    await inkPanel()
    await tap(button('Pen')!, kind)
    await stroke(580)
    expect(strokes()).toHaveLength(3)
    expect(store.getState().mode).toBe('draw')
  })
})

describe('desktop + mouse: the picked tool draws every stroke and the cursor follows it', () => {
  beforeEach(() => mount())

  it('pick Pen + blue with the mouse → stroke, stroke, stroke: each draws; the cursor is the ink cursor throughout', async () => {
    expect(pane().closest('.wb-canvas')!.getAttribute('data-cursor')).toBeNull()
    await openInkTools('mouse')
    await tap(button('Pen')!, 'mouse')
    await tap(document.querySelector('button[aria-label="blue"]')!, 'mouse')
    const wrap = pane().closest('.wb-canvas')!
    expect(wrap.getAttribute('data-cursor')).toBe('ink')
    for (let n = 1; n <= 3; n++) {
      await mouseStroke(n * 120)
      expect(strokes()).toHaveLength(n)
      expect(strokes().at(-1)).toMatchObject({ tool: 'pen', colour: 'blue' })
      expect(store.getState()).toMatchObject({ mode: 'draw', tool: 'pen', colour: 'blue' })
      expect(wrap.getAttribute('data-cursor')).toBe('ink')
    }
  })

  it('cursor: eraser for the eraser, the grab hand while Space pans or with select, none for idle', async () => {
    const wrap = pane().closest('.wb-canvas')!
    await openInkTools('mouse')
    await tap(button('Eraser')!, 'mouse')
    expect(wrap.getAttribute('data-cursor')).toBe('erase')
    await tap(button('Marker')!, 'mouse')
    expect(wrap.getAttribute('data-cursor')).toBe('ink')
    await act(async () => {
      window.dispatchEvent(new KeyboardEvent('keydown', { code: 'Space', key: ' ' }))
    })
    expect(wrap.getAttribute('data-cursor')).toBeNull() // React Flow's grab hand
    await act(async () => {
      window.dispatchEvent(new KeyboardEvent('keyup', { code: 'Space', key: ' ' }))
    })
    expect(wrap.getAttribute('data-cursor')).toBe('ink')
    await tap(button('Select')!, 'mouse')
    expect(wrap.getAttribute('data-cursor')).toBeNull()
    await act(async () => {
      store.getState().set({ mode: 'idle' })
    })
    expect(wrap.getAttribute('data-cursor')).toBeNull()
  })
})
