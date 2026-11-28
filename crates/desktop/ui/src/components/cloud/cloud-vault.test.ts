// 225 × 224: the configure step's master password, and a node row whose vault has no master password.
import '@/components/settings/test-dom'
import { QueryClient, QueryClientProvider, notifyManager } from '@tanstack/react-query'
import { act, createElement as h, type ReactElement, useState } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { useConnection } from '@/store/connection'
import { type CloudDialog, NodeRow, VaultPasswordDialog } from './CloudNodesSettings'
import { stageRows, vaultError, vaultRow } from './format'
import { PhoneConfigureStep } from './steps'
import type { CloudNode, ProviderInfo } from './types'

vi.hoisted(() => vi.stubGlobal('localStorage', { getItem: () => null, setItem: () => undefined }))
vi.mock('@/lib/notify', () => ({ notify: Object.assign(vi.fn(), { error: vi.fn(), success: vi.fn(), warning: vi.fn() }) }))
notifyManager.setScheduler(queueMicrotask)

const call = vi.fn(async (method: string, params: unknown[]) => (method === 'vault_add_password' ? { engine: params[0], password_set: true } : null))

let root: Root | undefined
beforeEach(() => {
  vi.clearAllMocks()
  useConnection.setState({ client: { call } as never })
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
const click = (el: Element) => act(async () => (el as HTMLElement).click())
const input = (label: string) => document.querySelector<HTMLInputElement>(`input[aria-label="${label}"]`)!
const type = (el: HTMLInputElement, value: string) =>
  act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(el, value)
    el.dispatchEvent(new Event('input', { bubbles: true }))
  })
const alert = () => document.querySelector('[role="alert"]')?.textContent ?? null

const DO: ProviderInfo = {
  provider: 'digitalocean',
  label: 'DigitalOcean',
  available: true,
  beta: false,
  desktop_only: false,
  connected: true,
  source: 'oauth',
  discovered: [],
  oauth: true,
  sizes: [{ memory_gb: 4, vcpus: 2, slug: 's-2vcpu-4gb', monthly_usd: 24 }],
  regions: [{ id: 'lon1', label: 'London', country: 'GB' }],
  volume_usd_per_gb: 0.1,
}
const QUOTE = { provider: 'digitalocean' as const, size_slug: 's-2vcpu-4gb', server_usd: 24, disk_usd: 1, total_usd: 25 }

/** The phone's configure step with the wizard's two password fields. */
function Configure({ onCreate }: { onCreate: () => void }) {
  const [password, setPassword] = useState('')
  const [confirm, setConfirm] = useState('')
  return h(PhoneConfigureStep, {
    provider: DO,
    spec: { provider: 'digitalocean', name: 'sky-cloud', region: 'lon1', memory_gb: 4, volume_gb: 10, k8s: null, desktop: true },
    sizes: DO.sizes,
    quote: QUOTE,
    nameError: null,
    busy: false,
    onName: () => {},
    onRegion: () => {},
    onMemory: () => {},
    onDesktop: () => {},
    onCreate,
    vault: { password, confirm, onPassword: setPassword, onConfirm: setConfirm },
  })
}

const node = (n: Partial<CloudNode> = {}): CloudNode => ({
  name: 'build-box',
  provider: 'digitalocean',
  region: 'ams3',
  region_label: 'Amsterdam',
  memory_gb: 2,
  size_slug: 's-1vcpu-2gb',
  volume_gb: 10,
  monthly_usd: 13,
  state: 'ready',
  stage: 'ready',
  error: null,
  provider_state: 'active',
  ip: null,
  peer_id: 'c4e8',
  reachable: true,
  path: 'direct',
  version: '2.32.0',
  auto_update: true,
  created_ms: null,
  k8s: null,
  ssh: null,
  desktop: true,
  vault: 'needs_password',
  link: null,
  ...n,
})

test('the master password is optional, else 12+ characters typed twice', () => {
  expect(vaultError('', '')).toBeNull()
  expect(vaultError('short', 'short')).toBe('Use at least 12 characters')
  expect(vaultError('otter-lantern-quarry', '')).toBe('Confirm the master password')
  expect(vaultError('otter-lantern-quarry', 'otter-lantern')).toBe('The passwords do not match')
  expect(vaultError('otter-lantern-quarry', 'otter-lantern-quarry')).toBeNull()
  // Characters, not UTF-16 units: twelve emoji are twelve characters.
  expect(vaultError('🦦'.repeat(12), '🦦'.repeat(12))).toBeNull() // no-emoji: allow
})

test('Create waits for a valid master password or none; the button keeps naming the price', async () => {
  const onCreate = vi.fn()
  await mount(h(Configure, { onCreate }))
  const create = () => button('Create node · $25/mo')!
  expect(document.body.textContent).toContain("Unlock from your devices; this password is the fallback. It can't be recovered.")
  expect(create().disabled).toBe(false)
  expect(input('Confirm master password').disabled).toBe(true)

  await type(input('Master password'), 'short')
  expect(document.body.textContent).toContain('Too short')
  expect(create().disabled).toBe(true)

  await type(input('Master password'), 'otter-lantern-quarry-91')
  expect(create().disabled).toBe(true)
  await type(input('Confirm master password'), 'otter-lantern')
  expect(alert()).toBe('The passwords do not match')
  expect(create().disabled).toBe(true)

  await type(input('Confirm master password'), 'otter-lantern-quarry-91')
  expect(alert()).toBeNull()
  expect(create().disabled).toBe(false)
  await click(create())
  expect(onCreate).toHaveBeenCalledOnce()
})

test('a ready node whose vault has no master password says so on one line and offers to set one', async () => {
  const onOpen = vi.fn<(d: CloudDialog) => void>()
  const row = (n: CloudNode) => h('ul', null, h(NodeRow, { node: n, onRestart: () => {}, onUpdate: () => {}, onTerminal: () => {}, onDesktop: () => {}, opening: false, onOpen }))
  await mount(row(node()))
  expect(document.querySelector('[data-vault]')?.textContent).toContain('Vault: device only — set a master password')
  await click(button('Set master password')!)
  expect(onOpen).toHaveBeenCalledWith('vault')

  expect(vaultRow(node({ vault: 'device_only' }))).toMatchObject({ tone: 'muted', action: true })
  expect(vaultRow(node({ vault: 'unsupported' }))).toEqual({ text: 'Vault protection arrives with the next theuth release on this node', tone: 'muted', action: false })
  expect(vaultRow(node({ vault: 'protected' }))).toBeNull()
  // Nothing while it is being made, nor for a node an older engine reports without the field.
  expect(vaultRow(node({ state: 'creating', vault: 'pending' }))).toBeNull()
  expect(vaultRow({ ...node(), vault: undefined } as never)).toBeNull()
  // A node whose release has no sealed vault: the progress row says so instead of "Protecting the vault".
  expect(stageRows({ node: node({ vault: 'unsupported' }), progress: undefined, version: null }).find((r) => r.stage === 'vault')?.label).toBe('Vault protection arrives with the next theuth release on this node')
})

test('Set master password sends the PasswordForm’s password to the node, not to this engine', async () => {
  const onClose = vi.fn()
  await mount(h(VaultPasswordDialog, { name: 'build-box', engine: 'c4e8', onClose }))
  const strong = 'Xk7#Qm2$vL9pRw4!'
  await type(input('new password for build-box'), strong)
  await type(input('confirm new password for build-box'), strong)
  await click(button('Add password')!)
  expect(call.mock.calls.filter(([m]) => m === 'vault_add_password').map(([, p]) => p)).toEqual([['c4e8', strong]])
  expect(onClose).toHaveBeenCalled()
})
