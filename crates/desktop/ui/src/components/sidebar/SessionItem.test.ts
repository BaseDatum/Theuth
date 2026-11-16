import '../settings/test-dom'
import type * as Rpc from '@/lib/rpc'
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

vi.mock('@tanstack/react-router', () => ({
  useNavigate: () => vi.fn(),
  Link: ({ to, params, ...props }: ComponentProps<'a'> & { to: string; params: Record<string, string> }) => createElement('a', { ...props, href: to.replace(/\$(\w+)/g, (_, key: string) => encodeURIComponent(params[key]!)) }),
}))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => ({}) }))
vi.mock('@/lib/meeting-link', () => ({ useMeetingForSession: () => null, openMeeting: vi.fn() }))

const child = receiveComponent('method.agent.output.SessionSummary', { id: 'child', title: 'child', model: 'm', parent: 'root', running: true, created_at: '2026-11-16T12:00:00Z', updated_at: '2026-11-16T12:00:00Z' })
const glyph = (label: string) => document.querySelector(`[role="img"][aria-label="${label}"]`)!
let unmount = async () => {}
let client: QueryClient
async function mount(subagents: SessionSummary[] = [child]) {
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  client = new QueryClient()
  await act(async () => root.render(createElement(QueryClientProvider, { client, children: createElement(SidebarProvider, null, createElement(SessionItem, {
    id: 'root', title: 'Background work', created: '2026-11-16T12:00:00Z', tags: [], pinned: false, archived: false, project: null, owner: '', epoch: 1, here: true, running: true, endless: false, subagents, active: false, selected: false, selecting: false, select: vi.fn(),
  })) })))
  unmount = async () => { await act(async () => root.unmount()) }
}
beforeEach(() => {
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
  expect(glyph('1 live terminal').getAttribute('class')).toContain('text-project-faience')
  expect(glyph('1 live terminal').getAttribute('class')).toContain('pty-live')
  expect(glyph('1 live terminal').querySelector('.pty-cursor')).not.toBeNull()
  expect(glyph('1 reminder pending').getAttribute('class')).toContain('lucide-timer')
  expect(glyph('1 reminder pending').getAttribute('class')).toContain('text-project-plum')
  expect(glyph('1 reminder pending').getAttribute('class')).not.toContain('animate-pulse')
  expect(glyph('1 subagent running').getAttribute('class')).toContain('lucide-network')
  expect(glyph('1 subagent running').getAttribute('class')).toContain('text-project-malachite')
  expect(glyph('1 subagent running').getAttribute('class')).toContain('animate-pulse')
  expect(glyph('Thinking').getAttribute('class')).toContain('text-info')
})

test('only an approval in this subtree turns the network glyph warning; resolving restores malachite', async () => {
  await mount()
  await act(async () => useApprovals.getState().add({ type: 'ApprovalRequested', id: 'other', session: 'unrelated', tool: 'shell', args: {}, risk: 'mutating' }))
  expect(glyph('1 subagent running').getAttribute('class')).toContain('text-project-malachite')
  await act(async () => useApprovals.getState().add({ type: 'ApprovalRequested', id: 'approval', session: 'child', tool: 'shell', args: {}, risk: 'mutating' }))
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
