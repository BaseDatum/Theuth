import './test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement, type ReactNode } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { queryKeys } from '@/lib/rpc'
import type * as Rpc from '@/lib/rpc'
import type { LoginInfo, ProviderInfo } from '@/rpc/bindings/method.agent.output'
import { disconnected } from '@/rpc/discovery'
import { useConnection } from '@/store/connection'
import { ModelProviders } from './ModelProviders'

const provider = (name: string, over: Partial<ProviderInfo> = {}): ProviderInfo => ({
  name,
  base_url: `https://${name}/v1/`,
  models: 3,
  builtin: true,
  configured: false,
  refreshable: false,
  local: false,
  runtime: null,
  has_key: false,
  account_of: null,
  supports_login: false,
  ...over,
})
const login = (name: string, logged_in = false): LoginInfo => ({ name, logged_in, account: null, expires_at: null })
const refreshed = (name: string) => ({ provider: provider(name), added: [], removed: [] })

const api = vi.hoisted(() => ({
  reset_provider_models: vi.fn(async (_name: string | null) => [] as unknown[]),
  refresh_all_models: vi.fn(async () => [] as unknown[]),
  refresh_provider_models: vi.fn(async (_name: string) => ({}) as unknown),
  add_account: vi.fn(async (kind: string, suffix: string, _key: string | null) => ({ name: `${kind}-${suffix}` }) as unknown),
  login: vi.fn(async (_name: string) => 'https://auth.example/authorize?acct=2'),
  remove_provider: vi.fn(async (_name: string) => null as unknown),
}))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => api }))
// The ⋯ menus render open, their items as buttons that select on click.
vi.mock('@/components/ui/dropdown-menu', () => ({
  DropdownMenu: ({ children }: { children?: ReactNode }) => children,
  DropdownMenuTrigger: ({ children }: { children?: ReactNode }) => children,
  DropdownMenuContent: ({ children }: { children?: ReactNode }) => createElement('div', { role: 'menu' }, children),
  DropdownMenuSeparator: () => createElement('hr'),
  DropdownMenuItem: ({ children, onSelect, disabled, 'aria-label': label }: { children?: ReactNode; onSelect?: () => void; disabled?: boolean; 'aria-label'?: string }) =>
    createElement('button', { role: 'menuitem', disabled, 'aria-label': label, onClick: () => onSelect?.() }, children),
}))

let host: HTMLDivElement
let root: Root

beforeEach(async () => {
  vi.clearAllMocks()
  api.reset_provider_models.mockResolvedValue([refreshed('anthropic')])
  api.refresh_all_models.mockResolvedValue([refreshed('anthropic'), refreshed('openai')])
  // A legacy engine supports every method: the new buttons show.
  useConnection.setState({ discovery: { ...disconnected(), phase: 'legacy' } })
  const client = new QueryClient()
  client.setQueryData(queryKeys.providers, [
    provider('rwx', { builtin: false, configured: true }),
    provider('anthropic-work', { builtin: false, account_of: 'anthropic', supports_login: true }),
    provider('anthropic', { supports_login: true }),
    provider('opencode', { configured: true, refreshable: true }),
  ])
  client.setQueryData(queryKeys.logins, [login('anthropic', true), login('anthropic-work')])
  client.setQueryData(['mesh'], { self_id: 'self', self_name: 'Laptop', peers: [], stats: [] })
  host = document.body.appendChild(document.createElement('div'))
  root = createRoot(host)
  await act(async () => root.render(createElement(QueryClientProvider, { client }, createElement(ModelProviders))))
})

afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
})

const buttons = () => [...document.body.querySelectorAll('button')]
const byLabel = (label: string) => document.body.querySelector<HTMLElement>(`[aria-label="${label}"]`)
const byText = (t: string) => buttons().find((b) => b.textContent?.trim() === t)
const click = (el: HTMLElement | null | undefined) => act(async () => el!.click())
const rowOf = (name: string) => document.body.querySelector(`[data-provider="${name}"]`)!
/** A row's buttons beside its name (its primary action), apart from ⋯ and the Limits toggle. */
const primaryOf = (name: string) =>
  [...rowOf(name).querySelectorAll('button')]
    .filter((b) => !b.closest('[role="menu"]') && !/^(More actions|Model limits)/.test(b.getAttribute('aria-label') ?? ''))
    .map((b) => b.textContent?.trim())
const menuOf = (name: string) => [...rowOf(name).querySelectorAll('[role="menuitem"]')].map((b) => b.textContent?.trim())
async function type(label: string, value: string) {
  const input = byLabel(label) as HTMLInputElement
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(input, value)
    input.dispatchEvent(new Event('input', { bubbles: true }))
  })
}

test('an account row sits under its kind with the kind label, its own Log in, Replace key and Remove; built-ins get Refresh, Reset and Add another account', () => {
  const t = host.textContent!
  expect(t.indexOf('Anthropic')).toBeLessThan(t.indexOf('anthropic-work'))
  expect(t.indexOf('anthropic-work')).toBeLessThan(t.indexOf('OpenCode Zen'))
  expect(t.indexOf('OpenCode Zen')).toBeLessThan(t.indexOf('Custom providers'))
  expect(t).toContain('anthropic-workAnthropic account')
  expect(t).toContain('anthropic-work · 3 models · no key — secret anthropic-work · not logged in')
  expect(t).toContain('Refresh all')
  // Every built-in row and account refreshes and resets; a custom row without a listing does neither.
  for (const n of ['anthropic', 'anthropic-work', 'opencode']) expect(byLabel(`Reset models of ${n}`)).not.toBeNull()
  expect(byLabel('Reset models of rwx')).toBeNull()
  expect(t.match(/Refresh models/g)).toHaveLength(3)
  // Add another account on the kinds only, Remove on the account (and the custom row) only.
  expect(byLabel('Add another Anthropic account')).not.toBeNull()
  expect(byLabel('Add another OpenCode Zen account')).not.toBeNull()
  expect(buttons().filter((b) => b.textContent?.includes('Add another account'))).toHaveLength(2)
  expect(byLabel('Remove anthropic-work')).not.toBeNull()
  expect(byLabel('Remove anthropic')).toBeNull()
  expect(buttons().filter((b) => b.textContent === 'Set key…')).toHaveLength(2)
  expect(byText('Log out')).toBeDefined()
  expect(byText('Log in')).toBeDefined()
})

test('each row shows one primary action beside its name and the rest in its ⋯ menu, never both', () => {
  // Logged in without a key: usable, so Refresh models; Set key… and Log out move to the menu.
  expect(primaryOf('anthropic')).toEqual(['Refresh models'])
  expect(menuOf('anthropic')).toEqual(['Reset models…', 'Set key…', 'Log out', 'Add another account…'])
  // Neither key nor login: Log in first, Set key… in the menu.
  expect(primaryOf('anthropic-work')).toEqual(['Log in'])
  expect(menuOf('anthropic-work')).toEqual(['Refresh models', 'Reset models…', 'Set key…', 'Remove…'])
  expect(primaryOf('opencode')).toEqual(['Refresh models'])
  expect(menuOf('opencode')).toEqual(['Reset models…', 'Replace key…', 'Add another account…'])
  // A custom row that does not refresh: only Limits beside the name, Remove in the menu.
  expect(primaryOf('rwx')).toEqual([])
  expect(menuOf('rwx')).toEqual(['Remove…'])
  for (const n of ['anthropic', 'anthropic-work', 'opencode', 'rwx']) expect(byLabel(`More actions for ${n}`)).not.toBeNull()
})

test('Set key… in the menu opens the key field on that row', async () => {
  await click([...rowOf('anthropic').querySelectorAll<HTMLElement>('[role="menuitem"]')].find((b) => b.textContent === 'Set key…'))
  expect(byLabel('value of anthropic')).not.toBeNull()
})

test('Remove asks first, then removes that provider only', async () => {
  await click(byLabel('Remove anthropic-work'))
  expect(api.remove_provider).not.toHaveBeenCalled()
  expect(document.body.textContent).toContain('Remove anthropic-work?')
  await click(byText('Remove'))
  expect(api.remove_provider).toHaveBeenCalledExactlyOnceWith('anthropic-work')
})

test('Reset models asks first, then resets that provider only', async () => {
  await click(byLabel('Reset models of anthropic'))
  expect(api.reset_provider_models).not.toHaveBeenCalled()
  expect(document.body.textContent).toContain("Reset Anthropic's models?")
  await click(buttons().filter((b) => b.textContent === 'Reset models').at(-1))
  expect(api.reset_provider_models).toHaveBeenCalledExactlyOnceWith('anthropic')
})

test('Refresh all calls refresh_all_models once', async () => {
  await click(byText('Refresh all'))
  expect(api.refresh_all_models).toHaveBeenCalledOnce()
})

test('Add another account: live suffix check and name preview; Add creates it, then logs the new account in', async () => {
  await click(byLabel('Add another Anthropic account'))
  const submit = () => byText('Add account') as HTMLButtonElement
  expect(document.body.textContent).toContain('Name: anthropic-…')
  expect(submit().disabled).toBe(true)

  await type('Account suffix', 'Work!')
  expect(document.body.querySelector('[role="alert"]')?.textContent).toBe('Use only lowercase letters, digits, _ and -')
  expect(submit().disabled).toBe(true)

  await type('Account suffix', 'work')
  expect(document.body.querySelector('[role="alert"]')?.textContent).toBe('anthropic-work already exists')
  expect(submit().disabled).toBe(true)

  await type('Account suffix', 'home')
  expect(document.body.querySelector('[role="alert"]')).toBeNull()
  expect(document.body.textContent).toContain('Name: anthropic-home')
  expect(submit().disabled).toBe(false)
  const loginAfter = () => [...document.body.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')].at(-1)!
  expect(loginAfter().checked).toBe(true)

  // A key replaces the login: the box clears and locks.
  await type('Account API key', 'sk-1')
  expect([loginAfter().checked, loginAfter().disabled]).toEqual([false, true])
  await type('Account API key', '')
  expect(loginAfter().checked).toBe(true)

  await click(submit())
  expect(api.add_account).toHaveBeenCalledExactlyOnceWith('anthropic', 'home', null)
  expect(api.login).toHaveBeenCalledExactlyOnceWith('anthropic-home')
})

test('a pending login of an account shows its URL with Copy and the private-window hint', async () => {
  // anthropic is logged in (Log out): the one Log in is anthropic-work's.
  await click(byText('Log in'))
  expect(api.login).toHaveBeenCalledExactlyOnceWith('anthropic-work')
  expect(host.textContent).toContain('https://auth.example/authorize?acct=2')
  expect(host.textContent).toContain('Open it in a private window to choose a different account.')
  expect(byLabel('Copy the sign-in URL of anthropic-work')).not.toBeNull()
})
