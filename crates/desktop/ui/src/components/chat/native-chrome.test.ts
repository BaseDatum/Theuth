import { readyConnection } from '@/components/settings/test-engine'
import type * as ConnectionModule from '@/store/connection'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { afterEach, expect, test, vi } from 'vitest'
import { SidebarProvider } from '@/components/ui/sidebar'
import { TooltipProvider } from '@/components/ui/tooltip'
import { useNativeChrome } from '@/lib/native-chrome'
import type * as Rpc from '@/lib/rpc'
import type * as Router from '@tanstack/react-router'
import { Header } from './Header'
import { Transcript } from './Transcript'

const storage = vi.hoisted(() => ({ getItem: () => null, setItem: () => undefined, removeItem: () => undefined }))
vi.hoisted(() => vi.stubGlobal('localStorage', storage))
vi.mock('@tanstack/react-router', async (original) => ({
  ...(await original<typeof Router>()),
  Link: ({ children }: { children: ReactNode }) => createElement('a', null, children),
  useNavigate: () => () => undefined,
  useRouterState: ({ select }: { select: (s: { location: { pathname: string } }) => unknown }) => select({ location: { pathname: '/' } }),
}))
vi.mock('@tauri-apps/api/core', () => ({
  invoke: async () => ({ supported: true }),
  addPluginListener: async () => ({ unregister: async () => {} }),
}))
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => new Proxy({}, { get: () => async () => [] }),
}))
vi.mock('@/store/connection', async (original) => {
  const m = await original<typeof ConnectionModule>()
  return { ...m, useConnection: readyConnection(m.createConnection()) }
})

/** An iPhone (or, `phone: false`, an iPad's wide window) in the iOS shell; `supported` is the plugin's answer. SSR
 * renders zustand's initial state, so the answer is written there. */
function shell(supported: boolean, phone = true): void {
  vi.stubGlobal('window', { __TAURI_INTERNALS__: {}, matchMedia: () => ({ matches: phone, addEventListener() {}, removeEventListener() {} }) })
  vi.stubGlobal('navigator', { userAgent: phone ? 'Mozilla/5.0 (iPhone; CPU iPhone OS 26_0 like Mac OS X)' : 'Mozilla/5.0 (iPad)', maxTouchPoints: 5 })
  vi.stubGlobal('localStorage', storage)
  useNativeChrome.getInitialState().supported = supported
}
afterEach(() => {
  useNativeChrome.getInitialState().supported = false
  vi.unstubAllGlobals()
})

const render = (node: ReactNode) =>
  renderToStaticMarkup(
    createElement(QueryClientProvider, {
      client: new QueryClient(),
      children: createElement(TooltipProvider, null, createElement(SidebarProvider, null, node)),
    }),
  )
const header = (html: string) => html.match(/<header[^>]*>/)?.[0] ?? ''

test('fallback (iOS < 26, no plugin): the phone keeps the web header with its drawer toggle, title and ⋯', () => {
  shell(false)
  const html = render(createElement(Header, { session: null, under: true }))
  expect(header(html)).not.toContain('data-native-chrome')
  expect(header(html)).not.toContain('invisible')
  expect(html).toContain('aria-label="Open sidebar"')
  expect(html).toContain('New chat')
  expect(html).toContain('aria-label="Session menu"')
})

test('fallback: an iPad wide window keeps the web header even where the plugin would answer', () => {
  shell(true, false)
  const html = render(createElement(Header, { session: null, under: true }))
  expect(header(html)).not.toContain('data-native-chrome')
})

test('native: the chat header is invisible and takes no room; only its two menus stay mounted for the native taps', () => {
  shell(true)
  const html = render(createElement(Header, { session: null, under: true }))
  expect(header(html)).toContain('data-native-chrome="true"')
  expect(header(html)).toMatch(/class="[^"]*\binvisible\b[^"]*\babsolute\b/)
  expect(html).not.toContain('aria-label="Open sidebar"')
  // The title is the native pill's; the chevron menu and the ⋯ trigger remain as the anchors the taps open.
  expect(html).not.toMatch(/<span class="truncate">New chat<\/span>/)
  expect(html).toContain('aria-label="More header controls"')
  expect(html).toContain('aria-label="Session menu"')
})

test('native without `under` (the new-chat pane): invisible but keeps its 52 px, so the page below does not move', () => {
  shell(true)
  const tag = header(render(createElement(Header, { session: null })))
  expect(tag).toContain('invisible')
  expect(tag).not.toMatch(/\babsolute\b/)
})

test('a header that does not own the window (a split pane, `first={false}`) never goes native', () => {
  shell(true)
  expect(header(render(createElement(Header, { session: null, first: false, under: true })))).not.toContain('data-native-chrome')
})

test('transcript under the bar: reaches up through the safe area and pads its content below the bar; else unchanged', () => {
  shell(true)
  const under = render(createElement(Transcript, { session: 's1', under: true }))
  expect(under).toContain('style="margin-top:calc(-1 * var(--safe-top))"')
  expect(under).toContain('style="padding-top:calc(var(--safe-top) + 52px)"')
  const plain = render(createElement(Transcript, { session: 's1' }))
  expect(plain).not.toContain('safe-top')
})
