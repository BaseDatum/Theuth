import { readyConnection } from '@/components/settings/test-engine'
import type * as ConnectionModule from '@/store/connection'
import '@/components/settings/test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement as h, Fragment } from 'react'
import { createRoot } from 'react-dom/client'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import { useDrafts } from '@/lib/drafts'
import type { McpServer } from "@/rpc/bindings/method.mcp_2dserver.output"
import { SessionMenu } from './SessionMenu'
import { ToolsSheet } from './ToolsSheet'
import { useToolsSheet } from './tools-sheet'

vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn(), message: vi.fn(), success: vi.fn() }) }))
vi.mock('@tanstack/react-router', async (original) => ({
  ...(await original<object>()),
  useNavigate: () => vi.fn(),
  useRouterState: ({ select }: { select: (s: { location: { pathname: string } }) => unknown }) => select({ location: { pathname: '/c/s1' } }),
}))

const listed: (string | null)[] = []
const fs = { name: 'fs', connected: true, gate: null, tools: [], repo: null, auth: 'none', home: null, home_name: null, home_here: true, everywhere: false, state: 'running', reason: null } as McpServer
// Every other reader answers an empty list; the menu and the sheet only need these.
const answers: Record<string, (...a: never[]) => unknown> = {
  mesh_status: () => ({ self_id: 'me', self_name: 'me', peers: [] }),
  get_approvals: () => ({ global: true, session: true }),
  get_communication: () => ({ global: true, effective: true, scope: 'owner', alias: null }),
  mcp_status: () => ({ connecting: [], connected: [], failed: [], homes_problem: null }),
  list_mcp: (session: string | null) => {
    listed.push(session)
    return [fs]
  },
}
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => new Proxy({}, { get: (_, k: string) => async (...a: never[]) => (answers[k] ?? (() => []))(...a) }),
}))
vi.mock('@/store/connection', async (original) => {
  const m = await original<typeof ConnectionModule>()
  return { ...m, useConnection: readyConnection(m.createConnection()) }
})

test('⋯ › Tools… opens the Tools sheet of that chat only, listing the servers for that chat', async () => {
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  // Two panes: s1 (its menu and sheet) and s2 (its sheet).
  await act(async () =>
    root.render(h(QueryClientProvider, { client: qc }, h(Fragment, null, h(SessionMenu, { session: 's1' }), h(ToolsSheet, { session: 's1' }), h(ToolsSheet, { session: 's2' })))),
  )
  const trigger = host.querySelector<HTMLButtonElement>('[aria-label="Session menu"]')!
  await act(async () => trigger.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })))
  const item = await vi.waitFor(() => {
    const found = [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((m) => m.textContent === 'Tools…')
    expect(found).toBeDefined()
    return found!
  })
  await act(async () => item.click())
  expect(useToolsSheet.getState().session).toBe('s1')
  await vi.waitFor(() => {
    const dialogs = document.querySelectorAll('[role="dialog"]')
    expect(dialogs).toHaveLength(1)
    expect(dialogs[0]!.textContent).toContain('Tools')
    expect(dialogs[0]!.textContent).toContain('fs')
  })
  expect(listed).toEqual(['s1'])
  await act(async () => root.unmount())
  host.remove()
})

test('the draft menu edits pending messaging and approvals and opens its own Tools sheet without creating a chat', async () => {
  useDrafts.setState({ settings: {}, opening: null })
  useToolsSheet.getState().hide()
  listed.length = 0
  const setters = { set_approvals: vi.fn(), set_communication: vi.fn(), create_session: vi.fn(), new_session: vi.fn() }
  Object.assign(answers, setters)
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  await act(async () => root.render(h(QueryClientProvider, { client: qc }, h(Fragment, null, h(SessionMenu, { session: '' }), h(ToolsSheet, { session: '' })))))
  const trigger = host.querySelector<HTMLButtonElement>('[aria-label="Session menu"]')!
  const show = async () => act(async () => trigger.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })))
  await show()
  const messaging = await vi.waitFor(() => {
    const item = [...document.querySelectorAll<HTMLElement>('[role="menuitemcheckbox"]')].find((m) => m.textContent === 'Agent messaging')!
    expect(item.getAttribute('aria-disabled')).not.toBe('true')
    return item
  })
  await act(async () => messaging.click())
  expect(useDrafts.getState().settings.communication).toBe(false)
  await show()
  const approvals = [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((m) => m.textContent?.includes('Approvals:'))!
  await act(async () => approvals.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true })))
  const off = await vi.waitFor(() => {
    const item = [...document.querySelectorAll<HTMLElement>('[role="menuitemradio"]')].find((m) => m.textContent === 'Off')
    expect(item).toBeDefined()
    return item!
  })
  await act(async () => off.click())
  expect(useDrafts.getState().settings.approvals).toBe(false)
  await show()
  expect(document.querySelector('[role="menuitemcheckbox"]')?.getAttribute('aria-checked')).toBe('false')
  const tools = [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((m) => m.textContent === 'Tools…')!
  await act(async () => tools.click())
  await vi.waitFor(() => expect(document.querySelector('[role="dialog"]')?.textContent).toContain('fs'))
  expect(listed).toEqual([null])
  for (const setter of Object.values(setters)) expect(setter).not.toHaveBeenCalled()
  await act(async () => root.unmount())
  host.remove()
  useDrafts.setState({ settings: {} })
})
