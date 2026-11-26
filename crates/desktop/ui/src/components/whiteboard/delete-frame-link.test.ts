// Owner report: a selected frame (an agent's `wb_frame` outline too) or link would not go on Delete/Backspace. A link
// click never selected it (its edges were controlled without `onEdgesChange`, so React Flow's select change was lost),
// and a whiteboard tile ignored every canvas key. The real canvas: click the frame's band or the link, press the key.
import '../settings/test-dom'
import { act, createElement as h } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import * as Y from 'yjs'
import type { CanvasHost } from './canvas/api'
import Canvas from './canvas/Canvas'
import { ENGINE, readBoard } from './canvas/model'
import { addCard, addFrame, addLink } from './canvas/ops'
import { createWhiteboardStore, type WbStore } from './canvas/store'

const HOST: CanvasHost = { doc: 'd', blobUrl: async () => null, blobPut: async () => {}, capture: async () => null, describePoint: async () => null, liveBudget: 8 }

let root: Root
let el: HTMLDivElement
let doc: Y.Doc
let store: WbStore
const restore: (() => void)[] = []
let ids: { a: string; b: string; frame: string; link: string }

/** jsdom lays nothing out: an element is as big as its inline style says (React Flow then draws the edges). */
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
  const w = window as unknown as { DOMMatrixReadOnly?: unknown }
  const matrix = w.DOMMatrixReadOnly
  w.DOMMatrixReadOnly = class { m22 = 1 }
  restore.push(() => (w.DOMMatrixReadOnly = matrix))
}

async function mount(embedded = false) {
  await act(async () => root.render(h(Canvas, { provider: { doc } as never, store, host: HOST, embedded })))
  await act(async () => {}) // the measured nodes land
}

beforeEach(() => {
  layout()
  Element.prototype.setPointerCapture ??= () => {}
  el = document.createElement('div')
  document.body.append(el)
  // The agent placed the frame and the link (as the provider applies engine writes); the owner deletes them.
  const engine = new Y.Doc()
  const a = addCard(engine, { kind: 'sticky', title: 'a', x: 40, y: 60, w: 200, h: 140, source: { t: 'sticky', text: 'a' } })
  const b = addCard(engine, { kind: 'sticky', title: 'b', x: 600, y: 60, w: 200, h: 140, source: { t: 'sticky', text: 'b' } })
  const frame = addFrame(engine, { title: 'Agent frame', x: 0, y: 0, w: 400, h: 300, cards: [a] })
  const link = addLink(engine, { from: a, to: b })
  engine.transact(() => {
    engine.getMap<Y.Map<unknown>>('frames').get(frame)!.set('by', 'session:s1')
    engine.getMap<Y.Map<unknown>>('links').get(link)!.set('by', 'session:s1')
  })
  doc = new Y.Doc()
  Y.applyUpdate(doc, Y.encodeStateAsUpdate(engine), ENGINE)
  ids = { a, b, frame, link }
  store = createWhiteboardStore()
  root = createRoot(el)
})

afterEach(() => {
  act(() => root.unmount())
  el.remove()
  document.body.innerHTML = ''
  for (const r of restore.splice(0).reverse()) r()
})

/** A mouse click as the browser sends it; the press focuses the nearest focusable ancestor (the node or edge). */
async function click(target: Element) {
  await act(async () => {
    for (const type of ['pointerdown', 'mousedown']) target.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true, button: 0, view: window }))
    target.closest<HTMLElement | SVGElement>('[tabindex]')?.focus()
    for (const type of ['pointerup', 'mouseup', 'click']) target.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true, button: 0, view: window }))
  })
}

async function key(k: string, extra: KeyboardEventInit = {}) {
  await act(async () => (document.activeElement ?? document.body).dispatchEvent(new KeyboardEvent('keydown', { key: k, bubbles: true, cancelable: true, ...extra })))
}

const band = () => document.querySelector(`.react-flow__node[data-id="${ids.frame}"] .wb-frame-band`)!
const linkHit = () => document.querySelector(`.react-flow__edge[data-id="${ids.link}"] .react-flow__edge-interaction`)

describe('Delete on a selected frame or link', () => {
  it("Delete removes the agent's frame (its card stays, unframed); ⌘Z puts it back", async () => {
    await mount()
    await click(band())
    expect(store.getState().selection).toEqual([ids.frame])
    await key('Delete')
    let b = readBoard(doc)
    expect(b.frames[ids.frame]).toBeUndefined()
    expect(b.cards[ids.a]).toMatchObject({ state: 'live' })
    expect(b.cards[ids.a]!.frame).toBeUndefined()
    expect(store.getState().selection).toEqual([])
    await key('z', { metaKey: true })
    b = readBoard(doc)
    expect(b.frames[ids.frame]).toMatchObject({ by: 'session:s1' })
    expect(b.cards[ids.a]!.frame).toBe(ids.frame)
  })

  it("a click selects the agent's link (its toolbar shows); Backspace removes it", async () => {
    await mount()
    expect(linkHit()).not.toBeNull()
    await click(linkHit()!)
    expect(store.getState().selection).toEqual([ids.link])
    expect(document.querySelector('[aria-label="Link actions"]')).not.toBeNull()
    await key('Backspace')
    expect(readBoard(doc).links[ids.link]).toBeUndefined()
    await key('z', { metaKey: true })
    expect(readBoard(doc).links[ids.link]).toBeDefined()
  })

  it('a mixed selection (card, frame, link) goes in one Delete and comes back in one ⌘Z', async () => {
    await mount()
    await act(async () => store.getState().set({ selection: [ids.b, ids.frame, ids.link] }))
    expect(document.querySelector('[aria-label="Selection"]')?.textContent).toContain('1 card · 1 frame · 1 link')
    await key('Delete')
    let b = readBoard(doc)
    expect([b.frames[ids.frame], b.links[ids.link], b.cards[ids.b]!.state]).toEqual([undefined, undefined, 'archived'])
    await key('z', { metaKey: true })
    b = readBoard(doc)
    expect(b.frames[ids.frame]).toBeDefined()
    expect(b.links[ids.link]).toBeDefined()
    expect(b.cards[ids.b]!.state).toBe('live')
    expect(b.cards[ids.a]!.frame).toBe(ids.frame)
  })

  it('the selection bar deletes frames and links without a keyboard', async () => {
    await mount()
    await act(async () => store.getState().set({ selection: [ids.frame, ids.link] }))
    await act(async () => document.querySelector<HTMLButtonElement>('button[aria-label="Delete selection"]')!.click())
    const b = readBoard(doc)
    expect(b.frames[ids.frame]).toBeUndefined()
    expect(b.links[ids.link]).toBeUndefined()
  })

  it('no delete while typing, read-only or presenting', async () => {
    await mount()
    await act(async () => store.getState().set({ selection: [ids.frame] }))
    const input = document.createElement('input')
    document.body.append(input)
    input.focus()
    await key('Backspace')
    input.remove()
    await act(async () => store.getState().set({ present: true }))
    await key('Delete')
    expect(readBoard(doc).frames[ids.frame]).toBeDefined()
    await act(async () => store.getState().set({ present: false }))
    await act(async () => root.render(h(Canvas, { provider: { doc } as never, store, host: HOST, readOnly: true })))
    await key('Delete')
    expect(readBoard(doc).frames[ids.frame]).toBeDefined()
  })

  it('a tile (embedded) deletes only while the focus is on its canvas', async () => {
    await mount(true)
    await act(async () => store.getState().set({ selection: [ids.frame] }))
    ;(document.activeElement as HTMLElement | null)?.blur()
    await key('Delete')
    expect(readBoard(doc).frames[ids.frame]).toBeDefined()
    await click(band())
    await key('Delete')
    expect(readBoard(doc).frames[ids.frame]).toBeUndefined()
  })
})
