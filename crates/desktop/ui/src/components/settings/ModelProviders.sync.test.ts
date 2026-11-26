import './test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement } from 'react'
import { createRoot } from 'react-dom/client'
import { expect, test, vi } from 'vitest'
import { queryKeys } from '@/lib/rpc'
import type * as Rpc from '@/lib/rpc'
import type { ProviderSyncReport } from '@/rpc/bindings/method.agent.output'
import { disconnected } from '@/rpc/discovery'
import { useConnection } from '@/store/connection'
import { ModelProviders } from './ModelProviders'

const report: ProviderSyncReport = {
  target: 'p50-engine',
  results: [
    { name: 'ollama', status: 'skipped', detail: 'its address is loopback: it points at this machine, not the target' },
    { name: 'rwx', status: 'added', detail: null },
    { name: 'favorite models', status: 'updated', detail: 'replaced with 3 favorites' },
  ],
}
const api = vi.hoisted(() => ({ sync_providers: vi.fn(async () => report as unknown) }))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => api }))

const rwx = { name: 'rwx', base_url: 'https://x.example/v1/', models: 1, builtin: false, configured: true, refreshable: true, local: false, runtime: null, has_key: true }
const peer = (id: string, name: string, reachable: boolean) => ({ id, name, reachable, last_seen_ms: 0, path: 'lan' })

test('Copy providers to… sends the picked online engine, keys off and favorites on by default, and shows one line per provider', async () => {
  useConnection.setState({ discovery: { ...disconnected(), phase: 'legacy' } })
  const client = new QueryClient()
  client.setQueryData(queryKeys.providers, [rwx])
  client.setQueryData(queryKeys.logins, [])
  client.setQueryData(['mesh'], {
    self_id: 'self-engine',
    self_name: 'Laptop',
    peers: [peer('self-engine', 'Laptop', true), peer('p50-engine', 'P50', true), peer('off-engine', 'Mini', false)],
    stats: [],
  })
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  await act(async () => root.render(createElement(QueryClientProvider, { client }, createElement(ModelProviders))))
  const buttons = () => [...document.body.querySelectorAll('button')]
  await act(async () => buttons().find((b) => b.textContent?.includes('Copy providers to'))!.click())
  const mini = buttons().find((b) => b.textContent?.startsWith('Mini'))!
  expect(mini.disabled).toBe(true)
  const [keys, favorites] = [...document.body.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')].map((c) => c.checked)
  expect([keys, favorites]).toEqual([false, true])
  await act(async () => buttons().find((b) => b.textContent?.startsWith('P50'))!.click())
  expect(api.sync_providers).toHaveBeenCalledWith('p50-engine', null, false, true)
  expect(document.body.textContent).toContain('Copied to P50')
  expect(document.body.textContent).toContain('rwx · added')
  expect(document.body.textContent).toContain('ollama · skipped')
  expect(document.body.textContent).toContain('favorite models · updated')
  await act(async () => root.unmount())
})
