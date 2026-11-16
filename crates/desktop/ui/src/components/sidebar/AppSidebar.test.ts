import '../settings/test-dom'
import type * as Rpc from '@/lib/rpc'
import type * as Popout from '@/lib/popout'
import type * as Talk from '@/lib/talk'
import type * as Discovery from '@/rpc/discovery'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement, type ComponentProps, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { toast } from 'sonner'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { openNewChat } from '@/components/chat/NewChatPane'
import { createNote } from '@/components/notes/create'
import { SidebarProvider } from '@/components/ui/sidebar'
import { canPopout, openPopout } from '@/lib/popout'
import * as talk from '@/lib/talk'
import type { DocSummary } from '@/rpc/bindings/method.agent.output'
import { AppSidebar } from './AppSidebar'

const env = vi.hoisted(() => ({ phase: 'on', session: 'talk-session' as string | null }))
const navigate = vi.fn()
vi.mock('@tanstack/react-router', () => ({
  useNavigate: () => navigate,
  useRouterState: ({ select }: { select(s: { location: { href: string; pathname: string } }): unknown }) => select({ location: { href: '/c/root', pathname: '/c/root' } }),
  Link: ({ to, params = {}, ...props }: ComponentProps<'a'> & { to: string; params?: Record<string, string>; preload?: string }) => createElement('a', { ...props, href: to.replace(/\$(\w+)/g, (_, key: string) => encodeURIComponent(params[key]!)) }),
}))
vi.mock('sonner', () => ({ toast: { error: vi.fn() } }))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => ({}) }))
vi.mock('@/lib/popout', async (original) => ({ ...(await original<typeof Popout>()), canPopout: vi.fn(() => true), openPopout: vi.fn(async () => {}) }))
vi.mock('@/lib/talk', async (original) => {
  const actual = await original<typeof Talk>()
  return { ...actual, open: vi.fn(), useTalk: () => ({ ...actual.OFF, phase: env.phase, session: env.session }) }
})
vi.mock('@/components/notes/create', () => ({ createNote: vi.fn() }))
vi.mock('@/rpc/discovery', async (original) => ({ ...(await original<typeof Discovery>()), supports: () => true }))
vi.mock('@/rpc/capabilities', () => ({ Capability: ({ children }: { children: ReactNode }) => children, useQuerySupport: () => false }))
vi.mock('@/components/link/EngineSwitcher', () => ({ EngineSwitcher: () => null }))
vi.mock('@/components/hud/AttentionBadge', () => ({ AttentionBadge: () => null }))
vi.mock('@/components/chat/BroadcastDialog', () => ({ BroadcastDialog: () => null, openBroadcastDialog: vi.fn() }))
vi.mock('@/components/chat/NewChatPane', () => ({ NewChatOnMenu: () => null, machineRows: () => [], openNewChat: vi.fn() }))
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

/** The row (menu item) whose text starts with `label`, and its popout button (null when it has none). */
const row = (label: string) => {
  const item = [...document.querySelectorAll<HTMLElement>('[data-sidebar="menu-item"]')].find((el) => el.textContent?.trim().startsWith(label))!
  return { item, button: item.querySelector<HTMLButtonElement>('button[aria-label="Open in new window"]') }
}
const click = (el: HTMLElement) => act(async () => el.click())

test('the running Talk item opens its existing talk target, without navigating or starting another talk', async () => {
  await mount()
  const { button } = row('Talk')
  expect(button!.title).toBe('Open in new window')
  expect(button!.querySelector('.lucide-app-window')).not.toBeNull()
  await click(button!)
  expect(openPopout).toHaveBeenCalledExactlyOnceWith({ kind: 'talk', session: 'talk-session' })
  expect(talk.open).not.toHaveBeenCalled()
  expect(navigate).not.toHaveBeenCalled()
})

test('every page row pops its own route alone; the click on the button never navigates the shell', async () => {
  await mount()
  const pages: [string, string][] = [
    ['Chat', '/new'],
    ['Meeting', '/meetings/new'],
    ['Attention', '/attention'],
    ['Agents', '/agents'],
    ['Library', '/artifacts'],
    ['Costs', '/costs'],
    ['Settings', '/settings/appearance'],
  ]
  for (const [label, path] of pages) {
    const { button } = row(label)
    await click(button!)
    expect(openPopout).toHaveBeenLastCalledWith({ kind: 'page', path })
  }
  expect(openPopout).toHaveBeenCalledTimes(pages.length)
  expect(navigate).not.toHaveBeenCalled()
  expect(openNewChat).not.toHaveBeenCalled()
  // The home mark is not a destination row.
  expect(row('THEUTH').button).toBeNull()
})

test('idle Talk starts a talk exactly as its click does, then pops that talk; a refusal toasts and opens nothing', async () => {
  env.phase = 'off'
  env.session = null
  vi.mocked(talk.open).mockResolvedValueOnce('fresh-talk')
  await mount()
  await click(row('Talk').button!)
  // `current` is the open chat (`/c/root`), as for the row's click.
  expect(talk.open).toHaveBeenCalledExactlyOnceWith({}, 'root')
  expect(openPopout).toHaveBeenCalledExactlyOnceWith({ kind: 'talk', session: 'fresh-talk' })
  expect(navigate).not.toHaveBeenCalled()

  vi.mocked(talk.open).mockRejectedValueOnce(new Error('no mic'))
  await click(row('Talk').button!)
  expect(toast.error).toHaveBeenCalledExactlyOnceWith('no mic')
  expect(openPopout).toHaveBeenCalledTimes(1)
})

test('New note creates through the row mutation, then pops the fresh note; a failure toasts and opens nothing', async () => {
  vi.mocked(createNote).mockResolvedValueOnce({ id: 'n1' } as DocSummary)
  await mount()
  await click(row('Note').button!)
  expect(createNote).toHaveBeenCalledTimes(1)
  expect(openPopout).toHaveBeenCalledExactlyOnceWith({ kind: 'note', doc: 'n1', fresh: true })
  expect(navigate).not.toHaveBeenCalled()

  vi.mocked(createNote).mockRejectedValueOnce(new Error('helper down'))
  await click(row('Note').button!)
  expect(toast.error).toHaveBeenCalledExactlyOnceWith('helper down')
  expect(openPopout).toHaveBeenCalledTimes(1)
})

test('iOS (no second windows): no row has the button', async () => {
  vi.mocked(canPopout).mockReturnValue(false)
  await mount()
  expect(document.querySelector('button[aria-label="Open in new window"]')).toBeNull()
  vi.mocked(canPopout).mockReturnValue(true)
})
