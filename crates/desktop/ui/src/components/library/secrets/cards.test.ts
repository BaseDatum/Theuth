import '@/components/settings/test-dom'
import { QueryClient, QueryClientProvider, notifyManager } from '@tanstack/react-query'
import { act, createElement as h, type ReactElement, type ReactNode } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { Licences } from '@/components/settings/Licences'
import type * as Rpc from '@/lib/rpc'
import { notify } from '@/lib/notify'
import { useConnection } from '@/store/connection'
import type { ConnInfo, LoginStep } from '@/rpc/bindings/method.secrets.output'
import { SendDialog } from './SendDialog'
import { CONN_DEFAULTS } from './types'
import { LoginCard, UnlockRequestCard } from './UnlockCard'

vi.hoisted(() => vi.stubGlobal('localStorage', { getItem: () => null, setItem: () => undefined }))
const fake = {
  answer_unlock: vi.fn(async (): Promise<boolean> => true),
  connection_login: vi.fn(async (): Promise<LoginStep> => ({ step: 'done' })),
  create_send: vi.fn(() => new Promise<never>(() => undefined)),
  list_secret_connections: vi.fn(
    async (): Promise<ConnInfo[]> => [
      { conn: 'personal', backend: 'bw', status: { state: 'ready' }, summary: 'me@example.org', ask: { read: true, write: true, delete: true, send: true }, settings: CONN_DEFAULTS },
    ],
  ),
  get_external_settings: vi.fn(async () => ({ send_max_file_mb: 100, copy_link_needs_press: true, timeout_secs: 20 })),
}
const invoke = vi.hoisted(() => vi.fn())
vi.mock('@tauri-apps/api/core', () => ({ invoke }))
vi.mock('@tanstack/react-router', () => ({ Link: ({ to, children }: { to: string; children: ReactNode }) => h('a', { href: to }, children) }))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => fake }))
vi.mock('@/lib/notify', () => ({ notify: Object.assign(vi.fn(), { error: vi.fn(), success: vi.fn(), warning: vi.fn() }) }))
notifyManager.setScheduler(queueMicrotask)

let root: Root | undefined
beforeEach(() => {
  useConnection.setState({ discovery: { ...useConnection.getState().discovery, phase: 'legacy' } })
  vi.clearAllMocks()
})
afterEach(async () => {
  await act(async () => root?.unmount())
  root = undefined
  document.body.innerHTML = ''
})
async function mount(element: ReactElement) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const host = document.createElement('div')
  document.body.append(host)
  root = createRoot(host)
  await act(async () => root!.render(h(QueryClientProvider, { client: qc }, element)))
}
const button = (text: string) => [...document.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent?.trim() === text)
const click = (el: HTMLElement) => act(async () => el.click())
const input = (label: string) => document.querySelector<HTMLInputElement>(`input[aria-label="${label}"]`)!
const type = (el: HTMLInputElement, value: string) =>
  act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(el, value)
    el.dispatchEvent(new Event('input', { bubbles: true }))
  })
const request = { id: 'r1', conn: 'personal', backend: 'bw', account: 'me@example.org', reason: 'bw/personal/2f1c#password' }

test('§9.4: an unlock card answered elsewhere goes away and says so instead of waiting', async () => {
  fake.answer_unlock.mockResolvedValueOnce(false)
  const onDone = vi.fn()
  await mount(h(UnlockRequestCard, { request, onDone }))
  await type(input('master password for personal'), 'pw')
  await click(button('Unlock')!)
  expect(fake.answer_unlock).toHaveBeenCalledWith('r1', 'pw')
  expect(onDone).toHaveBeenCalledOnce()
  expect(notify.warning).toHaveBeenCalledWith(expect.stringContaining('already answered'))
})

test('§9.4: a Cancel the engine refuses is reported and the card still goes', async () => {
  fake.answer_unlock.mockRejectedValueOnce(new Error('engine gone'))
  const onDone = vi.fn()
  await mount(h(UnlockRequestCard, { request, onDone }))
  await click(button('Cancel')!)
  expect(fake.answer_unlock).toHaveBeenCalledWith('r1', null)
  expect(notify.error).toHaveBeenCalledWith('engine gone')
  expect(onDone).toHaveBeenCalledOnce()
})

test('§9.4: the login card holds Continue until the 2FA code is typed, and offers Cancel', async () => {
  fake.connection_login.mockResolvedValueOnce({ step: 'need_two_factor', providers: ['authenticator'] })
  const onCancel = vi.fn()
  await mount(h(LoginCard, { conn: 'personal', email: 'me@example.org', method: 'password', onDone: () => undefined, onCancel }))
  await type(input('master password'), 'pw')
  await click(button('Log in')!)
  expect(button('Continue')?.disabled).toBe(true)
  await type(input('two-factor code'), '123456')
  expect(button('Continue')?.disabled).toBe(false)
  await click(button('Continue')!)
  expect(fake.connection_login).toHaveBeenLastCalledWith('personal', expect.objectContaining({ two_factor: ['authenticator', '123456'] }))
  await click(button('Cancel')!)
  expect(onCancel).toHaveBeenCalledOnce()
})

test('§12.5: a Send whose password is typed on a card points to the inbox while it waits', async () => {
  await mount(h(SendDialog, { open: true, onOpenChange: () => undefined, initial: { kind: 'text', source: 'db_password', label: 'db_password' } }))
  await click(button('Type one on a card')!)
  await click(button('Create Send')!)
  expect(fake.create_send).toHaveBeenCalledWith('personal', expect.objectContaining({ password: { ask: true } }))
  const status = document.querySelector('[role="status"]')!
  expect(status.textContent).toContain('A card asks for the Send')
  expect(status.querySelector('a')?.getAttribute('href')).toBe('/attention')
  expect(button('Creating…')?.disabled).toBe(true)
})

test('Licences: a slower earlier legal_text never replaces the file pressed last', async () => {
  const first = Promise.withResolvers<string>()
  invoke.mockReturnValueOnce(first.promise).mockResolvedValueOnce('NOTICE TEXT')
  await mount(h(Licences))
  await click(button('Licence')!)
  await click(button('Notice')!)
  expect(invoke).toHaveBeenNthCalledWith(1, 'legal_text', { name: 'LICENSE' })
  expect(invoke).toHaveBeenNthCalledWith(2, 'legal_text', { name: 'NOTICE' })
  await act(async () => first.resolve('GPL TEXT'))
  expect(document.body.textContent).toContain('NOTICE TEXT')
  expect(document.body.textContent).not.toContain('GPL TEXT')
})
