import '@/components/settings/test-dom'
import { QueryClient, QueryClientProvider, notifyManager } from '@tanstack/react-query'
import { act, createElement as h, type ReactElement } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { renderToStaticMarkup } from 'react-dom/server'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { Name } from '@/components/library/Tools'
import { useConnection } from '@/store/connection'
import type * as Rpc from '@/lib/rpc'
import type { ConnInfo, ListPage } from '@/rpc/bindings/method.secrets.output'
import { Browse } from './Browse'
import { REVEAL_MS } from './Reveal'
import { SendDialog } from './SendDialog'
import { CONN_DEFAULTS } from './types'

vi.hoisted(() => vi.stubGlobal('localStorage', { getItem: () => null, setItem: () => undefined }))
const SECRET = 'hunter2-the-revealed-value'
const fake = {
  list_external_secrets: vi.fn(
    async (): Promise<ListPage> => ({
      entries: [{ ref: 'bw/personal/2f1c', name: 'GitHub', kind: 'login', keys: ['password', 'username'], location: 'Personal / Dev', recoverable_until: null, versions: [{ id: '2026-10-30T00:00:00Z', stages: [], changed: '2026-10-30T00:00:00Z' }] }],
      next: null,
    }),
  ),
  reveal_external_secret: vi.fn(async () => SECRET),
  list_secret_connections: vi.fn(async (): Promise<ConnInfo[]> => [info()]),
  get_external_settings: vi.fn(async () => ({ send_max_file_mb: 100, copy_link_needs_press: true, timeout_secs: 20 })),
}
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => fake }))
vi.mock('@/lib/notify', () => ({ notify: Object.assign(vi.fn(), { error: vi.fn(), success: vi.fn() }) }))
notifyManager.setScheduler(queueMicrotask)

function info(allow_reveal = true): ConnInfo {
  return {
    conn: 'personal',
    backend: 'bw',
    login: 'api_key',
    status: { state: 'ready' },
    summary: 'me@example.org · vault.bitwarden.eu',
    ask: { read: true, write: true, delete: true, send: true },
    settings: { ...CONN_DEFAULTS, allow_reveal },
  }
}

let root: Root | undefined
let qc: QueryClient
beforeEach(() => {
  useConnection.setState({ discovery: { ...useConnection.getState().discovery, phase: 'legacy' } })
})
afterEach(async () => {
  await act(async () => root?.unmount())
  root = undefined
  qc?.clear()
  document.body.innerHTML = ''
  vi.useRealTimers()
})
async function mount(element: ReactElement) {
  qc = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: Infinity }, mutations: { retry: false, gcTime: Infinity } } })
  const host = document.createElement('div')
  document.body.append(host)
  root = createRoot(host)
  await act(async () => root!.render(h(QueryClientProvider, { client: qc }, element)))
  return host
}
const button = (text: string) => [...document.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent?.trim() === text)
const click = (el: HTMLElement) => act(async () => el.click())
/** The entry's name button, which opens its value rows. */
const row = () => [...document.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent?.startsWith('GitHub'))!

test('D25: Reveal is absent on every value row when the connection turns allow_reveal off', async () => {
  await mount(h(Browse, { info: info(false) }))
  // The row's name button opens its value rows.
  await click(row())
  expect(document.body.textContent).toContain('bw/personal/2f1c')
  expect(document.querySelector('[aria-label^="Reveal"]')).toBeNull()

  await act(async () => root!.render(h(QueryClientProvider, { client: qc }, h(Browse, { info: info(true) }))))
  expect(document.querySelector('[aria-label="Reveal password"]')).not.toBeNull()
  expect(document.querySelector('[aria-label="Reveal username"]')).not.toBeNull()
})

test('D25: a revealed value is shown masked, never enters the query or mutation cache, and is dropped after 30 s', async () => {
  await mount(h(Browse, { info: info(true) }))
  await click(row())
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] })
  await click(document.querySelector<HTMLButtonElement>('[aria-label="Reveal password"]')!)
  expect(fake.reveal_external_secret).toHaveBeenCalledWith('personal', 'bw/personal/2f1c#password')
  const field = document.querySelector<HTMLInputElement>('input[aria-label="value of password"]')!
  expect(field.value).toBe(SECRET)
  expect(field.type).toBe('password')

  const cached = JSON.stringify([...qc.getQueryCache().getAll().map((q) => q.state), ...qc.getMutationCache().getAll().map((m) => m.state)])
  expect(cached).toContain('GitHub')
  expect(cached).not.toContain(SECRET)

  await act(async () => vi.advanceTimersByTime(REVEAL_MS))
  expect(document.querySelector('input[aria-label="value of password"]')).toBeNull()
  expect(document.body.innerHTML).not.toContain(SECRET)
})

test('§8: Settings › Approvals labels a secret: always-ask key by connection, backend and direction', () => {
  expect(renderToStaticMarkup(h(Name, { name: 'secret:bw/personal:read' }))).toBe('<span title="secret:bw/personal:read">personal (Bitwarden) · read</span>')
  expect(renderToStaticMarkup(h(Name, { name: 'secret:aws/prod:delete' }))).toContain('prod (AWS) · delete')
  // Anything else keeps its plain, wrap-at-underscore rendering.
  expect(renderToStaticMarkup(h(Name, { name: 'secret_delete' }))).toBe('secret_<wbr/>delete')
  expect(renderToStaticMarkup(h(Name, { name: 'secret:bw/Personal:read' }))).not.toContain('Bitwarden')
})

test('§12.5: the Send dialog turns file mode off for a file over send_max_file_mb, and back on under it', async () => {
  const MB = 1024 * 1024
  await mount(h(SendDialog, { open: true, onOpenChange: () => undefined, initial: { kind: 'file', file: 'artifact:a1', label: 'artifact dump.sql', bytes: 101 * MB } }))
  const file = () => [...document.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent === 'File')!
  expect(file().disabled).toBe(true)
  expect(document.body.textContent).toContain('Over the 100 MB limit')
  expect(button('Create Send')?.disabled).toBe(true)

  await act(async () => root!.unmount())
  document.body.innerHTML = ''
  await mount(h(SendDialog, { open: true, onOpenChange: () => undefined, initial: { kind: 'file', file: 'artifact:a2', label: 'artifact small.pdf', bytes: 99 * MB } }))
  expect(document.body.textContent).not.toContain('Over the 100 MB limit')
  expect(file().disabled).toBe(false)
})
