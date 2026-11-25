import '../settings/test-dom'
import type * as Rpc from '@/lib/rpc'
import type * as Popout from '@/lib/popout'
import { canPopout, openPopout } from '@/lib/popout'
import { usePanes } from '@/lib/panes'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement, type ComponentProps } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { SidebarProvider } from '@/components/ui/sidebar'
import { receiveComponent } from '@/rpc/bindings/receive'
import type { SessionSummary } from '@/rpc/bindings/method.agent.output'
import { useApprovals } from '@/store/approvals'
import { empty } from '@/store/reduce'
import { useSessions } from '@/store/sessions'
import { SessionItem } from './SessionItem'

const navigate = vi.fn()
const select = vi.fn()
const rowClick = vi.fn()
const rowPointerDown = vi.fn()
const rowMouseDown = vi.fn()
vi.mock('@tanstack/react-router', () => ({
  useNavigate: () => navigate,
  Link: ({ to, params, ...props }: ComponentProps<'a'> & { to: string; params: Record<string, string> }) => createElement('a', { ...props, href: to.replace(/\$(\w+)/g, (_, key: string) => encodeURIComponent(params[key]!)) }),
}))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => ({}) }))
vi.mock('@/lib/meeting-link', () => ({ useMeetingForSession: () => null, openMeeting: vi.fn() }))
vi.mock('@/lib/popout', async (original) => ({ ...(await original<typeof Popout>()), canPopout: vi.fn(() => true), openPopout: vi.fn(async () => {}) }))

const child = receiveComponent('method.agent.output.SessionSummary', { id: 'child', title: 'child', model: 'm', parent: 'root', running: true, created_at: '2026-11-16T12:00:00Z', updated_at: '2026-11-16T12:00:00Z' })
const glyph = (label: string) => document.querySelector(`[role="img"][aria-label="${label}"]`)!
let unmount = async () => {}
let client: QueryClient
async function mount(subagents: SessionSummary[] = [child], endless = false, pinned = false) {
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  client = new QueryClient()
  await act(async () => root.render(createElement(QueryClientProvider, { client, children: createElement(SidebarProvider, null, createElement('div', { onClick: rowClick, onPointerDown: rowPointerDown, onMouseDown: rowMouseDown }, createElement(SessionItem, {
    id: 'root', title: 'Background work', created: '2026-11-16T12:00:00Z', tags: [], pinned, archived: false, project: null, owner: '', epoch: 1, here: true, running: true, endless, subagents, active: false, selected: false, selecting: false, select,
  }))) })))
  unmount = async () => { await act(async () => root.unmount()) }
}
beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(canPopout).mockReturnValue(true)
  usePanes.setState({ focused: null, drag: null })
  useSessions.setState({ sessions: {
    root: { ...empty('root'), activity: 'thinking', running: true, jobs: new Set(['j1', 'j2']), ptys: new Set(['pty1']), reminders: new Set(['r1']) },
    child: { ...empty('child'), running: true },
  } })
  useApprovals.getState().replace([])
})
afterEach(async () => {
  await unmount()
  client?.clear()
  document.body.innerHTML = ''
})

test('sidebar categories match the dot palette without changing their shapes, pulse or labels; thinking stays info', async () => {
  await mount()
  expect(glyph('2 background jobs').getAttribute('class')).toContain('lucide-cog')
  expect(glyph('2 background jobs').getAttribute('class')).toContain('text-project-lapis')
  expect(glyph('2 background jobs').getAttribute('class')).toContain('animate-pulse')
  expect(glyph('Terminal running').getAttribute('class')).toContain('text-project-faience')
  expect(glyph('Terminal running').getAttribute('class')).toContain('pty-live')
  expect(glyph('Terminal running').querySelector('.pty-cursor')).not.toBeNull()
  expect(glyph('1 reminder pending').getAttribute('class')).toContain('lucide-timer')
  expect(glyph('1 reminder pending').getAttribute('class')).toContain('text-project-plum')
  expect(glyph('1 reminder pending').getAttribute('class')).not.toContain('animate-pulse')
  expect(glyph('1 subagent running').getAttribute('class')).toContain('lucide-network')
  expect(glyph('1 subagent running').getAttribute('class')).toContain('text-project-malachite')
  expect(glyph('1 subagent running').getAttribute('class')).toContain('animate-pulse')
  expect(glyph('Thinking').getAttribute('class')).toContain('text-info')
})

test('an open but quiet terminal keeps a still terminal glyph labelled idle; a changing screen animates it; exit removes it', async () => {
  useSessions.setState({ sessions: { root: { ...empty('root'), terminals: new Set(['t1']) }, child: empty('child') } })
  await mount()
  const idle = glyph('Terminal idle')
  expect(idle.getAttribute('class')).toContain('text-project-faience')
  expect(idle.getAttribute('class')).not.toContain('pty-live')
  expect(idle.querySelector('title')?.textContent).toBe('Terminal idle')
  await act(async () => useSessions.getState().applyEvents([{ type: 'JobStarted', session: 'root', id: 't2', command: 'top', kind: 'pty', call_id: 'c2', log: '/l', name: null, pty: true }]))
  expect(glyph('2 terminals, 1 running').getAttribute('class')).toContain('pty-live')
  await act(async () => useSessions.getState().applyEvents([{ type: 'JobIdle', session: 'root', id: 't2', command: 'top', name: null, nudge: 1, quiet_secs: 1, secs: 1, user_typed_secs: null, why: '', output: '' }]))
  expect(glyph('2 terminals idle').getAttribute('class')).not.toContain('pty-live')
  await act(async () => useSessions.getState().applyEvents([
    { type: 'JobFinished', session: 'root', id: 't1', command: 'top', kind: 'pty', exit: 'killed', secs: 1, output: '', inline: false, pty: true },
    { type: 'JobFinished', session: 'root', id: 't2', command: 'top', kind: 'pty', exit: 'killed', secs: 1, output: '', inline: false, pty: true },
  ]))
  expect(document.querySelector('.lucide-square-terminal')).toBeNull()
})

test("a child's quiet terminal puts no terminal glyph on the root row", async () => {
  useSessions.setState({ sessions: { root: empty('root'), child: { ...empty('child'), terminals: new Set(['t1']) } } })
  await mount()
  expect(document.querySelector('.lucide-square-terminal')).toBeNull()
})

test('only an approval in this subtree turns the network glyph warning; resolving restores malachite', async () => {
  await mount()
  await act(async () => useApprovals.getState().add({ type: 'ApprovalRequested', id: 'other', session: 'unrelated', tool: 'shell', args: {}, risk: 'mutating', human_only: false }))
  expect(glyph('1 subagent running').getAttribute('class')).toContain('text-project-malachite')
  await act(async () => useApprovals.getState().add({ type: 'ApprovalRequested', id: 'approval', session: 'child', tool: 'shell', args: {}, risk: 'mutating', human_only: false }))
  expect(glyph('1 subagent running').getAttribute('class')).toContain('text-warning')
  expect(glyph('1 subagent running').getAttribute('class')).toContain('animate-pulse')
  await act(async () => useApprovals.getState().resolve('approval'))
  expect(glyph('1 subagent running').getAttribute('class')).toContain('text-project-malachite')
})

test('watch-only subagents use warning for requires_action, including live updates', async () => {
  const observed = receiveComponent('method.agent.output.ObservedMeta', { source: 'omp_task', external: 'child', state: 'requires_action', can_cancel: false, cost_rule: 'counted', detail: null })
  await mount([{ ...child, kind: 'observed', observed }])
  expect(glyph('1 subagent running').getAttribute('class')).toContain('text-warning')
  await act(async () => useSessions.setState((s) => ({ sessions: { ...s.sessions, child: { ...s.sessions.child!, observed: { ...observed, state: 'running' } } } })))
  expect(glyph('1 subagent running').getAttribute('class')).toContain('text-project-malachite')
})

test('a session hover action opens its chat without row selection, navigation or drag', async () => {
  await mount()
  const button = document.querySelector<HTMLButtonElement>('button[aria-label="Open in new window"]')!
  expect(button.title).toBe('Open in new window')
  expect(button.querySelector('.lucide-app-window')).not.toBeNull()
  expect(button.closest('a')).toBeNull()
  await act(async () => {
    button.dispatchEvent(Object.assign(new MouseEvent('pointerdown', { bubbles: true, button: 0, clientX: 10 }), { pointerId: 1, pointerType: 'mouse' }))
    button.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, button: 0 }))
    button.dispatchEvent(Object.assign(new MouseEvent('pointermove', { bubbles: true, clientX: 60 }), { pointerId: 1, pointerType: 'mouse' }))
    button.click()
  })
  expect(openPopout).toHaveBeenCalledExactlyOnceWith({ kind: 'chat', session: 'root' })
  expect(rowClick).not.toHaveBeenCalled()
  expect(rowPointerDown).not.toHaveBeenCalled()
  expect(rowMouseDown).not.toHaveBeenCalled()
  expect(select).not.toHaveBeenCalled()
  expect(navigate).not.toHaveBeenCalled()
  expect(usePanes.getState().drag).toBeNull()
})

test('no session hover action where second windows are unavailable', async () => {
  vi.mocked(canPopout).mockReturnValue(false)
  await mount()
  expect(document.querySelector('button[aria-label="Open in new window"]')).toBeNull()
})

test('the existing chat options menu retains Open in new window', async () => {
  await mount()
  const options = document.querySelector<HTMLButtonElement>('button[aria-label="Chat options"]')!
  await act(async () => options.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })))
  const item = [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((el) => el.textContent === 'Open in new window')!
  expect(item).toBeDefined()
  await act(async () => item.click())
  expect(openPopout).toHaveBeenCalledWith({ kind: 'chat', session: 'root' })
})

test('a fractal context session shows the ∞ badge labelled Fractal context after its title', async () => {
  await mount([child], true)
  expect(glyph('Fractal context').textContent).toBe('∞')
  expect(document.querySelector('[aria-label*="ndless"], [aria-label*="emory"]')).toBeNull()
})

test('a pinned chat shows a muted pin leftmost in its row, before the title; an unpinned one shows none', async () => {
  await mount([child], false, true)
  const link = document.querySelector('a[href="/c/root"]')!
  const pin = glyph('Pinned')
  expect(pin.querySelector('.lucide-pin')).not.toBeNull()
  expect(pin.className).toContain('text-fg-tertiary/70')
  expect(link.firstElementChild).toBe(pin)
  await unmount()
  document.body.innerHTML = ''
  await mount()
  expect(document.querySelector('[aria-label="Pinned"]')).toBeNull()
})
