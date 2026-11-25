// Owner report (218): an agent's sticky or diagram would not resize past a point, however far the corner was dragged.
// The resizer re-bound its drag on every render, and a finger's or the Pencil's drag listens on the handle itself, so
// the gesture died after its first step. The real canvas: a finger drags a card's corner far past its placement size,
// step by step, and the size the document keeps is the one dragged to.
import '../settings/test-dom'
import { act, createElement as h } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, expect, it } from 'vitest'
import * as Y from 'yjs'
import type { CanvasHost } from './canvas/api'
import Canvas from './canvas/Canvas'
import { readBoard } from './canvas/model'
import { addCard } from './canvas/ops'
import { createWhiteboardStore, type WbStore } from './canvas/store'

const HOST: CanvasHost = { doc: 'd', blobUrl: async () => null, blobPut: async () => {}, capture: async () => null, describePoint: async () => null, liveBudget: 8 }

let root: Root
let el: HTMLDivElement
let doc: Y.Doc
let store: WbStore
const restore: (() => void)[] = []

/** jsdom lays nothing out: an element is as big as its inline style says, and every observed element is measured at once. */
function layout() {
  const px = (v: string) => Number.parseFloat(v) || 0
  for (const [k, prop] of [['offsetWidth', 'width'], ['offsetHeight', 'height']] as const) {
    const was = Object.getOwnPropertyDescriptor(HTMLElement.prototype, k)
    Object.defineProperty(HTMLElement.prototype, k, { configurable: true, get(this: HTMLElement) { return px(this.style[prop]) } })
    restore.push(() => was && Object.defineProperty(HTMLElement.prototype, k, was))
  }
  const RO = globalThis.ResizeObserver
  globalThis.ResizeObserver = class {
    constructor(private cb: ResizeObserverCallback) {}
    observe(target: Element) {
      queueMicrotask(() => this.cb([{ target } as ResizeObserverEntry], this as unknown as ResizeObserver))
    }
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver
  restore.push(() => (globalThis.ResizeObserver = RO))
  Object.defineProperty(navigator, 'maxTouchPoints', { configurable: true, value: 5 })
  restore.push(() => Reflect.deleteProperty(navigator, 'maxTouchPoints'))
  // React Flow reads the viewport's zoom from its computed transform; the canvas sits at zoom 1.
  const w = window as unknown as { DOMMatrixReadOnly?: unknown }
  const matrix = w.DOMMatrixReadOnly
  w.DOMMatrixReadOnly = class { m22 = 1 }
  restore.push(() => (w.DOMMatrixReadOnly = matrix))
}

beforeEach(async () => {
  layout()
  Element.prototype.setPointerCapture ??= () => {}
  el = document.createElement('div')
  document.body.append(el)
  doc = new Y.Doc()
  store = createWhiteboardStore()
  root = createRoot(el)
})

afterEach(() => {
  act(() => root.unmount())
  el.remove()
  document.body.innerHTML = ''
  for (const r of restore.splice(0).reverse()) r()
})

/** A one-finger touch event on `target` at client (x, y). */
function touch(target: EventTarget, type: string, x: number, y: number) {
  const t = { identifier: 1, clientX: x, clientY: y, pageX: x, pageY: y, screenX: x, screenY: y, target }
  const e = new Event(type, { bubbles: true, cancelable: true })
  const list = type === 'touchend' ? [] : [t]
  Object.defineProperties(e, { touches: { value: list }, targetTouches: { value: list }, changedTouches: { value: [t] } })
  target.dispatchEvent(e)
}

async function dragCorner(id: string, steps: number, dx: number, dy: number) {
  await act(async () => store.getState().set({ selection: [id] }))
  const corner = document.querySelector(`.react-flow__node[data-id="${id}"] .react-flow__resize-control.handle.bottom.right`)
  expect(corner).not.toBeNull()
  await act(async () => touch(corner!, 'touchstart', 0, 0))
  for (let i = 1; i <= steps; i++) await act(async () => touch(corner!, 'touchmove', i * dx, i * dy))
  await act(async () => touch(corner!, 'touchend', steps * dx, steps * dy))
}

it('a finger drags a sticky and a diagram far past their placement sizes; the document keeps the new sizes', async () => {
  const sticky = addCard(doc, { kind: 'sticky', title: 'Idea', x: 0, y: 0, w: 220, h: 150, source: { t: 'sticky', text: 'Apple Pay first' } })
  const diagram = addCard(doc, { kind: 'diagram', title: 'Flow', x: 400, y: 0, w: 360, h: 260, source: { t: 'diagram', lang: 'mermaid', code: 'flowchart LR\n  A --> B' } })
  await act(async () => root.render(h(Canvas, { provider: { doc } as never, store, host: HOST })))
  await act(async () => {})

  // Screen distance over the zoom React Flow fitted the board at is world distance.
  const zoom = Number(/scale\(([\d.]+)\)/.exec(document.querySelector<HTMLElement>('.react-flow__viewport')!.style.transform)![1])
  await dragCorner(sticky, 40, 40, 30)
  await dragCorner(diagram, 50, 60, 40)

  const b = readBoard(doc)
  const near = (got: number, want: number) => expect(Math.abs(got - want)).toBeLessThanOrEqual(1)
  near(b.cards[sticky]!.w, 220 + 1600 / zoom)
  near(b.cards[sticky]!.h, 150 + 1200 / zoom)
  near(b.cards[diagram]!.w, 360 + 3000 / zoom)
  near(b.cards[diagram]!.h, 260 + 2000 / zoom)
  // What the canvas draws is what the document keeps.
  const drawn = (id: string) => document.querySelector<HTMLElement>(`.react-flow__node[data-id="${id}"]`)!.style
  expect([drawn(sticky).width, drawn(sticky).height]).toEqual([`${b.cards[sticky]!.w}px`, `${b.cards[sticky]!.h}px`])
  expect([drawn(diagram).width, drawn(diagram).height]).toEqual([`${b.cards[diagram]!.w}px`, `${b.cards[diagram]!.h}px`])
})
