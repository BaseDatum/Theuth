import { readyConnection } from '@/components/settings/test-engine'
import type * as ConnectionModule from '@/store/connection'
import '@/components/settings/test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement as h } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import { container } from '@/lib/platform'
import { hasLocal } from '@/link/engines'
import { useConnection } from '@/store/connection'
import { SessionMenu } from './SessionMenu'

vi.mock('@/lib/notify', () => ({ notify: Object.assign(vi.fn(), { error: vi.fn(), message: vi.fn(), success: vi.fn() }) }))
vi.mock('@tanstack/react-router', async (original) => ({
  ...(await original<object>()),
  useNavigate: () => vi.fn(),
  useRouterState: ({ select }: { select: (s: { location: { pathname: string } }) => unknown }) => select({ location: { pathname: '/c/s1' } }),
}))
const answers: Record<string, () => unknown> = {
  mesh_status: () => ({ self_id: 'me', self_name: 'me', peers: [] }),
  get_approvals: () => ({ global: true, session: true }),
  get_communication: () => ({ global: true, effective: true, scope: 'owner', alias: null }),
}
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => new Proxy({}, { get: (_, k: string) => async () => (answers[k] ?? (() => []))() }),
}))
vi.mock('@/store/connection', async (original) => {
  const m = await original<typeof ConnectionModule>()
  return { ...m, useConnection: readyConnection(m.createConnection()) }
})

const ready = useConnection.getState().discovery
afterEach(() => {
  vi.restoreAllMocks()
  Reflect.deleteProperty(window, '__TAURI_INTERNALS__')
  useConnection.setState({ discovery: ready })
  document.body.innerHTML = ''
})

/** The ⋯ menu's item labels for chat s1, once the menu has rendered its Tools… row. */
async function items(): Promise<string[]> {
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  await act(async () => root.render(h(QueryClientProvider, { client: new QueryClient({ defaultOptions: { queries: { retry: false } } }) }, h(SessionMenu, { session: 's1' }))))
  const trigger = host.querySelector<HTMLButtonElement>('[aria-label="Session menu"]')!
  await act(async () => trigger.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })))
  const labels = await vi.waitFor(() => {
    const all = [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].map((m) => m.textContent ?? '')
    expect(all).toContain('Tools…')
    return all
  })
  await act(async () => root.unmount())
  host.remove()
  return labels
}

test.each(['ios', 'web'] as const)('183 §12.10 lifted: an %s container without a local engine offers ⋯ Share with contact… when the engine has the share methods, never when one is missing', async (c) => {
  if (c === 'ios') {
    Object.assign(window, { __TAURI_INTERNALS__: {} })
    vi.spyOn(navigator, 'userAgent', 'get').mockReturnValue('Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15')
  }
  expect(container()).toBe(c)
  expect(hasLocal()).toBe(false)
  expect(await items()).toContain('Share with contact…')
  useConnection.setState({ discovery: { ...ready, unavailable: new Set(['a2a_export_put']) } })
  expect(await items()).not.toContain('Share with contact…')
})
