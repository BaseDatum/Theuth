import '../settings/test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement as h, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, expect, test, vi } from 'vitest'
import { keys } from '@/components/tasks/queries'
import { SidebarMenu, SidebarProvider } from '@/components/ui/sidebar'
import type * as Rpc from '@/lib/rpc'
import { queryKeys } from '@/lib/rpc'
import type * as Popout from '@/lib/popout'
import { openPopout } from '@/lib/popout'
import { notify as toast } from '@/lib/notify'
import { usePanes } from '@/lib/panes'
import type { SessionSummary } from "@/rpc/bindings/method.agent.output"
import type { AgentInfo } from '@/store/ui-events'
import { useSessions } from '@/store/sessions'
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
const deskOpen = vi.hoisted(() => vi.fn<(name: string) => Promise<string>>())
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({ list_sessions: async () => SESSIONS, agents_list: async () => AGENTS, desk_open: deskOpen }),
}))
vi.mock('@/lib/notify', () => ({ notify: { error: vi.fn() } }))
vi.mock('@/lib/popout', async (original) => ({ ...(await original<typeof Popout>()), canPopout: () => true, openPopout: vi.fn(async () => {}) }))

const agent = (name: string, desk: string | null, status = 'idle') => ({ desk, status, file: { name } })
const AGENTS = [agent('sergei', 'd-sergei'), agent('theuth', 'd-theuth', 'working'), agent('stas', 'd-stas'), agent('maya', null), agent('ghost', null, 'proposed')]
const session = (id: string, parent: string | null, running = false) => ({ id, parent, running, kind: 'chat' }) as unknown as SessionSummary
const SESSIONS = [session('d-theuth', null), session('d-sergei', null), session('sub-1', 'd-sergei', true), session('sub-2', 'sub-1', true), session('d-stas', null)]

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
    'sergei desk, idle, 2 subagents running',
    'stas desk, idle',
    'maya desk, idle',
  ])
  // The dot and the live-subagent count live inside the tile; no count while none runs.
  expect(tile('theuth').querySelector('[data-dot]')!.getAttribute('data-dot')).toBe('working')
  expect(tile('stas').querySelector('[data-dot]')!.getAttribute('data-dot')).toBe('idle')
  expect(tile('sergei').querySelector('[data-live-count]')!.textContent).toBe('2')
  expect(tile('stas').querySelector('[data-live-count]')).toBeNull()
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
  expect(document.querySelector('[role="tooltip"]')!.textContent).toBe('sergei · idle, 2 subagents running')
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

const info = (id: string, parent: string, state: string) => ({ id, name: id, parent, depth: 1, state: { state } }) as unknown as AgentInfo

test('the tree opens with a line naming the open desk, linking to it, with its live and finished counts; none without subagents', async () => {
  useSessions.getState().setChildren('d-sergei', [info('sub-1', 'd-sergei', 'running'), info('sub-2', 'sub-1', 'idle'), info('sub-3', 'd-sergei', 'error')])
  const { host, root } = await mount('sub-2')
  const header = host.querySelector<HTMLElement>('[data-desk-header]')!
  expect(header.textContent).toBe('sergei1 live · 2 finished')
  expect(header.getAttribute('href')).toBe('/c/d-sergei')
  expect(header.compareDocumentPosition(host.querySelector('[data-tree]')!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  await act(async () => root.unmount())
  const stas = await mount('d-stas')
  expect(stas.host.querySelector('[data-tree]')!.getAttribute('data-tree')).toBe('d-stas')
  expect(stas.host.querySelector('[data-desk-header]')).toBeNull()
  await act(async () => stas.root.unmount())
})

/** A mouse press-and-move on `el`; returns the session the panes store is dragging mid-gesture, then drops. */
const dragOf = async (el: HTMLElement) => {
  el.setPointerCapture = () => undefined
  el.hasPointerCapture = () => false
  // jsdom has no `PointerEvent`: a `MouseEvent` carrying the pointer fields React reads.
  const fire = (type: string, x: number) =>
    el.dispatchEvent(Object.assign(new MouseEvent(type, { bubbles: true, button: 0, clientX: x, clientY: 10 }), { pointerId: 1, pointerType: 'mouse' }))
  await act(async () => {
    fire('pointerdown', 10)
    fire('pointermove', 60)
  })
  const dragging = usePanes.getState().drag?.session
  await act(async () => fire('pointerup', 60))
  return dragging
}

test('a desk with a session drags into a pane like any session row, tile and tree header alike; one without is click-to-open only', async () => {
  useSessions.getState().setChildren('d-sergei', [info('sub-1', 'd-sergei', 'running')])
  const { host, root, tile } = await mount('d-sergei')
  // `draggable=false` keeps the browser's own link drag (the href ghost) off the tile.
  expect(tile('sergei').getAttribute('draggable')).toBe('false')
  expect(await dragOf(tile('sergei'))).toBe('d-sergei')
  expect(await dragOf(host.querySelector<HTMLElement>('[data-desk-header]')!)).toBe('d-sergei')
  expect(tile('maya').hasAttribute('data-drag-row')).toBe(false)
  expect(await dragOf(tile('maya'))).toBeUndefined()
  await act(async () => root.unmount())
})

test('desk tiles and the desk-tree header pop out their own chats', async () => {
  vi.mocked(openPopout).mockClear()
  useSessions.getState().setChildren('d-sergei', [info('sub-1', 'd-sergei', 'running')])
  const { host, root, tile } = await mount('d-sergei')
  for (const name of ['theuth', 'sergei', 'stas']) {
    const button = tile(name).closest('li')!.querySelector<HTMLButtonElement>('button[aria-label="Open in new window"]')!
    expect(button.title).toBe('Open in new window')
    expect(button.querySelector('.lucide-app-window')).not.toBeNull()
    expect(button.closest('a')).toBeNull()
    expect(await dragOf(button)).toBeUndefined()
    await act(async () => button.click())
    expect(openPopout).toHaveBeenLastCalledWith({ kind: 'chat', session: `d-${name}` })
  }
  const header = host.querySelector<HTMLElement>('[data-desk-header]')!
  const button = header.parentElement!.querySelector<HTMLButtonElement>('button[aria-label="Open in new window"]')!
  await act(async () => button.click())
  expect(openPopout).toHaveBeenLastCalledWith({ kind: 'chat', session: 'd-sergei' })
  expect(openPopout).toHaveBeenCalledTimes(4)
  await act(async () => root.unmount())
})

test('an unopened desk pops out through its own desk_open, then opens the desk chat; a refusal toasts and opens nothing', async () => {
  vi.mocked(openPopout).mockClear()
  const { host, root, tile } = await mount(undefined)
  // The group label (with its Broadcast button) is a header, not a destination.
  expect(host.querySelector('[data-sidebar="group-label"] [aria-label="Open in new window"]')).toBeNull()
  const button = () => tile('maya').closest('li')!.querySelector<HTMLButtonElement>('button[aria-label="Open in new window"]')!
  deskOpen.mockResolvedValueOnce('d-maya')
  await act(async () => button().click())
  expect(deskOpen).toHaveBeenCalledExactlyOnceWith('maya')
  expect(openPopout).toHaveBeenCalledExactlyOnceWith({ kind: 'chat', session: 'd-maya' })
  deskOpen.mockRejectedValueOnce(new Error('roster busy'))
  await act(async () => button().click())
  expect(toast.error).toHaveBeenCalledExactlyOnceWith('roster busy')
  expect(openPopout).toHaveBeenCalledTimes(1)
  await act(async () => root.unmount())
})
