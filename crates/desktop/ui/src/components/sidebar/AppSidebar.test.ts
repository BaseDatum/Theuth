import '../settings/test-dom'
import type * as Rpc from '@/lib/rpc'
import type * as Popout from '@/lib/popout'
import type * as Talk from '@/lib/talk'
import type * as Discovery from '@/rpc/discovery'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement, type ComponentProps, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { SidebarProvider } from '@/components/ui/sidebar'
import { openPopout } from '@/lib/popout'
import { AppSidebar } from './AppSidebar'

const env = vi.hoisted(() => ({ phase: 'on', session: 'talk-session' as string | null }))
const navigate = vi.fn()
vi.mock('@tanstack/react-router', () => ({
  useNavigate: () => navigate,
  useRouterState: ({ select }: { select(s: { location: { href: string; pathname: string } }): unknown }) => select({ location: { href: '/c/root', pathname: '/c/root' } }),
  Link: ({ to, params = {}, ...props }: ComponentProps<'a'> & { to: string; params?: Record<string, string>; preload?: string }) => createElement('a', { ...props, href: to.replace(/\$(\w+)/g, (_, key: string) => encodeURIComponent(params[key]!)) }),
}))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => ({}) }))
vi.mock('@/lib/popout', async (original) => ({ ...(await original<typeof Popout>()), canPopout: () => true, openPopout: vi.fn(async () => {}) }))
vi.mock('@/lib/talk', async (original) => { const actual = await original<typeof Talk>(); return { ...actual, useTalk: () => ({ ...actual.OFF, phase: env.phase, session: env.session }) } })
vi.mock('@/rpc/discovery', async (original) => ({ ...(await original<typeof Discovery>()), supports: () => true }))
vi.mock('@/rpc/capabilities', () => ({ Capability: ({ children }: { children: ReactNode }) => children, useQuerySupport: () => false }))
vi.mock('@/components/link/EngineSwitcher', () => ({ EngineSwitcher: () => null }))
vi.mock('@/components/hud/AttentionBadge', () => ({ AttentionBadge: () => null }))
vi.mock('@/components/chat/BroadcastDialog', () => ({ BroadcastDialog: () => null, openBroadcastDialog: vi.fn() }))
vi.mock('@/components/chat/NewChatPane', () => ({ NewChatOnMenu: () => null, openNewChat: vi.fn() }))
vi.mock('@/components/meeting/MeetingList', () => ({ useCanRecord: () => true }))
vi.mock('./SearchTrigger', () => ({ SearchTrigger: () => null }))
vi.mock('./SessionList', () => ({ SessionList: () => null }))
vi.mock('./Desks', () => ({ Desks: () => null }))

let unmount = async () => {}
let client: QueryClient
async function mount() {
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  client = new QueryClient()
  await act(async () => root.render(createElement(QueryClientProvider, { client, children: createElement(SidebarProvider, null, createElement(AppSidebar, { session: 'root' })) })))
  unmount = async () => { await act(async () => root.unmount()) }
}
beforeEach(() => {
  vi.clearAllMocks()
  env.phase = 'on'
  env.session = 'talk-session'
})
afterEach(async () => {
  await unmount()
  client?.clear()
  document.body.innerHTML = ''
})

test('the running Talk item opens its existing talk target, without navigating or starting another talk', async () => {
  await mount()
  const buttons = document.querySelectorAll<HTMLButtonElement>('button[aria-label="Open in new window"]')
  expect(buttons).toHaveLength(1)
  const button = buttons[0]!
  expect(button.closest('[data-sidebar="menu-item"]')?.textContent).toContain('Talk')
  expect(button.title).toBe('Open in new window')
  expect(button.querySelector('.lucide-app-window')).not.toBeNull()
  await act(async () => button.click())
  expect(openPopout).toHaveBeenCalledExactlyOnceWith({ kind: 'talk', session: 'talk-session' })
  expect(navigate).not.toHaveBeenCalled()
})

test('home, creation and navigation launchers without existing targets get no popout action', async () => {
  env.phase = 'off'
  env.session = null
  await mount()
  const text = document.body.textContent ?? ''
  for (const label of ['THEUTH', 'Chat', 'Talk', 'Note', 'Meeting', 'Attention', 'Agents', 'Library', 'Costs', 'Settings']) expect(text).toContain(label)
  expect(document.querySelector('button[aria-label="Open in new window"]')).toBeNull()
})
