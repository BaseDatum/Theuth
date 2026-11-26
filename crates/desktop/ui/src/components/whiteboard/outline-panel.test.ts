// Owner request: on desktop and iPad (≥ 768 px) the outline is a side panel docked at the whiteboard's left, beside a
// live canvas (not a full view over a hidden one): a row selects its item on the canvas and brings it into view, the
// pointer on a row highlights it there, and the canvas selection is marked in the panel. Phones keep the full view.
import '../settings/test-dom'
import { QueryClientProvider, QueryClient } from '@tanstack/react-query'
import { act, createElement as h } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import * as Y from 'yjs'
import { TooltipProvider } from '@/components/ui/tooltip'
import type * as Rpc from '@/lib/rpc'
import { useConnection } from '@/store/connection'
import { addCard } from './canvas/ops'
import { WhiteboardShell } from './WhiteboardView'

const rpc = vi.hoisted(() => ({ api: {} as Record<string, unknown> }))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => rpc.api }))
vi.mock('@tanstack/react-router', async (original) => ({ ...(await original<object>()), useNavigate: () => () => undefined, useRouterState: () => '/' }))

window.HTMLElement.prototype.scrollIntoView = () => {}
Element.prototype.setPointerCapture ??= () => {}

let root: Root
let el: HTMLDivElement
let phone = false
const MATCH_MEDIA = window.matchMedia
let ids: { near: string; far: string }

beforeEach(() => {
  phone = false
  // `useIsMobile` reads `(max-width: 767px)`; reduced motion makes every pan and zoom immediate.
  window.matchMedia = ((q: string) => ({ matches: q.includes('max-width') ? phone : q.includes('reduced-motion'), addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {} })) as never
  rpc.api = new Proxy({}, { get: (_, name: string) => async () => (name === 'load_session' ? new Promise(() => {}) : name.startsWith('list_') ? [] : null) })
  useConnection.setState({
    client: { call: async (m: string) => (m === 'whiteboard_sessions' ? [] : null) } as never,
    discovery: { ...useConnection.getState().discovery, phase: 'legacy' },
    info: { favorite_models: [], default_model: 'm/one' } as never,
  })
})

afterEach(() => {
  act(() => root.unmount())
  el.remove()
  document.body.innerHTML = ''
  window.matchMedia = MATCH_MEDIA
})

async function mount() {
  el = document.createElement('div')
  document.body.append(el)
  const doc = new Y.Doc()
  ids = {
    near: addCard(doc, { kind: 'sticky', title: 'Near', x: 0, y: 0, w: 220, h: 150, source: { t: 'sticky', text: 'a' } }),
    far: addCard(doc, { kind: 'sticky', title: 'Far away', x: 3000, y: 2000, w: 220, h: 150, source: { t: 'sticky', text: 'b' } }),
  }
  const provider = { doc, meta: { title: 'Plan', kind: 'board' }, onStatus: () => () => {} }
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  root = createRoot(el)
  await act(async () => root.render(h(QueryClientProvider, { client: qc }, h(TooltipProvider, null, h(WhiteboardShell, { doc: 'wb-doc-1', provider: provider as never, frame: 'relative size-full' })))))
  await vi.waitFor(() => expect(el.querySelector('.react-flow__pane')).not.toBeNull())
}

const q = <T extends Element = HTMLElement>(sel: string) => document.querySelector<T>(sel)
const panel = () => q('aside[data-outline-panel]')
const row = (id: string) => q<HTMLButtonElement>(`aside [data-outline-id="${id}"]`)
/** The element that hides the canvas under the phone's full outline. */
const canvasLayer = () => q('.react-flow__pane')!.closest('[aria-hidden]')
const key = (k: string, init: KeyboardEventInit = {}, target: EventTarget = window) => act(async () => void target.dispatchEvent(new KeyboardEvent('keydown', { key: k, bubbles: true, cancelable: true, ...init })))
const viewport = () => q('.react-flow__viewport')!.style.transform
const node = (id: string) => q(`.react-flow__node[data-id="${id}"] .wb-card`)!

describe('the outline on desktop and iPad: a side panel beside the live canvas', () => {
  it('opens with O at the left; the canvas stays visible and interactive beside it, and O closes it', async () => {
    await mount()
    expect(panel()).toBeNull()
    await key('o')
    expect(panel()).not.toBeNull()
    expect(panel()!.style.width).toBe('300px')
    expect(row(ids.near)).not.toBeNull()
    expect(canvasLayer()?.getAttribute('aria-hidden')).toBe('false')
    expect(canvasLayer()!.className).not.toMatch(/invisible|opacity-0|pointer-events-none/)
    // The canvas sits beside the panel, never under it.
    expect(q<HTMLElement>('[data-canvas-region]')!.style.left).toBe('300px')
    await key('o')
    expect(panel()).toBeNull()
    expect(q<HTMLElement>('[data-canvas-region]')!.style.left).toBe('0px')
  })

  it('a row selects its card on the canvas and brings it into view', async () => {
    await mount()
    await key('o')
    const before = viewport()
    await act(async () => row(ids.far)!.click())
    await vi.waitFor(() => expect(node(ids.far).classList.contains('selected')).toBe(true))
    expect(node(ids.near).classList.contains('selected')).toBe(false)
    expect(viewport()).not.toBe(before)
    expect(row(ids.far)!.getAttribute('aria-current')).toBe('true')
  })

  it('the pointer on a row highlights its card on the canvas; leaving clears it', async () => {
    await mount()
    await key('o')
    const li = row(ids.near)!.closest('li')!
    await act(async () => void li.dispatchEvent(new MouseEvent('pointerover', { bubbles: true, relatedTarget: null })))
    expect(node(ids.near).classList.contains('hover')).toBe(true)
    await act(async () => void li.dispatchEvent(new MouseEvent('pointerout', { bubbles: true, relatedTarget: document.body })))
    expect(node(ids.near).classList.contains('hover')).toBe(false)
  })

  it('a card selected on the canvas is marked in the panel', async () => {
    await mount()
    await key('o')
    expect(row(ids.near)!.getAttribute('aria-current')).toBeNull()
    await act(async () => q(`.react-flow__node[data-id="${ids.near}"]`)!.click())
    await vi.waitFor(() => expect(row(ids.near)!.getAttribute('aria-current')).toBe('true'))
    expect(row(ids.far)!.getAttribute('aria-current')).toBeNull()
  })

  it('Esc with the focus in the panel closes it; Present hides it', async () => {
    await mount()
    await key('o')
    row(ids.near)!.focus()
    await key('Escape', {}, row(ids.near)!)
    expect(panel()).toBeNull()
    await key('o')
    expect(panel()).not.toBeNull()
    await key('P', { shiftKey: true })
    expect(panel()).toBeNull()
  })
})

describe('the outline on a phone', () => {
  it('is the full default view, the canvas hidden under it, and no side panel', async () => {
    phone = true
    await mount()
    expect(panel()).toBeNull()
    expect(q('[aria-label="Whiteboard outline: Plan"], [aria-label^="Whiteboard outline"]')).not.toBeNull()
    expect(canvasLayer()?.getAttribute('aria-hidden')).toBe('true')
    expect(canvasLayer()!.className).toMatch(/invisible/)
  })
})
