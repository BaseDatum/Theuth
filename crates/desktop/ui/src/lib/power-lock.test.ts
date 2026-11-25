import '@/components/settings/test-dom'
import { act, createElement as h } from 'react'
import { createRoot } from 'react-dom/client'
import { expect, test, vi } from 'vitest'
import type { Api } from '@/rpc/bindings/client'
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

test('221 D20: mounts before the connection opens; once connected, sleep and screen-lock reach secrets_power_event and any other payload is dropped', async () => {
  useConnection.setState({ api: null, discovery: { ...useConnection.getState().discovery, phase: 'legacy' } })
  const root = createRoot(document.createElement('div'))
  await act(async () => root.render(h(() => (usePowerLock(), null))))
  expect(shell.handler).toBeUndefined()
  await act(async () => useConnection.setState({ api: fake as unknown as Api }))
  for (const payload of [{ kind: 'sleep' }, { kind: 'screen_lock' }, { kind: 'reboot' }, { kind: 1 }, null, 'sleep']) shell.handler!({ payload })
  expect(fake.secrets_power_event.mock.calls).toEqual([['sleep'], ['screen_lock']])
  await act(async () => root.unmount())
})
