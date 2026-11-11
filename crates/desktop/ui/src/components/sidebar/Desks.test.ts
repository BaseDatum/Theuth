import '../settings/test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement as h, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, expect, test, vi } from 'vitest'
import { keys } from '@/components/tasks/queries'
import { SidebarMenu, SidebarProvider } from '@/components/ui/sidebar'
import type * as Rpc from '@/lib/rpc'
import { queryKeys } from '@/lib/rpc'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import { Desks, deskLabels } from './Desks'

vi.mock('@tanstack/react-router', async (original) => ({
  ...(await original<object>()),
  useNavigate: () => vi.fn(),
  Link: ({ to, params, children, ...rest }: { to: string; params: { session: string }; children: ReactNode }) =>
    h('a', { href: to.replace('$session', params.session), ...rest }, children),
}))
vi.mock('./AgentTree', () => ({
  AgentTree: (p: { session: string; liveOnly?: boolean }) => h('div', { 'data-tree': p.session, 'data-live-only': String(p.liveOnly ?? false) }),
}))
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({ list_sessions: async () => SESSIONS, agents_list: async () => AGENTS }),
}))

const agent = (name: string, desk: string | null, status = 'idle') => ({ desk, status, file: { name } })
const AGENTS = [agent('sergei', 'd-sergei'), agent('theuth', 'd-theuth', 'working'), agent('stas', 'd-stas'), agent('maya', null), agent('ghost', null, 'proposed')]
const session = (id: string, parent: string | null, running = false) => ({ id, parent, running, kind: 'chat' }) as unknown as SessionSummary
const SESSIONS = [session('d-theuth', null), session('d-sergei', null), session('sub-1', 'd-sergei', true), session('sub-2', 'sub-1'), session('d-stas', null)]

afterEach(() => {
  document.body.innerHTML = ''
})

const mount = async (active: string | undefined) => {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  qc.setQueryData(keys.agents, AGENTS)
  qc.setQueryData(queryKeys.sessions, SESSIONS)
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  await act(async () => root.render(h(QueryClientProvider, { client: qc }, h(SidebarProvider, null, h(SidebarMenu, null, h(Desks, { active }))))))
  const tile = (name: string) => host.querySelector<HTMLElement>(`[aria-label^="${name} desk"]`)!
  return { host, root, tile }
}

test('a tile shows the first letter; names sharing it get the shortest prefix (≤ 3) that tells them apart', () => {
  expect(deskLabels(['theuth', 'sergei', 'stas', 'maya'])).toEqual(['T', 'Se', 'St', 'M'])
  expect(deskLabels(['al', 'alex', 'Bob'])).toEqual(['Al', 'Ale', 'B'])
  expect(deskLabels(['stas', 'stasya', 'zed'])).toEqual(['Sta', 'Sta', 'Z'])
  expect(deskLabels(['Ann', 'anna'])).toEqual(['Ann', 'Ann'])
  expect(deskLabels(['x'])).toEqual(['X'])
})

test('one tile per active desk in a wrapping grid, theuth first; text, aria-label, working dot and subagent state inside', async () => {
  const { host, root, tile } = await mount(undefined)
  const grid = host.querySelector('ul.grid')!
  const tiles = [...grid.querySelectorAll<HTMLElement>('[data-sidebar="menu-button"]')]
  expect(tiles.map((t) => t.querySelector('.pointer-coarse\\:hidden')!.textContent)).toEqual(['T', 'Se', 'St', 'M'])
  expect(tiles.map((t) => t.getAttribute('aria-label'))).toEqual([
    'theuth desk, working',
    'sergei desk, idle, 1 subagent running',
    'stas desk, idle',
    'maya desk, idle',
  ])
  // The dot and the glyph live inside the tile.
  expect(tile('theuth').querySelector('[data-dot]')!.getAttribute('data-dot')).toBe('working')
  expect(tile('stas').querySelector('[data-dot]')!.getAttribute('data-dot')).toBe('idle')
  expect(tile('sergei').querySelector('[aria-label="1 subagent running"]')).not.toBeNull()
  expect(tile('stas').querySelector('svg')).toBeNull()
  // A desk links to its session; one without asks to open it.
  expect(tile('sergei').getAttribute('href')).toBe('/c/d-sergei')
  expect(tile('maya').tagName).toBe('BUTTON')
  // No desk is open: nothing highlighted, no tree.
  expect(host.querySelector('[data-active="true"]')).toBeNull()
  expect(host.querySelector('[data-tree]')).toBeNull()
  await act(async () => root.unmount())
})

test('focus shows the full name and state in a tooltip', async () => {
  const { root, tile } = await mount(undefined)
  await act(async () => tile('sergei').focus())
  expect(document.querySelector('[role="tooltip"]')!.textContent).toBe('sergei · idle, 1 subagent running')
  await act(async () => root.unmount())
})

test('the open desk, or the desk owning the open subagent, is highlighted and its whole tree hangs below the grid, once', async () => {
  for (const active of ['d-sergei', 'sub-2']) {
    const { host, root, tile } = await mount(active)
    expect(tile('sergei').getAttribute('data-active')).toBe('true')
    expect(host.querySelectorAll('[data-active="true"]')).toHaveLength(1)
    const trees = host.querySelectorAll('[data-tree]')
    expect([...trees].map((t) => [t.getAttribute('data-tree'), t.getAttribute('data-live-only')])).toEqual([['d-sergei', 'false']])
    expect(host.querySelector('ul.grid')!.compareDocumentPosition(trees[0]!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    await act(async () => root.unmount())
  }
  const { host, root } = await mount('some-chat')
  expect(host.querySelector('[data-active="true"]')).toBeNull()
  expect(host.querySelector('[data-tree]')).toBeNull()
  await act(async () => root.unmount())
})
