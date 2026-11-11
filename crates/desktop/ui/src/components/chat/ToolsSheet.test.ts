import '@/components/settings/test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement as h, Fragment } from 'react'
import { createRoot } from 'react-dom/client'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import type { McpServer } from '@/rpc/bindings/McpServer'
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
