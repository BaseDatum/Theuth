import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import type * as Router from '@tanstack/react-router'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import { AlertDialog } from '@/components/ui/alert-dialog'
import { CloudNodeWizard, specFromNode } from './CloudNodeWizard'
import { CloudNodesSection, DestroyBody, dispatchTerminalOpen, NodeRow } from './CloudNodesSettings'
import { Route as CloudNewRoute } from '@/routes/cloud.new'
import { DESKTOP_NOTE, checkName, countryCode, createLabel, defaultMemory, defaultName, desktopBlocked, destroySaving, etaText, fromPrice, localQuote, sizesOf, stageRows, statusOf, userOf } from './format'
import { NodeDesktopView } from './NodeDesktop'
import { applyCloudEvent, cloudKeys } from './queries'
import { nearestRegion } from './region'
import { ChooseStep, ConnectPanel, DesktopToggle, FLANNEL_WARNING, K8sConfigure, PhoneConfigureStep, PhoneProviderStep, PriceNote, ProviderCards, VAULT_LABEL } from './steps'
import type { CloudList, CloudNode, K8sInfo, ProviderInfo, Region, SizeList } from './types'
import { peerSyncQueryKey } from '@/lib/peer-sync'
import type { PeerSync } from '@/rpc/bindings/method.sessionsync.receiver'

const mockOpenPopout = vi.fn(async (_t: unknown) => undefined)
const mockCanPopout = vi.fn(() => true)
const mockNavigate = vi.fn()
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => ({}) }))
vi.mock('@tanstack/react-router', async (original) => ({ ...(await original<typeof Router>()), useNavigate: () => mockNavigate }))
vi.mock('@/lib/popout', async (original) => ({
  ...(await original<typeof import('@/lib/popout')>()),
  canPopout: () => mockCanPopout(),
  openPopout: (t: unknown) => mockOpenPopout(t),
}))

const regions = (rows: [string, string, string][]): Region[] => rows.map(([id, label, country]) => ({ id, label, country }))
const DO_REGIONS = regions([['nyc3', 'New York', 'US'], ['sfo3', 'San Francisco', 'US'], ['tor1', 'Toronto', 'CA'], ['lon1', 'London', 'GB'], ['ams3', 'Amsterdam', 'NL'], ['fra1', 'Frankfurt', 'DE'], ['blr1', 'Bangalore', 'IN'], ['sgp1', 'Singapore', 'SG'], ['syd1', 'Sydney', 'AU']])
const FLY_REGIONS = regions([['lhr', 'London', 'GB'], ['ams', 'Amsterdam', 'NL'], ['fra', 'Frankfurt', 'DE'], ['iad', 'Ashburn, Virginia', 'US'], ['ord', 'Chicago', 'US'], ['sjc', 'San Jose', 'US'], ['yyz', 'Toronto', 'CA'], ['sin', 'Singapore', 'SG'], ['syd', 'Sydney', 'AU'], ['nrt', 'Tokyo', 'JP'], ['gru', 'São Paulo', 'BR']])

const provider = (p: Partial<ProviderInfo> & Pick<ProviderInfo, 'provider' | 'label'>): ProviderInfo => ({
  available: true,
  beta: false,
  desktop_only: false,
  connected: false,
  source: null,
  discovered: [],
  oauth: false,
  sizes: [],
  regions: [],
  volume_usd_per_gb: 0.1,
  ...p,
})
const DO = provider({ provider: 'digitalocean', label: 'DigitalOcean', oauth: true, connected: true, regions: DO_REGIONS, sizes: [{ memory_gb: 2, vcpus: 1, slug: 's-1vcpu-2gb', monthly_usd: 12 }, { memory_gb: 4, vcpus: 2, slug: 's-2vcpu-4gb', monthly_usd: 24 }, { memory_gb: 8, vcpus: 4, slug: 's-4vcpu-8gb', monthly_usd: 48 }] })
const FLY = provider({ provider: 'fly', label: 'Fly.io', beta: true, regions: FLY_REGIONS, volume_usd_per_gb: 0.15, sizes: [{ memory_gb: 2, vcpus: 1, slug: 'shared-cpu-1x', monthly_usd: 10.69 }, { memory_gb: 4, vcpus: 2, slug: 'shared-cpu-2x', monthly_usd: 21.38 }] })
const K8S = provider({ provider: 'kubernetes', label: 'Kubernetes', desktop_only: true, connected: true, volume_usd_per_gb: null, sizes: [{ memory_gb: 2, vcpus: 1, slug: '2Gi', monthly_usd: null }] })
const SSH = provider({ provider: 'ssh', label: 'Install via SSH', desktop_only: true, connected: true, volume_usd_per_gb: null, discovered: [{ id: 'ssh:config', label: 'SSH config', detail: '3 hosts', path: '~/.ssh/config' }] })
const LIGHTSAIL = provider({ provider: 'lightsail', label: 'AWS Lightsail', available: false, sizes: [{ memory_gb: 2, vcpus: 2, slug: 'small_3_0', monthly_usd: 12 }] })
const PROVIDERS = [DO, FLY, K8S, SSH, LIGHTSAIL]

const node = (n: Partial<CloudNode> = {}): CloudNode => ({
  name: 'sky-cloud',
  provider: 'digitalocean',
  region: 'lon1',
  region_label: 'London',
  memory_gb: 2,
  size_slug: 's-1vcpu-2gb',
  volume_gb: 10,
  monthly_usd: 13,
  state: 'ready',
  stage: null,
  error: null,
  provider_state: null,
  ip: null,
  peer_id: null,
  reachable: true,
  path: 'direct',
  version: '2.31.2',
  auto_update: true,
  created_ms: null,
  k8s: null,
  ssh: null,
  desktop: true,
  vault: null,
  link: null,
  ...n,
})

const html = (el: ReactNode) => renderToStaticMarkup(el as never)

const PRICED: SizeList = { sizes: DO.sizes, priced: true, error: null }

function wizard(entry: 'first-run' | 'add-to-mesh', initial: Parameters<typeof CloudNodeWizard>[0]['initial'], sizes: SizeList | null = PRICED): string {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  qc.setQueryData(cloudKeys.providers, PROVIDERS)
  qc.setQueryData<CloudList>(cloudKeys.list, { nodes: [], errors: [] })
  if (sizes) qc.setQueryData(cloudKeys.sizes('digitalocean', 'lon1'), sizes)
  if (sizes) qc.setQueryData(cloudKeys.sizes('fly', 'lhr'), { sizes: FLY.sizes, priced: true, error: null })
  return html(createElement(QueryClientProvider, { client: qc }, createElement(CloudNodeWizard, { entry, initial, onClose: () => undefined })))
}

const checked = (out: string, gb: number) => new RegExp(`aria-checked="true" data-size="${gb}"`).test(out)

test('the create button names the monthly total from the live sizes; the phone cost card splits server and disk', () => {
  const q = localQuote('digitalocean', DO.sizes, DO.volume_usd_per_gb, 2)
  expect(q).toEqual({ provider: 'digitalocean', size_slug: 's-1vcpu-2gb', server_usd: 12, disk_usd: 1, total_usd: 13 })
  expect(createLabel({ provider: 'digitalocean', k8s: null }, q)).toBe('Create node · $13/mo')
  expect(createLabel({ provider: 'fly', k8s: null }, localQuote('fly', FLY.sizes, FLY.volume_usd_per_gb, 2))).toBe('Create node · $12.19/mo')
  expect(createLabel({ provider: 'kubernetes', k8s: { context: 'rwx-dev', namespace: null, storage_class: null } }, null)).toBe('Deploy to rwx-dev')
  expect(createLabel({ provider: 'ssh', k8s: null }, null)).toBe('Install and join')
  const spec = { provider: 'digitalocean' as const, name: 'sky-cloud', region: 'lon1', memory_gb: 4, volume_gb: 10, k8s: null, ssh: null, desktop: true }
  const props = { provider: DO, spec, sizes: DO.sizes, quote: localQuote('digitalocean', DO.sizes, 0.1, 4), nameError: null, busy: false, onName: () => {}, onRegion: () => {}, onMemory: () => {}, onDesktop: () => {}, onCreate: () => {} }
  const out = html(createElement(PhoneConfigureStep, props))
  expect(out).toContain('Create node · $25/mo')
  expect(out).toContain('$24/mo + $1 disk')
  expect(out).toContain('No open ports · updates itself · watchable desktop')
  expect(html(createElement(PhoneConfigureStep, { ...props, spec: { ...spec, desktop: false } }))).toContain('No open ports · updates itself</div>')
})

test('desktop on: 4 GB is the default and Recommended; off: the 2 GB minimum; Fly defaults to 4 GB Recommended', () => {
  const on = wizard('add-to-mesh', { step: 'configure', spec: { provider: 'digitalocean', region: 'lon1' } })
  expect(checked(on, 4)).toBe(true)
  expect(on).toContain('Recommended')
  expect(on).toContain('Create node · $25/mo')
  expect(on).toMatch(/role="switch"[^>]*aria-checked="true"/)
  expect(on).toContain('Lets agents use a browser and apps you can watch and take over.')
  expect(on).toContain('Heavy computer use and browsing.')
  expect(on).not.toContain(DESKTOP_NOTE)
  const off = wizard('add-to-mesh', { step: 'configure', spec: { provider: 'digitalocean', region: 'lon1', desktop: false } })
  expect(checked(off, 2)).toBe(true)
  expect(off).not.toContain('Recommended')
  expect(off).toContain('minimum')
  expect(off).toContain('Create node · $13/mo')
  // 2 GB picked with the desktop on: a note, not a blocker.
  const small = wizard('add-to-mesh', { step: 'configure', spec: { provider: 'digitalocean', region: 'lon1', memory_gb: 2 } })
  expect(small).toContain(DESKTOP_NOTE)
  expect(small).toMatch(/<button[^>]*>Create node · \$13\/mo/)
  expect(small).not.toMatch(/<button[^>]*disabled=""[^>]*>Create node/)
  expect(defaultMemory(true)).toBe(4)
  expect(defaultMemory(false)).toBe(2)
  expect(defaultMemory(false, 'digitalocean')).toBe(2)
  expect(defaultMemory(false, 'fly')).toBe(4)
  // Fly has no desktop yet, but defaults to 4 GB Recommended (2 GB stays minimum).
  const flyCfg = wizard('add-to-mesh', { step: 'configure', spec: { provider: 'fly', region: 'lhr' } })
  expect(checked(flyCfg, 4)).toBe(true)
  expect(flyCfg).toContain('Recommended')
  expect(flyCfg).toContain('minimum')
})

test('Fly.io and Kubernetes: the desktop switch is disabled with its reason', () => {
  expect(desktopBlocked('fly')).toBe('Desktop: coming soon on Fly.io')
  expect(desktopBlocked('kubernetes')).toBe('Desktop: coming soon on Kubernetes')
  expect(desktopBlocked('ssh')).toBe('Desktop: not available via SSH')
  expect(desktopBlocked('digitalocean')).toBeNull()
  const fly = html(createElement(DesktopToggle, { kind: 'fly', on: true, onChange: () => {} }))
  expect(fly).toContain('Desktop: coming soon on Fly.io')
  expect(fly).toMatch(/role="switch"[^>]*aria-checked="false"[^>]*disabled=""/)
})

test('non-OAuth provider (Fly.io): token input is visible immediately and Continue is disabled until non-empty', () => {
  const oauthConnect = { mode: 'oauth' as const, waiting: false, error: null, token: '' }
  const panel = html(createElement(ConnectPanel, { p: FLY, connect: oauthConnect, onToken: () => {}, onPasteInstead: () => {}, onCancel: () => {} }))
  expect(panel).toContain('aria-label="Fly.io API token"')
  expect(panel).not.toContain('Paste a token instead')

  const phoneEmpty = html(
    createElement(PhoneProviderStep, {
      providers: PROVIDERS,
      selected: 'fly',
      connect: oauthConnect,
      busy: false,
      onSelect: () => {},
      onToken: () => {},
      onPasteInstead: () => {},
      onCancel: () => {},
      onNext: () => {},
    }),
  )
  expect(phoneEmpty).toContain('aria-label="Fly.io API token"')
  expect(phoneEmpty).toMatch(/<button[^>]*disabled=""[^>]*>Continue<\/button>/)

  const phoneFilled = html(
    createElement(PhoneProviderStep, {
      providers: PROVIDERS,
      selected: 'fly',
      connect: { ...oauthConnect, token: 'fo1_secret' },
      busy: false,
      onSelect: () => {},
      onToken: () => {},
      onPasteInstead: () => {},
      onCancel: () => {},
      onNext: () => {},
    }),
  )
  expect(phoneFilled).not.toMatch(/<button[^>]*disabled=""[^>]*>Continue<\/button>/)

  // Settings row does not hardcode a beta tag for Fly nodes.
  const flyRow = html(
    createElement(NodeRow, {
      node: node({ name: 'lab-fly', provider: 'fly', region: 'lhr', region_label: 'London' }),
      onRestart: () => {},
      onUpdate: () => {},
      onTerminal: () => {},
      onDesktop: () => {},
      opening: false,
      onOpen: () => {},
    }),
  )
  expect(flyRow).not.toContain('>beta<')
})

test('no price without one from the provider: unpriced or failed sizes, loading skeleton, Fly links its pricing', () => {
  const unpriced: SizeList = { sizes: DO.sizes, priced: false, error: null }
  expect(sizesOf(unpriced)?.every((s) => s.monthly_usd === null)).toBe(true)
  expect(sizesOf({ ...PRICED, error: 'rate limited' })?.every((s) => s.monthly_usd === null)).toBe(true)
  const out = wizard('add-to-mesh', { step: 'configure', spec: { provider: 'digitalocean', region: 'lon1' } }, unpriced)
  expect(out).toMatch(/<button[^>]*>Create node<\/button>/)
  expect(out).toContain('price unavailable')
  expect(out).not.toMatch(/\$\d/)
  const loading = wizard('add-to-mesh', { step: 'configure', spec: { provider: 'digitalocean', region: 'lon1' } }, null)
  expect(loading).toContain('aria-label="Loading sizes"')
  expect(loading).not.toMatch(/\$\d/)
  const fly = html(createElement(PriceNote, { kind: 'fly' }))
  expect(fly).toContain('href="https://fly.io/docs/about/pricing/"')
  expect(fly).toContain('price from Fly')
})

test('first run: no price before sign-in; the chip only from live prices of a connected non-beta provider', () => {
  expect(fromPrice(PROVIDERS, true, {})).toBeNull()
  expect(fromPrice(PROVIDERS, true, { digitalocean: PRICED })).toBe('from $12/mo')
  expect(fromPrice(PROVIDERS, true, { digitalocean: { ...PRICED, priced: false } })).toBeNull()
  expect(fromPrice([{ ...DO, connected: false }], true, { digitalocean: PRICED })).toBeNull()
  // beta: true is filtered out of fromPrice; beta: false is included.
  expect(fromPrice([{ ...FLY, connected: true, beta: true }], true, { fly: { sizes: FLY.sizes, priced: true, error: null } })).toBeNull()
  expect(fromPrice([{ ...FLY, connected: true, beta: false }], true, { fly: { sizes: FLY.sizes, priced: true, error: null } })).toBe('from $11/mo')
  const out = wizard('first-run', {}, null)
  expect(out).toContain('Where should theuth run?')
  expect(out).not.toMatch(/\$\d/)
  expect(out).toContain('your own account')
  expect(out).toContain('Just this iPhone')
  expect(out).toContain('You can add a cloud node later')
  // add-to-mesh skips ①; the provider cards name no price.
  const providers = wizard('add-to-mesh', {}, null)
  expect(providers).toContain('Pick a provider')
  expect(providers).not.toMatch(/\$\d/)
})

test('ChooseStep adapts local label and copy for iPad vs iPhone without emoji, and countryCode returns plain ISO codes', () => {
  const iphoneDefault = html(createElement(ChooseStep, { fromPrice: 'from $12/mo', choice: 'cloud', onChoice: () => {}, onContinue: () => {} }))
  expect(iphoneDefault).toContain('Just this iPhone')
  expect(iphoneDefault).toContain('from $12/mo')
  expect(iphoneDefault).toContain('when this device sleeps or is offline')
  expect(iphoneDefault).toContain('aria-pressed="true"')
  expect(iphoneDefault).toContain('aria-pressed="false"')
  expect(iphoneDefault).not.toMatch(/\p{Extended_Pictographic}|\p{Emoji_Presentation}/u)

  const ipadWizard = wizard('first-run', { device: 'ipad' }, null)
  expect(ipadWizard).toContain('Just this iPad')
  expect(ipadWizard).toContain('iPadOS')
  expect(ipadWizard).not.toContain('iPhone')
  expect(ipadWizard).not.toContain('your phone')
  expect(ipadWizard).toContain('md:max-w-[500px]')

  expect(countryCode('gb')).toBe('GB')
  expect(countryCode('US')).toBe('US')
  expect(countryCode('')).toBe('')
  expect(countryCode('USA')).toBe('')
})

test('vault protection and the sealed status appear only where the engine has the sealed vault', () => {
  // No `vault_status` / `vault_password` in this test's discovery: Configure has no vault field.
  expect(wizard('add-to-mesh', { step: 'configure', spec: { provider: 'digitalocean', region: 'lon1' } })).not.toContain(VAULT_LABEL)
  const spec = { provider: 'digitalocean' as const, name: 'sky-cloud', region: 'lon1', memory_gb: 4, volume_gb: 10, k8s: null, ssh: null, desktop: true }
  const props = { provider: DO, spec, sizes: DO.sizes, quote: null, nameError: null, busy: false, onName: () => {}, onRegion: () => {}, onMemory: () => {}, onDesktop: () => {}, onCreate: () => {} }
  const on = html(createElement(PhoneConfigureStep, { ...props, vault: { password: '', confirm: '', onPassword: () => {}, onConfirm: () => {} } }))
  expect(on).toContain('Vault protection')
  expect(on).toContain('Unlock from your devices; this password is the fallback.')
  expect(on).toMatch(/type="password"[^>]*aria-label="Master password"/)
  expect(on).toMatch(/type="password"[^>]*aria-label="Confirm master password"/)
  expect(statusOf(node({ sealed: true }))).toMatchObject({ text: 'sealed', tone: 'warn' })
  expect(statusOf(node({ sealed: false })).text).toBe('online · direct')
  expect(statusOf(node({ sealed: true, reachable: false })).text).toBe('offline')
})

test('the desktop viewer watches by default and offers Take over', () => {
  const out = html(createElement(NodeDesktopView, { name: 'sky-cloud', access: null, placeholder: createElement('div', null, 'screen') }))
  expect(out).toContain('watching')
  expect(out).toContain('Take over')
  expect(out).not.toContain('Hand back')
})

test('AWS Lightsail is listed but disabled as coming soon; Kubernetes and SSH only on desktop', () => {
  const props = { providers: PROVIDERS, selected: 'digitalocean' as const, k8sContexts: 3, sshHosts: 3, onSelect: () => {} }
  const phone = html(createElement(ProviderCards, { ...props, phone: true }))
  expect(phone).toMatch(/aria-disabled="true"[^>]*data-provider="lightsail"/)
  expect(phone).toContain('coming soon')
  expect(phone).not.toContain('data-provider="kubernetes"')
  expect(phone).not.toContain('data-provider="ssh"')
  expect(phone).not.toMatch(/aria-disabled="true"[^>]*data-provider="digitalocean"/)
  const desktop = html(createElement(ProviderCards, { ...props, phone: false }))
  expect(desktop).toContain('data-provider="kubernetes"')
  expect(desktop).toContain('3 contexts in ~/.kube/config')
  expect(desktop).toContain('uses your cluster')
  expect(desktop).toContain('data-provider="ssh"')
  expect(desktop).toContain('Install via SSH')
  expect(desktop).toContain('3 hosts in ~/.ssh/config')
  expect(desktop).toContain('uses your server')
  // The phone wizard's provider step never offers Kubernetes or SSH either.
  const phoneWizard = wizard('add-to-mesh', { step: 'provider' })
  expect(phoneWizard).not.toContain('data-provider="kubernetes"')
  expect(phoneWizard).not.toContain('data-provider="ssh"')
})

test('the flannel warning shows only when the cluster is known not to enforce NetworkPolicies', () => {
  const info: K8sInfo = { contexts: ['rwx-dev'], current_context: 'rwx-dev', namespaces: ['default'], storage_classes: [{ name: 'nfs-csi', provisioner: 'nfs', default: false }], no_default_class: true, cni: 'flannel', enforces_network_policy: false }
  const render = (i: K8sInfo) => html(createElement(K8sConfigure, { info: i, sizes: K8S.sizes, k8s: { context: 'rwx-dev', namespace: 'theuth-nodes', storage_class: 'nfs-csi', create_namespace: true, deny_cluster_network: false }, memory_gb: 2, busy: false, onChange: () => {}, onMemory: () => {}, onDeploy: () => {} }))
  const off = render(info)
  // Rendered HTML escapes the apostrophe.
  const warning = FLANNEL_WARNING.replace("'", '&#x27;')
  expect(off).toContain(warning)
  expect(off).toContain('This cluster runs flannel')
  expect(off).toContain('No default class in this cluster')
  expect(off).toContain('Deploy to rwx-dev')
  expect(off).toContain('Uses your cluster · 2 GB request · 10 Gi volume')
  expect(render({ ...info, cni: 'calico', enforces_network_policy: true })).not.toContain(warning)
  expect(render({ ...info, cni: null, enforces_network_policy: null })).not.toContain(warning)
})

test('destroy names the node and the monthly saving, less the disk while it is kept', () => {
  const n = node()
  expect(destroySaving(n, true, 0.1)).toBe(12)
  expect(destroySaving(n, false, 0.1)).toBe(13)
  const render = (keep: boolean) => html(createElement(AlertDialog, { open: true }, createElement(DestroyBody, { node: n, perGb: 0.1, keep, onKeep: () => {} })))
  const kept = render(true)
  expect(kept).toContain('Destroy sky-cloud?')
  expect(kept).toContain('You&#x27;ll save $12/mo.')
  expect(kept).toContain('Keep the 10 GB disk ($1/mo) so you can bring it back')
  expect(kept).toMatch(/type="checkbox"[^>]*checked=""/)
  const deleted = render(false)
  expect(deleted).toContain('You&#x27;ll save $13/mo.')
  expect(deleted).toContain('The disk and everything on it is deleted.')
})

test('progress: events fold into the list and the progress entry; the stage rows follow them', () => {
  const qc = new QueryClient()
  qc.setQueryData<CloudList>(cloudKeys.list, { nodes: [], errors: [] })
  applyCloudEvent(qc, { type: 'CloudNodeChanged', node: node({ state: 'creating', stage: 'server', reachable: null, path: null, version: null }) })
  applyCloudEvent(qc, { type: 'CloudNodeProgress', name: 'sky-cloud', stage: 'storage', line: 'volume attached', pct: 40, eta_secs: 200 })
  applyCloudEvent(qc, { type: 'CloudNodeProgress', name: 'sky-cloud', stage: 'install', line: 'installing theuth 2.31.2', pct: 62, eta_secs: 125 })
  const list = qc.getQueryData<CloudList>(cloudKeys.list)
  const progress = qc.getQueryData<{ stage: string; pct: number; eta_secs: number; lines: string[] }>(cloudKeys.progress('sky-cloud'))
  expect(list?.nodes.map((n) => n.state)).toEqual(['creating'])
  expect(progress).toMatchObject({ stage: 'install', pct: 62, eta_secs: 125, lines: ['volume attached', 'installing theuth 2.31.2'] })
  expect(etaText(progress?.eta_secs)).toBe('About 2 minutes left')

  const rows = stageRows({ node: list?.nodes[0] ?? null, progress: progress as never, version: '2.31.2' })
  expect(rows.map((r) => r.status)).toEqual(['done', 'done', 'active', 'waiting', 'waiting', 'waiting'])
  expect(rows.map((r) => r.label)).toEqual(['Server created in London', 'Storage attached (10 GB)', 'Installing theuth 2.31.2 (signed)', 'Joining your mesh', 'Protecting the vault', 'Ready'])

  applyCloudEvent(qc, { type: 'CloudNodeChanged', node: node({ state: 'failed', stage: 'install', error: 'cloud-init timed out' }) })
  const failed = stageRows({ node: qc.getQueryData<CloudList>(cloudKeys.list)?.nodes[0] ?? null, progress: progress as never, version: null })
  expect(failed.map((r) => r.status)).toEqual(['done', 'done', 'failed', 'waiting', 'waiting', 'waiting'])

  applyCloudEvent(qc, { type: 'CloudNodeChanged', node: node() })
  // A Changed event carries no price: the row keeps the one `cloud_list` gave.
  applyCloudEvent(qc, { type: 'CloudNodeChanged', node: node({ state: 'busy', monthly_usd: null }) })
  expect(qc.getQueryData<CloudList>(cloudKeys.list)?.nodes[0]).toMatchObject({ state: 'busy', monthly_usd: 13 })
  applyCloudEvent(qc, { type: 'CloudNodeChanged', node: node() })
  expect(stageRows({ node: qc.getQueryData<CloudList>(cloudKeys.list)?.nodes[0] ?? null, progress: undefined, version: null }).every((r) => r.status === 'done')).toBe(true)

  applyCloudEvent(qc, { type: 'CloudNodeRemoved', name: 'sky-cloud' })
  expect(qc.getQueryData<CloudList>(cloudKeys.list)?.nodes).toEqual([])
  expect(qc.getQueryData(cloudKeys.progress('sky-cloud'))).toBeUndefined()
})

test('nearest region from the time zone', () => {
  const pick = (r: Region[], zone: string) => nearestRegion(r, zone)?.id
  expect(pick(DO_REGIONS, 'Europe/London')).toBe('lon1')
  expect(pick(FLY_REGIONS, 'Europe/London')).toBe('lhr')
  expect(pick(DO_REGIONS, 'Europe/Berlin')).toBe('fra1')
  expect(pick(DO_REGIONS, 'America/Los_Angeles')).toBe('sfo3')
  expect(pick(FLY_REGIONS, 'America/Los_Angeles')).toBe('sjc')
  expect(pick(DO_REGIONS, 'America/New_York')).toBe('nyc3')
  expect(pick(DO_REGIONS, 'Asia/Kolkata')).toBe('blr1')
  expect(pick(DO_REGIONS, 'Asia/Tokyo')).toBe('sgp1')
  expect(pick(FLY_REGIONS, 'Asia/Tokyo')).toBe('nrt')
  expect(pick(DO_REGIONS, 'Australia/Perth')).toBe('syd1')
  expect(pick(FLY_REGIONS, 'America/Sao_Paulo')).toBe('gru')
  // An unlisted city falls back to its area; a zone that places nobody, to the first region.
  expect(pick(DO_REGIONS, 'Europe/Ljubljana')).toBe('fra1')
  expect(pick(DO_REGIONS, 'UTC')).toBe('nyc3')
  expect(nearestRegion([], 'Europe/London')).toBeNull()
})

test('node names follow check_name; the default is <user>-cloud, numbered past taken names', () => {
  expect(checkName('sky-cloud')).toBeNull()
  for (const bad of ['', 'Sky', '1cloud', 'cloud-', 'a_b', 'x'.repeat(41)]) expect(checkName(bad)).not.toBeNull()
  expect(userOf('/Users/sky/.theuth')).toBe('sky')
  expect(userOf('/home/ana.m/.local/share/theuth')).toBe('ana.m')
  expect(userOf('C:\\Users\\Bob\\AppData')).toBe('Bob')
  expect(defaultName('sky')).toBe('sky-cloud')
  expect(defaultName('ana.m')).toBe('ana-m-cloud')
  expect(defaultName('Bob', ['bob-cloud'])).toBe('bob-cloud-2')
  expect(defaultName(null)).toBe('my-cloud')
  expect(defaultName('42')).toBe('n42-cloud')
})

test('bootstrapping cloud node: statusOf, NodeRow, and CloudNodeWizard done step show syncing until acked > 0', () => {
  expect(statusOf(node(), true)).toMatchObject({ text: 'Syncing mesh history…', tone: 'sync' })
  expect(statusOf(node(), false)).toMatchObject({ text: 'online · direct', tone: 'ok' })
  expect(statusOf(node({ reachable: false }), true)).toMatchObject({ text: 'offline', tone: 'bad' })
  expect(statusOf(node({ sealed: true }), true)).toMatchObject({ text: 'sealed', tone: 'warn' })

  const syncingPeer: PeerSync = { peer: 'peer-cloud-1', up: true, head: 1000, acked: 0, behind_out: 1000, held: false, held_by_peer: false }
  const syncedPeer: PeerSync = { ...syncingPeer, acked: 1000, behind_out: 0 }
  const readyNode = node({ peer_id: 'peer-cloud-1', state: 'ready', reachable: true, path: 'direct' })

  const renderSection = (rows: PeerSync[]) => {
    const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
    qc.setQueryData(cloudKeys.providers, PROVIDERS)
    qc.setQueryData<CloudList>(cloudKeys.list, { nodes: [readyNode], errors: [] })
    qc.setQueryData(peerSyncQueryKey, rows)
    return html(createElement(QueryClientProvider, { client: qc }, createElement(CloudNodesSection, {})))
  }

  const rowSyncing = renderSection([syncingPeer])
  expect(rowSyncing).toContain('Syncing mesh history…')
  expect(rowSyncing).toContain('data-syncing=""')
  expect(rowSyncing).not.toContain('online · direct')

  const rowReady = renderSection([syncedPeer])
  expect(rowReady).toContain('online · direct')
  expect(rowReady).not.toContain('Syncing mesh history…')

  const renderWizardDone = (rows: PeerSync[]) => {
    const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
    qc.setQueryData(cloudKeys.providers, PROVIDERS)
    qc.setQueryData<CloudList>(cloudKeys.list, { nodes: [readyNode], errors: [] })
    qc.setQueryData(peerSyncQueryKey, rows)
    return html(createElement(QueryClientProvider, { client: qc }, createElement(CloudNodeWizard, { entry: 'add-to-mesh', initial: { step: 'done', node: 'sky-cloud' }, onClose: () => undefined })))
  }

  const doneSyncing = renderWizardDone([syncingPeer])
  expect(doneSyncing).toContain('Syncing mesh history — sky-cloud will be ready for chats once initial sync finishes')
  expect(doneSyncing).toContain('sky-cloud joined your mesh')

  const doneReady = renderWizardDone([syncedPeer])
  expect(doneReady).toContain('sky-cloud is ready')
  expect(doneReady).toContain('Ready')
  expect(doneReady).not.toContain('Syncing mesh history —')
})

test('reopen status: creating and failed rows expose View status; specFromNode preserves every configure field', () => {
  const creating = node({ name: 'alpha-cloud', state: 'creating', stage: 'install', region: 'fra1', memory_gb: 8, desktop: false, version: null })
  const failed = node({ name: 'beta-cloud', state: 'failed', stage: 'join', error: 'mesh join timed out', region: 'nyc3', memory_gb: 4, desktop: true, version: null })
  const ready = node({ name: 'gamma-cloud', state: 'ready' })
  const k8sNode = node({ name: 'k8s-cloud', provider: 'kubernetes', region: null, memory_gb: 4, desktop: false, k8s: { context: 'rwx-dev', namespace: 'theuth-nodes', storage_class: 'nfs-csi', create_namespace: false, deny_cluster_network: true } })

  expect(specFromNode(creating)).toEqual({ provider: 'digitalocean', name: 'alpha-cloud', region: 'fra1', memory_gb: 8, volume_gb: 10, desktop: false, k8s: null })
  expect(specFromNode(k8sNode)).toEqual({ provider: 'kubernetes', name: 'k8s-cloud', region: undefined, memory_gb: 4, volume_gb: 10, desktop: false, k8s: k8sNode.k8s })

  const renderRows = (nodes: CloudNode[]) => {
    const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
    qc.setQueryData(cloudKeys.providers, PROVIDERS)
    qc.setQueryData<CloudList>(cloudKeys.list, { nodes, errors: [] })
    return html(createElement(QueryClientProvider, { client: qc }, createElement(CloudNodesSection, {})))
  }

  expect(renderRows([creating])).toContain('View status')
  expect(renderRows([failed])).toContain('View status')
  expect(renderRows([ready])).not.toContain('View status')
})

test('reopened CloudNodeWizard handles creating, failed, already-ready, and disappeared nodes without trapping on a spinner', () => {
  const renderProgress = (nodes: CloudNode[], target = 'sky-cloud') => {
    const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
    qc.setQueryData(cloudKeys.providers, PROVIDERS)
    qc.setQueryData<CloudList>(cloudKeys.list, { nodes, errors: [] })
    return html(createElement(QueryClientProvider, { client: qc }, createElement(CloudNodeWizard, { entry: 'add-to-mesh', initial: { step: 'progress', node: target }, onClose: () => undefined })))
  }

  const creating = renderProgress([node({ state: 'creating', stage: 'install', version: null })])
  expect(creating).toContain('Starting sky-cloud')
  expect(creating).toContain('role="progressbar"')

  const failed = renderProgress([node({ state: 'failed', stage: 'install', error: 'cloud-init timed out', version: null })])
  expect(failed).toContain('sky-cloud could not start')
  expect(failed).toContain('cloud-init timed out')
  expect(failed).toContain('Destroy and retry')

  // Already ready: immediately advances to DoneView instead of showing a spinner.
  const alreadyReady = renderProgress([node({ state: 'ready' })])
  expect(alreadyReady).toContain('sky-cloud is ready')
  expect(alreadyReady).not.toContain('Starting sky-cloud')
  expect(alreadyReady).not.toContain('role="progressbar"')

  // Disappeared (destroyed elsewhere): explains and offers Close instead of spinning forever.
  const missing = renderProgress([], 'ghost-cloud')
  expect(missing).toContain('ghost-cloud is no longer here')
  expect(missing).toContain('Close')
  expect(missing).not.toContain('role="progressbar"')
})

test('/cloud/new search params accept node for reopening provisioning status on phone', () => {
  const validate = CloudNewRoute.options.validateSearch as (s: Record<string, unknown>) => { entry?: string; node?: string }
  expect(validate({ entry: 'add-to-mesh', node: 'sky-cloud' })).toEqual({ entry: 'add-to-mesh', node: 'sky-cloud' })
  expect(validate({ node: 'sky-cloud' })).toEqual({ node: 'sky-cloud' })
  expect(validate({ entry: 'add-to-mesh', node: '' })).toEqual({ entry: 'add-to-mesh' })
  expect(validate({ node: 123 })).toEqual({})
})


test('Open terminal opens a node-terminal popout window (never a session route) on desktop, and falls back to a sheet on iOS', () => {
  mockOpenPopout.mockClear()
  mockNavigate.mockClear()
  mockCanPopout.mockReturnValue(true)

  const openSheet = vi.fn()
  dispatchTerminalOpen({ shell: 'main', engine: 'peer-1' }, 'sky-do', openSheet)
  expect(mockOpenPopout).toHaveBeenCalledWith({ kind: 'node-terminal', engine: 'peer-1', shell: 'main', name: 'sky-do' })
  expect(openSheet).not.toHaveBeenCalled()
  expect(mockNavigate).not.toHaveBeenCalled()

  // iOS (`!canPopout()`): opens the sheet with the same shell target, still no session route.
  mockOpenPopout.mockClear()
  mockCanPopout.mockReturnValue(false)
  dispatchTerminalOpen({ shell: 'main', engine: 'peer-1' }, 'sky-do', openSheet)
  expect(mockOpenPopout).not.toHaveBeenCalled()
  expect(openSheet).toHaveBeenCalledWith({ shell: 'main', engine: 'peer-1', name: 'sky-do' })
  expect(mockNavigate).not.toHaveBeenCalled()
})

test('ready cloud node status comes from live mesh peer; absent peer says "not in the mesh"; lifecycle words stay on the cloud record', () => {
  const now = Date.now()
  const staleOffline = node({ name: 'lan-box', peer_id: 'peer-lan-1', state: 'ready', reachable: false, path: 'offline' })
  // Live LAN peer overrides stale cloud_list "offline".
  expect(statusOf(staleOffline, false, { reachable: true, path: 'lan', rtt_ms: 12, last_seen_ms: now }, now)).toEqual({ text: 'LAN 12 ms', tone: 'ok' })
  expect(statusOf(staleOffline, false, { reachable: true, path: 'relay', rtt_ms: 41, last_seen_ms: now }, now)).toEqual({ text: 'Relay 41 ms', tone: 'ok' })
  expect(statusOf(staleOffline, false, { reachable: false, path: 'offline', rtt_ms: null, last_seen_ms: now - 3 * 3_600_000 }, now)).toEqual({ text: 'offline 3h', tone: 'bad' })
  // Mesh list loaded, but peer is absent: "not in the mesh", never "offline".
  expect(statusOf(staleOffline, false, null, now)).toEqual({ text: 'not in the mesh', tone: 'muted' })
  // Lifecycle words stay on the cloud record regardless of live peer.
  expect(statusOf(node({ state: 'creating', stage: 'install' }), false, null, now)).toEqual({ text: 'creating · installing', tone: 'busy' })
  expect(statusOf(node({ state: 'failed', error: 'boom' }), false, null, now)).toEqual({ text: 'failed', tone: 'bad' })
  expect(statusOf(node({ state: 'destroying' }), false, null, now)).toEqual({ text: 'destroying…', tone: 'busy' })
  expect(statusOf(node({ state: 'ready', sealed: true }), false, { reachable: true, path: 'lan', rtt_ms: 12, last_seen_ms: now }, now)).toMatchObject({ text: 'sealed', tone: 'warn' })

  // CloudNodesSection matches by peer_id against live mesh status and renders "LAN 12 ms" / "not in the mesh".
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  qc.setQueryData(cloudKeys.providers, PROVIDERS)
  const missingPeerNode = node({ name: 'orphan-peer', peer_id: 'peer-missing', state: 'ready', reachable: false, path: 'offline' })
  qc.setQueryData<CloudList>(cloudKeys.list, { nodes: [staleOffline, missingPeerNode], errors: [] })
  const mesh = {
    self_id: 'self-1',
    self_name: 'laptop',
    mesh_name: 'home',
    peers: [{ id: 'peer-lan-1', name: 'lan-box', addrs: [], reachable: true, last_seen_ms: now, hub_pref: 'eligible' as const, path: 'lan' as const, rtt_ms: 12 }],
    lease: { holder: 'self-1', since_ms: now, reason: 'prefer' },
    traffic: [],
    replica: null,
    stats: [],
    relay: null,
    messaging_off: [],
    nearby: false,
    quarantined: [],
    docs_homes_on: false,
  }
  const rendered = html(createElement(QueryClientProvider, { client: qc }, createElement(CloudNodesSection, { mesh })))
  expect(rendered).toContain('LAN 12 ms')
  expect(rendered).toContain('not in the mesh')
  expect(rendered).not.toContain('>offline<')

  // State event that clears peer_id on a ready node preserves the known peer_id.
  applyCloudEvent(qc, { type: 'CloudNodeChanged', node: { ...staleOffline, peer_id: null } })
  expect(qc.getQueryData<CloudList>(cloudKeys.list)?.nodes.find((n) => n.name === 'lan-box')?.peer_id).toBe('peer-lan-1')
})

test('ready cloud NodeRow renders visible Update button when a newer release is available, hides it when null, and shows updateError visibly', () => {
  const ready = node({ name: 'prod-1', peer_id: 'peer-prod-1', state: 'ready', version: '2.31.2' })
  const withoutUpdate = html(
    createElement(NodeRow, {
      node: ready,
      update: null,
      onRestart: () => {},
      onUpdate: () => {},
      onTerminal: () => {},
      onDesktop: () => {},
      opening: false,
      onOpen: () => {},
    }),
  )
  expect(withoutUpdate).not.toContain('data-update-node=')

  const withUpdate = html(
    createElement(NodeRow, {
      node: ready,
      update: { version: '2.35.0', applied: false, pending: false, onApply: () => {} },
      updateError: 'update_apply: permission denied',
      onRestart: () => {},
      onUpdate: () => {},
      onTerminal: () => {},
      onDesktop: () => {},
      opening: false,
      onOpen: () => {},
    }),
  )
  expect(withUpdate).toContain('data-update-node="prod-1"')
  expect(withUpdate).toContain('Update 2.35.0')
  expect(withUpdate).toContain('data-update-error="peer-prod-1"')
  expect(withUpdate).toContain('update_apply: permission denied')
})


