import '@/components/settings/test-dom'
import { act, createElement as h } from 'react'
import { createRoot } from 'react-dom/client'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import { useConnection } from '@/store/connection'
import { usePowerLock } from './power-lock'

const fake = { secrets_power_event: vi.fn(async () => null) }
const shell = vi.hoisted(() => ({ handler: undefined as ((e: { payload: unknown }) => void) | undefined }))
vi.mock('@tauri-apps/api/event', () => ({
  listen: async (_: string, handler: (e: { payload: unknown }) => void) => {
    shell.handler = handler
    return () => undefined
  },
}))
vi.mock('./tauri', () => ({ isTauri: () => true }))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => fake }))

test('221 D20: the shell’s sleep and screen-lock reach secrets_power_event; any other payload is dropped', async () => {
  useConnection.setState({ discovery: { ...useConnection.getState().discovery, phase: 'legacy' } })
  const root = createRoot(document.createElement('div'))
  await act(async () => root.render(h(() => (usePowerLock(), null))))
  for (const payload of [{ kind: 'sleep' }, { kind: 'screen_lock' }, { kind: 'reboot' }, { kind: 1 }, null, 'sleep']) shell.handler!({ payload })
  expect(fake.secrets_power_event.mock.calls).toEqual([['sleep'], ['screen_lock']])
  await act(async () => root.unmount())
})
