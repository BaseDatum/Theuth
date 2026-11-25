import '@/components/settings/test-dom'
import { QueryClient, QueryClientProvider, notifyManager } from '@tanstack/react-query'
import { act, createElement as h, type ReactElement, type ReactNode } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { Licences } from '@/components/settings/Licences'
import type * as Rpc from '@/lib/rpc'
import { notify } from '@/lib/notify'
import { useConnection } from '@/store/connection'
import type { ConnInfo, LoginStep, SendRecord } from '@/rpc/bindings/method.secrets.output'
import { SendDialog } from './SendDialog'
import { Sends } from './Sends'
import { CONN_DEFAULTS } from './types'
import { LoginCard, UnlockRequestCard } from './UnlockCard'

const send = (o: Partial<SendRecord>): SendRecord => ({
  access_count: 0,
  conn: 'personal',
  created_ms: 0,
  deletion_ms: 2_000_000_000_000,
  expiration_ms: null,
  file_bytes: null,
  file_name: null,
  has_password: false,
  id: 's',
  kind: 'file',
  link_secret: null,
  max_access: null,
  name: 'Shared from theuth',
  source_ref: 'db_password',
  status: 'active',
  ...o,
})

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
  list_sends: vi.fn(async (): Promise<SendRecord[]> => [
    send({ id: 'text', file_name: '.env', file_bytes: null }),
    send({ id: 'file', file_name: 'dump.sql', file_bytes: 2048, source_ref: 'artifact:a1' }),
  ]),
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

test('§12.5: a text sent as a file shows its file name and no size; a real file keeps its size', async () => {
  await mount(h(Sends, { info: (await fake.list_secret_connections())[0]! }))
  const rows = [...document.querySelectorAll('.truncate')].map((e) => e.textContent ?? '')
  const text = rows.find((r) => r.startsWith('.env'))!
  expect(text).toBe('.env · active')
  expect(rows.find((r) => r.startsWith('dump.sql'))).toContain('dump.sql · 2.0 KB')
})

function ssoFake() {
  const browser = Promise.withResolvers<string>()
  const deps = {
    listen: vi.fn(async () => 8066),
    wait: vi.fn(() => browser.promise),
    cancel: vi.fn(async () => undefined),
    open: vi.fn(),
    begin: vi.fn(async () => ({ authorize_url: 'https://id.example/connect/authorize?x', state: 'st', redirect_port: 8066 })),
    complete: vi.fn(async (): Promise<LoginStep> => ({ step: 'done' })),
  }
  return { deps, browser }
}
const sso = (onDone = vi.fn(), deps = ssoFake().deps) => h(LoginCard, { conn: 'work', email: 'me@example.org', method: 'sso', org: 'acme', onDone, ssoDeps: deps })
/** The card's submit (the method toggle has a "Log in with SSO" item too). */
const ssoSubmit = () => [...document.querySelectorAll<HTMLButtonElement>('button[type="submit"]')].find((b) => b.textContent === 'Log in with SSO')

test('§9.6: SSO binds, begins on that port, opens the browser, waits, and answers 2FA with the same redirect', async () => {
  const { deps, browser } = ssoFake()
  deps.complete.mockResolvedValueOnce({ step: 'need_two_factor', providers: ['authenticator', 'email'] })
  const onDone = vi.fn()
  await mount(sso(onDone, deps))
  await click(ssoSubmit()!)
  expect(deps.begin).toHaveBeenCalledWith('work', 'acme', 8066)
  expect(deps.open).toHaveBeenCalledWith('https://id.example/connect/authorize?x')
  expect(deps.wait).toHaveBeenCalledWith(8066)
  expect(document.querySelector('[role="status"]')?.textContent).toContain('Finish signing in in your browser')
  expect(button('Cancel')).toBeDefined()

  await act(async () => browser.resolve('http://localhost:8066/?code=c&state=st'))
  expect(deps.complete).toHaveBeenLastCalledWith('work', 'st', 'http://localhost:8066/?code=c&state=st', null)
  expect(button('Continue')?.disabled).toBe(true)
  await type(input('two-factor code'), '654321')
  await click(button('Continue')!)
  expect(deps.complete).toHaveBeenLastCalledWith('work', 'st', 'http://localhost:8066/?code=c&state=st', ['authenticator', '654321'])
  expect(onDone).toHaveBeenCalledOnce()
})

test('§9.6: Cancel while the browser is open closes the listener and returns to the start', async () => {
  const { deps, browser } = ssoFake()
  await mount(sso(vi.fn(), deps))
  await click(ssoSubmit()!)
  await click(button('Cancel')!)
  expect(deps.cancel).toHaveBeenCalled()
  expect(ssoSubmit()).toBeDefined()
  // The browser coming back afterwards completes nothing.
  await act(async () => browser.resolve('http://localhost:8066/?code=c&state=st'))
  expect(deps.complete).not.toHaveBeenCalled()
})

test('§9.6: engine refusals show verbatim; one before the browser answers closes the listener', async () => {
  const { deps } = ssoFake()
  const prevalidate = 'SSO is not enabled for this organization (SSO identifier acme)'
  deps.begin.mockRejectedValueOnce(new Error(prevalidate))
  await mount(sso(vi.fn(), deps))
  await click(ssoSubmit()!)
  expect(document.body.textContent).toContain(prevalidate)
  expect(deps.cancel).toHaveBeenCalledOnce()
  expect(deps.open).not.toHaveBeenCalled()

  // After the browser answered, the shell already closed the listener; the refusal still shows verbatim.
  const refusal = "this account unlocks with trusted devices / Key Connector, which theuth doesn't support yet"
  deps.complete.mockRejectedValueOnce(new Error(refusal))
  deps.wait.mockResolvedValueOnce('http://localhost:8066/?code=c&state=st')
  await click(ssoSubmit()!)
  expect(document.body.textContent).toContain(refusal)
  expect(ssoSubmit()?.disabled).toBe(false)
})

test('§9.6: the password option points an SSO organisation to SSO or an API key', async () => {
  await mount(h(LoginCard, { conn: 'work', email: 'me@example.org', method: 'password', onDone: () => undefined }))
  expect(document.body.textContent).toContain('Your organisation requires SSO? Use Log in with SSO or API key.')
})
