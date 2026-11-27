import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import type * as Router from '@tanstack/react-router'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import { AlertDialog } from '@/components/ui/alert-dialog'
import { CloudNodeWizard } from './CloudNodeWizard'
import { DestroyBody } from './CloudNodesSettings'
import { checkName, createLabel, defaultName, destroySaving, etaText, fromPrice, localQuote, stageRows, userOf } from './format'
import { applyCloudEvent, cloudKeys } from './queries'
import { nearestRegion } from './region'
import { FLANNEL_WARNING, K8sConfigure, PhoneConfigureStep, ProviderCards } from './steps'
import type { CloudList, CloudNode, K8sInfo, ProviderInfo, Region } from './types'

vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => ({}) }))
vi.mock('@tanstack/react-router', async (original) => ({ ...(await original<typeof Router>()), useNavigate: () => () => undefined }))

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
const LIGHTSAIL = provider({ provider: 'lightsail', label: 'AWS Lightsail', available: false, sizes: [{ memory_gb: 2, vcpus: 2, slug: 'small_3_0', monthly_usd: 12 }] })
const PROVIDERS = [DO, FLY, K8S, LIGHTSAIL]

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
  ...n,
})

const html = (el: ReactNode) => renderToStaticMarkup(el as never)

function wizard(entry: 'first-run' | 'add-to-mesh', initial: Parameters<typeof CloudNodeWizard>[0]['initial']): string {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  qc.setQueryData(cloudKeys.providers, PROVIDERS)
  qc.setQueryData<CloudList>(cloudKeys.list, { nodes: [], errors: [] })
  return html(createElement(QueryClientProvider, { client: qc }, createElement(CloudNodeWizard, { entry, initial, onClose: () => undefined })))
}

test('the create button names the monthly total of the quote; the phone cost card splits server and disk', () => {
  const q = localQuote(DO, 2)
  expect(q).toEqual({ provider: 'digitalocean', size_slug: 's-1vcpu-2gb', server_usd: 12, disk_usd: 1, total_usd: 13 })
  expect(createLabel({ provider: 'digitalocean', k8s: null }, q)).toBe('Create node · $13/mo')
  expect(createLabel({ provider: 'fly', k8s: null }, localQuote(FLY, 2))).toBe('Create node · $12.19/mo')
  expect(createLabel({ provider: 'kubernetes', k8s: { context: 'rwx-dev', namespace: null, storage_class: null } }, null)).toBe('Deploy to rwx-dev')
  const spec = { provider: 'digitalocean' as const, name: 'sky-cloud', region: 'lon1', memory_gb: 4, volume_gb: 10, k8s: null }
  const out = html(createElement(PhoneConfigureStep, { provider: DO, spec, quote: localQuote(DO, 4), nameError: null, busy: false, onName: () => {}, onRegion: () => {}, onMemory: () => {}, onCreate: () => {} }))
  expect(out).toContain('Create node · $25/mo')
  expect(out).toContain('$24/mo + $1 disk')
  expect(out).toContain('No open ports · updates itself')
})

test('the wizard starts at 2 GB, the minimum, with the hint and the 2 GB price on the button', () => {
  const out = wizard('add-to-mesh', { step: 'configure', spec: { provider: 'digitalocean', region: 'lon1' } })
  expect(out).toMatch(/aria-checked="true"[^>]*>2 GB</)
  expect(out).not.toMatch(/aria-checked="true"[^>]*>4 GB</)
  expect(out).toContain('2 GB is the minimum. 4 GB for heavy browsing or local models.')
  expect(out).toContain('Create node · $13/mo')
  expect(out).toContain('Step 2 of 3')
  expect(Math.min(...DO.sizes.map((s) => s.memory_gb))).toBe(2)
})

test('first run: the choose screen prices the cheapest 2 GB server across available providers', () => {
  expect(fromPrice(PROVIDERS, true)).toBe('from $12/mo')
  expect(fromPrice([FLY, LIGHTSAIL], true)).toBe('from $11/mo')
  const out = wizard('first-run', {})
  expect(out).toContain('Where should theuth run?')
  expect(out).toContain('from $12/mo')
  expect(out).toContain('Just this iPhone')
  expect(out).toContain('You can add a cloud node later')
  // add-to-mesh skips ①.
  expect(wizard('add-to-mesh', {})).toContain('Pick a provider')
})

test('AWS Lightsail is listed but disabled as coming soon; Kubernetes only on desktop', () => {
  const props = { providers: PROVIDERS, selected: 'digitalocean' as const, k8sContexts: 3, onSelect: () => {} }
  const phone = html(createElement(ProviderCards, { ...props, phone: true }))
  expect(phone).toMatch(/aria-disabled="true"[^>]*data-provider="lightsail"/)
  expect(phone).toContain('coming soon')
  expect(phone).not.toContain('data-provider="kubernetes"')
  expect(phone).not.toMatch(/aria-disabled="true"[^>]*data-provider="digitalocean"/)
  const desktop = html(createElement(ProviderCards, { ...props, phone: false }))
  expect(desktop).toContain('data-provider="kubernetes"')
  expect(desktop).toContain('3 contexts in ~/.kube/config')
  expect(desktop).toContain('uses your cluster')
  // The phone wizard's provider step never offers Kubernetes either.
  expect(wizard('add-to-mesh', { step: 'provider' })).not.toContain('data-provider="kubernetes"')
})

test('the flannel warning shows only when the cluster is known not to enforce NetworkPolicies', () => {
  const info: K8sInfo = { contexts: ['rwx-dev'], current_context: 'rwx-dev', namespaces: ['default'], storage_classes: [{ name: 'nfs-csi', provisioner: 'nfs', default: false }], no_default_class: true, cni: 'flannel', enforces_network_policy: false }
  const render = (i: K8sInfo) => html(createElement(K8sConfigure, { info: i, k8s: { context: 'rwx-dev', namespace: 'theuth-nodes', storage_class: 'nfs-csi' }, memory_gb: 2, busy: false, onChange: () => {}, onDeploy: () => {} }))
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
  expect(rows.map((r) => r.status)).toEqual(['done', 'done', 'active', 'waiting', 'waiting'])
  expect(rows.map((r) => r.label)).toEqual(['Server created in London', 'Storage attached (10 GB)', 'Installing theuth 2.31.2 (signed)', 'Joining your mesh', 'Ready'])

  applyCloudEvent(qc, { type: 'CloudNodeChanged', node: node({ state: 'failed', stage: 'install', error: 'cloud-init timed out' }) })
  const failed = stageRows({ node: qc.getQueryData<CloudList>(cloudKeys.list)?.nodes[0] ?? null, progress: progress as never, version: null })
  expect(failed.map((r) => r.status)).toEqual(['done', 'done', 'failed', 'waiting', 'waiting'])

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
