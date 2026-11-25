// The ink tool latches through every stroke (owner report on v2.30.2: the first stroke after a pick drew, and its pen-up
// dropped the tool). The real sequence through the real canvas, cluster, timer and native-ink connection: pick in the
// popover, then stroke after stroke. On the iPad the first stroke goes through the pointer path (native capture is off
// while the cluster is up) and its pen-down hides the cluster; the strokes after it are native. The fake plugin keeps
// the plugin's own rules: its recogniser takes a Pencil touch only if it was on when the touch began, and the page
// still gets every Pencil pointer event (route A). On the desktop the mouse inks; the cursor follows the tool.
import '../settings/test-dom'
import { act, createElement as h, createRef } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { useStore } from 'zustand'
import * as Y from 'yjs'
import type { InkEvent, NativeInk, NativeTool, StrokeBatch, attachNativeInk } from '@/lib/ink-native'
import { Cluster } from './Cluster'
import type { CanvasHost, WhiteboardCanvasHandle } from './canvas/api'
import Canvas from './canvas/Canvas'
import { readBoard } from './canvas/model'
import { createWhiteboardStore, type WbStore } from './canvas/store'
import { type ClusterTimer, clusterTimer } from './cluster-timer'
import { type NativeConnection, connectNativeInk } from './native-ink'

const HOST: CanvasHost = { doc: 'd', blobUrl: async () => null, blobPut: async () => {}, capture: async () => null, describePoint: async () => null, liveBudget: 8 }

/** The plugin as the overlay behaves (InkOverlay / CaptureSurfaces route A). */
type Plugin = {
  enabled: boolean
  tool: NativeTool | null
  /** A Pencil touch is down on the web view. */
  touch: boolean
  /** The recogniser took the touch in flight (it was on when the touch began). */
  native: boolean
  /** `set_enabled(true)` arrived while a Pencil touch it did not take was down. */
  enabledUnderTouch: number
  seq: number
  on: { strokes(b: StrokeBatch): void; event(e: InkEvent): void } | null
}

function attach(p: Plugin): typeof attachNativeInk {
  return async (on) => {
    p.on = on
    const ink: NativeInk = {
      result: { native: true, route: 'A' },
      setEnabled: (enabled) => {
        if (enabled && !p.enabled && p.touch && !p.native) p.enabledUnderTouch++
        p.enabled = enabled
      },
      setViewport() {},
      setTool: (t) => {
        p.tool = t
        // PencilKit's picker reports the selection back with its own float width and colour spelling.
        queueMicrotask(() => p.on?.event({ kind: 'tool', tool: t.tool, colour: t.colour.toUpperCase(), width: Math.fround(t.width) }))
      },
      setPickerVisible() {},
      ack() {},
      recognizeText: async () => null,
      detach: async () => {},
    }
    return ink
  }
}

let root: Root
let el: HTMLDivElement
let doc: Y.Doc
let store: WbStore
let timer: ClusterTimer
let conn: NativeConnection | null
let plugin: Plugin
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
        addable: { sticky: true, note: true, artifact: true, excerpt: true, diff: true, terminal: true, web: true, image: true, clipboard: true },
      }),
  )
}

async function mount(native: boolean) {
  Element.prototype.setPointerCapture ??= () => {}
  el = document.createElement('div')
  document.body.append(el)
  doc = new Y.Doc()
  store = createWhiteboardStore()
  plugin = { enabled: false, tool: null, touch: false, native: false, enabledUnderTouch: 0, seq: 1, on: null }
  timer = clusterTimer({ onChange: (open) => store.getState().set({ clusterOpen: open }) })
  store.subscribe((s, p) => s.penDown && !p.penDown && timer.pen()) // the shell (WhiteboardView)
  conn = await connectNativeInk({ store, handle: () => canvas.current, dark: () => false, showTools: () => timer.show(), attach: native ? attach(plugin) : async () => null })
  root = createRoot(el)
  await act(async () => root.render(h(Shell, { s: store })))
}

afterEach(() => {
  act(() => root.unmount())
  conn?.dispose()
  timer.dispose()
  el.remove()
  document.body.innerHTML = ''
})

const pane = () => document.querySelector('.react-flow__pane')!
const button = (label: string) => document.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)
/** The native hand-over commits, then waits two animation frames for the paint before it acks: wait those out. */
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
  const trigger = document.querySelector<HTMLButtonElement>('button[aria-label^="Ink tools"]')!
  await tap(trigger, kind)
  expect(button('Pen')).not.toBeNull()
}

/**
 * A Pencil stroke on the canvas (iPad). The page gets its pointer events either way; the recogniser takes it when it is
 * on as the touch begins, and then delivers it at pen-idle as the overlay does.
 */
async function pencilStroke(x0: number) {
  await act(async () => {
    plugin.touch = true
    plugin.native = plugin.enabled
    if (plugin.native) plugin.on!.event({ kind: 'state', drawing: true })
    pointer(pane(), 'pointerdown', 'pen', x0, 100)
    for (let i = 1; i <= 20; i++) pointer(pane(), 'pointermove', 'pen', x0 + i * 4, 100 + Math.sin(i / 3) * 20)
    plugin.touch = false // UIKit ends the touch before the page gets its pointerup
    pointer(pane(), 'pointerup', 'pen', x0 + 80, 100)
    if (plugin.native) {
      plugin.on!.event({ kind: 'state', drawing: false })
      const t = plugin.tool!
      const pts = Array.from({ length: 21 }, (_, i) => [x0 + i * 4, 100 + Math.sin(i / 3) * 20, 0.4, i * 8]).flat()
      plugin.on!.strokes({ batch: plugin.seq, strokes: [{ seq: plugin.seq++, tool: t.tool === 'eraser' ? 'pen' : t.tool, colour: t.colour, width: t.width, t0: Date.now(), view: { tx: 0, ty: 0, zoom: 1 }, pts, retry: 0 }] })
    }
    plugin.native = false
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
  beforeEach(() => mount(true))

  it('pick Marker + red → stroke, stroke, stroke: each draws with the marker in red; capture never turns on under a pen', async () => {
    await openInkTools('touch')
    expect(plugin.enabled).toBe(false) // off while the cluster is up
    await tap(button('Marker')!, 'pen')
    await tap(document.querySelector('button[aria-label="red"]')!, 'pen')
    await settle()
    expect(store.getState()).toMatchObject({ mode: 'draw', tool: 'marker', colour: 'red' })

    for (let n = 1; n <= 4; n++) {
      await pencilStroke(n * 120)
      expect(strokes()).toHaveLength(n)
      expect(strokes().at(-1)).toMatchObject({ tool: 'marker', colour: 'red' })
      expect(store.getState()).toMatchObject({ mode: 'draw', tool: 'marker', colour: 'red', penDown: false, clusterOpen: false })
      // After the first stroke's pen-up the recogniser is on, with the marker in red.
      expect(plugin.enabled).toBe(true)
      expect(plugin.tool).toMatchObject({ tool: 'marker', colour: '#d9480f' })
    }
    // The first stroke's pen-down hid the cluster; capture came back at its pen-up, never under the pen.
    expect(plugin.enabledUnderTouch).toBe(0)
    expect(strokes().map((s) => s.tool)).toEqual(['marker', 'marker', 'marker', 'marker'])
  })

  it('the tool comes down only by a pick of another tool, Select, or Esc', async () => {
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
    await tap(document.querySelector('button[aria-label^="Ink tools"]')!, 'touch')
    await tap(button('Pen')!, 'touch')
    expect(store.getState().mode).toBe('draw')
    await act(async () => {
      document.body.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
    })
    expect(store.getState().mode).toBe('idle')
  })
})

describe('desktop + mouse: the picked tool draws every stroke and the cursor follows it', () => {
  beforeEach(() => mount(false))

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
