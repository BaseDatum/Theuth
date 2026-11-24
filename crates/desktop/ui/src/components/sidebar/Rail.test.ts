import '../settings/test-dom'
import type * as SessionListModule from './SessionList'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement as h, type ComponentProps, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { openNewChat } from '@/components/chat/NewChatPane'
import { SidebarProvider, useSidebar } from '@/components/ui/sidebar'
import { useSidebarReveal } from '@/lib/sidebar-reveal'
import { receiveComponent } from '@/rpc/bindings/receive'
import type { SessionSummary } from '@/rpc/bindings/method.agent.output'
import { buildRows } from './SessionList'
import { CLOSE_DELAY, POPOVER_CAP, RailSections, railSections } from './Rail'

const s = (id: string, project: string | null, updated_at: string, pinned = false): SessionSummary =>
  receiveComponent('method.agent.output.SessionSummary', {
    id,
    created_at: updated_at,
    updated_at,
    title: `Chat ${id}`,
    tags: [],
    model: 'm',
    parent: null,
    name: null,
    effort: null,
    agent: null,
    cwd: null,
    pinned,
    archived: false,
    project,
    notebook: null,
    retention: 'durable',
    running: false,
    owner: '',
    epoch: 1,
    memory: 'epochs',
    here: true,
  })

const env = vi.hoisted(() => ({ sessions: [] as SessionSummary[], active: new Set<string>() }))
vi.mock('./SessionList', async (original) => {
  const m = await original<typeof SessionListModule>()
  return { ...m, useSidebarSessions: () => ({ sessions: env.sessions, subs: new Map(), activeSet: env.active, recent: new Map(), prompting: new Set() }) }
})
// The project glyph is the project style system's; the rail must ask it, not draw its own.
vi.mock('./ProjectStyle', () => ({
  ProjectIcon: ({ project, className }: { project: string | null; className?: string }) => h('svg', { 'data-project-icon': project ?? '', className }),
}))
vi.mock('./Desks', () => ({ useDeskList: () => [] }))
vi.mock('@/rpc/capabilities', () => ({ Capability: ({ children }: { children: ReactNode }) => children }))
vi.mock('@/components/chat/NewChatPane', () => ({ openNewChat: vi.fn() }))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof import('@/lib/rpc')>()), useApi: () => ({}) }))
vi.mock('@tanstack/react-router', () => ({
  useNavigate: () => vi.fn(),
  Link: ({ to, params = {}, ...props }: ComponentProps<'a'> & { to: string; params?: Record<string, string> }) =>
    h('a', { ...props, href: to.replace(/\$(\w+)/g, (_, key: string) => encodeURIComponent(params[key]!)) }),
}))

let sidebar: { open: boolean } = { open: false }
function Probe() {
  sidebar = useSidebar()
  return null
}

let unmount = async () => {}
async function mount() {
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  const client = new QueryClient()
  await act(async () =>
    root.render(h(QueryClientProvider, { client }, h(SidebarProvider, { defaultOpen: false, rail: true }, h(Probe), h(RailSections, { active: undefined })))),
  )
  unmount = async () => {
    await act(async () => root.unmount())
    client.clear()
  }
}
beforeEach(() => {
  vi.clearAllMocks()
  env.sessions = [
    s('run', 'theuth', '2026-11-24T10:00:00Z'),
    s('pin', null, '2026-11-23T10:00:00Z', true),
    ...Array.from({ length: POPOVER_CAP + 2 }, (_, i) => s(`t${i}`, 'theuth', `2026-11-2${i % 3}T0${i}:00:00Z`)),
    s('v1', 'verified-listings', '2026-11-10T10:00:00Z'),
    s('u1', null, '2026-11-09T10:00:00Z'),
  ]
  env.active = new Set(['run'])
  useSidebarReveal.setState({ key: null })
})
afterEach(async () => {
  await unmount()
  document.body.innerHTML = ''
})

const icons = () => [...document.querySelectorAll<HTMLButtonElement>('[data-rail-section]')]
const icon = (key: string) => document.querySelector<HTMLButtonElement>(`[data-rail-section="${key}"]`)!
const popover = () => document.querySelector<HTMLElement>('[data-slot="popover-content"]')
const click = (el: HTMLElement) => act(async () => el.click())
const titles = () => [...popover()!.querySelectorAll('a')].map((a) => a.textContent)

test('railSections: the expanded list\'s sections in its order, day headers folded in, archived left out', () => {
  const sections = railSections(buildRows(env.sessions, [], undefined, false, env.active))
  expect(sections.map((x) => [x.key, x.kind, x.label, x.items.length])).toEqual([
    ['h:active', 'active', 'Active', 1],
    ['h:pinned', 'pinned', 'Pinned', 1],
    ['p:theuth', 'project', 'theuth', POPOVER_CAP + 2],
    ['p:verified-listings', 'project', 'verified-listings', 1],
    ['p:', 'ungrouped', 'Ungrouped', 1],
  ])
  // No projects at all: the plain day-grouped list is one section.
  const plain = railSections(buildRows([s('a', null, '2026-11-24T10:00:00Z'), s('b', null, '2020-01-01T00:00:00Z')], []))
  expect(plain.map((x) => [x.key, x.kind, x.items.length])).toEqual([['p:', 'ungrouped', 2]])
})

test('the rail shows one icon per section: Active with its running dot, Pinned, each project in its ProjectIcon, Ungrouped', async () => {
  await mount()
  expect(icons().map((b) => b.dataset.railSection)).toEqual(['h:active', 'h:pinned', 'p:theuth', 'p:verified-listings', 'p:'])
  expect(icon('h:active').querySelector('.lucide-zap')).not.toBeNull()
  expect(icon('h:active').querySelector('[aria-label="running"]')).not.toBeNull()
  expect(icon('h:pinned').querySelector('.lucide-pin')).not.toBeNull()
  expect(icon('p:theuth').querySelector('[data-project-icon="theuth"]')).not.toBeNull()
  expect(icon('p:verified-listings').querySelector('[data-project-icon="verified-listings"]')).not.toBeNull()
  expect(icon('p:').querySelector('.lucide-messages-square')).not.toBeNull()
  expect(icon('p:theuth').getAttribute('aria-label')).toBe(`theuth, ${POPOVER_CAP + 2} chats`)
})

test('nothing running: no dot on Active (its section only exists while something is or lingers)', async () => {
  env.active = new Set()
  await mount()
  expect(icons().map((b) => b.dataset.railSection)).toEqual(['h:pinned', 'p:theuth', 'p:verified-listings', 'p:'])
})

test('a section\'s popover lists its chats (capped, with Show all) under its name, count and + new chat', async () => {
  await mount()
  await click(icon('p:verified-listings'))
  expect(popover()).not.toBeNull()
  expect(popover()!.textContent).toContain('verified-listings')
  expect(titles()).toEqual([expect.stringContaining('Chat v1')])
  expect(popover()!.querySelector('a')!.getAttribute('href')).toBe('/c/v1')
  await click(popover()!.querySelector<HTMLButtonElement>('button[aria-label="New chat in “verified-listings”"]')!)
  expect(openNewChat).toHaveBeenCalledWith(expect.any(Function), 'verified-listings')
  expect(popover()).toBeNull()

  await click(icon('p:theuth'))
  expect(titles()).toHaveLength(POPOVER_CAP)
  const all = [...popover()!.querySelectorAll('button')].find((b) => b.textContent?.startsWith('Show all'))!
  expect(all.textContent).toBe(`Show all ${POPOVER_CAP + 2} →`)
  // Pinned has no + (it is not a place a new chat goes).
  await click(icon('h:pinned'))
  expect(titles()).toEqual([expect.stringContaining('Chat pin')])
  expect(popover()!.querySelector('button[aria-label^="New chat"]')).toBeNull()
})

test('clicking a chat in the popover closes it; the rail stays collapsed', async () => {
  await mount()
  await click(icon('h:active'))
  const chat = popover()!.querySelector<HTMLAnchorElement>('a[href="/c/run"]')!
  await click(chat)
  expect(popover()).toBeNull()
  expect(sidebar.open).toBe(false)
})

test('Show all expands the sidebar and asks it to reveal that section', async () => {
  await mount()
  await click(icon('p:theuth'))
  const all = [...popover()!.querySelectorAll('button')].find((b) => b.textContent?.startsWith('Show all'))!
  await click(all)
  expect(popover()).toBeNull()
  expect(sidebar.open).toBe(true)
  expect(useSidebarReveal.getState().key).toBe('p:theuth')
})

test('a mouse opens the popover by hovering and it closes a short delay after the pointer leaves; a click pins it', async () => {
  vi.useFakeTimers()
  try {
    await mount()
    const pointer = (type: string, target: Element) =>
      act(async () => {
        const e = new MouseEvent(type, { bubbles: true, relatedTarget: type === 'pointerover' ? document.body : null })
        Object.defineProperty(e, 'pointerType', { value: 'mouse' })
        target.dispatchEvent(e)
      })
    await pointer('pointerover', icon('h:pinned'))
    expect(popover()).not.toBeNull()
    await pointer('pointerout', icon('h:pinned'))
    expect(popover()).not.toBeNull()
    await act(async () => vi.advanceTimersByTime(CLOSE_DELAY + 10))
    expect(popover()).toBeNull()

    await pointer('pointerover', icon('h:pinned'))
    await click(icon('h:pinned'))
    await pointer('pointerout', icon('h:pinned'))
    await act(async () => vi.advanceTimersByTime(CLOSE_DELAY + 10))
    expect(popover()).not.toBeNull()
  } finally {
    vi.useRealTimers()
  }
})

test('keyboard: Enter/Space (a click without a pointer) opens with focus inside, arrows move, Esc closes back to the icon', async () => {
  await mount()
  const trigger = icon('p:theuth')
  trigger.focus()
  await click(trigger)
  const items = () => [...popover()!.querySelectorAll<HTMLElement>('[data-rail-item]')]
  await act(async () => new Promise((r) => setTimeout(r, 0)))
  expect(items()).toContain(document.activeElement)
  const key = (k: string) => act(async () => void document.activeElement!.dispatchEvent(new KeyboardEvent('keydown', { key: k, bubbles: true })))
  items()[0]!.focus()
  await key('ArrowDown')
  expect(document.activeElement).toBe(items()[1])
  await key('ArrowUp')
  await key('ArrowUp')
  expect(document.activeElement).toBe(items().at(-1))
  await key('Escape')
  await act(async () => new Promise((r) => setTimeout(r, 0)))
  expect(popover()).toBeNull()
  expect(document.activeElement).toBe(trigger)
})
