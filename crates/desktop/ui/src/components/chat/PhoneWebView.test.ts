import '../settings/test-dom'
import { act, createElement as h, Fragment, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { SidebarProvider } from '@/components/ui/sidebar'
import { useDrafts } from '@/lib/drafts'
import { leaves, usePanes } from '@/lib/panes'
import { crossLayout, webViewOpened } from '@/lib/web-arrival'
import { pillOf, useWebViews, webLeaf } from '@/lib/web-views'
import { PhoneWebView, SWIPE_MINIMISE_PX, swipeMinimises } from './PhoneWebView'
import { WebFrame } from './WebFrame'
import { WebPill } from './WebPill'

// The real `lib/web-view-native` on the iOS shell: what reaches `invoke` is what the shell would do.
const shell = vi.hoisted(() => ({ log: [] as string[], phone: true }))
vi.mock('@/lib/platform', async (original) => ({ ...(await original<object>()), container: () => 'ios', isDesktop: () => false }))
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string, args: { visible?: boolean }) => {
    shell.log.push(cmd === 'web_view_visible' ? `${cmd}:${args.visible}` : cmd)
  }),
}))

const ID = '0123456789abcdef'
const OTHER = 'fedcba9876543210'

beforeEach(() => {
  shell.log = []
  shell.phone = true
  window.matchMedia = () => ({ matches: shell.phone, addEventListener() {}, removeEventListener() {} }) as unknown as MediaQueryList
  usePanes.setState({ root: null, focused: null, drag: null })
  useWebViews.setState({ views: {}, front: null })
  useDrafts.setState({ by: {} })
})
afterEach(() => {
  document.body.innerHTML = ''
})

const mount = async (node: ReactNode) => {
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  await act(async () => root.render(h(SidebarProvider, null, node)))
  return { host, root }
}
const page = (id = ID, url = 'https://example.com/') => useWebViews.getState().open({ id, session: 's1', url, title: 'Example' })
const button = (host: HTMLElement, label: string) => host.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!
// jsdom has no `PointerEvent`; React reads a `pointer*` event's coordinates from any `MouseEvent` (no `pointerId`: equal).
const swipe = async (el: Element, dx: number, dy: number) => {
  await act(async () => el.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true, clientX: 100, clientY: 10 })))
  await act(async () => el.dispatchEvent(new MouseEvent('pointerup', { bubbles: true, clientX: 100 + dx, clientY: 10 + dy })))
}

test('minimise hides the page into the pill without destroying it; the pill brings the same view back, not navigated', async () => {
  page()
  useWebViews.getState().show(ID)
  const { host, root } = await mount(h(Fragment, null, h(PhoneWebView), h(WebPill)))
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

test('show_web on a phone while the composer has focus or holds a draft arrives as the pill, never over the screen; otherwise full screen', () => {
  const composer = document.body.appendChild(document.createElement('div'))
  composer.setAttribute('data-composer', '')
  const box = composer.appendChild(document.createElement('textarea'))
  box.focus()
  webViewOpened({ id: ID, session: 's1', url: 'https://example.com/', title: 'Example' }, true)
  expect(useWebViews.getState().front).toBeNull()
  expect(pillOf(useWebViews.getState().views)?.latest.id).toBe(ID)

  box.blur()
  useDrafts.setState({ by: { s1: { text: 'half a sentence', pastes: new Map() } } })
  webViewOpened({ id: OTHER, session: 's1', url: 'https://example.org/', title: 'Other' }, true)
  expect(useWebViews.getState().front).toBeNull()
  expect(pillOf(useWebViews.getState().views)).toMatchObject({ latest: { id: OTHER }, count: 2 })

  useDrafts.setState({ by: {} })
  webViewOpened({ id: ID, session: 's1', url: 'https://example.com/next', title: 'Next' }, true)
  expect(useWebViews.getState().front).toBe(ID)
  // A chat not on screen never gets a page over it.
  webViewOpened({ id: OTHER, session: 's2', url: 'https://example.org/', title: 'Other' }, false)
  expect(useWebViews.getState().front).toBe(ID)
  expect(useWebViews.getState().views[OTHER]?.minimised).toBe(true)
  expect(usePanes.getState().root).toBeNull()
})

test('the pill shows the latest page and a count; the count lists every page and any of them restores', async () => {
  page(ID, 'https://example.com/')
  page(OTHER, 'https://example.org/')
  useWebViews.getState().patch(OTHER, { title: 'Other' })
  useWebViews.getState().minimise(ID)
  useWebViews.getState().minimise(OTHER)
  const { host, root } = await mount(h(WebPill))
  expect(button(host, 'Show Other')).not.toBeNull()
  expect(host.querySelector('[data-web-pill] img')?.getAttribute('src')).toBe('https://example.org/favicon.ico')
  await act(async () => button(host, '2 web pages').click())
  const items = [...document.querySelectorAll('ul[aria-label="Web pages"] button')]
  expect(items.map((b) => b.textContent)).toEqual(['Other', 'Example'])
  await act(async () => (items[1] as HTMLButtonElement).click())
  expect(useWebViews.getState().front).toBe(ID)
  expect(pillOf(useWebViews.getState().views)).toMatchObject({ latest: { id: OTHER }, count: 1 })
  expect(button(host, '2 web pages')).toBeNull()
  // A favicon that fails gives way to the globe; nothing else is fetched.
  await act(async () => host.querySelector('[data-web-pill] img')!.dispatchEvent(new Event('error')))
  expect(host.querySelector('[data-web-pill] img')).toBeNull()
  expect(host.querySelector('[data-web-pill] .lucide-globe')).not.toBeNull()
  await act(async () => root.unmount())
})

test('a tablet keeps the tile: show_web places it beside the chat, the frame attaches in its pane, nothing goes full screen or into the pill', async () => {
  shell.phone = false
  webViewOpened({ id: ID, session: 's1', url: 'https://example.com/', title: 'Example' }, true)
  expect(leaves(usePanes.getState().root!)).toEqual(['s1', webLeaf(ID)])
  expect(useWebViews.getState().front).toBeNull()
  const { host, root } = await mount(h(Fragment, null, h(WebFrame, { id: ID }), h(WebPill)))
  await vi.waitFor(() => expect(shell.log).toEqual(['web_view_attach']))
  expect(button(host, 'Open in browser')).not.toBeNull()
  expect(button(host, 'Minimise')).toBeNull()
  expect(host.querySelector('[data-web-pill]')).toBeNull()
  await act(async () => root.unmount())
})

test('crossing into the phone layout puts web tiles into the pill; widening again makes the full-screen page a tile beside its chat', () => {
  page()
  usePanes.getState().place('s1', webLeaf(ID), 'right')
  crossLayout(true, 's1')
  expect(useWebViews.getState().views[ID]?.minimised).toBe(true)
  expect(leaves(usePanes.getState().root!)).toEqual(['s1', webLeaf(ID)])
  usePanes.setState({ root: null })
  useWebViews.getState().show(ID)
  crossLayout(false, 's1')
  expect(useWebViews.getState().front).toBeNull()
  expect(leaves(usePanes.getState().root!)).toEqual(['s1', webLeaf(ID)])
  // The page of a chat that is not on screen waits in the pill instead.
  useWebViews.getState().open({ id: OTHER, session: 's9', url: 'https://example.org/', title: 'Other' })
  useWebViews.getState().show(OTHER)
  crossLayout(false, 's1')
  expect(useWebViews.getState().views[OTHER]?.minimised).toBe(true)
  expect(leaves(usePanes.getState().root!)).not.toContain(webLeaf(OTHER))
})
