import '../settings/test-dom'
import type * as Rpc from '@/lib/rpc'
import type * as Popout from '@/lib/popout'
import type * as Talk from '@/lib/talk'
import type * as Discovery from '@/rpc/discovery'
import type * as RailModule from './Rail'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement, type ComponentProps, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { toast } from 'sonner'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { openNewChat } from '@/components/chat/NewChatPane'
import { createNote } from '@/components/notes/create'
import { SidebarProvider } from '@/components/ui/sidebar'
import { canPopout, openPopout, popoutLabel, type PopoutTarget } from '@/lib/popout'
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
vi.mock('./Rail', async (original) => ({ ...(await original<typeof RailModule>()), RailSections: () => createElement('div', { 'data-rail-sections': '' }) }))

let unmount = async () => {}
let client: QueryClient
async function mount(props: { open?: boolean } = {}) {
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  client = new QueryClient()
  const shell = createElement(SidebarProvider, { rail: true, defaultOpen: props.open ?? true }, createElement(AppSidebar, { session: 'root' }))
  await act(async () => root.render(createElement(QueryClientProvider, { client, children: shell })))
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
    ['Inbox', '/attention'],
    ['Meeting', '/meetings/new'],
    ['Agents', '/agents'],
    ['Library', '/artifacts'],
    ['Costs', '/costs'],
    ['Settings', '/settings/appearance'],
  ]
  for (const [label, path] of pages) {
    const { button } = row(label)
    await click(button!)
    expect(openPopout).toHaveBeenLastCalledWith(label === 'Chat' ? { kind: 'page', path, key: expect.any(String) } : { kind: 'page', path })
  }
  expect(openPopout).toHaveBeenCalledTimes(pages.length)
  expect(navigate).not.toHaveBeenCalled()
  expect(openNewChat).not.toHaveBeenCalled()
})

test('no THEUTH home row: Chat is the first row and nothing links to the retired overview', async () => {
  await mount()
  const rows = [...document.querySelectorAll<HTMLElement>('[data-sidebar="menu-item"]')].map((el) => el.textContent?.trim() ?? '')
  expect(rows[0]).toMatch(/^Chat/)
  expect(document.body.textContent).not.toContain('THEUTH')
  expect(document.querySelector('[aria-label="theuth home"]')).toBeNull()
  expect(document.querySelector('a[href="/"]:not([aria-label="Talk"]):not([aria-label^="Talk "])')).toBeNull()
  expect(document.querySelector('a[aria-label="New chat"]')!.getAttribute('href')).toBe('/new')
})

test('collapsed on a desktop: the icon rail, its toggle expanding it again; expanded, the same toggle collapses it to the rail', async () => {
  await mount({ open: false })
  expect(document.querySelector('[data-sidebar="menu-item"]')).toBeNull()
  expect(document.querySelector('[data-rail-sections]')).not.toBeNull()
  const labels = [...document.querySelectorAll<HTMLElement>('[data-slot="sidebar"] [aria-label]')].map((el) => el.getAttribute('aria-label'))
  for (const name of ['Expand sidebar', 'Broadcast…', 'New chat', 'Inbox', 'Talk (running)', 'New note', 'Meeting', 'Agents', 'Library', 'Costs', 'Settings'])
    expect(labels).toContain(name)
  // No popout buttons, no "New chat on" in the rail.
  expect(document.querySelector('button[aria-label="Open in new window"]')).toBeNull()
  expect(document.querySelector('[data-slot="sidebar"]')!.getAttribute('data-collapsible')).toBe('icon')
  await click(document.querySelector<HTMLButtonElement>('button[aria-label="Expand sidebar"]')!)
  expect(row('Chat').item).toBeDefined()
  expect(document.querySelector('[data-rail-sections]')).toBeNull()
  await click(document.querySelector<HTMLButtonElement>('button[aria-label="Collapse sidebar"]')!)
  expect(document.querySelector('[data-rail-sections]')).not.toBeNull()
})

test('the rail\'s Chat opens a new chat; Talk talks where the row would', async () => {
  vi.mocked(talk.open).mockResolvedValueOnce('t2')
  await mount({ open: false })
  await click(document.querySelector<HTMLElement>('a[aria-label="New chat"]')!)
  expect(openNewChat).toHaveBeenCalledExactlyOnceWith(navigate)
  await click(document.querySelector<HTMLElement>('button[aria-label="Talk (running)"]')!)
  expect(talk.open).toHaveBeenCalledExactlyOnceWith({}, 'root')
  expect(navigate).toHaveBeenCalledWith({ to: '/talk/$session', params: { session: 't2' } })
})

test('Inbox is the row right under Chat with the inbox tray icon (the route stays /attention)', async () => {
  await mount()
  const rows = [...document.querySelectorAll<HTMLElement>('[data-sidebar="menu-item"]')].map((el) => el.textContent?.trim() ?? '')
  const chat = rows.findIndex((t) => t.startsWith('Chat'))
  expect(chat).toBeGreaterThanOrEqual(0)
  expect(rows[chat + 1]).toMatch(/^Inbox/)
  expect(row('Inbox').item.querySelector('a')!.getAttribute('href')).toBe('/attention')
  expect(row('Inbox').item.querySelector('a > svg')!.getAttribute('class')).toMatch(/\blucide-inbox\b/)
})

test('every New chat press opens a fresh window (its own label); a second Costs press brings the same window forward', async () => {
  await mount()
  const labels = async (name: string) => {
    vi.mocked(openPopout).mockClear()
    await click(row(name).button!)
    await click(row(name).button!)
    return vi.mocked(openPopout).mock.calls.map(([t]: [PopoutTarget]) => popoutLabel(t))
  }
  const [first, second] = await labels('Chat')
  expect(first).toMatch(/^popout-page-new-/)
  expect(second).toMatch(/^popout-page-new-/)
  expect(first).not.toBe(second)
  expect(await labels('Costs')).toEqual(['popout-page-costs', 'popout-page-costs'])
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
