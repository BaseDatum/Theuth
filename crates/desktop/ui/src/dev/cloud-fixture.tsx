// 224 visual check, dev only: `bunx vite` then open `/src/dev/cloud-fixture.html` (light; `?theme=dark` for dark).
// The real cloud-node wizard, Settings › Machines › Cloud nodes and the engine-switcher entry, no engine: a Proxy Api
// answers every `cloud_*` call with the catalog's own prices and regions (`crates/cloudnodes/src/providers/catalog.rs`).
// Each screen runs in its own iframe (`?state=<name>`); phone states are 390×844 in the iOS shell, desktop ~1100×720.
// `scripts/cloud-fixture-shots.ts` saves one PNG per state into docs/design/224-assets/.
import '@/styles.css'
import { QueryClientProvider } from '@tanstack/react-query'
import { RouterProvider, createMemoryHistory, createRootRoute, createRouter } from '@tanstack/react-router'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { AddCloudNodeEntry } from '@/components/cloud/AddCloudNodeEntry'
import { CloudNodeWizard, type WizardEntry, type WizardInitial } from '@/components/cloud/CloudNodeWizard'
import { type CloudDialog, CloudNodesSettings } from '@/components/cloud/CloudNodesSettings'
import { localQuote } from '@/components/cloud/format'
import { cloudKeys } from '@/components/cloud/queries'
import { Card } from '@/components/cloud/steps'
import { NodeDesktopPanel } from '@/components/cloud/NodeDesktop'
import type { CloudApi, CloudList, CloudNode, K8sInfo, ProviderInfo, ProviderKind, Region, SizeOption } from '@/components/cloud/types'
import { TooltipProvider } from '@/components/ui/tooltip'
import { type Api, METHODS, SUBSCRIPTIONS } from '@/rpc/bindings/client'
import type { RpcClient } from '@/rpc/client'
import { active } from '@/store/engine-scope'

const query = new URLSearchParams(location.search)
const dark = query.get('theme') === 'dark'
document.documentElement.classList.toggle('dark', dark)
const state = query.get('state')
const phone = state?.startsWith('phone-') ?? false
/** States where the engine advertises the sealed vault and `cloud_create(spec, vault_password)` (design 225). */
const VAULT = state === 'phone-configure-vault' || state === 'desktop-configure-vault' || state === 'settings-machines' || state === 'vault-password'

// ---- the catalog (catalog.rs) ----

const sizes = (rows: [number, number, string, number | null][]): SizeOption[] => rows.map(([memory_gb, vcpus, slug, monthly_usd]) => ({ memory_gb, vcpus, slug, monthly_usd }))
const regions = (rows: [string, string, string][]): Region[] => rows.map(([id, label, country]) => ({ id, label, country }))
const DO_REGIONS = regions([['nyc3', 'New York', 'US'], ['sfo3', 'San Francisco', 'US'], ['tor1', 'Toronto', 'CA'], ['lon1', 'London', 'GB'], ['ams3', 'Amsterdam', 'NL'], ['fra1', 'Frankfurt', 'DE'], ['blr1', 'Bangalore', 'IN'], ['sgp1', 'Singapore', 'SG'], ['syd1', 'Sydney', 'AU']])
const FLY_REGIONS = regions([['lhr', 'London', 'GB'], ['ams', 'Amsterdam', 'NL'], ['fra', 'Frankfurt', 'DE'], ['iad', 'Ashburn, Virginia', 'US'], ['ord', 'Chicago', 'US'], ['sjc', 'San Jose', 'US'], ['yyz', 'Toronto', 'CA'], ['sin', 'Singapore', 'SG'], ['syd', 'Sydney', 'AU'], ['nrt', 'Tokyo', 'JP'], ['gru', 'São Paulo', 'BR']])
const LS_REGIONS = regions([['us-east-1', 'Virginia', 'US'], ['eu-west-2', 'London', 'GB'], ['eu-central-1', 'Frankfurt', 'DE']])

// Desktop: doctl was found and imported, Fly is not set up, kubectl has three contexts. Phone: nothing connected yet.
const PROVIDERS: ProviderInfo[] = [
  {
    provider: 'digitalocean',
    label: 'DigitalOcean',
    available: true,
    beta: false,
    desktop_only: false,
    // Signed in from the configure step on (the phone got there through Sign in with DigitalOcean).
    connected: state !== 'desktop-provider' && state !== 'phone-provider' && state !== 'phone-choose',
    source: phone ? 'oauth' : 'imported',
    discovered: phone ? [] : [{ id: 'doctl:default', label: 'doctl login', detail: 'context "default"', path: '~/Library/Application Support/doctl/config.yaml' }],
    oauth: true,
    sizes: sizes([[2, 1, 's-1vcpu-2gb', 12], [4, 2, 's-2vcpu-4gb', 24], [8, 4, 's-4vcpu-8gb', 48]]),
    regions: DO_REGIONS,
    volume_usd_per_gb: 0.1,
  },
  {
    provider: 'fly',
    label: 'Fly.io',
    available: true,
    beta: true,
    desktop_only: false,
    connected: state === 'settings-machines' || state === 'destroy-confirm' || state === 'resize' || state === 'logs',
    source: null,
    discovered: [],
    oauth: false,
    sizes: sizes([[2, 1, 'shared-cpu-1x', 10.69], [4, 2, 'shared-cpu-2x', 21.38], [8, 4, 'shared-cpu-4x', 42.76]]),
    regions: FLY_REGIONS,
    volume_usd_per_gb: 0.15,
  },
  {
    provider: 'kubernetes',
    label: 'Kubernetes',
    available: true,
    beta: false,
    desktop_only: true,
    connected: true,
    source: 'kubeconfig',
    discovered: [],
    oauth: false,
    sizes: sizes([[2, 1, '2Gi', null], [4, 1, '4Gi', null], [8, 1, '8Gi', null]]),
    regions: [],
    volume_usd_per_gb: null,
  },
  {
    provider: 'lightsail',
    label: 'AWS Lightsail',
    available: false,
    beta: false,
    desktop_only: false,
    connected: false,
    source: null,
    discovered: [],
    oauth: false,
    sizes: sizes([[2, 2, 'small_3_0', 12], [4, 2, 'medium_3_0', 24], [8, 2, 'large_3_0', 44]]),
    regions: LS_REGIONS,
    volume_usd_per_gb: 0.1,
  },
]

const K8S: K8sInfo = {
  contexts: ['rwx-dev', 'minikube', 'prod-eu'],
  current_context: 'rwx-dev',
  namespaces: ['default', 'kube-system', 'monitoring'],
  storage_classes: [
    { name: 'nfs-csi', provisioner: 'nfs.csi.k8s.io', default: false },
    { name: 'local-path', provisioner: 'rancher.io/local-path', default: false },
  ],
  no_default_class: true,
  cni: 'flannel',
  enforces_network_policy: false,
}

const now = Date.now()
const node = (n: Partial<CloudNode> & Pick<CloudNode, 'name' | 'provider' | 'state'>): CloudNode => ({
  region: null,
  region_label: null,
  memory_gb: 2,
  size_slug: null,
  volume_gb: 10,
  monthly_usd: null,
  stage: null,
  error: null,
  provider_state: null,
  ip: null,
  peer_id: null,
  reachable: null,
  path: null,
  version: null,
  auto_update: true,
  created_ms: now - 86_400_000,
  desktop: true,
  k8s: null,
  ssh: null,
  vault: null,
  link: null,
  ...n,
})

const SKY: CloudNode = node({ name: 'sky-cloud', provider: 'digitalocean', state: 'ready', region: 'lon1', region_label: 'London', size_slug: 's-1vcpu-2gb', monthly_usd: 13, provider_state: 'active', ip: '203.0.113.24', peer_id: 'e1f2', reachable: true, path: 'direct', version: '2.31.2', vault: 'protected' })
// The wizard's screens see only the node they follow; Settings sees every kind of row.
const NODES: CloudNode[] =
  state === 'phone-progress' || state === 'desktop-progress'
    ? [{ ...SKY, state: 'creating', stage: 'vault', vault: 'pending', path: 'direct', version: null }]
    : state === 'phone-done'
      ? [SKY]
      : state?.startsWith('phone-') || state?.startsWith('desktop-')
        ? []
        : [
            SKY,
            node({ name: 'lab-fly', provider: 'fly', state: 'ready', region: 'lhr', region_label: 'London', size_slug: 'shared-cpu-1x', monthly_usd: 12.19, peer_id: 'a7c3', reachable: true, path: 'relay', version: '2.31.2', vault: 'device_only' }),
            node({ name: 'vault-box', provider: 'digitalocean', state: 'ready', region: 'fra1', region_label: 'Frankfurt', size_slug: 's-2vcpu-4gb', memory_gb: 4, monthly_usd: 25, peer_id: 'b9d1', reachable: true, path: 'direct', version: '2.31.2', sealed: true, vault: 'protected' }),
            // The create job lost its password (the engine restarted mid-create): device only, asking for one.
            node({ name: 'build-box', provider: 'digitalocean', state: 'ready', region: 'ams3', region_label: 'Amsterdam', size_slug: 's-1vcpu-2gb', monthly_usd: 13, peer_id: 'c4e8', reachable: true, path: 'direct', version: '2.31.2', vault: 'needs_password' }),
            node({ name: 'night-runner', provider: 'kubernetes', state: 'creating', stage: 'join', size_slug: '2Gi', k8s: { context: 'rwx-dev', namespace: 'theuth-nodes', storage_class: 'nfs-csi', create_namespace: true, deny_cluster_network: false } }),
            node({ name: 'theuth-test', provider: 'digitalocean', state: 'orphan', region: 'nyc3', region_label: null, size_slug: 's-1vcpu-2gb', monthly_usd: 13, version: '2.31.2' }),
          ]
const LIST: CloudList = { nodes: NODES, errors: state === 'settings-machines' ? [{ provider: 'fly', message: 'token rejected (401); sign in again' }] : [] }

const LOGS = [
  '2026-11-27T09:12:01Z INFO theuth 2.31.2 starting (data /data/theuth)',
  '2026-11-27T09:12:01Z INFO vault unlocked (sealed key from /data)',
  '2026-11-27T09:12:02Z INFO mesh: endpoint e1f2…9a0c, relay iroh.relay.theuth.io',
  '2026-11-27T09:12:03Z INFO mesh: peer 333 (MacBook Pro) direct 18 ms',
  '2026-11-27T09:12:03Z INFO mesh: peer sky’s iPhone relay 41 ms',
  '2026-11-27T09:30:00Z INFO routines: "morning brief" scheduled 07:00 Europe/London',
  '2026-11-27T10:02:44Z INFO update: stable 2.31.2 is current',
  '2026-11-27T11:15:09Z WARN browser: page load slow (8.2 s) https://example.com/report',
  '2026-11-27T11:40:51Z INFO agent: turn finished in 41 s (session 01a21…)',
]

const ok = <T,>(v: T) => Promise.resolve(v)
const info = (k: ProviderKind) => PROVIDERS.find((p) => p.provider === k) as ProviderInfo
const cloud: CloudApi = {
  cloud_providers: () => ok(PROVIDERS),
  cloud_import_credential: (k) => ok({ ...info(k), connected: true, source: 'imported' }),
  cloud_set_token: (k) => ok({ ...info(k), connected: true, source: 'pasted' }),
  cloud_forget_credential: () => ok(null),
  // The browser half has no fixture: the wait stays until Cancel.
  cloud_oauth_start: (k) => ok({ url: `https://relay.theuth.io/oauth/${k}/start?fixture` }),
  cloud_oauth_cancel: () => ok(null),
  cloud_quote: (spec) => ok(localQuote(spec.provider, info(spec.provider).sizes, info(spec.provider).volume_usd_per_gb, spec.memory_gb, spec.volume_gb ?? 10) as never),
  // Live sizes: DigitalOcean priced (fake numbers, fixture only); Fly.io and Kubernetes give no prices; the
  // `-price-unavailable` state has DigitalOcean fail, the `-loading` one never answers (the skeleton).
  cloud_sizes: (k) => {
    if (state === 'phone-configure-loading') return new Promise(() => undefined)
    const unpriced = k !== 'digitalocean' || state === 'phone-configure-price-unavailable'
    const sizes = info(k).sizes.map((s) => ({ ...s, monthly_usd: unpriced ? null : s.monthly_usd }))
    return ok({ sizes, priced: !unpriced, error: state === 'phone-configure-price-unavailable' ? 'the pricing API did not answer' : null })
  },
  cloud_desktop: () => ok({ ws_url: 'ws://127.0.0.1:9/fixture', password: 'fixture', geometry: '1280×800', engine: 'e1f2' }),
  cloud_k8s_info: () => ok(K8S),
  cloud_ssh_hosts: () => ok({ ssh_available: true, hosts: [{ alias: 'prod-box', hostname: '10.0.0.10', user: 'ubuntu', port: 22, identity: null, proxy_jump: null }] }),
  cloud_ssh_probe: () => ok({ os: 'linux', arch: 'x86_64', ok: true, error: null }),
  // The fixture ignores the vault password: nothing is created.
  cloud_create: (spec) =>
    ok(
      node({
        name: spec.name,
        provider: spec.provider,
        state: 'creating',
        region: spec.region,
        region_label: info(spec.provider).regions.find((r) => r.id === spec.region)?.label ?? null,
        stage: 'server',
        ssh: spec.ssh ? { host: spec.ssh.host, user: spec.ssh.user ?? null, port: spec.ssh.port ?? null, identity: spec.ssh.identity ?? null, name: spec.ssh.name ?? null } : null,
      }),
    ),
  cloud_list: () => ok(LIST),
  cloud_status: (name) => ok(NODES.find((n) => n.name === name) as CloudNode),
  cloud_restart: (name) => ok(NODES.find((n) => n.name === name) ?? SKY),
  cloud_resize: (name, memory_gb) => ok({ ...(NODES.find((n) => n.name === name) ?? SKY), state: 'busy' as const, memory_gb }),
  cloud_update: (name) => ok(NODES.find((n) => n.name === name) ?? SKY),
  cloud_destroy: () => ok(['droplet 4821', 'firewall theuth-sky-cloud']),
  cloud_terminal: (name) => ok({ session: 'fixture', job: `pty-${name}`, engine: 'e1f2' }),
  cloud_logs: (name) => ok({ name, lines: LOGS }),
  // Phone-only pairing (224 §16.12) has no fixture: no node answers.
  cloud_link_pairing: () => Promise.reject(new Error('no node in the fixture')),
  cloud_link_paired: (name) => ok(NODES.find((n) => n.name === name) ?? SKY),
}
const api = new Proxy(cloud as unknown as Api, {
  get: (t, name: string) =>
    (t as unknown as Record<string, unknown>)[name] ??
    (async () => {
      throw new Error(`fixture: ${name} not answered`)
    }),
})

// Discovery that offers every method (the cloud ones too, before the bindings know them) and the cloud topic.
const cloudMethods = Object.keys(cloud)
const doc = {
  // The sealed vault is advertised only in the states that show it, with `cloud_create`'s `vault_password` parameter.
  methods: [
    ...[...METHODS, ...cloudMethods, 'subscribe_topics'].map((name) =>
      name === 'cloud_create' && VAULT ? { name, params: [{ name: 'spec', required: true, schema: {} }, { name: 'vault_password', required: false, schema: { type: ['string', 'null'] } }] } : { name, params: [] },
    ),
    ...(VAULT ? [{ name: 'vault_status' }] : []),
  ],
  'x-theuth': {
    subscriptions: Object.entries(SUBSCRIPTIONS).map(([subscribe, s]) => ({ subscribe, notification: s.notification, unsubscribe: s.unsubscribe })),
    events: [{ topic: 'cloudnodes.events', version: 1 }],
  },
}
active().connection.setState({
  api,
  client: {} as RpcClient,
  state: 'open',
  info: { name: '333', version: '2.31.2', data_dir: '/Users/sky/.theuth' } as never,
  discovery: { phase: 'ready', capabilities: null, document: doc as never, unavailable: new Set() },
} as never)
const qc = active().qc
if (state === 'phone-progress' || state === 'desktop-progress')
  qc.setQueryData(cloudKeys.progress('sky-cloud'), { stage: 'vault', pct: 95, eta_secs: 10, lines: ['droplet 4821 active', 'volume attached', 'installing theuth 2.31.2', 'Joined the mesh; connecting', 'Protecting the vault'] })

if (phone) {
  // The iOS shell as `main.tsx` marks it, with an iPhone's safe areas.
  document.documentElement.dataset.container = 'ios'
  document.documentElement.style.setProperty('--safe-top', '47px')
  document.documentElement.style.setProperty('--safe-bottom', '34px')
}

// ---- screens ----

const LONDON = { provider: 'digitalocean' as const, region: 'lon1', name: 'sky-cloud' }
const WIZARD: Record<string, { entry: WizardEntry; initial: WizardInitial }> = {
  'phone-choose': { entry: 'first-run', initial: {} },
  'phone-provider': { entry: 'first-run', initial: { step: 'provider', spec: { provider: 'digitalocean' } } },
  'phone-configure': { entry: 'first-run', initial: { step: 'configure', spec: LONDON } },
  'phone-configure-desktop-off': { entry: 'first-run', initial: { step: 'configure', spec: { ...LONDON, desktop: false } } },
  'phone-configure-2gb-desktop': { entry: 'first-run', initial: { step: 'configure', spec: { ...LONDON, memory_gb: 2 } } },
  'phone-configure-price-unavailable': { entry: 'first-run', initial: { step: 'configure', spec: LONDON } },
  'phone-configure-loading': { entry: 'first-run', initial: { step: 'configure', spec: LONDON } },
  'phone-configure-vault': { entry: 'first-run', initial: { step: 'configure', spec: LONDON, vault: { password: 'otter-lantern-quarry-91', confirm: 'otter-lantern-quarry-91' } } },
  'phone-progress': { entry: 'first-run', initial: { step: 'progress', spec: LONDON, node: 'sky-cloud' } },
  'phone-done': { entry: 'first-run', initial: { step: 'done', spec: LONDON, node: 'sky-cloud' } },
  'desktop-provider': { entry: 'desktop-dialog', initial: { step: 'provider', spec: { provider: 'digitalocean' } } },
  'desktop-configure': { entry: 'desktop-dialog', initial: { step: 'configure', spec: LONDON, advancedOpen: true } },
  'desktop-configure-vault': { entry: 'desktop-dialog', initial: { step: 'configure', spec: LONDON, vault: { password: 'otter-lantern-quarry-91', confirm: 'otter-lantern-quarry-91' } } },
  'desktop-k8s': { entry: 'desktop-dialog', initial: { step: 'configure', spec: { provider: 'kubernetes', name: 'sky-cloud' } } },
  'desktop-progress': { entry: 'desktop-dialog', initial: { step: 'progress', spec: LONDON, node: 'sky-cloud' } },
}
const SETTINGS: Record<string, { dialog: CloudDialog; node: string } | null> = {
  'settings-machines': null,
  'destroy-confirm': { dialog: 'destroy', node: 'sky-cloud' },
  resize: { dialog: 'resize', node: 'sky-cloud' },
  logs: { dialog: 'logs', node: 'sky-cloud' },
  'desktop-view': null,
  'vault-password': { dialog: 'vault', node: 'build-box' },
}

/** `desktop-view` has no VNC server: a drawn stand-in for the node's screen (a browser an agent is using). */
function FakeScreen() {
  return (
    <div className="flex h-full w-full flex-col bg-layer-3 p-6">
      <div className="mx-auto flex w-full max-w-3xl flex-1 flex-col overflow-hidden rounded-lg border border-border bg-background shadow-lg">
        <div className="flex h-8 items-center gap-2 border-b border-border bg-layer-2 px-3 text-xs text-fg-secondary">
          <span className="size-2.5 rounded-full bg-fg-tertiary/40" />
          <span className="size-2.5 rounded-full bg-fg-tertiary/40" />
          <span className="size-2.5 rounded-full bg-fg-tertiary/40" />
          <span className="ml-3 flex-1 truncate rounded bg-background px-2 py-0.5">https://example.com/quarterly-report</span>
        </div>
        <div className="flex flex-1 flex-col gap-3 p-6">
          <div className="h-5 w-2/3 rounded bg-layer-3" />
          <div className="h-3 w-full rounded bg-layer-2" />
          <div className="h-3 w-5/6 rounded bg-layer-2" />
          <div className="h-3 w-4/6 rounded bg-layer-2" />
          <div className="mt-4 h-32 w-full rounded bg-layer-2" />
        </div>
      </div>
    </div>
  )
}

/** A stand-in for the phone's engine switcher (not mountable here), with the real entry card in it. */
function Engines() {
  return (
    <div className="flex min-h-dvh flex-col gap-2.5 bg-background px-4 pt-[calc(var(--safe-top)+12px)] text-foreground">
      <div className="text-sm font-semibold">Engines</div>
      <Card className="flex items-center gap-2.5">
        <span className="grid size-7 place-items-center rounded-[7px] bg-layer-3">📱</span>
        <div>
          <div className="text-sm font-semibold">This iPhone</div>
          <div className="text-xs text-fg-secondary">local</div>
        </div>
      </Card>
      <Card selected className="flex items-center gap-2.5">
        <span className="grid size-7 place-items-center rounded-[7px] bg-primary text-xs font-bold text-primary-foreground">M</span>
        <div>
          <div className="text-sm font-semibold">333</div>
          <div className="text-xs text-fg-secondary">MacBook Pro · online</div>
        </div>
      </Card>
      <AddCloudNodeEntry onClick={() => undefined} />
      <button type="button" className="rounded-[10px] border border-border bg-card py-2.5 text-sm font-semibold">
        Pair with a mesh…
      </button>
    </div>
  )
}

function Scenario({ name }: { name: string }) {
  if (name === 'phone-engines-entry') return <Engines />
  const w = WIZARD[name]
  if (w)
    return w.entry === 'desktop-dialog' ? (
      <div className="h-dvh bg-background" />
    ) : (
      <div className="h-dvh overflow-y-auto">
        <CloudNodeWizard entry={w.entry} initial={w.initial} onClose={() => undefined} />
      </div>
    )
  if (name in SETTINGS) {
    const open = SETTINGS[name]
    return (
      <div className="h-dvh overflow-y-auto bg-background text-foreground">
        <div className="mx-auto flex max-w-[768px] flex-col px-6 py-6">
          <h2 className="mb-4 text-lg font-semibold">Settings › Machines</h2>
          <CloudNodesSettings initial={open ?? undefined} />
          {name === 'desktop-view' && <NodeDesktopPanel name="sky-cloud" access={null} mobile={false} onClose={() => undefined} placeholder={<FakeScreen />} />}
        </div>
      </div>
    )
  }
  return <p className="p-6 text-sm">Unknown state {name}</p>
}

/** The desktop wizard is a dialog: it mounts beside the page, not inside it. */
function DesktopDialog({ name }: { name: string }) {
  const w = WIZARD[name]
  return w?.entry === 'desktop-dialog' ? <CloudNodeWizard entry={w.entry} initial={w.initial} onClose={() => undefined} /> : null
}

const STATES: { name: string; label: string; phone: boolean }[] = [
  ['phone-choose', '① Choose (first run)'],
  ['phone-provider', '② Pick a provider'],
  ['phone-configure', '③ Your node (desktop on: 4 GB recommended)'],
  ['phone-configure-desktop-off', '③ Desktop off: 2 GB'],
  ['phone-configure-2gb-desktop', '③ 2 GB with the desktop: the note'],
  ['phone-configure-price-unavailable', '③ Price unavailable'],
  ['phone-configure-loading', '③ Sizes loading'],
  ['phone-configure-vault', '③ Vault protection (sealed vault on)'],
  ['phone-progress', '④ Starting'],
  ['phone-done', '⑤ Ready'],
  ['phone-engines-entry', 'Engine switcher entry'],
  ['desktop-provider', 'Desktop · provider'],
  ['desktop-configure', 'Desktop · configure'],
  ['desktop-configure-vault', 'Desktop · configure, vault protection'],
  ['desktop-k8s', 'Desktop · Kubernetes'],
  ['desktop-progress', 'Desktop · creating'],
  ['settings-machines', 'Settings › Machines'],
  ['destroy-confirm', 'Destroy confirmation'],
  ['resize', 'Resize'],
  ['logs', 'View logs'],
  ['desktop-view', 'View desktop'],
  ['vault-password', 'Set a node’s master password'],
].map(([name, label]) => ({ name: name as string, label: label as string, phone: (name as string).startsWith('phone-') }))

function Gallery() {
  const src = (name: string) => `${location.pathname}?state=${name}${dark ? '&theme=dark' : ''}`
  return (
    <main className="min-h-dvh overflow-auto bg-background p-6 text-foreground">
      <header className="mb-6 flex items-baseline gap-4">
        <h1 className="text-lg font-medium">224 · cloud nodes</h1>
        <a className="text-sm text-fg-tertiary underline" href={dark ? location.pathname : `${location.pathname}?theme=dark`}>
          {dark ? 'Light' : 'Dark'}
        </a>
      </header>
      <section className="mb-10 flex flex-wrap gap-6">
        {STATES.filter((s) => s.phone).map((s) => (
          <figure key={s.name} className="flex flex-col gap-1.5">
            <figcaption className="text-xs text-fg-tertiary">{s.label}</figcaption>
            <iframe title={s.label} src={src(s.name)} width={390} height={844} className="rounded-[36px] border-[6px] border-border-strong" />
          </figure>
        ))}
      </section>
      <section className="flex flex-col gap-6">
        {STATES.filter((s) => !s.phone).map((s) => (
          <figure key={s.name} className="flex flex-col gap-1.5">
            <figcaption className="text-xs text-fg-tertiary">{s.label}</figcaption>
            <iframe title={s.label} src={src(s.name)} width={1100} height={720} className="border border-border-subtle" />
          </figure>
        ))}
      </section>
    </main>
  )
}

const root = createRootRoute({
  component: () => (
    <TooltipProvider>
      {state ? (
        <>
          <Scenario name={state} />
          <DesktopDialog name={state} />
        </>
      ) : (
        <Gallery />
      )}
    </TooltipProvider>
  ),
})
const router = createRouter({ routeTree: root, history: createMemoryHistory({ initialEntries: ['/'] }) })

createRoot(document.getElementById('root') as HTMLElement).render(
  <StrictMode>
    <QueryClientProvider client={qc}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  </StrictMode>,
)
