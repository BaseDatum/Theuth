import '../settings/test-dom'
import type * as Rpc from '@/lib/rpc'
import type * as Popout from '@/lib/popout'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement, type ComponentProps } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { SidebarProvider } from '@/components/ui/sidebar'
import { openPopout } from '@/lib/popout'
import { usePanes } from '@/lib/panes'
import { receiveComponent } from '@/rpc/bindings/receive'
import { useSessions } from '@/store/sessions'
import type { AgentInfo } from '@/store/ui-events'
import { AgentTree, SubagentRow } from './AgentTree'

vi.mock('@tanstack/react-router', () => ({
  useNavigate: () => vi.fn(),
  Link: ({ to, params, ...props }: ComponentProps<'a'> & { to: string; params: Record<string, string> }) => createElement('a', { ...props, href: to.replace(/\$(\w+)/g, (_, key: string) => encodeURIComponent(params[key]!)) }),
}))
vi.mock('@/rpc/capabilities', () => ({ useQuerySupport: () => false }))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => api }))
vi.mock('@/lib/popout', async (original) => ({ ...(await original<typeof Popout>()), canPopout: () => true, openPopout: vi.fn(async () => {}) }))

const row = receiveComponent('method.agent.output.AgentInfo', { id: 'child', parent: 'root', depth: 1, name: 'worker', agent: null, model: 'm', state: { state: 'running' }, started_at: '2026-11-16T12:00:00Z', finished_at: null, last: '' })
let rows: AgentInfo[]
const api = { list_agents: async () => rows }
let unmount = async () => {}
let client: QueryClient
async function mount(sidebar = true) {
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  await act(async () => root.render(createElement(QueryClientProvider, { client, children: createElement(SidebarProvider, null,
    sidebar ? createElement(AgentTree, { session: 'root', active: undefined }) : createElement(SubagentRow, { row, active: false, now: 0, cost: undefined }),
  ) })))
  unmount = async () => { await act(async () => root.unmount()) }
}
beforeEach(() => {
  vi.clearAllMocks()
  rows = [row]
  useSessions.setState({ sessions: {} })
  usePanes.setState({ focused: null, drag: null })
})
afterEach(async () => {
  await unmount()
  client?.clear()
  document.body.innerHTML = ''
})

test.each([false, true])('a sidebar agent row pops out its chat (watch-only=%s), without starting a drag', async (observed) => {
  if (observed) rows = [{ ...row, kind: 'observed', observed: receiveComponent('method.agent.output.ObservedMeta', { source: 'omp_task', external: 'child', state: 'running', can_cancel: false, cost_rule: 'unreported', detail: null }) }]
  await mount()
  const button = document.querySelector<HTMLButtonElement>('button[aria-label="Open in new window"]')!
  expect(button.title).toBe('Open in new window')
  expect(button.querySelector('.lucide-app-window')).not.toBeNull()
  expect(button.closest('a')).toBeNull()
  expect(document.querySelectorAll('button[aria-label="Open in new window"]')).toHaveLength(1)
  await act(async () => {
    button.dispatchEvent(Object.assign(new MouseEvent('pointerdown', { bubbles: true, button: 0, clientX: 10 }), { pointerId: 1, pointerType: 'mouse' }))
    button.dispatchEvent(Object.assign(new MouseEvent('pointermove', { bubbles: true, clientX: 60 }), { pointerId: 1, pointerType: 'mouse' }))
    button.click()
  })
  expect(usePanes.getState().drag).toBeNull()
  expect(openPopout).toHaveBeenCalledExactlyOnceWith({ kind: 'chat', session: 'child' })
})

test('the shared Agents-page row does not gain sidebar-only chrome', async () => {
  await mount(false)
  expect(document.querySelector('a[href="/c/child"]')).not.toBeNull()
  expect(document.querySelector('button[aria-label="Open in new window"]')).toBeNull()
})
