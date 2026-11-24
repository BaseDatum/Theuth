import '../settings/test-dom'
import { act, createElement as h } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { usePanes } from '@/lib/panes'
import { tiling, untile } from '@/lib/panes.testing'
import { type WebView, useWebViews, webLeaf } from '@/lib/web-views'
import type { Entry } from '@/store/sessions'
import { type BrowserTarget, BrowserButton, browserTargets, openTarget, targetAction } from './BrowserTargets'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => undefined) }))

const S = 'chat-1'
const A = '0123456789abcdef'
const B = 'fedcba9876543210'

const call = (id: string, url: string, title: string, callId = id): Entry => ({
  kind: 'tool',
  id: callId,
  callId,
  name: 'show_web',
  args: { url, title },
  pending: false,
  child: false,
  result: { output: `web view opened · ${title} · ${url} · tile web:${id}`, isError: false, files: [] },
})
const view = (id: string, session: string, url: string, title: string): WebView => ({ id, session, url, title, current: url, loading: false })

beforeEach(() => {
  untile()
  useWebViews.setState({ views: {}, front: null })
})

let root: Root | null = null
afterEach(() => {
  act(() => root?.unmount())
  root = null
  document.body.innerHTML = ''
})

test('none: no running browser and no show_web call', () => {
  const plain: Entry = { id: 'x', kind: 'tool', callId: 'x', name: 'show_web', args: { url: 'https://a.test' }, pending: true, child: false }
  expect(browserTargets(false, [plain], {}, S)).toEqual([])
})

test('one: the agent browser alone, or one page alone (closed: known only from its call)', () => {
  expect(browserTargets(true, [], {}, S)).toEqual([{ kind: 'agent' }])
  expect(browserTargets(false, [call(A, 'https://example.com/x', 'Example')], {}, S)).toEqual([
    { kind: 'web', id: A, url: 'https://example.com/x', title: 'Example' },
  ])
})

test('both: the agent browser first, then the page; a second call for the same frame is one target, the latest wins', () => {
  const entries = [call(A, 'https://example.com/a', 'A1', 'c1'), call(A, 'https://example.com/b', 'A2', 'c2')]
  expect(browserTargets(true, entries, {}, S)).toEqual([{ kind: 'agent' }, { kind: 'web', id: A, url: 'https://example.com/b', title: 'A2' }])
})

test('several: transcript order, the store’s live address and title win, store-only frames of this chat last, other chats ignored', () => {
  const views = {
    [B]: { ...view(B, S, 'http://localhost:3000', 'Dev'), current: 'http://localhost:3000/next', title: 'Next page' },
    other: view('other', 'chat-2', 'https://elsewhere.test', 'Elsewhere'),
    store: view('store', S, 'https://store.test', 'Store only'),
  }
  const targets = browserTargets(false, [call(A, 'https://example.com', 'Example'), call(B, 'http://localhost:3000', 'Dev')], views, S)
  expect(targets).toEqual([
    { kind: 'web', id: A, url: 'https://example.com', title: 'Example' },
    { kind: 'web', id: B, url: 'http://localhost:3000/next', title: 'Next page' },
    { kind: 'web', id: 'store', url: 'https://store.test', title: 'Store only' },
  ])
})

const page: BrowserTarget = { kind: 'web', id: A, url: 'https://example.com/x', title: 'Example' }

test('open, tiling: a closed page goes back into the pane tree right of the chat, re-recorded in the store', () => {
  tiling([S])
  const ctx = { session: S, host: S, full: false, openAgent: vi.fn(), route: vi.fn() }
  openTarget(page, ctx)
  expect(usePanes.getState().leaves).toEqual([S, webLeaf(A)])
  expect(useWebViews.getState().views[A]).toMatchObject({ session: S, url: page.url, title: 'Example' })
  expect(ctx.route).not.toHaveBeenCalled()
  openTarget({ kind: 'agent' }, ctx)
  expect(ctx.openAgent).toHaveBeenCalledOnce()
})

test('open, no tiles: a phone with native views shows the page full screen; otherwise its /web/$id route', () => {
  const route = vi.fn()
  openTarget(page, { session: S, host: null, full: true, openAgent: vi.fn(), route })
  expect(useWebViews.getState().front).toBe(A)
  expect(usePanes.getState().leaves).toEqual([])
  openTarget(page, { session: S, host: null, full: false, openAgent: vi.fn(), route })
  expect(route).toHaveBeenCalledWith(page)
})

function mount(targets: BrowserTarget[]) {
  const onOpen = vi.fn()
  const el = document.createElement('div')
  document.body.append(el)
  root = createRoot(el)
  act(() => root?.render(h(BrowserButton, { targets, busy: false, onOpen })))
  return { onOpen, button: el.querySelector('button') as HTMLButtonElement }
}
const items = () => [...document.querySelectorAll('[role="menuitem"]')]

test('one target: a click opens it, no menu; the label names it', () => {
  const { onOpen, button } = mount([page])
  expect(button.getAttribute('aria-label')).toBe('Open example.com')
  expect(targetAction({ kind: 'agent' })).toBe("Open agent's browser")
  act(() => button.click())
  expect(onOpen).toHaveBeenCalledWith(page)
  expect(items()).toEqual([])
})

test('several targets: a click lists them, picking one opens it; a right-click lists them too', () => {
  const other: BrowserTarget = { kind: 'web', id: B, url: 'http://localhost:3000', title: '' }
  const { onOpen, button } = mount([{ kind: 'agent' }, page, other])
  act(() => {
    // jsdom has no PointerEvent; Radix's trigger reads only `button` and `ctrlKey`.
    button.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true, button: 0, ctrlKey: false }))
  })
  expect(items().map((i) => i.textContent)).toEqual(["Agent's browser (live)", 'Exampleexample.com', 'localhost:3000localhost:3000'])
  expect(onOpen).not.toHaveBeenCalled()
  act(() => (items()[1] as HTMLElement).click())
  expect(onOpen).toHaveBeenCalledWith(page)
  expect(items()).toEqual([])
  act(() => {
    button.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }))
  })
  expect(items()).toHaveLength(3)
  act(() => (items()[0] as HTMLElement).click())
  expect(onOpen).toHaveBeenLastCalledWith({ kind: 'agent' })
})
