import '../settings/test-dom'
import { act, createElement as h, Fragment, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { SidebarProvider } from '@/components/ui/sidebar'
import { useDrafts } from '@/lib/drafts'
import { usePanes } from '@/lib/panes'
import { tiling, untile } from '@/lib/panes.testing'
import { crossLayout, webViewOpened } from '@/lib/web-arrival'
import { pillOf, useWebViews, webLeaf } from '@/lib/web-views'
import { Panes } from './Panes'
import { PhoneWebView, SWIPE_MINIMISE_PX, swipeMinimises } from './PhoneWebView'
import { WebFrame } from './WebFrame'
import { WebPill } from './WebPill'
import { openWebView } from './WebViewCard'

// The real `lib/web-view-native` on the iOS shell: what reaches `invoke` is what the shell would do.
const shell = vi.hoisted(() => ({ log: [] as string[], phone: true }))
vi.mock('@/lib/platform', async (original) => ({ ...(await original<object>()), container: () => 'ios', isDesktop: () => false }))
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string, args: { visible?: boolean; y?: number; height?: number }) => {
    shell.log.push(cmd === 'web_view_visible' ? `${cmd}:${args.visible}` : cmd === 'web_view_bounds' ? `${cmd}:${args.y}:${args.height}` : cmd)
  }),
}))
vi.mock('@tanstack/react-router', async (original) => ({ ...(await original<object>()), useNavigate: () => () => undefined }))

const ID = '0123456789abcdef'
const OTHER = 'fedcba9876543210'

// jsdom has no pointer capture: a browser's, by pointer id. A captured pointer's events go to the capturing element.
const captured = new Map<number, Element>()
Object.assign(Element.prototype, {
  setPointerCapture(this: Element, id: number) {
    captured.set(id, this)
  },
  hasPointerCapture(this: Element, id: number) {
    return captured.get(id) === this
  },
  releasePointerCapture(this: Element, id: number) {
    if (captured.get(id) === this) captured.delete(id)
  },
})

beforeEach(() => {
  shell.log = []
  shell.phone = true
  captured.clear()
  window.matchMedia = () => ({ matches: shell.phone, addEventListener() {}, removeEventListener() {} }) as unknown as MediaQueryList
  untile()
  useWebViews.setState({ views: {}, front: null })
  useDrafts.setState({ by: {} })
})
afterEach(async () => {
  vi.restoreAllMocks()
  document.body.innerHTML = ''
  // An unmount's `detach` reaches the shell a few promise turns later: let it land in its own test's log.
  const { promise, resolve } = Promise.withResolvers<void>()
  setTimeout(resolve, 0)
  await promise
})

const mount = async (node: ReactNode) => {
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  await act(async () => root.render(h(SidebarProvider, null, node)))
  return { host, root }
}
const page = (id = ID, url = 'https://example.com/', session = 's1') => useWebViews.getState().open({ id, session, url, title: 'Example' })
const button = (host: ParentNode, label: string) => host.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!
// jsdom has no `PointerEvent`; React reads a `pointer*` event's coordinates from any `MouseEvent` (no `pointerId`: equal).
// The event goes where a browser sends it: to the element holding the pointer's capture, else to `target`.
const pointer = async (type: string, target: Element, x: number, y: number) => {
  const to = captured.get(undefined as unknown as number) ?? target
  await act(async () => to.dispatchEvent(new MouseEvent(type, { bubbles: true, clientX: x, clientY: y })))
}
const swipe = async (el: Element, dx: number, dy: number) => {
  await pointer('pointerdown', el, 100, 10)
  await pointer('pointerup', el, 100 + dx, 10 + dy)
}
const rect = (y: number, height: number) => ({ left: 0, top: y, width: 390, height, right: 390, bottom: y + height, x: 0, y, toJSON() {} }) as DOMRect
const frame = () => {
  const { promise, resolve } = Promise.withResolvers<void>()
  requestAnimationFrame(() => resolve())
  return promise
}

test('minimise hides the page into the pill without destroying it; the pill brings the same view back, not navigated', async () => {
  page()
  useWebViews.getState().show(ID)
  const { host, root } = await mount(h(Fragment, null, h(PhoneWebView), h(WebPill, { session: 's1' })))
  await vi.waitFor(() => expect(shell.log).toEqual(['web_view_attach']))
  expect(host.querySelector('[data-web-pill]')).toBeNull()
  for (const label of ['Back', 'Forward', 'Reload', 'Open in Safari', 'Minimise', 'Close']) expect(button(host, label)).not.toBeNull()

  await act(async () => button(host, 'Minimise').click())
  await vi.waitFor(() => expect(shell.log).toEqual(['web_view_attach', 'web_view_visible:false']))
  expect(host.querySelector('[data-phone-web]')).toBeNull()
  expect(useWebViews.getState().views[ID]).toMatchObject({ minimised: true })

  await act(async () => button(host, 'Show Example').click())
  await vi.waitFor(() => expect(shell.log).toEqual(['web_view_attach', 'web_view_visible:false', 'web_view_attach']))
  expect(useWebViews.getState().front).toBe(ID)
  expect(host.querySelector('[data-web-pill]')).toBeNull()
  // Never destroyed, never reloaded: the shell keeps the page, its scroll, its form input and its history.
  expect(shell.log).not.toContain('web_view_close')
  expect(shell.log).not.toContain('web_view_navigate')
  await act(async () => root.unmount())
})

test('the toolbar Close destroys the page: web_view_close, the frame forgotten, no pill', async () => {
  page()
  useWebViews.getState().show(ID)
  const { host, root } = await mount(h(Fragment, null, h(PhoneWebView), h(WebPill, { session: 's1' })))
  await vi.waitFor(() => expect(shell.log).toEqual(['web_view_attach']))
  await act(async () => button(host, 'Close').click())
  await vi.waitFor(() => expect(shell.log).toContain('web_view_close'))
  expect(useWebViews.getState()).toMatchObject({ front: null, views: {} })
  expect(host.querySelector('[data-phone-web]')).toBeNull()
  expect(host.querySelector('[data-web-pill]')).toBeNull()
  await act(async () => root.unmount())
})

test('the page clears the home indicator: the full-screen frame is inset by the bottom safe area', async () => {
  page()
  useWebViews.getState().show(ID)
  const { host, root } = await mount(h(PhoneWebView))
  // Top: the toolbar sits under `--safe-top`; bottom: the placeholder (and the native view over it) ends above `--safe-bottom`.
  expect(host.querySelector('[data-phone-web]')!.className.split(' ')).toEqual(expect.arrayContaining(['pt-[var(--safe-top)]', 'pb-[var(--safe-bottom)]']))
  await act(async () => root.unmount())
})

test('a downward swipe of at least 60 px on the toolbar minimises; a shorter or sideways one does not', async () => {
  expect(SWIPE_MINIMISE_PX).toBe(60)
  expect(swipeMinimises(0, 59)).toBe(false)
  expect(swipeMinimises(0, 60)).toBe(true)
  expect(swipeMinimises(-30, 90)).toBe(true)
  expect(swipeMinimises(100, 70)).toBe(false)
  expect(swipeMinimises(0, -80)).toBe(false)
  page()
  useWebViews.getState().show(ID)
  const { host, root } = await mount(h(PhoneWebView))
  const toolbar = host.querySelector('[data-phone-web] header')!
  await swipe(toolbar, 0, SWIPE_MINIMISE_PX - 1)
  await swipe(toolbar, 100, 70)
  expect(useWebViews.getState().front).toBe(ID)
  await swipe(toolbar, 0, SWIPE_MINIMISE_PX)
  expect(useWebViews.getState().front).toBeNull()
  expect(useWebViews.getState().views[ID]?.minimised).toBe(true)
  await vi.waitFor(() => expect(shell.log).toContain('web_view_visible:false'))
  expect(shell.log).not.toContain('web_view_close')
  await act(async () => root.unmount())
})

test('a swipe whose pointer leaves the 52 px toolbar still minimises: the toolbar captures the pointer once it moves, not on a tap', async () => {
  page()
  useWebViews.getState().show(ID)
  const { host, root } = await mount(h(PhoneWebView))
  const toolbar = host.querySelector('[data-phone-web] header')!
  const placeholder = host.querySelector('[data-web-frame]')!
  // A tap on a button takes no capture: its click stays the button's.
  await pointer('pointerdown', button(host, 'Back'), 100, 10)
  await pointer('pointerup', button(host, 'Back'), 100, 10)
  expect(captured.size).toBe(0)
  // A mouse or trackpad has no implicit capture: past the toolbar its events go to the page under it.
  await pointer('pointerdown', toolbar, 100, 10)
  await pointer('pointermove', toolbar, 100, 30)
  await pointer('pointermove', placeholder, 100, 60)
  await pointer('pointerup', placeholder, 100, 10 + SWIPE_MINIMISE_PX)
  expect(useWebViews.getState().front).toBeNull()
  expect(useWebViews.getState().views[ID]?.minimised).toBe(true)
  expect(captured.size).toBe(0)
  await act(async () => root.unmount())
})

test('a sheet or dialog over the page hides the view; once it closes the view is framed to where the placeholder is now, then shown', async () => {
  let box = rect(108, 700)
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    if (this.hasAttribute('data-web-frame')) return box
    return this.getAttribute('role') === 'dialog' ? rect(0, 844) : rect(0, 0)
  })
  page()
  useWebViews.getState().show(ID)
  const { root } = await mount(h(PhoneWebView))
  await vi.waitFor(() => expect(shell.log).toEqual(['web_view_attach']))
  const dialog = document.body.appendChild(document.createElement('div'))
  dialog.setAttribute('role', 'dialog')
  await vi.waitFor(() => expect(shell.log).toEqual(['web_view_attach', 'web_view_visible:false']))
  // The keyboard (or a rotation) moved the placeholder while the view was hidden.
  box = rect(108, 400)
  await act(async () => frame())
  dialog.remove()
  await vi.waitFor(() => expect(shell.log).toEqual(['web_view_attach', 'web_view_visible:false', 'web_view_bounds:108:400', 'web_view_visible:true']))
  await act(async () => root.unmount())
})

test('a visual viewport resize or scroll, or a window scroll, frames the view again even when the placeholder did not move', async () => {
  // Any jsdom target stands in for `window.visualViewport` (Node's own `EventTarget` refuses jsdom's events).
  const viewport = document.createElement('div')
  Object.defineProperty(window, 'visualViewport', { value: viewport, configurable: true })
  try {
    page()
    useWebViews.getState().show(ID)
    const { root } = await mount(h(PhoneWebView))
    await vi.waitFor(() => expect(shell.log).toEqual(['web_view_attach']))
    viewport.dispatchEvent(new Event('scroll'))
    await vi.waitFor(() => expect(shell.log).toEqual(['web_view_attach', 'web_view_bounds:0:0']))
    viewport.dispatchEvent(new Event('resize'))
    await vi.waitFor(() => expect(shell.log.filter((c) => c.startsWith('web_view_bounds'))).toHaveLength(2))
    window.dispatchEvent(new Event('scroll'))
    await vi.waitFor(() => expect(shell.log.filter((c) => c.startsWith('web_view_bounds'))).toHaveLength(3))
    // Minimised: nothing to frame.
    await act(async () => useWebViews.getState().minimise(ID))
    window.dispatchEvent(new Event('scroll'))
    await act(async () => frame())
    expect(shell.log.filter((c) => c.startsWith('web_view_bounds'))).toHaveLength(3)
    await act(async () => root.unmount())
  } finally {
    Reflect.deleteProperty(window, 'visualViewport')
  }
})

test('a route to another chat puts the full-screen page into the pill; the chat under a page is inert', async () => {
  const panes = (session: string) => h(SidebarProvider, null, h(Panes, { session, kinds: [], render: (id: string) => h('div', { 'data-body': id }, h('button', { type: 'button' }, 'Header')) }))
  page()
  useWebViews.getState().show(ID)
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  await act(async () => root.render(panes('s1')))
  expect(useWebViews.getState().front).toBe(ID)
  expect(host.querySelector('[data-phone-web]')).not.toBeNull()
  expect(host.querySelector('[data-pane="s1"]')!.hasAttribute('inert')).toBe(true)

  // A notification, a deep link, or the agent showing another chat's page.
  await act(async () => root.render(panes('s2')))
  expect(useWebViews.getState().front).toBeNull()
  expect(useWebViews.getState().views[ID]?.minimised).toBe(true)
  expect(host.querySelector('[data-phone-web]')).toBeNull()
  expect(host.querySelector('[data-pane="s2"]')!.hasAttribute('inert')).toBe(false)

  // Settings and back into another chat: the panes mount again, the page does not reappear over it.
  await act(async () => root.render(panes('s1')))
  await act(async () => useWebViews.getState().show(ID))
  await act(async () => root.render(null))
  await act(async () => root.render(panes('s2')))
  expect(useWebViews.getState().front).toBeNull()
  // The page's own chat keeps it full screen.
  await act(async () => root.render(panes('s1')))
  await act(async () => useWebViews.getState().show(ID))
  await act(async () => root.render(panes('s1')))
  expect(useWebViews.getState().front).toBe(ID)
  expect(shell.log).not.toContain('web_view_close')
  await act(async () => root.unmount())
})

test('show_web on a phone while the composer has focus or holds a draft arrives as the pill, never over the screen; otherwise full screen', () => {
  const composer = document.body.appendChild(document.createElement('div'))
  composer.setAttribute('data-composer', '')
  const box = composer.appendChild(document.createElement('textarea'))
  box.focus()
  webViewOpened({ id: ID, session: 's1', url: 'https://example.com/', title: 'Example' }, true)
  expect(useWebViews.getState().front).toBeNull()
  expect(pillOf(useWebViews.getState().views, 's1')?.latest.id).toBe(ID)

  box.blur()
  useDrafts.setState({ by: { s1: { text: 'half a sentence', pastes: new Map() } } })
  webViewOpened({ id: OTHER, session: 's1', url: 'https://example.org/', title: 'Other' }, true)
  expect(useWebViews.getState().front).toBeNull()
  expect(pillOf(useWebViews.getState().views, 's1')).toMatchObject({ latest: { id: OTHER }, count: 2 })

  useDrafts.setState({ by: {} })
  webViewOpened({ id: ID, session: 's1', url: 'https://example.com/next', title: 'Next' }, true)
  expect(useWebViews.getState().front).toBe(ID)
  // A chat not on screen never gets a page over it.
  webViewOpened({ id: OTHER, session: 's2', url: 'https://example.org/', title: 'Other' }, false)
  expect(useWebViews.getState().front).toBe(ID)
  expect(useWebViews.getState().views[OTHER]?.minimised).toBe(true)
  expect(usePanes.getState().leaves).toEqual([])
})

test("show_web of a remote engine's loopback address arrives as the card on a phone: no full-screen placeholder, no pill", () => {
  webViewOpened({ id: ID, session: 's1', url: 'http://localhost:3000/', title: 'Dev' }, true)
  expect(useWebViews.getState().front).toBeNull()
  expect(useWebViews.getState().views[ID]).toMatchObject({ url: 'http://localhost:3000/' })
  expect(useWebViews.getState().views[ID]?.minimised).toBeUndefined()
  expect(pillOf(useWebViews.getState().views, 's1')).toBeNull()
  expect(shell.log).toEqual([])
})

test("the card's Open on a phone drops the keyboard first, as the pill does", () => {
  const box = document.body.appendChild(document.createElement('textarea'))
  box.focus()
  expect(document.activeElement).toBe(box)
  openWebView('s1', { id: ID, url: 'https://example.com/', title: 'Example' }, true)
  expect(useWebViews.getState().front).toBe(ID)
  expect(document.activeElement).not.toBe(box)
})

test("the pill shows the latest of its chat's pages and a count; the count lists them and any restores; another chat's page is not there", async () => {
  page(ID, 'https://example.com/')
  page(OTHER, 'https://example.org/')
  page('aaaaaaaaaaaaaaaa', 'https://elsewhere.example/', 's2')
  useWebViews.getState().patch(OTHER, { title: 'Other' })
  useWebViews.getState().minimise(ID)
  useWebViews.getState().minimise(OTHER)
  useWebViews.getState().minimise('aaaaaaaaaaaaaaaa')
  const { host, root } = await mount(h(WebPill, { session: 's1' }))
  expect(button(host, 'Show Other')).not.toBeNull()
  expect(host.querySelector('[data-web-pill] img')?.getAttribute('src')).toBe('https://example.org/favicon.ico')
  await act(async () => button(host, '2 web pages').click())
  const items = [...document.querySelectorAll('ul[aria-label="Web pages"] button')]
  expect(items.map((b) => b.textContent)).toEqual(['Other', 'Example'])
  await act(async () => (items[1] as HTMLButtonElement).click())
  expect(useWebViews.getState().front).toBe(ID)
  expect(pillOf(useWebViews.getState().views, 's1')).toMatchObject({ latest: { id: OTHER }, count: 1 })
  expect(button(host, '2 web pages')).toBeNull()
  // A favicon that fails gives way to the globe; nothing else is fetched.
  await act(async () => host.querySelector('[data-web-pill] img')!.dispatchEvent(new Event('error')))
  expect(host.querySelector('[data-web-pill] img')).toBeNull()
  expect(host.querySelector('[data-web-pill] .lucide-globe')).not.toBeNull()
  await act(async () => root.unmount())
  // The other chat's pill has its own page.
  const other = await mount(h(WebPill, { session: 's2' }))
  expect(button(other.host, 'Show Example')).not.toBeNull()
  expect(button(other.host, '2 web pages')).toBeNull()
  await act(async () => other.root.unmount())
})

test('a tablet keeps the tile: show_web places it beside the chat, the frame attaches in its pane, nothing goes full screen or into the pill', async () => {
  shell.phone = false
  tiling(['s1'])
  webViewOpened({ id: ID, session: 's1', url: 'https://example.com/', title: 'Example' }, true)
  expect(usePanes.getState().leaves).toEqual(['s1', webLeaf(ID)])
  expect(useWebViews.getState().front).toBeNull()
  const { host, root } = await mount(h(Fragment, null, h(WebFrame, { id: ID }), h(WebPill, { session: 's1' })))
  await vi.waitFor(() => expect(shell.log).toEqual(['web_view_attach']))
  expect(button(host, 'Open in browser')).not.toBeNull()
  expect(useWebViews.getState().views[ID]?.minimised).toBeUndefined()
  expect(host.querySelector('[data-web-pill]')).toBeNull()
  await act(async () => root.unmount())
})

test('crossing into the phone layout puts web tiles into the pill; widening again makes the full-screen page a tile beside its chat', () => {
  page()
  tiling(['s1'])
  usePanes.getState().place('s1', webLeaf(ID), 'right')
  crossLayout(true, 's1')
  expect(useWebViews.getState().views[ID]?.minimised).toBe(true)
  expect(usePanes.getState().leaves).toEqual(['s1', webLeaf(ID)])
  // Back on a layout without the tile (its pane went with the phone layout's own).
  tiling(['s1'])
  useWebViews.getState().show(ID)
  crossLayout(false, 's1')
  expect(useWebViews.getState().front).toBeNull()
  expect(usePanes.getState().leaves).toEqual(['s1', webLeaf(ID)])
  // The page of a chat that is not on screen is left for its card: not minimised, since no pill shows off a phone.
  useWebViews.getState().open({ id: OTHER, session: 's9', url: 'https://example.org/', title: 'Other' })
  useWebViews.getState().show(OTHER)
  crossLayout(false, 's1')
  expect(useWebViews.getState().front).toBeNull()
  expect(useWebViews.getState().views[OTHER]?.minimised).toBe(false)
  expect(usePanes.getState().leaves).not.toContain(webLeaf(OTHER))
})
