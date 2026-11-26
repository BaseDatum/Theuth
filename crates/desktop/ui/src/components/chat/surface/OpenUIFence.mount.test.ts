// The React #300 report (a Card > Tabs > TabItem > ListBlock reply): mounted inline as a fence and in the app view
// (the artifact tile / panel `Open as app` lands in), streamed line by line, closed, tab-switched, remounted.
import '../../settings/test-dom'
import { act, createElement as h, type ReactNode } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, expect, test, vi } from 'vitest'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { libraryHash } from '@theuth/apps'
import type * as Rpc from '@/lib/rpc'
import type * as Tiles from '@/lib/tiles'
import { AppView } from '@/components/apps/AppView'
// The lazy runtime chunk loaded up front, as `preloadAppView` does in the app: a Suspense retry inside `act` never flushes.
import '@/components/apps/AppHost'
import { RenderBoundary } from '@/components/apps/RenderBoundary'
import { useConnection } from '@/store/connection'
import { FenceContext } from './JsonRenderFence'
import { OpenUIFence } from './OpenUIFence'

const SOURCE = `root = Card([tabs])
tabs = Tabs([a, b])
a = TabItem("a", "A. Extend sre-monitoring (recommended)", [ta, la])
ta = TextContent("One collector. Teach the existing sre-monitoring collector to read config keys.")
la = ListBlock([ListItem("No new collector, secrets or IRSA"), ListItem("Comparison comes almost free"), ListItem("Still Python CronJob")])
b = TabItem("b", "B. Keep E2 as designed", [tb, lb])
tb = TextContent("New Go collector → Alloy → Mimir → per-tier Grafana, as on the board now.")
lb = ListBlock([ListItem("Clean metrics design"), ListItem("Two collectors with overlapping access"), ListItem("Config collected twice")])`
const LINES = SOURCE.split('\n')

const ARTIFACT = { call_id: 'rec1', session: 's1', name: 'x', path: '/x', bytes: SOURCE.length, created_at: 0 }
const app = {
  get_app: vi.fn(async () => ({ artifact: ARTIFACT, source: SOURCE, library_hash: libraryHash, bindings: [], diagnostics: [], recent_calls: [] })),
}
vi.mock('@/components/apps/theme', () => ({ useDark: () => false }))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => app }))
vi.mock('@/rpc/capabilities', () => ({ useQuerySupport: () => true }))
vi.mock('@/lib/notify', () => ({ notify: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn() }) }))
vi.mock('@/lib/tiles', async (original) => ({ ...(await original<typeof Tiles>()), tileable: () => false }))

const errors = vi.spyOn(console, 'error')
let root: Root | undefined
let host: HTMLElement
afterEach(() => {
  act(() => root?.unmount())
  root = undefined
  host?.remove()
  errors.mockClear()
})

async function mount(node: ReactNode) {
  host = document.createElement('div')
  document.body.append(host)
  root = createRoot(host)
  await act(async () => root!.render(node))
}
const render = (node: ReactNode) => act(async () => root!.render(node))
/** `get_app` resolves outside `act`: wait for what it renders. */
const until = (check: () => void) => vi.waitFor(check, { timeout: 4000 })

const fence = (code: string, isIncomplete: boolean, running = isIncomplete) =>
  h(FenceContext, { value: { running, hasLaterAssistant: false, session: 's1' } }, h(OpenUIFence, { code, isIncomplete, language: 'openui' }))
const client = new QueryClient()
const tile = (props: Parameters<typeof AppView>[0]) => h(QueryClientProvider, { client }, h(AppView, props))

/** The visible tab panel's text. */
const panel = () => host.querySelector('[role="tabpanel"][data-state="active"]')?.textContent ?? ''
async function press(label: string) {
  const tab = [...host.querySelectorAll('[role="tab"]')].find((t) => t.textContent?.includes(label)) as HTMLElement
  await act(async () => {
    tab.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, button: 0 }))
    tab.dispatchEvent(new MouseEvent('click', { bubbles: true, button: 0 }))
  })
}
const hookErrors = () => errors.mock.calls.flat().some((a) => /hooks|#300|#310/i.test(String(a)))

test('a fence that streamed and then closed on a partial program falls back instead of throwing #300', async () => {
  // Line 2 references `a`/`b` before any statement defines them: fine while open, a check failure once closed.
  const partial = LINES.slice(0, 2).join('\n')
  await mount(fence(partial, true))
  expect(host.querySelector('[data-streamdown="openui"]')).not.toBeNull()
  await render(fence(partial, false, false))
  expect(host.querySelector('[data-render-error]')).toBeNull()
  expect(host.textContent).toContain('diagram omitted')
  // And back: the same instance renders the full program once it arrives.
  await render(fence(SOURCE, false, false))
  expect(host.querySelector('[data-render-error]')).toBeNull()
  expect(panel()).toContain('One collector')
  expect(hookErrors()).toBe(false)
})

test('inline: streamed line by line, closed, tabs switch, unmount and remount', async () => {
  await mount(fence(LINES[0]!, true))
  for (let i = 2; i <= LINES.length; i++) await render(fence(LINES.slice(0, i).join('\n'), true))
  await render(fence(SOURCE, false))
  expect(host.querySelector('[data-render-error]')).toBeNull()
  // While it streamed, the tabs followed the tab being written: the last one.
  expect(panel()).toContain('Two collectors with overlapping access')
  await press('A. Extend')
  expect(panel()).toContain('Comparison comes almost free')
  await press('B. Keep E2')
  expect(panel()).toContain('Config collected twice')
  await render(h('div'))
  await render(fence(SOURCE, false))
  expect(panel()).toContain('One collector')
  expect(hookErrors()).toBe(false)
})

test('tile: a streaming draft previews line by line, then the committed record runs; tabs switch; remount', async () => {
  useConnection.setState({ info: { app_enabled: true }, state: 'open' } as never)
  const draft = (source: string) => ({ source, edit: false, bytes: source.length, status: 'streaming' as const })
  await mount(tile({ session: 's1', draft: draft(LINES[0]!) }))
  for (let i = 2; i <= LINES.length; i++) await render(tile({ session: 's1', draft: draft(LINES.slice(0, i).join('\n')) }))
  await until(() => expect(panel()).toContain('Config collected twice'))
  expect(host.querySelector('[data-render-error]')).toBeNull()
  const artifact = ARTIFACT as never
  await render(tile({ session: 's1', artifact }))
  await until(() => expect(host.textContent).not.toMatch(/drafting|loading/))
  await until(() => expect(panel()).not.toBe(''))
  expect(host.querySelector('[data-render-error]')).toBeNull()
  await press('A. Extend')
  expect(panel()).toContain('One collector')
  await press('B. Keep E2')
  expect(panel()).toContain('Config collected twice')
  await render(h('div'))
  await render(tile({ session: 's1', artifact }))
  await until(() => expect(panel()).toContain('One collector'))
  expect(hookErrors()).toBe(false)
}, 20_000)

test('a block that throws shows one line with Retry; Retry remounts it; siblings stay up', async () => {
  let fail = true
  const Bomb = () => {
    if (fail) throw new Error('boom')
    return h('p', null, 'recovered')
  }
  await mount(h('div', null, h(RenderBoundary, null, h(Bomb)), h('p', null, 'sibling')))
  expect(host.textContent).toContain('This block failed to render — boom')
  expect(host.textContent).toContain('sibling')
  fail = false
  const retry = [...host.querySelectorAll('button')].find((b) => b.textContent === 'Retry')!
  await act(async () => retry.click())
  expect(host.textContent).toContain('recovered')
  expect(host.querySelector('[data-render-error]')).toBeNull()
})
