// 224: reopening cloud node provisioning status from Settings › Machines, from the global status-bar chip,
// and on phone, plus global `cloudnodes.events@1` subscription via `useInvalidateOn()`.
import '@/components/settings/test-dom'
import { QueryClient, QueryClientProvider, notifyManager } from '@tanstack/react-query'
import { act, createElement as h, type ReactElement } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { notify } from '@/lib/notify'
import { useInvalidateOn } from '@/lib/rpc'
import { useConnection } from '@/store/connection'
import { dispatch } from '@/store/sessions'
import { CloudNodesSection } from './CloudNodesSettings'
import { CloudCreatingChipInner } from './CloudStatusChip'
import type { CloudProgress } from './events'
import { cloudKeys, useCloudEvents } from './queries'
import type { CloudList, CloudNode, ProviderInfo } from './types'

const { navigate, isMobile } = vi.hoisted(() => ({
  navigate: vi.fn(),
  isMobile: vi.fn(() => false),
}))
vi.hoisted(() => vi.stubGlobal('localStorage', { getItem: () => null, setItem: () => undefined }))
vi.mock('@tanstack/react-router', () => ({ useNavigate: () => navigate }))
vi.mock('@/hooks/use-mobile', () => ({ useIsMobile: () => isMobile() }))
vi.mock('@/lib/notify', () => ({ notify: Object.assign(vi.fn(), { error: vi.fn(), success: vi.fn(), warning: vi.fn() }) }))
notifyManager.setScheduler(queueMicrotask)

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
  sizes: [
    { memory_gb: 2, vcpus: 1, slug: 's-1vcpu-2gb', monthly_usd: 12 },
    { memory_gb: 4, vcpus: 2, slug: 's-2vcpu-4gb', monthly_usd: 24 },
    { memory_gb: 8, vcpus: 4, slug: 's-4vcpu-8gb', monthly_usd: 48 },
  ],
  regions: [
    { id: 'lon1', label: 'London', country: 'GB' },
    { id: 'fra1', label: 'Frankfurt', country: 'DE' },
    { id: 'nyc3', label: 'New York', country: 'US' },
  ],
  volume_usd_per_gb: 0.1,
}

const node = (n: Partial<CloudNode> = {}): CloudNode => ({
  name: 'alpha-cloud',
  provider: 'digitalocean',
  region: 'lon1',
  region_label: 'London',
  memory_gb: 2,
  size_slug: 's-1vcpu-2gb',
  volume_gb: 10,
  monthly_usd: 13,
  state: 'creating',
  stage: 'install',
  error: null,
  provider_state: 'new',
  ip: null,
  peer_id: null,
  reachable: null,
  path: null,
  version: null,
  auto_update: true,
  created_ms: null,
  k8s: null,
  desktop: true,
  vault: 'pending',
  link: null,
  ssh: null,
  ...n,
})

const cloudDestroy = vi.fn(async (_name: string, _keep: boolean) => ['droplet'])
const fakeApi = {
  cloud_destroy: cloudDestroy,
  cloud_providers: vi.fn(async () => [DO]),
  cloud_sizes: vi.fn(async () => ({ sizes: DO.sizes, priced: true, error: null })),
  cloud_list: vi.fn(async () => ({ nodes: [], errors: [] })),
  mesh_peer_sync: vi.fn(async () => []),
}

let root: Root | undefined
let qc: QueryClient

beforeEach(() => {
  vi.clearAllMocks()
  isMobile.mockReturnValue(false)
  useConnection.setState({ api: fakeApi as never })
  qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
})

afterEach(async () => {
  await act(async () => root?.unmount())
  root = undefined
  document.body.innerHTML = ''
})

async function mount(element: ReactElement, client = qc) {
  const host = document.createElement('div')
  document.body.append(host)
  root = createRoot(host)
  await act(async () => root!.render(h(QueryClientProvider, { client }, element)))
}

const buttons = (text: string) => [...document.querySelectorAll<HTMLButtonElement>('button')].filter((b) => b.textContent?.trim() === text)
const click = (el: Element) => act(async () => (el as HTMLElement).click())

test('useInvalidateOn subscribes globally to cloudnodes.events@1, fires notices when Settings is closed, and ref-counts with useCloudEvents without double-applying', async () => {
  qc.setQueryData<CloudList>(cloudKeys.list, { nodes: [], errors: [] })

  function GlobalHost({ withCloudView }: { withCloudView: boolean }) {
    useInvalidateOn()
    return withCloudView ? h(CloudViewChild) : null
  }
  function CloudViewChild() {
    useCloudEvents()
    return null
  }

  await mount(h(GlobalHost, { withCloudView: true }))

  await act(async () => {
    dispatch([
      { key: 'cloudnodes.events@1', event: { type: 'CloudNodeChanged', node: node({ name: 'alpha-cloud', state: 'creating', stage: 'server' }) } },
      { key: 'cloudnodes.events@1', event: { type: 'CloudNodeProgress', name: 'alpha-cloud', stage: 'install', line: 'installing theuth', pct: 55, eta_secs: 90 } },
    ])
  })

  // Both useInvalidateOn and useCloudEvents are mounted: the progress line must fold once, not twice.
  const progress = qc.getQueryData<CloudProgress>(cloudKeys.progress('alpha-cloud'))
  expect(progress).toEqual({ stage: 'install', pct: 55, eta_secs: 90, lines: ['installing theuth'] })
  expect(qc.getQueryData<CloudList>(cloudKeys.list)?.nodes.map((n) => n.name)).toEqual(['alpha-cloud'])

  // Unmount the Settings / cloud view while keeping the global listener alive.
  await act(async () => root!.render(h(QueryClientProvider, { client: qc }, h(GlobalHost, { withCloudView: false }))))

  await act(async () => {
    dispatch([
      { key: 'cloudnodes.events@1', event: { type: 'CloudNodeProgress', name: 'alpha-cloud', stage: 'join', line: 'joining mesh', pct: 80, eta_secs: 30 } },
      { key: 'cloudnodes.events@1', event: { type: 'CloudNodeChanged', node: node({ name: 'alpha-cloud', state: 'ready', stage: 'ready' }) } },
    ])
  })

  expect(qc.getQueryData<CloudProgress>(cloudKeys.progress('alpha-cloud'))?.lines).toEqual(['installing theuth', 'joining mesh'])
  expect(notify.success).toHaveBeenCalledWith('alpha-cloud is ready', { id: 'cloud-alpha-cloud' })

  // A failed create while Settings is closed still fires notify.error with the failure reason.
  await act(async () => {
    dispatch([
      { key: 'cloudnodes.events@1', event: { type: 'CloudNodeChanged', node: node({ name: 'beta-cloud', state: 'creating', stage: 'server' }) } },
      { key: 'cloudnodes.events@1', event: { type: 'CloudNodeChanged', node: node({ name: 'beta-cloud', state: 'failed', stage: 'install', error: 'quota exceeded' }) } },
    ])
  })
  expect(notify.error).toHaveBeenCalledWith('beta-cloud could not be created', { id: 'cloud-beta-cloud', description: 'quota exceeded' })
})

test('creating rows open their own status dialog titled with the node name (multiple nodes provisioning at once)', async () => {
  const alpha = node({ name: 'alpha-cloud', state: 'creating', stage: 'server' })
  const beta = node({ name: 'beta-cloud', state: 'creating', stage: 'join' })
  qc.setQueryData(cloudKeys.providers, [DO])
  qc.setQueryData<CloudList>(cloudKeys.list, { nodes: [alpha, beta], errors: [] })

  await mount(h(CloudNodesSection, {}))

  const viewAlpha = document.querySelector<HTMLButtonElement>('[data-cloud-node="alpha-cloud"] button')!
  expect(viewAlpha.textContent?.trim()).toBe('View status')
  await click(viewAlpha)

  let dialog = document.querySelector('[role="dialog"]')!
  expect(dialog.querySelector('h2')?.textContent).toBe('alpha-cloud')
  expect(dialog.textContent).not.toContain('New cloud node')
  expect(dialog.textContent).toContain('Starting alpha-cloud')

  await click(buttons('Close')[0]!)
  expect(document.querySelector('[role="dialog"]')).toBeNull()

  const viewBeta = document.querySelector<HTMLButtonElement>('[data-cloud-node="beta-cloud"] button')!
  expect(viewBeta.textContent?.trim()).toBe('View status')
  await click(viewBeta)

  dialog = document.querySelector('[role="dialog"]')!
  expect(dialog.querySelector('h2')?.textContent).toBe('beta-cloud')
  expect(dialog.textContent).toContain('Starting beta-cloud')
})

test('reopening a failed node shows the error and Destroy and retry preserves its provider, name, region, memory, and desktop', async () => {
  const failed = node({
    name: 'fra-box',
    state: 'failed',
    stage: 'install',
    error: 'cloud-init timed out',
    region: 'fra1',
    region_label: 'Frankfurt',
    memory_gb: 8,
    desktop: false,
  })
  qc.setQueryData(cloudKeys.providers, [DO])
  qc.setQueryData(cloudKeys.sizes('digitalocean', 'fra1'), { sizes: DO.sizes, priced: true, error: null })
  qc.setQueryData<CloudList>(cloudKeys.list, { nodes: [failed], errors: [] })

  await mount(h(CloudNodesSection, {}))

  const viewBtn = [...document.querySelectorAll<HTMLButtonElement>('[data-cloud-node="fra-box"] button')].find((b) => b.textContent?.trim() === 'View status')!
  await click(viewBtn)

  const dialog = document.querySelector('[role="dialog"]')!
  expect(dialog.querySelector('h2')?.textContent).toBe('fra-box')
  expect(dialog.textContent).toContain('cloud-init timed out')

  const retryBtn = [...dialog.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent?.trim() === 'Destroy and retry')!
  await click(retryBtn)

  expect(cloudDestroy).toHaveBeenCalledWith('fra-box', false)
  // Back on configure with the failed node's values preserved (not reset to defaults).
  const nameInput = dialog.querySelector<HTMLInputElement>('input[aria-label="Name"]')!
  expect(nameInput.value).toBe('fra-box')
  expect(dialog.querySelector('[data-size="8"]')?.getAttribute('aria-checked')).toBe('true')
  expect(dialog.querySelector('button[role="switch"]')?.getAttribute('aria-checked')).toBe('false')
  expect(dialog.textContent).toContain('Frankfurt')
})

test('CloudNodesSection opens directly from ?progress=<name>, clears the search param on Close, and routes to /cloud/new on phone', async () => {
  const alpha = node({ name: 'alpha-cloud', state: 'creating', stage: 'install' })
  qc.setQueryData(cloudKeys.providers, [DO])
  qc.setQueryData<CloudList>(cloudKeys.list, { nodes: [alpha], errors: [] })

  await mount(h(CloudNodesSection, { progress: 'alpha-cloud' }))
  const dialog = document.querySelector('[role="dialog"]')!
  expect(dialog.querySelector('h2')?.textContent).toBe('alpha-cloud')

  await click(buttons('Close')[0]!)
  expect(document.querySelector('[role="dialog"]')).toBeNull()
  expect(navigate).toHaveBeenCalledWith({ to: '/settings/$tab', params: { tab: 'machines' }, search: {}, replace: true })

  // Phone: View status navigates to the full-page wizard with ?node=<name>.
  navigate.mockClear()
  isMobile.mockReturnValue(true)
  await act(async () => root!.render(h(QueryClientProvider, { client: qc }, h(CloudNodesSection, {}))))
  const viewBtn = document.querySelector<HTMLButtonElement>('[data-cloud-node="alpha-cloud"] button')!
  await click(viewBtn)
  expect(navigate).toHaveBeenCalledWith({ to: '/cloud/new', search: { entry: 'add-to-mesh', node: 'alpha-cloud' } })
})

test('status-bar chip links back to a single creating node or the Machines list when several are creating', async () => {
  qc.setQueryData<CloudList>(cloudKeys.list, { nodes: [node({ name: 'done-cloud', state: 'ready' })], errors: [] })
  await mount(h(CloudCreatingChipInner, {}))
  expect(document.querySelector('[data-cloud-creating]')).toBeNull()

  // Single creating node on desktop -> opens /settings/machines?progress=<name>.
  await act(async () => {
    qc.setQueryData<CloudList>(cloudKeys.list, { nodes: [node({ name: 'alpha-cloud', state: 'creating' })], errors: [] })
  })
  const singleBtn = document.querySelector<HTMLButtonElement>('[data-cloud-creating="1"]')!
  expect(singleBtn.textContent?.trim()).toBe('Starting alpha-cloud')
  await click(singleBtn)
  expect(navigate).toHaveBeenCalledWith({ to: '/settings/$tab', params: { tab: 'machines' }, search: { progress: 'alpha-cloud' } })

  // Several creating nodes on desktop -> count chip opens Settings › Machines.
  navigate.mockClear()
  await act(async () => {
    qc.setQueryData<CloudList>(cloudKeys.list, {
      nodes: [node({ name: 'alpha-cloud', state: 'creating' }), node({ name: 'beta-cloud', state: 'creating' })],
      errors: [],
    })
  })
  const multiBtn = document.querySelector<HTMLButtonElement>('[data-cloud-creating="2"]')!
  expect(multiBtn.textContent?.trim()).toBe('2 cloud nodes starting')
  await click(multiBtn)
  expect(navigate).toHaveBeenCalledWith({ to: '/settings/$tab', params: { tab: 'machines' } })

  // Mobile: each creating node links directly to /cloud/new?entry=add-to-mesh&node=<name>.
  navigate.mockClear()
  const onOpen = vi.fn()
  await act(async () => root!.render(h(QueryClientProvider, { client: qc }, h(CloudCreatingChipInner, { mobile: true, onOpen }))))
  const mobileBtns = [...document.querySelectorAll<HTMLButtonElement>('[data-cloud-creating="2"] button')]
  expect(mobileBtns.map((b) => b.textContent?.trim())).toEqual(['Starting alpha-cloud', 'Starting beta-cloud'])
  await click(mobileBtns[1]!)
  expect(onOpen).toHaveBeenCalledOnce()
  expect(navigate).toHaveBeenCalledWith({ to: '/cloud/new', search: { entry: 'add-to-mesh', node: 'beta-cloud' } })
})
