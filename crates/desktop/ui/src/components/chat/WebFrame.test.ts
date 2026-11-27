import '../settings/test-dom'
import { act, createElement as h, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { SidebarProvider } from '@/components/ui/sidebar'
import { usePanes, useRoutedPane } from '@/lib/panes'
import { tiling, untile } from '@/lib/panes.testing'
import { openPopout } from '@/lib/popout'
import { installWebViewEvents, useWebViews, webLeaf } from '@/lib/web-views'
import { IDLE, REFUSE_MS, started, useTileAgent } from '@/lib/web-view-agent'
import { AgentTileBanner } from './AgentTileBanner'
import type { Entry } from '@/store/sessions'
import { WebFrame } from './WebFrame'
import { openWebView, WebViewCard, webCall } from './WebViewCard'

/** Settles the last `viewCall` (Stop's shell `stop`). */
const shellStop = vi.hoisted(() => ({ settle: null as null | ((v: unknown) => void) }))
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
  viewCall: vi.fn((_id: string, _op: string, _args: unknown) => {
    const { promise, resolve } = Promise.withResolvers<unknown>()
    shellStop.settle = resolve
    return promise
  }),
}))
vi.mock('@/lib/web-view-native', () => ({
  nativeWebViews: () => native.has,
  webLabel: (id: string) => `web-${id}`,
  labelId: (label: string) => label.replace(/^web-/, ''),
  viewportHeight: () => 768,
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
  viewCall: native.viewCall,
}))
vi.mock('@/lib/popout', async (original) => ({ ...(await original<object>()), canPopout: () => true, openPopout: vi.fn(async () => {}) }))

const ID = '0123456789abcdef'
const W = webLeaf(ID)
const calls = (): unknown[][] => Object.values(native).flatMap((f): unknown[][] => (typeof f === 'function' ? f.mock.calls : []))
const RECT = { left: 300, top: 52, width: 640, height: 480, right: 940, bottom: 532, x: 300, y: 52, toJSON() {} } as DOMRect

beforeEach(() => {
  native.has = true
  for (const f of Object.values(native)) if (typeof f === 'function') f.mockClear()
  native.attach.mockImplementation(async () => {})
  vi.mocked(openPopout).mockReset().mockImplementation(async () => {})
  tiling(['s1'])
  useWebViews.setState({ views: {} })
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue(RECT)
})
afterEach(() => {
  vi.restoreAllMocks()
  untile()
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

test('a notice popover over the frame hides the native layer; a tooltip does not', async () => {
  frame()
  const { root } = await mount(h(WebFrame, { id: ID }))
  const tip = document.body.appendChild(document.createElement('div'))
  tip.setAttribute('data-radix-popper-content-wrapper', '')
  tip.innerHTML = '<div data-slot="tooltip-content">Back</div>'
  await act(async () => new Promise<void>((r) => requestAnimationFrame(() => r())))
  expect(native.visible).not.toHaveBeenCalledWith(ID, false)
  const toast = document.body.appendChild(document.createElement('li'))
  toast.setAttribute('data-notice-popover', '')
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
  expect(usePanes.getState().leaves).toEqual(['s1', W])
  expect(useWebViews.getState().views[ID]?.popped).toBeUndefined()
  await act(async () => button(host, 'Open in new window').click())
  expect(openPopout).toHaveBeenLastCalledWith({ kind: 'web', id: ID, url: 'https://example.com/', title: 'Example' })
  expect(useWebViews.getState().views[ID]?.popped).toBe(true)
  expect(usePanes.getState().leaves).toEqual(['s1'])
  await act(async () => root.unmount())
})

test('a view a pop-out window holds is not taken back: the refused attach focuses that window and drops the tile', async () => {
  frame()
  usePanes.getState().place('s1', W, 'right')
  native.attach.mockRejectedValueOnce('in-popout: popout-web-0123456789abcdef')
  const { root } = await mount(h(WebFrame, { id: ID }))
  await vi.waitFor(() => expect(usePanes.getState().leaves).toEqual(['s1']))
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
  // A view whose load never committed reports no URL: the field keeps the address it was asked for.
  state({ label: `web-${ID}`, url: '', title: '', loading: false })
  expect(useWebViews.getState().views[ID]).toMatchObject({ current: 'https://example.com/b', title: 'B', loading: false })
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
  expect(usePanes.getState().leaves).toEqual(['s1', W])
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
  const open = [...host.querySelectorAll('button')].find((b) => b.textContent === 'Reopen')!
  await act(async () => open.click())
  expect(usePanes.getState().leaves).toEqual(['s1', W])
  expect(useWebViews.getState().views[ID]).toMatchObject({ session: 's1', url: 'http://localhost:3000/', title: 'Dev server' })
  await act(async () => root.unmount())
})

test('after a restart (empty store) the card pop-out records the frame as popped, so a later Open focuses that window', async () => {
  const { host, root } = await mount(h(WebViewCard, { tool: done, session: 's1' }))
  await act(async () => button(host, 'Open in new window').click())
  expect(useWebViews.getState().views[ID]).toMatchObject({ session: 's1', popped: true })
  const open = [...host.querySelectorAll('button')].find((b) => b.textContent === 'Reopen')!
  await act(async () => open.click())
  expect(usePanes.getState().leaves).toEqual(['s1'])
  expect(openPopout).toHaveBeenCalledTimes(2)
  // A desktop window narrower than 768 px opens pages full screen (`full`): a popped page still focuses its window,
  // never an empty full-screen frame whose attach the shell refuses.
  openWebView('s1', { id: ID, url: 'http://localhost:3000/', title: 'Dev server' }, true)
  expect(openPopout).toHaveBeenCalledTimes(3)
  expect(useWebViews.getState().front).toBeNull()
  await act(async () => root.unmount())
})

test('an agent call on the tile shows the bar under the toolbar; Stop says Stopping until the shell stop settles, and refuses the tile for a minute', async () => {
  frame()
  useTileAgent.setState({ tiles: {}, shown: {} })
  const { host, root } = await mount(h(WebFrame, { id: ID }))
  expect(host.querySelector('[role="status"]')).toBeNull()
  await act(async () => useTileAgent.setState({ tiles: { [ID]: started(IDLE, 'c1') } }))
  const bar = host.querySelector('[role="status"][aria-label="Agent is using this page"]')!
  expect(bar.textContent).toContain('Agent is using this page')
  // Between the toolbar and the placeholder the native view is laid over.
  expect(bar.previousElementSibling?.tagName).toBe('HEADER')
  expect(bar.nextElementSibling?.hasAttribute('data-web-frame')).toBe(true)
  const before = Date.now()
  await act(async () => button(host, 'Stop the agent on this page').click())
  expect(native.viewCall).toHaveBeenCalledWith(ID, 'stop', {})
  expect(host.querySelector('[role="status"]')!.textContent).toContain('Stopping…')
  expect(button(host, 'Stop the agent on this page').disabled).toBe(true)
  expect(useTileAgent.getState().tiles[ID]?.refusedUntil).toBeGreaterThanOrEqual(before + REFUSE_MS)
  await act(async () => shellStop.settle!(null))
  expect(host.querySelector('[role="status"]')).toBeNull()
  await act(async () => root.unmount())
})

test('an agent call on a tile no frame shows (unmounted, or under a dialog) puts a banner with Stop over the window', async () => {
  frame('https://mail.example.com/inbox')
  useTileAgent.setState({ tiles: {}, shown: {} })
  const banner = () => document.querySelector('[role="status"][aria-label="Agent is using mail.example.com in a hidden tile"]')
  const app = await mount(h(AgentTileBanner))
  await act(async () => useTileAgent.setState({ tiles: { [ID]: started(IDLE, 'c1') } }))
  expect(banner()!.textContent).toContain('Agent is using mail.example.com in a hidden tile')

  // The frame on screen tells of it itself.
  const tile = await mount(h(WebFrame, { id: ID }))
  expect(banner()).toBeNull()
  // A dialog over the frame hides the native view, and the bar with it: the banner comes back.
  const dialog = document.body.appendChild(document.createElement('div'))
  await act(async () => {
    dialog.setAttribute('role', 'dialog')
    // The overlay check runs on the next animation frame after the mutation.
    for (let i = 0; i < 2; i++) await new Promise((r) => requestAnimationFrame(r))
  })
  expect(native.visible).toHaveBeenLastCalledWith(ID, false)
  expect(banner()).not.toBeNull()

  await act(async () => document.querySelector<HTMLButtonElement>('button[aria-label="Stop the agent on mail.example.com"]')!.click())
  expect(native.viewCall).toHaveBeenLastCalledWith(ID, 'stop', {})
  expect(document.querySelector('[role="status"][aria-label="Agent is using mail.example.com in a hidden tile"]')!.textContent).toContain('Stopping…')
  await act(async () => shellStop.settle!(null))
  expect(banner()).toBeNull()
  await act(async () => tile.root.unmount())
  await act(async () => app.root.unmount())
})
