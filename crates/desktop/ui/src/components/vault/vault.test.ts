import '@/components/settings/test-dom'
import { QueryClient, QueryClientProvider, notifyManager } from '@tanstack/react-query'
import { act, createElement as h, type ReactElement } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { RpcError } from '@/rpc/client'
import { useConnection } from '@/store/connection'
import type { UnsealRequestView, VaultStatus } from './api'
import { UNSEAL_POLL_MS, UnsealHost } from './UnsealHost'
import { UnsealPrompt } from './UnsealPrompt'
import { VaultSettings } from './VaultSettings'

vi.hoisted(() => vi.stubGlobal('localStorage', { getItem: () => null, setItem: () => undefined }))
vi.mock('@/lib/notify', () => ({ notify: Object.assign(vi.fn(), { error: vi.fn(), success: vi.fn(), warning: vi.fn() }) }))
vi.mock('@/components/tasks/queries', () => ({ useMesh: () => ({ data: undefined }) }))
notifyManager.setScheduler(queueMicrotask)

const status: VaultStatus = {
  engine: 'e1',
  name: 'sky-mac',
  state: 'unsealed',
  unsealed_by: 'keychain',
  methods: [{ kind: 'keychain', id: 'keychain', label: 'This Mac’s keychain', created_ms: 0 }],
  password_set: false,
  warning: null,
  asking: [],
  error: null,
}
const unlock: UnsealRequestView = { id: 'u1', node: 'n1', node_name: 'sky-cloud', purpose: 'unlock', created_ms: 0 }

const replies: Record<string, (params: unknown[]) => unknown> = {
  vault_status: () => status,
  vault_unlock_devices: () => [],
  vault_answer_unseal: () => true,
  vault_unseal_requests: () => [],
  vault_add_password: () => ({ ...status, password_set: true }),
}
const call = vi.fn(async (method: string, params: unknown[]) => replies[method]!(params))

let root: Root | undefined
beforeEach(() => {
  vi.clearAllMocks()
  useConnection.setState({ client: { call } as never })
})
afterEach(async () => {
  await act(async () => root?.unmount())
  root = undefined
  document.body.innerHTML = ''
  vi.useRealTimers()
})
async function mount(element: ReactElement) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const host = document.createElement('div')
  document.body.append(host)
  root = createRoot(host)
  await act(async () => root!.render(h(QueryClientProvider, { client: qc }, element)))
}
const button = (text: string) => [...document.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent?.trim() === text)
const click = (el: Element) => act(async () => (el as HTMLElement).click())
const input = (label: string) => document.querySelector<HTMLInputElement>(`input[aria-label="${label}"]`)!
const type = (el: HTMLInputElement, value: string) =>
  act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(el, value)
    el.dispatchEvent(new Event('input', { bubbles: true }))
  })
const calls = (method: string) => call.mock.calls.filter(([m]) => m === method).map(([, params]) => params)

test('225: Unlock with "always" on answers vault_answer_unseal(id, true, true) and drops the card', async () => {
  const onDone = vi.fn()
  await mount(h(UnsealPrompt, { request: unlock, onDone }))
  expect(document.body.textContent).toContain('sky-cloud restarted — unlock its vault?')
  await click(document.querySelector('[role="switch"]')!)
  await click(button('Unlock')!)
  expect(calls('vault_answer_unseal')).toEqual([['u1', true, true]])
  expect(onDone).toHaveBeenCalledWith('u1')
})

test('225: Not now declines without always, even with the switch on', async () => {
  await mount(h(UnsealPrompt, { request: unlock }))
  await click(document.querySelector('[role="switch"]')!)
  await click(button('Not now')!)
  expect(calls('vault_answer_unseal')).toEqual([['u1', false, false]])
})

test('225: an enroll asks to Allow; a gate that says no sends nothing', async () => {
  const beforeApprove = vi.fn(async () => false)
  await mount(h(UnsealPrompt, { request: { ...unlock, purpose: 'enroll' }, beforeApprove }))
  expect(document.body.textContent).toContain('sky-cloud wants this device to unlock its vault')
  await click(button('Allow')!)
  expect(beforeApprove).toHaveBeenCalledOnce()
  expect(calls('vault_answer_unseal')).toEqual([])
})

test('225: the host shows polled requests, hides an answered one, and stops polling an engine without vault methods', async () => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] })
  call.mockImplementation(async (method: string) => (method === 'vault_unseal_requests' ? [unlock] : true))
  await mount(h(UnsealHost))
  expect(document.body.textContent).toContain('sky-cloud restarted')
  await click(button('Unlock')!)
  expect(document.body.textContent).not.toContain('sky-cloud restarted')

  await act(async () => root?.unmount())
  root = undefined
  call.mockClear()
  call.mockImplementation(async () => {
    throw new RpcError(-32601, 'Method not found')
  })
  await mount(h(UnsealHost))
  await act(async () => vi.advanceTimersByTime(UNSEAL_POLL_MS * 3))
  expect(calls('vault_unseal_requests')).toHaveLength(1)
  call.mockImplementation(async (method: string, params: unknown[]) => replies[method]!(params))
})

test('225: Add password refuses a short or mismatched password inline without calling the engine', async () => {
  await mount(h(VaultSettings, { focus: null }))
  await click(button('Add password')!)
  const strong = 'Xk7#Qm2$vL9pRw4!'

  await type(input('new password for sky-mac'), 'short')
  await type(input('confirm new password for sky-mac'), 'short')
  await click(button('Add password')!)
  expect(document.querySelector('[role="alert"]')?.textContent).toBe('Use at least 12 characters')

  await type(input('new password for sky-mac'), strong)
  await type(input('confirm new password for sky-mac'), `${strong}x`)
  await click(button('Add password')!)
  expect(document.querySelector('[role="alert"]')?.textContent).toBe('The passwords do not match')
  expect(input('confirm new password for sky-mac').value).toBe('')
  expect(calls('vault_add_password')).toEqual([])

  await type(input('confirm new password for sky-mac'), strong)
  await click(button('Add password')!)
  expect(calls('vault_add_password')).toEqual([[null, strong]])
  expect(button('Change password')).toBeDefined()
})
