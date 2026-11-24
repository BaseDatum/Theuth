import '../settings/test-dom'
import type * as SessionListModule from './SessionList'
import type * as AgentTreeModule from './AgentTree'
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
import { usePanes } from '@/lib/panes'
import { useSessions } from '@/store/sessions'
import { empty } from '@/store/reduce'
import type { AgentInfo } from '@/store/ui-events'
import { CLOSE_DELAY, POPOVER_CAP, RailSections, railSections, settleRail, workspaceTarget } from './Rail'
import { SessionItem } from './SessionItem'

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
  ProjectStyleDialog: ({ project, open }: { project: string; open: boolean }) => (open ? h('div', { 'data-style-dialog': project }) : null),
}))
// The subtree is `AgentTree`'s (its own tests); here only which chat gets one, and that it is live-only.
vi.mock('./AgentTree', async (original) => ({
  ...(await original<typeof AgentTreeModule>()),
  AgentTree: ({ session, liveOnly }: { session: string; liveOnly?: boolean }) =>
    h('div', { 'data-agent-tree': session, 'data-live-only': String(!!liveOnly) }, h('a', { href: `/c/${session}-sub` }, 'sub')),
}))
vi.mock('./Desks', () => ({ useDeskList: () => [] }))
vi.mock('@/rpc/capabilities', () => ({ Capability: ({ children }: { children: ReactNode }) => children, useQuerySupport: () => false }))
vi.mock('@/components/chat/NewChatPane', () => ({ openNewChat: vi.fn() }))
const api = vi.hoisted(() => ({ pin_session: vi.fn(async () => {}), rename_session: vi.fn(async () => {}) }))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof import('@/lib/rpc')>()), useApi: () => api }))
vi.mock('@/lib/owner', async (original) => ({ ...(await original<typeof import('@/lib/owner')>()), routed: (_id: string, call: () => Promise<unknown>) => call() }))
const navigate = vi.hoisted(() => vi.fn())
vi.mock('@tanstack/react-router', () => ({
  useNavigate: () => navigate,
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
  usePanes.setState({ leaves: [], focused: null })
  useSessions.setState({ sessions: {} })
  localStorage.clear()
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
  await click(icon('h:pinned'))
  await click(popover()!.querySelector<HTMLAnchorElement>('a[href="/c/pin"]')!)
  expect(popover()).toBeNull()
  await act(async () => void icon('h:active').dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true })))
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

const key = (el: Element, k: string) => act(async () => void el.dispatchEvent(new KeyboardEvent('keydown', { key: k, bubbles: true })))
/** Live subagents of `id` in its slice's tree, as the agent events leave them. */
const liveChildren = (id: string, names: string[]) =>
  useSessions.setState((st) => ({
    sessions: {
      ...st.sessions,
      [id]: { ...empty(id), children: Object.fromEntries(names.map((n) => [`${id}-${n}`, { id: `${id}-${n}`, name: n, depth: 1, state: { state: 'running' } } as unknown as AgentInfo])) },
    },
  }))

test('workspaceTarget: the focused chat leaf, else the newest laid-out or stored chat leaf, else the newest chat; drafts and unlisted ids never', () => {
  const sessions = env.sessions
  const base = { leaves: [] as string[], focused: null, stored: [] as string[], sessions }
  expect(workspaceTarget({ ...base, leaves: ['t1', 'pin', 'web:x'], focused: 'pin' })).toBe('pin')
  // Focused is a draft (or nothing): the most recent chat of the layout.
  expect(workspaceTarget({ ...base, leaves: ['t1', 'pin', 'draft:1'], focused: 'draft:1' })).toBe('pin')
  // No workspace mounted since the load: the persisted layout's chats.
  expect(workspaceTarget({ ...base, stored: ['u1', 'v1', 'gone'] })).toBe('v1')
  // No layout: the most recently active chat.
  expect(workspaceTarget(base)).toBe('run')
  expect(workspaceTarget({ ...base, sessions: [] })).toBeNull()
})

test('Active: a mouse click opens the workspace at its focused chat; hover still shows the popover with "Open workspace"', async () => {
  usePanes.setState({ leaves: ['t1', 'pin'], focused: 't1' })
  await mount()
  await click(icon('h:active'))
  expect(navigate).toHaveBeenCalledExactlyOnceWith({ to: '/c/$session', params: { session: 't1' } })
  expect(popover()).toBeNull()
  await key(icon('h:active'), 'ArrowRight')
  const open = [...popover()!.querySelectorAll('button')].find((b) => b.textContent === 'Open workspace')!
  await click(open)
  expect(navigate).toHaveBeenCalledTimes(2)
  expect(popover()).toBeNull()
})

test('Active on touch: the first tap opens its popover, the second (while open) opens the workspace', async () => {
  env.sessions = [s('x', null, '2026-11-24T10:00:00Z')]
  env.active = new Set(['x'])
  await mount()
  const tap = async () => {
    await act(async () => {
      const e = new MouseEvent('pointerdown', { bubbles: true })
      Object.defineProperty(e, 'pointerType', { value: 'touch' })
      icon('h:active').dispatchEvent(e)
    })
    await act(async () => void icon('h:active').dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 1 })))
  }
  await tap()
  expect(popover()).not.toBeNull()
  expect(navigate).not.toHaveBeenCalled()
  await tap()
  expect(navigate).toHaveBeenCalledExactlyOnceWith({ to: '/c/$session', params: { session: 'x' } })
})

test('a chat open in a pane shows its live-only subagent tree; a link in it closes the popover', async () => {
  usePanes.setState({ leaves: ['pin'], focused: 'pin' })
  await mount()
  await click(icon('h:pinned'))
  const tree = popover()!.querySelector<HTMLElement>('[data-agent-tree="pin"]')!
  expect(tree.dataset.liveOnly).toBe('true')
  await click(tree.querySelector('a')!)
  expect(popover()).toBeNull()
})

test('another chat with subagents running: "⑂ N" (none when none run) whose ▸ lists them; one opens and closes the popover; the section icon lights', async () => {
  liveChildren('v1', ['scout', 'critic'])
  await mount()
  expect(icon('p:verified-listings').querySelector('[aria-label="running"]')).not.toBeNull()
  expect(icon('h:pinned').querySelector('[aria-label="running"]')).toBeNull()
  await click(icon('p:verified-listings'))
  expect(popover()!.querySelector('[data-agent-tree]')).toBeNull()
  const count = popover()!.querySelector<HTMLButtonElement>('button[aria-label="2 subagents running"]')!
  expect(count.textContent).toContain('⑂ 2')
  expect(count.className).toContain('text-success')
  await click(count)
  const subs = [...popover()!.querySelectorAll<HTMLAnchorElement>('[aria-label="Running subagents"] a')]
  expect(subs.map((a) => a.textContent)).toEqual(['scout', 'critic'])
  await click(subs[1]!)
  expect(popover()).toBeNull()
  // A section without running subagents shows no count.
  await click(icon('h:pinned'))
  expect(popover()!.querySelector('button[aria-label$="running"]')).toBeNull()
})

test('a project header has its ⋯ menu beside the +: Icon & colour… opens the style dialog and the popover stays', async () => {
  await mount()
  await click(icon('p:verified-listings'))
  const menu = popover()!.querySelector<HTMLButtonElement>('button[aria-label="Project options"]')!
  expect(menu.previousElementSibling?.getAttribute('aria-label')).toBe('New chat in “verified-listings”')
  menu.focus()
  await key(menu, 'Enter')
  const style = [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((i) => i.textContent === 'Icon & colour…')!
  await click(style)
  expect(document.querySelector('[data-style-dialog="verified-listings"]')).not.toBeNull()
  expect(popover()).not.toBeNull()
})

test('a chat row has its ⋯ menu: opening it keeps the popover; Pin acts in place; Rename edits inline and saves', async () => {
  await mount()
  await click(icon('p:verified-listings'))
  const options = () => popover()!.querySelector<HTMLButtonElement>('button[aria-label="Chat options"]')!
  options().focus()
  await key(options(), 'Enter')
  expect(popover()).not.toBeNull()
  const item = (label: string) => [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((i) => i.textContent === label)!
  expect(item('Move to project')).toBeDefined()
  await click(item('Pin'))
  expect(api.pin_session).toHaveBeenCalledExactlyOnceWith('v1', true)
  expect(popover()).not.toBeNull()

  options().focus()
  await key(options(), 'Enter')
  await click(item('Rename'))
  const input = popover()!.querySelector<HTMLInputElement>('input')!
  expect(input.value).toBe('Chat v1')
  await act(async () => {
    const set = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')!.set!
    set.call(input, 'Listings')
    input.dispatchEvent(new Event('input', { bubbles: true }))
  })
  await key(input, 'Enter')
  expect(api.rename_session).toHaveBeenCalledExactlyOnceWith('v1', 'Listings')
  expect(popover()).not.toBeNull()
})

test('a popover chat row drags into the panes exactly as the expanded row does; the popover steps aside, then closes', async () => {
  // jsdom has no pointer capture; the press-drag sets it on its source row.
  Object.assign(window.HTMLElement.prototype, { setPointerCapture() {}, releasePointerCapture() {}, hasPointerCapture: () => false })
  const pointer = (el: Element, type: string, x: number) =>
    act(async () => {
      const e = new MouseEvent(type, { bubbles: true, cancelable: true, button: 0, clientX: x, clientY: 10 })
      Object.defineProperties(e, { pointerType: { value: 'mouse' }, pointerId: { value: 1 } })
      el.dispatchEvent(e)
    })
  /** Press, travel past the slop, and report the drag the panes see mid-gesture. */
  const drag = async (row: Element) => {
    await pointer(row, 'pointerdown', 10)
    await pointer(row, 'pointermove', 60)
    const live = usePanes.getState().drag
    return live && { session: live.session, title: live.title }
  }

  await mount()
  await click(icon('p:verified-listings'))
  const row = popover()!.querySelector<HTMLAnchorElement>('a[href="/c/v1"]')!
  expect(row.getAttribute('draggable')).toBe('false')
  expect(row.hasAttribute('data-drag-row')).toBe(true)
  const fromPopover = await drag(row)
  expect(fromPopover).toEqual({ session: 'v1', title: 'Chat v1' })
  // Mid-drag the popover is still mounted (its row holds the gesture) but unseen and unhit.
  expect(popover()!.className).toContain('pointer-events-none')
  expect(popover()!.className).toContain('invisible')
  await pointer(row, 'pointerup', 60)
  // The click that follows a drag is the drag's: no navigation; the popover closes once the drag is over.
  await click(row)
  expect(navigate).not.toHaveBeenCalled()
  expect(usePanes.getState().drag).toBeNull()
  expect(popover()).toBeNull()

  // The expanded sidebar's row for the same chat: the same source, the same payload.
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  const client = new QueryClient()
  const v1 = env.sessions.find((x) => x.id === 'v1')!
  await act(async () =>
    root.render(
      h(QueryClientProvider, { client }, h(SidebarProvider, null, h(SessionItem, {
        id: 'v1', title: v1.title, created: v1.created_at, tags: [], pinned: false, archived: false, project: v1.project, owner: '', epoch: 1,
        here: true, running: false, endless: false, subagents: undefined, active: false, selected: false, selecting: false, select: () => {},
      }))),
    ),
  )
  const expanded = host.querySelector<HTMLAnchorElement>('a[href="/c/v1"]')!
  expect(expanded.getAttribute('draggable')).toBe('false')
  expect(await drag(expanded)).toEqual(fromPopover)
  await pointer(expanded, 'pointerup', 60)
  await act(async () => root.unmount())
})

/** A scroller whose geometry jsdom does not lay out: fixed sizes, writable offsets, a log of `overflow-y` while laid out. */
function scroller({ scrollHeight, clientHeight, scrollTop, scrollLeft }: { scrollHeight: number; clientHeight: number; scrollTop: number; scrollLeft: number }) {
  const el = document.createElement('nav')
  el.style.overflowY = 'auto'
  const laidOut: string[] = []
  const pos = { scrollTop, scrollLeft }
  Object.defineProperties(el, {
    scrollHeight: { get: () => scrollHeight },
    clientHeight: { get: () => clientHeight },
    offsetHeight: { get: () => (laidOut.push(el.style.overflowY), clientHeight) },
    scrollTop: { get: () => pos.scrollTop, set: (v: number) => (pos.scrollTop = v) },
    scrollLeft: { get: () => pos.scrollLeft, set: (v: number) => (pos.scrollLeft = v) },
  })
  return { el, laidOut }
}

// iPad: the rail mounts mid-collapse (the container still animating its width) and its middle came up blank.
test('settleRail: a sideways or past-the-end scroll is undone, and the scroll layer is rebuilt (overflow off for a layout, then back)', () => {
  const { el, laidOut } = scroller({ scrollHeight: 400, clientHeight: 300, scrollTop: 900, scrollLeft: 120 })
  settleRail(el)
  expect([el.scrollLeft, el.scrollTop]).toEqual([0, 100])
  expect(laidOut).toEqual(['hidden'])
  expect(el.style.overflowY).toBe('auto')
  // In range: the reader's place is kept.
  const fits = scroller({ scrollHeight: 400, clientHeight: 300, scrollTop: 40, scrollLeft: 0 })
  settleRail(fits.el)
  expect(fits.el.scrollTop).toBe(40)
})
