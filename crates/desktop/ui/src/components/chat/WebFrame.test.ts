import '../settings/test-dom'
import { act, createElement as h, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { SidebarProvider } from '@/components/ui/sidebar'
import { leaves, usePanes } from '@/lib/panes'
import { useWebViews, webLeaf } from '@/lib/web-views'
import type { Entry } from '@/store/sessions'
import { WebFrame } from './WebFrame'
import { WebViewCard, webCall } from './WebViewCard'

const native = vi.hoisted(() => ({
  has: true,
  attach: vi.fn(async () => {}),
  bounds: vi.fn(async () => {}),
  visible: vi.fn(async () => {}),
  navigate: vi.fn(async () => {}),
  reload: vi.fn(async () => {}),
  history: vi.fn(async () => {}),
  close: vi.fn(async () => {}),
  onState: vi.fn(async () => () => {}),
}))
vi.mock('@/lib/web-view-native', () => ({
  nativeWebViews: () => native.has,
  webLabel: (id: string) => `web-${id}`,
  attach: native.attach,
  bounds: native.bounds,
  visible: native.visible,
  navigate: native.navigate,
  reload: native.reload,
  history: native.history,
  close: native.close,
  onState: native.onState,
}))
vi.mock('@/lib/popout', async (original) => ({ ...(await original<object>()), canPopout: () => true, openPopout: vi.fn(async () => {}) }))

const ID = '0123456789abcdef'
const calls = () => Object.values(native).flatMap((f) => (typeof f === 'function' ? f.mock.calls : []))

beforeEach(() => {
  native.has = true
  for (const f of Object.values(native)) if (typeof f === 'function') f.mockClear()
  usePanes.setState({ root: null, focused: null, drag: null })
  useWebViews.setState({ views: {} })
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({ left: 300, top: 52, width: 640, height: 480, right: 940, bottom: 532, x: 300, y: 52, toJSON() {} } as DOMRect)
})
afterEach(() => {
  vi.restoreAllMocks()
  document.body.innerHTML = ''
})

const mount = async (node: ReactNode) => {
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  await act(async () => root.render(h(SidebarProvider, null, node)))
  return { host, root }
}

test('the frame attaches its native web view at the placeholder rectangle, hides it on unmount; closing the frame destroys it', async () => {
  useWebViews.getState().open({ id: ID, session: 's1', url: 'https://example.com/', title: 'Example' })
  const { host, root } = await mount(h(WebFrame, { id: ID, onClose: () => useWebViews.getState().close(ID) }))
  expect(native.attach).toHaveBeenCalledWith(ID, 'https://example.com/', { x: 300, y: 52, width: 640, height: 480 })
  expect(host.querySelector('[aria-label="Open in new window"] .lucide-app-window')).not.toBeNull()
  // The address field navigates on Enter, with `http://` before a bare host.
  const input = host.querySelector<HTMLInputElement>('input[aria-label="Address"]')!
  await act(async () => {
    const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!
    set.call(input, 'localhost:3000')
    input.dispatchEvent(new Event('input', { bubbles: true }))
  })
  await act(async () => input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })))
  expect(native.navigate).toHaveBeenCalledWith(ID, 'http://localhost:3000')
  await act(async () => root.unmount())
  expect(native.visible).toHaveBeenLastCalledWith(ID, false)
  expect(native.close).not.toHaveBeenCalled()
  useWebViews.getState().close(ID)
  expect(native.close).toHaveBeenCalledWith(ID)
})

test('a pane drag hides the native layer and shows the address instead; the drop shows it again', async () => {
  useWebViews.getState().open({ id: ID, session: 's1', url: 'https://example.com/', title: 'Example' })
  const { host, root } = await mount(h(WebFrame, { id: ID }))
  await act(async () => usePanes.setState({ drag: { session: 's2', title: 'x', x: 0, y: 0, target: null } }))
  expect(native.visible).toHaveBeenLastCalledWith(ID, false)
  expect(host.querySelector('[data-web-frame]')!.textContent).toContain('https://example.com/')
  await act(async () => usePanes.setState({ drag: null }))
  expect(native.visible).toHaveBeenLastCalledWith(ID, true)
  await act(async () => root.unmount())
})

test('without a native layer the frame shows the address and Open in browser, and never calls the shell', async () => {
  native.has = false
  useWebViews.getState().open({ id: ID, session: 's1', url: 'http://localhost:5173/', title: 'Vite' })
  const { host, root } = await mount(h(WebFrame, { id: ID, onClose: () => useWebViews.getState().close(ID) }))
  const buttons = [...host.querySelectorAll('button')].map((b) => b.textContent)
  expect(buttons).toContain('Open in browser')
  expect(buttons).not.toContain('Load anyway')
  expect(host.textContent).toContain('http://localhost:5173/')
  await act(async () => host.querySelector<HTMLButtonElement>('button[aria-label="Close pane"]')!.click())
  await act(async () => root.unmount())
  expect(calls()).toEqual([[ID]]) // only the store's `close`, which the native module itself turns into nothing here
})

const tool = (over: Partial<Extract<Entry, { kind: 'tool' }>>): Extract<Entry, { kind: 'tool' }> => ({
  id: 'e1',
  kind: 'tool',
  callId: 'c1',
  name: 'show_web',
  args: { url: 'localhost:3000' },
  pending: false,
  child: false,
  ...over,
})
const RECEIPT = `web view opened · Dev server · http://localhost:3000/ · tile web:${ID}\nThe user sees and clicks this page; you do not. Read a page with browser_* or read_url.`

test('the card reads the frame from the receipt, the arguments while pending; Open puts its tile right of the session', async () => {
  expect(webCall(tool({ pending: true }))).toEqual({ id: null, url: 'localhost:3000', title: 'localhost:3000' })
  const done = tool({ result: { output: RECEIPT, isError: false, files: [] } })
  expect(webCall(done)).toEqual({ id: ID, url: 'http://localhost:3000/', title: 'Dev server' })
  const { host, root } = await mount(h(WebViewCard, { tool: done, session: 's1' }))
  const open = [...host.querySelectorAll('button')].find((b) => b.textContent === 'Open')!
  await act(async () => open.click())
  expect(leaves(usePanes.getState().root!)).toEqual(['s1', webLeaf(ID)])
  expect(useWebViews.getState().views[ID]).toMatchObject({ session: 's1', url: 'http://localhost:3000/', title: 'Dev server' })
  await act(async () => root.unmount())
})
