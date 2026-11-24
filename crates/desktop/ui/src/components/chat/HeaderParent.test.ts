import '../settings/test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement as h, type ReactElement } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, expect, test, vi } from 'vitest'
import { TooltipProvider } from '@/components/ui/tooltip'
import { receiveComponent } from '@/rpc/bindings/receive'
import type { SessionSummary } from '@/rpc/bindings/method.agent.output'

const navigate = vi.hoisted(() => vi.fn())
vi.mock('@tanstack/react-router', async (original) => ({ ...(await original<object>()), useNavigate: () => navigate }))
vi.mock('@/rpc/capabilities', async (original) => ({ ...(await original<object>()), useQuerySupport: () => true }))
const parent: SessionSummary = receiveComponent('method.agent.output.SessionSummary', {
  id: 'p1', created_at: '2026-11-24T10:00:00Z', updated_at: '2026-11-24T10:00:00Z', title: 'Chat UI polish: sidebar rail', tags: [], model: 'm',
  parent: null, name: null, effort: null, agent: null, cwd: null, pinned: false, archived: false, project: null, notebook: null,
  retention: 'durable', running: false, owner: '', epoch: 1, memory: 'epochs', here: true,
})
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof import('@/lib/rpc')>()), useApi: () => ({ list_sessions: async () => [parent] }) }))

const { ParentItem } = await import('./Header')

let unmount = async () => {}
afterEach(async () => {
  await unmount()
  document.body.innerHTML = ''
  navigate.mockClear()
})
async function mount(el: ReactElement) {
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  const client = new QueryClient()
  await act(async () => root.render(h(QueryClientProvider, { client }, h(TooltipProvider, null, el))))
  await act(async () => new Promise((r) => setTimeout(r, 0)))
  unmount = async () => {
    await act(async () => root.unmount())
    client.clear()
  }
  return host
}

test("a subagent's header: an up-arrow button naming the parent, no parent title text; it opens the parent", async () => {
  const host = await mount(h(ParentItem, { parent: 'p1', mode: 'full' }))
  const button = host.querySelector<HTMLButtonElement>('button[aria-label="Open parent: Chat UI polish: sidebar rail"]')!
  expect(button).not.toBeNull()
  expect(button.querySelector('.lucide-corner-left-up')).not.toBeNull()
  // The freed room is the subagent title's: the parent's name is only the label/tooltip, never text in the bar.
  expect(host.textContent).not.toContain('Chat UI polish')
  expect(host.textContent).not.toContain('↳')
  await act(async () => button.click())
  expect(navigate).toHaveBeenCalledExactlyOnceWith({ to: '/c/$session', params: { session: 'p1' } })
})

test('the compact bar draws the same button (phone, narrow panes, pop-out windows share the header)', async () => {
  const host = await mount(h(ParentItem, { parent: 'p1', mode: 'bar' }))
  expect(host.querySelector('button[aria-label="Open parent: Chat UI polish: sidebar rail"]')).not.toBeNull()
  expect(host.textContent).not.toContain('Chat UI polish')
})

test('a top-level chat has no parent button', async () => {
  const host = await mount(h(ParentItem, { parent: null, mode: 'full' }))
  expect(host.querySelector('button')).toBeNull()
  expect(host.innerHTML).toBe('')
})
