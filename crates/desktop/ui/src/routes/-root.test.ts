// `-` prefix: the router plugin skips it (not a route), vitest still runs it.
import '../components/settings/test-dom'
import { act, createElement as h, type ComponentType, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { expect, test, vi } from 'vitest'
import { Route } from './__root'

const env = vi.hoisted(() => ({ pathname: '/costs' }))
vi.mock('@tanstack/react-router', async (original) => ({
  ...(await original<object>()),
  createRootRoute: (options: { component: ComponentType }) => ({ options }),
  Outlet: () => h('main', { 'data-outlet': env.pathname }),
  useRouterState: ({ select }: { select(s: { location: { pathname: string } }): unknown }) => select({ location: { pathname: env.pathname } }),
  useNavigate: () => vi.fn(),
  useParams: () => ({}),
}))
// The shell's own pieces: a page window must render none of them (and their module graphs need a real browser).
vi.mock('@/components/chat/NewChatPane', () => ({ openNewChat: vi.fn() }))
vi.mock('@/components/chat/Panes', () => ({ DragOverlay: () => h('div', { 'data-drag': true }) }))
vi.mock('@/components/chat/slash', () => ({ settings: vi.fn() }))
vi.mock('@/components/routines/RoutineChat', () => ({ RoutineDraftBridge: () => null }))
vi.mock('@/components/autoresearch/SetupPane', () => ({ StudyDraftBridge: () => null }))
vi.mock('@/lib/costs/queries', () => ({ CostsPrefetch: () => h('i', { 'data-prefetch': true }) }))
vi.mock('@/rpc/capabilities', () => ({ Capability: ({ children }: { children: ReactNode }) => children }))
vi.mock('@/lib/hotkeys', () => ({ useHotkeys: vi.fn() }))
vi.mock('@/lib/popout', () => ({ POPOUT: { kind: 'page', path: '/costs' }, isPopout: () => true }))
vi.mock('@/lib/tauri', () => ({ isIsland: () => false, isOverlay: () => false }))
vi.mock('@/components/sidebar/AppSidebar', () => ({ AppSidebar: () => h('nav', { 'data-rail': true }) }))
vi.mock('@/components/sidebar/SearchDialog', () => ({ SearchDialog: () => h('div', { 'data-search': true }) }))
vi.mock('@/components/meeting/MeetingBar', () => ({ MeetingBar: () => h('div', { 'data-meeting-bar': true }) }))
vi.mock('@/components/costs/Resident', () => ({ CostsResident: () => h('div', { 'data-resident': true }), ResidentCtx: {} }))

test('a page window renders its route alone — no rail, search, meeting bar or resident costs host — loads its own cube and titles itself', async () => {
  const Shell = Route.options.component!
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  await act(async () => root.render(h(Shell)))
  expect(host.querySelector('[data-outlet="/costs"]')).not.toBeNull()
  for (const shell of ['data-rail', 'data-search', 'data-drag', 'data-meeting-bar', 'data-resident']) expect(host.querySelector(`[${shell}]`)).toBeNull()
  // Its own query cache: without the cube bridge the popped Costs page would stay empty.
  expect(host.querySelector('[data-prefetch]')).not.toBeNull()
  expect(document.title).toBe('Costs')
  // Another page of the same window retitles it.
  env.pathname = '/attention'
  await act(async () => root.render(h(Shell, { key: 'next' })))
  expect(host.querySelector('[data-outlet="/attention"]')).not.toBeNull()
  expect(document.title).toBe('Inbox')
  await act(async () => root.unmount())
})
