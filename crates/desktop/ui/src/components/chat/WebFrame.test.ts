import '../settings/test-dom'
import { act, createElement as h, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { SidebarProvider } from '@/components/ui/sidebar'
import { leaves, usePanes, useRoutedPane } from '@/lib/panes'
import { openPopout } from '@/lib/popout'
import { installWebViewEvents, useWebViews, webLeaf } from '@/lib/web-views'
import type { Entry } from '@/store/sessions'
import { WebFrame } from './WebFrame'
import { WebViewCard, webCall } from './WebViewCard'

const native = vi.hoisted(() => ({
  has: true,
  attach: vi.fn(async (_id: string, _url: string, _r: unknown): Promise<void> => {}),
  bounds: vi.fn(async () => {}),
  visible: vi.fn(async (_id: string, _v: boolean) => {}),
  detach: vi.fn(async () => {}),
  navigate: vi.fn(async (_id: string, _url: string) => {}),
  reload: vi.fn(async () => {}),
  history: vi.fn(async () => {}),
  close: vi.fn(async () => {}),
  onState: vi.fn(async (_cb: (s: { label: string; url: string; title: string; loading: boolean }) => void) => () => {}),
  onClosed: vi.fn(async (_cb: (s: { label: string }) => void) => () => {}),
}))
vi.mock('@/lib/web-view-native', () => ({
  nativeWebViews: () => native.has,
  webLabel: (id: string) => `web-${id}`,
  labelId: (label: string) => label.replace(/^web-/, ''),
  attach: native.attach,
  bounds: native.bounds,
  visible: native.visible,
  detach: native.detach,
  navigate: native.navigate,
  reload: native.reload,
  history: native.history,
  close: native.close,
  onState: native.onState,
  onClosed: native.onClosed,
}))
vi.mock('@/lib/popout', async (original) => ({ ...(await original<object>()), canPopout: () => true, openPopout: vi.fn(async () => {}) }))

const ID = '0123456789abcdef'
const W = webLeaf(ID)
const calls = () => Object.values(native).flatMap((f) => (typeof f === 'function' ? f.mock.calls : []))
const RECT = { left: 300, top: 52, width: 640, height: 480, right: 940, bottom: 532, x: 300, y: 52, toJSON() {} } as DOMRect

beforeEach(() => {
  native.has = true
  for (const f of Object.values(native)) if (typeof f === 'function') f.mockClear()
  native.attach.mockImplementation(async () => {})
  vi.mocked(openPopout).mockReset().mockImplementation(async () => {})
  usePanes.setState({ root: null, focused: null, drag: null })
  useWebViews.setState({ views: {} })
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue(RECT)
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
const frame = (url = 'https://example.com/') => useWebViews.getState().open({ id: ID, session: 's1', url, title: 'Example' })
const button = (host: HTMLElement, label: string) => host.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!
const type = async (input: HTMLInputElement, value: string) => {
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(input, value)
    input.dispatchEvent(new Event('input', { bubbles: true }))
  })
  await act(async () => input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })))
}

test('the frame attaches its native web view at the placeholder rectangle, detaches it on unmount; closing the frame destroys it', async () => {
  frame()
  const { host, root } = await mount(h(WebFrame, { id: ID, onClose: () => useWebViews.getState().close(ID) }))
  expect(native.attach).toHaveBeenCalledWith(ID, 'https://example.com/', { x: 300, y: 52, width: 640, height: 480 })
  expect(host.querySelector('[aria-label="Open in new window"] .lucide-app-window')).not.toBeNull()
  await act(async () => root.unmount())
  expect(native.detach).toHaveBeenCalledWith(ID)
  expect(native.close).not.toHaveBeenCalled()
  useWebViews.getState().close(ID)
  expect(native.close).toHaveBeenCalledWith(ID)
})

test('the address field navigates http(s) only; `current` moves once the navigation is accepted; other schemes mark the field', async () => {
  frame()
  const { host, root } = await mount(h(WebFrame, { id: ID }))
  const input = host.querySelector<HTMLInputElement>('input[aria-label="Address"]')!
  await type(input, 'javascript:alert(1)')
  expect(native.navigate).not.toHaveBeenCalled()
  expect(input.getAttribute('aria-invalid')).toBe('true')
  expect(useWebViews.getState().views[ID]?.current).toBe('https://example.com/')
  await type(input, 'localhost:3000')
  expect(native.navigate).toHaveBeenCalledWith(ID, 'http://localhost:3000')
  expect(input.hasAttribute('aria-invalid')).toBe(false)
  expect(useWebViews.getState().views[ID]?.current).toBe('http://localhost:3000')
  await act(async () => root.unmount())
})

test('a pane drag hides the native layer and shows the address instead; the drop shows it again', async () => {
  frame()
  const { host, root } = await mount(h(WebFrame, { id: ID }))
  await act(async () => usePanes.setState({ drag: { session: 's2', title: 'x', x: 0, y: 0, target: null } }))
  expect(native.visible).toHaveBeenLastCalledWith(ID, false)
  expect(host.querySelector('[data-web-frame]')!.textContent).toContain('https://example.com/')
  await act(async () => usePanes.setState({ drag: null }))
  expect(native.visible).toHaveBeenLastCalledWith(ID, true)
  await act(async () => root.unmount())
})

test('a toast over the frame hides the native layer; a tooltip does not', async () => {
  frame()
  const { root } = await mount(h(WebFrame, { id: ID }))
  const tip = document.body.appendChild(document.createElement('div'))
  tip.setAttribute('data-radix-popper-content-wrapper', '')
  tip.innerHTML = '<div data-slot="tooltip-content">Back</div>'
  await act(async () => new Promise<void>((r) => requestAnimationFrame(() => r())))
  expect(native.visible).not.toHaveBeenCalledWith(ID, false)
  const toast = document.body.appendChild(document.createElement('li'))
  toast.setAttribute('data-sonner-toast', '')
  await vi.waitFor(() => expect(native.visible).toHaveBeenLastCalledWith(ID, false))
  toast.remove()
  await vi.waitFor(() => expect(native.visible).toHaveBeenLastCalledWith(ID, true))
  await act(async () => root.unmount())
})

test('pop out waits for the window: a refusal leaves the tile; an open window marks the frame popped and closes its tile', async () => {
  frame()
  usePanes.getState().place('s1', W, 'right')
  const { host, root } = await mount(h(WebFrame, { id: ID }))
  vi.mocked(openPopout).mockRejectedValueOnce(new Error('no window'))
  await act(async () => button(host, 'Open in new window').click())
  expect(leaves(usePanes.getState().root!)).toEqual(['s1', W])
  expect(useWebViews.getState().views[ID]?.popped).toBeUndefined()
  await act(async () => button(host, 'Open in new window').click())
  expect(openPopout).toHaveBeenLastCalledWith({ kind: 'web', id: ID, url: 'https://example.com/', title: 'Example' })
  expect(useWebViews.getState().views[ID]?.popped).toBe(true)
  expect(usePanes.getState().root).toBeNull()
  await act(async () => root.unmount())
})

test('a view a pop-out window holds is not taken back: the refused attach focuses that window and drops the tile', async () => {
  frame()
  usePanes.getState().place('s1', W, 'right')
  native.attach.mockRejectedValueOnce('in-popout: popout-web-0123456789abcdef')
  const { root } = await mount(h(WebFrame, { id: ID }))
  await vi.waitFor(() => expect(usePanes.getState().root).toBeNull())
  expect(openPopout).toHaveBeenCalledWith({ kind: 'web', id: ID, url: 'https://example.com/', title: 'Example' })
  expect(useWebViews.getState().views[ID]?.popped).toBe(true)
  await act(async () => root.unmount())
})

test('without a native layer the frame shows the address and Open in browser, and never calls the shell', async () => {
  native.has = false
  frame('http://localhost:5173/')
  const { host, root } = await mount(h(WebFrame, { id: ID, onClose: () => useWebViews.getState().close(ID) }))
  const buttons = [...host.querySelectorAll('button')].map((b) => b.textContent)
  expect(buttons).toContain('Open in browser')
  expect(buttons).not.toContain('Load anyway')
  expect(host.textContent).toContain('http://localhost:5173/')
  await act(async () => button(host, 'Close pane').click())
  await act(async () => root.unmount())
  expect(calls()).toEqual([[ID]]) // only the store's `close`, which the native module itself turns into nothing here
})

test('the left-most web tile carries the sidebar toggle, like a chat header', async () => {
  frame()
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  await act(async () => root.render(h(SidebarProvider, { open: false }, h(WebFrame, { id: ID, first: true }))))
  expect(button(host, 'Open sidebar')).not.toBeNull()
  await act(async () => root.render(h(SidebarProvider, { open: false }, h(WebFrame, { id: ID, first: false }))))
  expect(button(host, 'Open sidebar')).toBeNull()
  await act(async () => root.unmount())
})

test('the main window follows every frame: state events patch it, a destroyed view is no longer popped', async () => {
  frame()
  useWebViews.getState().patch(ID, { popped: true })
  const off = installWebViewEvents()
  const state = native.onState.mock.calls[0]![0]
  const closed = native.onClosed.mock.calls[0]![0]
  state({ label: `web-${ID}`, url: 'https://example.com/b', title: 'B', loading: true })
  expect(useWebViews.getState().views[ID]).toMatchObject({ current: 'https://example.com/b', title: 'B', loading: true, popped: true })
  closed({ label: `web-${ID}` })
  expect(useWebViews.getState().views[ID]?.popped).toBe(false)
  off()
})

test('the URL chat closed beside a web tile is shown again beside it (a sidebar click on that URL changes nothing else)', async () => {
  usePanes.getState().place('s1', W, 'right')
  const Route = () => {
    useRoutedPane('s1')
    return null
  }
  const { root } = await mount(h(Route))
  await act(async () => void usePanes.getState().close('s1'))
  expect(leaves(usePanes.getState().root!)).toEqual(['s1', W])
  await act(async () => root.unmount())
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
const done = tool({ result: { output: RECEIPT, isError: false, files: [] } })

test('the card reads the frame from the receipt, the arguments while pending; Open puts its tile right of the session', async () => {
  expect(webCall(tool({ pending: true }))).toEqual({ id: null, url: 'localhost:3000', title: 'localhost:3000' })
  expect(webCall(done)).toEqual({ id: ID, url: 'http://localhost:3000/', title: 'Dev server' })
  const { host, root } = await mount(h(WebViewCard, { tool: done, session: 's1' }))
  const open = [...host.querySelectorAll('button')].find((b) => b.textContent === 'Open')!
  await act(async () => open.click())
  expect(leaves(usePanes.getState().root!)).toEqual(['s1', W])
  expect(useWebViews.getState().views[ID]).toMatchObject({ session: 's1', url: 'http://localhost:3000/', title: 'Dev server' })
  await act(async () => root.unmount())
})

test('after a restart (empty store) the card pop-out records the frame as popped, so a later Open focuses that window', async () => {
  const { host, root } = await mount(h(WebViewCard, { tool: done, session: 's1' }))
  await act(async () => button(host, 'Open in new window').click())
  expect(useWebViews.getState().views[ID]).toMatchObject({ session: 's1', popped: true })
  const open = [...host.querySelectorAll('button')].find((b) => b.textContent === 'Open')!
  await act(async () => open.click())
  expect(usePanes.getState().root).toBeNull()
  expect(openPopout).toHaveBeenCalledTimes(2)
  await act(async () => root.unmount())
})
