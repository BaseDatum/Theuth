import './test-dom'
import { QueryClient, QueryClientProvider, notifyManager } from '@tanstack/react-query'
import { Fragment, act, createElement } from 'react'
import { createRoot } from 'react-dom/client'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import { useRestartPending } from '@/lib/restart-pending'
import type { ConfigFlagKey } from '@/rpc/api'
import { ConfigFlagRow } from './ConfigFlagRow'
import { RestartBanner } from './page/RestartBanner'

// Every flag is saved on and still runs off: each waits for a restart.
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({ config_flag_get: async (key: ConfigFlagKey) => ({ key, enabled: true, running: false, restart_required: true }) }),
}))
// React Query notifies observers on a macrotask by default; on a microtask they settle inside one `act`.
notifyManager.setScheduler(queueMicrotask)

test('the restart banner names each pending flag by its feature, not its row label (188 X2)', async () => {
  useRestartPending.getState().reset()
  const host = document.createElement('div')
  document.body.append(host)
  const root = createRoot(host)
  const rows = [
    createElement(ConfigFlagRow, { key: 'd', flag: 'decision.enabled', name: 'Decision models', label: 'Enabled', description: '' }),
    createElement(ConfigFlagRow, { key: 'c', flag: 'computer.enabled', name: 'Computer use', label: 'Enabled', description: '' }),
    createElement(RestartBanner, { key: 'b' }),
  ]
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  await act(async () => root.render(createElement(QueryClientProvider, { client: qc }, createElement(Fragment, null, rows))))
  expect(host.querySelector('[role="status"]')?.textContent).toContain('2 changes apply after theuth restarts: Decision models, Computer use.')
  await act(async () => root.unmount())
  useRestartPending.getState().reset()
})
