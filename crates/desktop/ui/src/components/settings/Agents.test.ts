import './test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement } from 'react'
import { createRoot } from 'react-dom/client'
import { beforeEach, expect, test, vi } from 'vitest'
import { queryKeys } from '@/lib/rpc'
import type * as Rpc from '@/lib/rpc'
import type { EntryInfo } from '@/rpc/bindings/method.agent.output'
import { Agents } from './Agents'

const notify = vi.hoisted(() =>
  Object.assign(vi.fn(), { error: vi.fn(), message: vi.fn(), success: vi.fn(), warning: vi.fn() }),
)
const api = vi.hoisted(() => ({
  list_acp: vi.fn(async (_probe: boolean): Promise<EntryInfo[]> => []),
  list_acp_registry: vi.fn(async () => []),
  add_acp: vi.fn(),
  remove_acp: vi.fn(),
  acp_login: vi.fn(async (_name: string, _method?: string | null): Promise<string> => 'ok'),
}))

vi.mock('@/lib/notify', () => ({ notify }))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => api }))

const REPORTS = { permission: false, turn_end: false, session_load: false }

function entry(overrides: Partial<EntryInfo> & Pick<EntryInfo, 'name'>): EntryInfo {
  return {
    command: overrides.name,
    args: ['--acp'],
    description: null,
    status: null,
    reports: REPORTS,
    auth_methods: [],
    ...overrides,
  }
}

async function mount(entries: EntryInfo[]) {
  const client = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  client.setQueryData(queryKeys.acp, entries)
  client.setQueryData(queryKeys.acpRegistry, [])
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  await act(async () => root.render(createElement(QueryClientProvider, { client }, createElement(Agents))))
  return {
    host,
    async unmount() {
      await act(async () => root.unmount())
      host.remove()
    },
  }
}

beforeEach(() => {
  vi.clearAllMocks()
})

test('single agent method shows Log in button, waits for browser, and refreshes on success', async () => {
  const deferred = Promise.withResolvers<string>()
  const claude = entry({
    name: 'claude',
    status: 'login required: OAuth',
    auth_methods: [{ id: 'oauth', name: 'OAuth', kind: 'agent' }],
  })
  const refreshed = entry({
    name: 'claude',
    status: 'ok',
    auth_methods: [{ id: 'oauth', name: 'OAuth', kind: 'agent' }],
  })
  api.acp_login.mockImplementationOnce(() => deferred.promise)
  api.list_acp.mockImplementationOnce(async () => [refreshed])

  const { host, unmount } = await mount([claude])
  expect(host.querySelector('select')).toBeNull()
  const button = [...host.querySelectorAll('button')].find((b) => b.textContent === 'Log in')!
  expect(button).toBeDefined()
  expect(button.disabled).toBe(false)

  await act(async () => {
    button.click()
    await new Promise((r) => setTimeout(r, 0))
  })
  expect(api.acp_login).toHaveBeenCalledWith('claude', 'oauth')
  const waiting = [...host.querySelectorAll('button')].find((b) => b.textContent === 'Waiting for browser…')
  expect(waiting).toBeDefined()
  expect(waiting?.disabled).toBe(true)

  await act(async () => {
    deferred.resolve('ok')
    await deferred.promise
    await new Promise((r) => setTimeout(r, 0))
  })
  expect(api.list_acp).toHaveBeenCalledWith(true)
  expect(notify.success).toHaveBeenCalledWith('Logged in to claude')
  expect([...host.querySelectorAll('button')].some((b) => b.textContent === 'Log in')).toBe(false)
  expect(host.textContent).toContain('ok')
  await unmount()
})

test('multiple agent methods render a select defaulting to the first and pass the chosen id', async () => {
  const multi = entry({
    name: 'enterprise',
    status: 'login required: OAuth',
    auth_methods: [
      { id: 'cli', name: 'Terminal', kind: 'terminal' },
      { id: 'oauth', name: 'OAuth', kind: 'agent' },
      { id: 'sso', name: 'Enterprise SSO', kind: 'agent' },
    ],
  })
  api.acp_login.mockResolvedValueOnce('ok')
  api.list_acp.mockResolvedValueOnce([{ ...multi, status: 'ok' }])

  const { host, unmount } = await mount([multi])
  const select = host.querySelector('select') as HTMLSelectElement | null
  expect(select).not.toBeNull()
  expect([...select!.options].map((o) => [o.value, o.textContent])).toEqual([
    ['oauth', 'OAuth'],
    ['sso', 'Enterprise SSO'],
  ])
  expect(select!.value).toBe('oauth')

  await act(async () => {
    const setter = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value')!.set!
    setter.call(select, 'sso')
    select!.dispatchEvent(new Event('change', { bubbles: true }))
  })
  expect(select!.value).toBe('sso')

  const button = [...host.querySelectorAll('button')].find((b) => b.textContent === 'Log in')!
  await act(async () => {
    button.click()
  })
  expect(api.acp_login).toHaveBeenCalledWith('enterprise', 'sso')
  await unmount()
})

test('terminal-only harness shows the command instead of a Log in button, and ok rows show neither', async () => {
  const term = entry({
    name: 'codex',
    status: 'login required: A=1 codex-acp --acp --login',
    auth_methods: [{ id: 'cli-login', name: 'Log in', kind: 'terminal' }],
  })
  const ready = entry({
    name: 'opencode',
    status: 'ok',
    auth_methods: [{ id: 'oauth', name: 'OAuth', kind: 'agent' }],
  })

  const { host, unmount } = await mount([term, ready])
  expect(host.querySelector('select')).toBeNull()
  expect([...host.querySelectorAll('button')].some((b) => b.textContent === 'Log in')).toBe(false)
  expect(host.querySelector('code')?.textContent).toBe('A=1 codex-acp --acp --login')
  expect(host.textContent).toContain('ok')
  await unmount()
})
