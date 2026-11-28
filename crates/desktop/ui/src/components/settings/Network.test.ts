import { receiveComponent } from '@/rpc/bindings/receive'
import './test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { Children, type ComponentType, type ReactNode, act, createElement, isValidElement } from 'react'
import { createRoot } from 'react-dom/client'
import { renderToStaticMarkup } from 'react-dom/server'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import { queryKeys, useInvalidateOn } from '@/lib/rpc'
import { dispatch } from '@/store/sessions'
import { useConnection } from '@/store/connection'
import { disconnected } from '@/rpc/discovery'
import type { EngineStats } from "@/rpc/bindings/method.nodes.output"
import type { MeshStatus } from "@/rpc/bindings/method.nodes.output"
import type { NearbyEngine } from "@/rpc/bindings/method.nodes.output"
import type { PairCeremony } from "@/rpc/bindings/method.a2a.output"
import type { RotationState } from "@/rpc/bindings/method.nodes.output"
import type { UnpairPreview } from "@/rpc/bindings/method.link.output"
import type { ConfigFlag } from "@/rpc/bindings/method.rpc_2dcompose.output"
import type { ConfigFlagKey } from '@/rpc/bindings/method.rpc_2dcompose.input'
import type { PeerSync } from '@/rpc/bindings/method.sessionsync.receiver'
import { MachineRow, machineTag, presenceOf } from './MachineRow'
import { MachineTab, MachinesTab, pathChip, relayChip } from './Network'
import { PairBody, type PairView, pairView, usePairFlow } from './PairDialog'
import { RotationProgress, UnpairBody, UnpairDialog, useRotation } from './UnpairDialog'

const state = vi.hoisted(() => ({ on: false }))
const mockNotify = vi.hoisted(() => Object.assign(vi.fn(), { error: vi.fn(), message: vi.fn(), success: vi.fn() }))
const mockNavigate = vi.hoisted(() => vi.fn())
const mockCanPopout = vi.hoisted(() => vi.fn(() => true))
const mockOpenPopout = vi.hoisted(() => vi.fn(async (_t: unknown) => undefined))

vi.mock('@/lib/notify', () => ({ notify: mockNotify }))
vi.mock('@tanstack/react-router', async (original) => ({ ...(await original<object>()), useNavigate: () => mockNavigate }))
vi.mock('@/lib/popout', async (original) => ({
  ...(await original<typeof import('@/lib/popout')>()),
  canPopout: () => mockCanPopout(),
  openPopout: (t: unknown) => mockOpenPopout(t),
}))
vi.mock('@/components/chat/TerminalViewer', () => ({
  Terminal: (p: { source: { shell: string; engine: string }; label: string }) =>
    createElement('div', { 'data-node-terminal': `${p.source.engine}:${p.source.shell}`, 'data-name': p.label }),
}))
// Portals render nothing on the server: the dialogs' content renders in place. `state.on` (the DOM tests) uses the real ones.
vi.mock('@/components/ui/dialog', async (original) => {
  const a = await original<typeof import('@/components/ui/dialog')>()
  const pick = <P extends object>(live: ComponentType<P>, fake: (p: P) => ReactNode) => (p: P) => (state.on ? createElement(live, p) : fake(p))
  return {
    ...a,
    Dialog: pick(a.Dialog, (p: { children?: ReactNode }) => p.children),
    DialogContent: pick(a.DialogContent, (p: { children?: ReactNode }) => createElement('div', null, p.children)),
    DialogTitle: pick(a.DialogTitle, (p: { children?: ReactNode }) => createElement('h2', null, p.children)),
  }
})

// Static markup drops handlers: switches and buttons register theirs by label so a test can "use" them.
const handlers: Record<string, ((v?: never) => void) | undefined> = {}
const text = (n: ReactNode): string =>
  Children.toArray(n)
    .map((c) => (typeof c === 'string' || typeof c === 'number' ? String(c) : isValidElement<{ children?: ReactNode }>(c) ? text(c.props.children) : ''))
    .join('')
vi.mock('@/components/ui/switch', async (original) => {
  const a = await original<typeof import('@/components/ui/switch')>()
  return {
    Switch: (p: React.ComponentProps<typeof a.Switch>) => {
      const label = p['aria-label']
      if (state.on) return createElement(a.Switch, p)
      if (label) handlers[label] = p.onCheckedChange as never
      return createElement('button', { role: 'switch', 'aria-checked': p.checked, 'aria-label': label })
    },
  }
})
vi.mock('@/components/ui/button', async (original) => {
  const a = await original<typeof import('@/components/ui/button')>()
  return {
    ...a,
    Button: (p: React.ComponentProps<typeof a.Button>) => {
      if (state.on) return createElement(a.Button, p)
      handlers[p['aria-label'] ?? text(p.children).trim()] = p.onClick as never
      return createElement('button', { disabled: p.disabled, 'aria-label': p['aria-label'] }, p.children)
    },
  }
})

// The page reads the clock itself: the fixture is relative to it.
const NOW = Date.now()
const H = 3_600_000
const stat = (id: string, os: string, os_name: string, cls: EngineStats['class']): EngineStats =>
  ({ id, uptime_since_ms: 0, class: cls, availability_30d: 1, harnesses: [], repos: [], display: false, gpu: false, os, os_name, arch: '', host: '' })
const MESH: MeshStatus = {
  self_id: 'mac7f3a9c21aaaa',
  self_name: "Sky's MacBook",
  mesh_name: 'home',
  peers: [
    { id: 'xeus', name: 'xeus', addrs: [], reachable: true, last_seen_ms: NOW, hub_pref: 'prefer', path: 'lan', rtt_ms: 2 },
    { id: 'cloud', name: 'cloud-1', addrs: [], reachable: true, last_seen_ms: NOW, hub_pref: 'eligible', path: 'relay', rtt_ms: 41 },
    { id: 'studio', name: 'studio', addrs: [], reachable: false, last_seen_ms: NOW - 3 * H, hub_pref: 'eligible', path: 'offline', rtt_ms: null },
  ],
  lease: { holder: 'xeus', since_ms: NOW - H, reason: 'prefer' },
  traffic: [],
  replica: null,
  stats: [stat('mac7f3a9c21aaaa', 'macos', 'macOS', 'laptop'), stat('xeus', 'linux', 'Arch Linux', 'desktop'), stat('cloud', 'linux', 'Ubuntu', 'server')],
  relay: { url: 'https://iroh.relay.theuth.io', connected: true, rtt_ms: 38, error: null },
  messaging_off: ['cloud'],
  nearby: true,
  quarantined: [],
  docs_homes_on: false,
}
const XEUS2: NearbyEngine = { id: 'xeus2', name: 'xeus-2', os_name: 'Arch Linux', addr: '192.168.8.140', seen_ms: NOW }
const PREVIEW: UnpairPreview = {
  id: 'xeus',
  name: 'xeus',
  connected: true,
  last_seen_ms: NOW,
  secrets: ['ANTHROPIC_API_KEY', 'GRAFANA_TOKEN'],
  sign_ins: ['oauth/github', 'oauth/google'],
  offline: ['studio'],
  old_build: ['cloud'],
  devices: 2,
}
/** config.yaml's `allow_private_network` keys; absent = on. */
const LAN: Partial<Record<ConfigFlagKey, boolean>> = {}
const api = {
  config_flag_get: vi.fn(async (key: ConfigFlagKey): Promise<ConfigFlag> => ({ key, enabled: LAN[key] ?? true, running: true, restart_required: !(LAN[key] ?? true) })),
  config_flag_set: vi.fn(async (key: ConfigFlagKey, on: boolean): Promise<ConfigFlag> => ({ key, enabled: on, running: true, restart_required: !on })),
  mesh_status: vi.fn(async () => MESH),
  mesh_nearby: vi.fn(async (): Promise<NearbyEngine[]> => []),
  mesh_nearby_watch: vi.fn(async () => null),
  mesh_set_messaging: vi.fn(async () => null),
  mesh_confirm_peer: vi.fn(async (_id: string, _keep: boolean) => null),
  mesh_set_nearby: vi.fn(async () => null),
  mesh_rename: vi.fn(async () => null),
  mesh_remove: vi.fn(async () => null),
  mesh_unpair_preview: vi.fn(async () => PREVIEW),
  vault_rotation: vi.fn(async (): Promise<RotationState | null> => null),
  vault_rotate: vi.fn(async () => {
    throw new Error('disk full')
  }),
  mesh_set_hub: vi.fn(async () => null),
  sessions_status: vi.fn(async () => ({ here: 3, elsewhere: 5, log_seq: 9, last_snapshot: null })),
  sessions_sync: vi.fn(async (): Promise<PeerSync[]> => []),
  list_nodes: vi.fn(async () => []),
  mesh_pair_nearby: vi.fn(async (_id: string) => CEREMONY),
  mesh_pair_answer: vi.fn(async (_id: string, _accept: boolean) => null),
  engine_shell: vi.fn(async (engine?: string | null, shell?: string | null) => ({ engine: engine ?? MESH.self_id, shell: shell ?? 'main' })),
  engine_terminal: vi.fn(async () => {
    throw new Error('engine_terminal is deprecated')
  }),
  shell_close: vi.fn(async () => null),
}
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => api }))

beforeEach(() => {
  useConnection.getState().setDiscovery({ ...disconnected(), phase: 'legacy' })
  vi.clearAllMocks()
  mockCanPopout.mockReturnValue(true)
  api.mesh_pair_nearby.mockImplementation(async () => CEREMONY)
  api.engine_shell.mockImplementation(async (engine?: string | null, shell?: string | null) => ({ engine: engine ?? MESH.self_id, shell: shell ?? 'main' }))
  for (const k of Object.keys(handlers)) delete handlers[k]
})

async function render(el: ReactNode, qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })) {
  const paint = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: el }))
  for (let pass = 0; pass < 2; pass++) {
    paint()
    for (const q of qc.getQueryCache().findAll()) if (q.options.queryFn) await qc.fetchQuery({ ...q.options, queryKey: q.queryKey }).catch(() => undefined)
  }
  return { html: paint(), qc, paint }
}

test('188 This machine: the name, short id, LAN/mDNS and relay chips and the Nearby switch, then the mesh rows from Advanced, shown flat', async () => {
  const { html } = await render(createElement(MachineTab))
  const at = (s: string) => html.indexOf(s)
  const order = ['>This machine<', 'Let nearby machines find this one while pairing', '>Mesh<', 'Prefer this node as hub', '>Sessions<', 'Replication log', '>Rotate vault key<']
  expect(order.map(at).every((i, n, a) => i >= 0 && (n === 0 || i > a[n - 1]!))).toBe(true)
  expect(html).toContain('mac7f3a9')
  expect(html).toContain('LAN · mDNS ✓')
  expect(html).toContain('Relay · iroh.relay.theuth.io · 38 ms ✓')
  expect(html).toContain('>home<')
  expect(html).toContain('3 here · 5 elsewhere')
  expect(html).toContain('seq 9')
  expect(html).toContain('hub: xeus · prefer')
  // Nothing collapsed, and the machines list lives on its own tab.
  expect(html).not.toContain('Advanced')
  expect(html).not.toContain('Agents can message')
})

test('188 Machines: the machines in this mesh, Nearby, then pairing by string; the Nearby switch stays on This machine', async () => {
  const { html } = await render(createElement(MachinesTab))
  const at = (s: string) => html.indexOf(s)
  const order = ['>Machines in this mesh<', '>Nearby<', '>Pair with a string<', 'Paste another node&#x27;s pairing string']
  expect(order.map(at).every((i, n, a) => i >= 0 && (n === 0 || i > a[n - 1]!))).toBe(true)
  expect(html).toContain('Only shown while this page is open.')
  expect(html).not.toContain('aria-label="Let nearby machines find this one while pairing"')
  // Machines: class line, path chips, hub badge; this machine has no switch, each peer has one.
  expect(html).toContain('Arch Linux · desktop')
  expect(html).toContain('LAN 2 ms')
  expect(html).toContain('Relay 41 ms')
  expect(html).toContain('offline 3h')
  expect(html).toMatch(/xeus<span[^>]*>hub<\/span>/)
  expect(html).not.toContain("Agents can message Sky&#x27;s MacBook")
  for (const p of ['xeus', 'cloud-1', 'studio']) expect(html).toContain(`aria-label="Agents can message ${p}"`)
  // cloud is on this machine's deny list: switch off, the refusal names both machines.
  expect(html).toMatch(/aria-checked="false" aria-label="Agents can message cloud-1"/)
  expect(html).toMatch(/aria-checked="true" aria-label="Agents can message xeus"/)
  expect(html).toContain('agent messaging between Sky&#x27;s MacBook and cloud-1 is off on Sky&#x27;s MacBook')
})

test('switches call their RPCs: Agents can message → mesh_set_messaging(id, on); the Nearby switch → mesh_set_nearby', async () => {
  await render(createElement(MachinesTab))
  await render(createElement(MachineTab))
  handlers['Agents can message xeus']!(false as never)
  handlers['Agents can message cloud-1']!(true as never)
  handlers['Let nearby machines find this one while pairing']!(false as never)
  await vi.waitFor(() => expect(api.mesh_set_nearby).toHaveBeenCalledWith(false))
  expect(api.mesh_set_messaging).toHaveBeenCalledWith('xeus', false)
  expect(api.mesh_set_messaging).toHaveBeenCalledWith('cloud', true)
})

test('16 §5 Local network access: on only when both allow_private_network keys are; a change writes both', async () => {
  const lan = /aria-checked="(true|false)" aria-label="Local network access"/
  LAN['browser.allow_private_network'] = false
  const off = await render(createElement(MachineTab))
  expect(off.html.match(lan)?.[1]).toBe('false')
  expect(off.html).toContain('Restricted contacts never can.')
  expect(off.html).toContain('Applies after theuth restarts.')
  delete LAN['browser.allow_private_network']
  const on = await render(createElement(MachineTab))
  expect(on.html.match(lan)?.[1]).toBe('true')
  expect(on.html).not.toContain('Applies after theuth restarts.')
  handlers['Local network access']!(false as never)
  await vi.waitFor(() => expect(api.config_flag_set).toHaveBeenCalledTimes(2))
  expect(api.config_flag_set).toHaveBeenCalledWith('web.allow_private_network', false)
  expect(api.config_flag_set).toHaveBeenCalledWith('browser.allow_private_network', false)
  expect(on.paint().match(lan)?.[1]).toBe('false')
})

test('16 §5 Local network access: a write that fails on the second key puts the first back', async () => {
  api.config_flag_set.mockImplementationOnce(async (key, on) => ({ key, enabled: on, running: true, restart_required: !on }))
  api.config_flag_set.mockImplementationOnce(async () => {
    throw new Error('config.yaml: read-only')
  })
  await render(createElement(MachineTab))
  handlers['Local network access']!(false as never)
  await vi.waitFor(() => expect(api.config_flag_set).toHaveBeenCalledTimes(3))
  expect(api.config_flag_set.mock.calls).toEqual([
    ['web.allow_private_network', false],
    ['browser.allow_private_network', false],
    ['web.allow_private_network', true],
  ])
})

test('Nearby: empty, then the rows a MeshEvent.nearby carries, each with Pair…', async () => {
  const { html, qc, paint } = await render(createElement(MachinesTab))
  expect(html).toContain('No unpaired machine found on this network.')
  qc.setQueryData(queryKeys.meshNearby, [XEUS2])
  const after = paint()
  expect(after).not.toContain('No unpaired machine found')
  expect(after).toContain('xeus-2')
  expect(after).toContain('192.168.8.140')
  expect(after).toContain('aria-label="Pair with xeus-2"')
})

test('Nearby off: the section points at the switch on This machine instead of "No unpaired machine found"', async () => {
  api.mesh_status.mockResolvedValueOnce({ ...MESH, nearby: false })
  const { html } = await render(createElement(MachinesTab))
  expect(html).toContain('Nearby is off')
  expect(html).toContain('” on This machine to see machines here.')
  expect(html).not.toContain('No unpaired machine found')
})

test('183 R1/R5 quarantine: a row with the warning; Remove is the primary → mesh_confirm_peer(id, false), Keep paired… only opens a confirm; an empty list adds nothing', async () => {
  const none = await render(createElement(MachinesTab))
  expect(none.html).not.toContain('not paired with this machine')
  api.mesh_status.mockResolvedValueOnce({ ...MESH, quarantined: [{ id: 'ghost', name: 'ghost-pc', added_by: 'old', added_by_name: 'old-laptop' }] })
  const { html } = await render(createElement(MachinesTab))
  expect(html).toContain('Added by old-laptop after it was unpaired — not paired with this machine until you remove or keep it')
  expect(html).toMatch(/<button aria-label="Keep ghost-pc paired">Keep paired…<\/button><button aria-label="Remove ghost-pc">Remove<\/button>/)
  handlers['Keep ghost-pc paired']!({} as never)
  handlers['Remove ghost-pc']!({} as never)
  await vi.waitFor(() => expect(api.mesh_confirm_peer).toHaveBeenCalledWith('ghost', false))
  expect(api.mesh_confirm_peer).not.toHaveBeenCalledWith('ghost', true)
})

test('path and relay words', () => {
  expect(pathChip({ reachable: true, path: 'lan', rtt_ms: null, last_seen_ms: NOW }, NOW).text).toBe('LAN')
  expect(pathChip({ reachable: true, path: 'offline', rtt_ms: null, last_seen_ms: NOW }, NOW).text).toBe('online')
  expect(pathChip({ reachable: false, path: 'lan', rtt_ms: 3, last_seen_ms: 0 }, NOW)).toEqual({ text: 'offline', dot: 'off' })
  expect(relayChip({ url: 'https://r.example', connected: false, rtt_ms: null, error: 'dns failed' })).toEqual({ text: 'Relay · r.example · dns failed', ok: false })
  expect(relayChip(undefined).ok).toBe(false)
})

test('bootstrapping peer shows Syncing mesh history in pathChip, MachinesTab, and MachineRow until acked > 0', async () => {
  const peer = { reachable: true, path: 'relay' as const, rtt_ms: 41, last_seen_ms: NOW }
  const syncing: PeerSync = { peer: 'cloud', up: true, head: 1000, acked: 0, behind_out: 1000, held: false, held_by_peer: false, last_error: null }
  expect(pathChip(peer, NOW, syncing)).toEqual({ text: 'Syncing mesh history…', dot: 'sync' })
  expect(pathChip({ ...peer, reachable: false }, NOW, syncing).dot).toBe('off')
  expect(pathChip(peer, NOW, { ...syncing, held: true })).toEqual({ text: 'Relay 41 ms', dot: 'relay' })
  expect(pathChip(peer, NOW, { ...syncing, acked: 1000, behind_out: 0 })).toEqual({ text: 'Relay 41 ms', dot: 'relay' })

  // MachinesTab reflects bootstrapping state from sessions_sync and flips back once acked > 0.
  api.sessions_sync.mockResolvedValueOnce([syncing])
  const syncingView = await render(createElement(MachinesTab))
  expect(syncingView.html).toContain('Syncing mesh history…')
  expect(syncingView.html).not.toContain('Relay 41 ms')
  expect(syncingView.html).toContain('LAN 2 ms')

  api.sessions_sync.mockResolvedValueOnce([{ ...syncing, acked: 1000, behind_out: 0 }])
  const readyView = await render(createElement(MachinesTab))
  expect(readyView.html).not.toContain('Syncing mesh history…')
  expect(readyView.html).toContain('Relay 41 ms')

  // MachineRow supports optional syncing presence without breaking online/offline/sealed defaults.
  const machine = { id: 'cloud', name: 'cloud-1', kind: 'cloud' as const, client: 'local', forwarded: true, online: true, sealed: false, hub: false, connected: false, path: 'relay' as const }
  expect(presenceOf(machine, true)).toBe('syncing')
  expect(machineTag(machine, true)).toBe('Syncing mesh history…')
  expect(presenceOf({ ...machine, online: false }, true)).toBe('offline')
  expect(presenceOf({ ...machine, sealed: true }, true)).toBe('sealed')
  const rowHtml = renderToStaticMarkup(createElement(MachineRow, { machine, syncing: true }))
  expect(rowHtml).toContain('data-presence="syncing"')
  expect(rowHtml).toContain('Syncing mesh history…')
})

const CEREMONY: PairCeremony = { id: 'c1', peer: 'xeus2', name: 'xeus-2', os_name: 'Arch Linux', code: '482913', state: 'waiting', expires_ms: NOW + 120_000, addr: '192.168.8.140:4433' }
const body = (view: PairView, error: string | null = null) =>
  renderToStaticMarkup(createElement(PairBody, { name: 'xeus-2', detail: 'Arch Linux · 192.168.8.140', ceremony: CEREMONY, view, error, onPair: vi.fn(), onCancel: vi.fn() }))

test('pair dialog: the code, the check line, the warning, Pair and Cancel; each end state', () => {
  expect(pairView(undefined, null, false)).toBe('starting')
  expect(pairView(CEREMONY, null, false)).toBe('code')
  expect(pairView({ ...CEREMONY, state: 'accepted' }, null, true)).toBe('waiting')
  expect(pairView({ ...CEREMONY, state: 'accepted' }, null, false)).toBe('code')
  for (const s of ['paired', 'rejected', 'expired'] as const) expect(pairView({ ...CEREMONY, state: s }, null, true)).toBe(s)
  expect(pairView(CEREMONY, 'Nearby pairing is not built yet (183 phase 2)', false)).toBe('failed')

  const code = body('code')
  expect(code).toContain('482 913')
  expect(code).toContain('Check that xeus-2 shows the same code')
  expect(code).toContain('Pairing shares your secrets vault, sessions and tasks — only pair your own machines.')
  expect(code).not.toContain('message each other')
  expect(code).toMatch(/<button>Pair<\/button><button>Cancel<\/button>/)
  expect(body('waiting')).toMatch(/Waiting for xeus-2 to press Pair….*<button disabled="">Pair<\/button>/)
  expect(body('starting')).toContain('Connecting to xeus-2…')
  expect(body('paired')).toContain('Paired with xeus-2.')
  expect(body('rejected')).toContain('Not paired: xeus-2 pressed Cancel, or the codes did not match.')
  expect(body('expired')).toContain('The pairing window closed')
  expect(body('failed', 'Nearby pairing is not built yet (183 phase 2)')).toContain('Nearby pairing is not built yet')
})

test('pair dialog buttons answer: Pair and Cancel', () => {
  const onPair = vi.fn()
  const onCancel = vi.fn()
  renderToStaticMarkup(createElement(PairBody, { name: 'xeus-2', detail: '', ceremony: CEREMONY, view: 'code', error: null, onPair, onCancel }))
  handlers.Pair!()
  handlers.Cancel!()
  expect(onPair).toHaveBeenCalledOnce()
  expect(onCancel).toHaveBeenCalledOnce()
})

const unpair = (over: Partial<Parameters<typeof UnpairBody>[0]> = {}) =>
  renderToStaticMarkup(
    createElement(UnpairBody, {
      name: 'xeus',
      preview: PREVIEW,
      previewError: null,
      onRetryPreview: vi.fn(),
      previewRetrying: false,
      previewFailedTwice: false,
      nameOf: (id: string) => ({ cloud: 'cloud-1' })[id] ?? id,
      reset: false,
      onReset: vi.fn(),
      error: null,
      busy: false,
      onCancel: vi.fn(),
      onConfirm: vi.fn(),
      ...over,
    }),
  )

test('unpair dialog: the secret names, sign in again, the lines that apply, the checkbox off by default', () => {
  const html = unpair()
  expect(html).toContain('Unpair xeus?')
  expect(html).toContain('Replace these at their providers if xeus is lost')
  expect(html).toMatch(/font-mono">ANTHROPIC_API_KEY<.*font-mono">GRAFANA_TOKEN</)
  expect(html).toContain('Sign in again: github, google')
  expect(html).toContain('studio is offline: it gets the new key when it reconnects')
  expect(html).toContain('cloud-1 runs an older theuth: it cannot read secrets after this until it is upgraded')
  expect(html).toContain('Phones paired to this mesh trust a key xeus knows')
  expect(html).toMatch(/<input type="checkbox"[^>]*\/>.*Also reset phone and iPad pairing/)
  expect(html).not.toMatch(/type="checkbox"[^>]*checked/)
  expect(html).not.toContain('must scan the pairing QR again')
  expect(html).toContain('Unpair and rotate key')
  // Ticked: the consequence is spelled out.
  const ticked = unpair({ reset: true })
  expect(ticked).toMatch(/type="checkbox"[^>]*checked/)
  expect(ticked).toContain('Every phone and iPad paired to this mesh must scan the pairing QR again')
  // Nothing to replace, no lines; a refusal shows on the confirm step.
  const none = unpair({ preview: { ...PREVIEW, secrets: [], sign_ins: [], offline: [], old_build: [], devices: 0 }, error: 'mesh_remove: resetting phone and iPad pairing is not built yet (183 phase 2)' })
  expect(none).toContain('xeus had no secrets.')
  expect(none).not.toContain('is offline')
  expect(none).toContain('resetting phone and iPad pairing is not built yet')
})

test('191 D8 unpair dialog: with contacts, they are told this machine left, and a lost or stolen one needs them to remove you too; none, no lines', () => {
  const html = unpair({ contacts: [{ name: 'bob' }, { name: 'alice' }] })
  expect(html).toContain('Your contacts are told this machine left. A contact that is offline learns it when it comes back.')
  expect(html).toContain('If xeus was lost or stolen: Ask bob and alice to remove you too, then pair again. Until they do, the removed machine may still reach them.')
  for (const none of [unpair(), unpair({ contacts: [] })]) {
    expect(none).not.toContain('Your contacts are told')
    expect(none).not.toContain('lost or stolen')
  }
})

test('unpair confirm is disabled until the preview resolved; the device-reset box is always there', () => {
  const confirm = /<button disabled="">Unpair and rotate key/
  expect(unpair({ preview: undefined })).toMatch(confirm)
  expect(unpair({ preview: undefined, previewError: 'offline' })).toMatch(confirm)
  expect(unpair()).not.toMatch(confirm)
  const none = unpair({ preview: { ...PREVIEW, devices: 0 } })
  expect(none).not.toContain('Phones paired to this mesh')
  expect(none).toContain('Also reset phone and iPad pairing')
})

test('183 R5 unpair preview error: Retry; after a failed retry "Unpair without the list…", never the main confirm', () => {
  const onRetryPreview = vi.fn()
  const once = unpair({ preview: undefined, previewError: 'unknown method', onRetryPreview })
  expect(once).toContain('Could not read what xeus had: unknown method')
  expect(once).toContain('>Retry</button>')
  expect(once).not.toContain('Unpair without the list')
  handlers.Retry!()
  expect(onRetryPreview).toHaveBeenCalledOnce()
  expect(unpair({ preview: undefined, previewError: 'x', previewRetrying: true })).toMatch(/<button disabled="">Retrying…/)
  const twice = unpair({ preview: undefined, previewError: 'unknown method', previewFailedTwice: true })
  expect(twice).toContain('Unpair without the list…')
  expect(twice).toMatch(/<button disabled="">Unpair and rotate key/)
})

test('unpair confirm sends mesh_remove(id, reset_devices) with the box off by default', async () => {
  const { html } = await render(createElement(UnpairDialog, { id: 'xeus', name: 'xeus', nameOf: (id: string) => id, onClose: vi.fn() }))
  expect(api.mesh_unpair_preview).toHaveBeenCalledWith('xeus')
  expect(html).toContain('ANTHROPIC_API_KEY')
  handlers['Unpair and rotate key']!()
  await vi.waitFor(() => expect(api.mesh_remove).toHaveBeenCalledWith('xeus', false))
})

test('183 R5 unpair without the list: it asks first, says the list could not be read, then sends mesh_remove', async () => {
  state.on = true
  api.mesh_unpair_preview.mockRejectedValue(new Error('unknown method mesh_unpair_preview'))
  await mount(createElement(UnpairDialog, { id: 'xeus', name: 'xeus', nameOf: (id: string) => id, onClose: vi.fn() }))
  await settle()
  expect(document.body.textContent).toContain('Could not read what xeus had')
  expect(button('Unpair without the list…')).toBeUndefined()
  await run(async () => button('Retry')!.click())
  await settle()
  await run(async () => button('Unpair without the list…')!.click())
  expect(document.body.textContent).toContain('The list of what xeus had could not be read')
  expect(api.mesh_remove).not.toHaveBeenCalled()
  await run(async () => button('Unpair without the list')!.click())
  await vi.waitFor(() => expect(api.mesh_remove).toHaveBeenCalledWith('xeus', false))
  api.mesh_unpair_preview.mockImplementation(async () => PREVIEW)
})

const ROT: RotationState = receiveComponent("method.nodes.output.RotationState", {
  key: { id: 'k2', epoch: 1, initiator: 'mac', members: [], minted_ms: NOW },
  removed: ['xeus'],
  phase: 'sweeping',
  started_ms: NOW,
  rows_total: 40,
  rows_done: 34,
  rows_stuck: [],
  peers: [
    { id: 'cloud', name: 'cloud-1', online: true, has_key: true, old_build: false },
    { id: 'studio', name: 'studio', online: false, has_key: false, old_build: false },
    { id: 'old', name: 'pi', online: true, has_key: false, old_build: true },
  ],
  old_destroyed: false,
  error: null,
  device_reset_pending: false,
  removed_names: [],
})
const progress = (state: RotationState | null, error: string | null = null, onRetry = vi.fn()) =>
  renderToStaticMarkup(createElement(RotationProgress, { state, nameOf: (id: string) => ({ cloud: 'cloud-1', studio: 'studio', old: 'pi' })[id] ?? id.slice(0, 8), error, retrying: false, onRetry, now: NOW }))

test('rotation progress: phases, machines done and pending, stuck rows, error and Retry', () => {
  const sweeping = progress(ROT)
  expect(sweeping).toContain('Rotating vault key · 34 of 40 secrets re-encrypted · 1 of 3 machines has the new key')
  expect(sweeping).toContain('aria-valuenow="85"')
  expect(sweeping).toContain('After unpairing xeus')
  expect(progress({ ...ROT, removed: ['cloud'] })).toContain('After unpairing cloud-1')
  // 183 R2: the record's name, else "a removed machine" — never the engine id.
  const hex = 'd080cd6b79c8ac5b20430736dacc0ff73882371914770ba56596a90463d0e246'
  const named = renderToStaticMarkup(createElement(RotationProgress, { state: { ...ROT, removed: [hex], removed_names: ['c'] }, nameOf: () => 'another machine', error: null, retrying: false, onRetry: vi.fn(), now: NOW }))
  expect(named).toContain('After unpairing c<')
  const unnamed = renderToStaticMarkup(createElement(RotationProgress, { state: { ...ROT, removed: [hex], peers: [{ ...ROT.peers[0]!, id: 'zz', name: '' }] }, nameOf: () => 'another machine', error: null, retrying: false, onRetry: vi.fn(), now: NOW }))
  expect(unnamed).toContain('After unpairing a removed machine')
  for (const html of [named, unnamed]) expect(html).not.toMatch(/[0-9a-f]{8}/)
  expect(sweeping).toMatch(/cloud-1<\/span><span[^>]*>has the new key/)
  expect(sweeping).toMatch(/studio<\/span><span[^>]*>offline: gets the key when it reconnects/)
  expect(sweeping).toMatch(/pi<\/span><span[^>]*>runs an older theuth/)

  const waiting = progress({ ...ROT, phase: 'waiting', peers: [ROT.peers[1]!] })
  expect(waiting).toContain('New vault key in use · studio gets it when it reconnects · old key destroyed after the last machine re-encrypts')
  expect(waiting).not.toContain('progressbar')

  expect(progress({ ...ROT, phase: 'done', old_destroyed: true })).toMatch(/Vault key rotated \(started \d\d:\d\d\) · old key destroyed/)
  expect(progress({ ...ROT, rows_stuck: ['A', 'B'] })).toContain('2 secrets could not be re-encrypted: A, B · set them again')

  const onRetry = vi.fn()
  const failed = progress({ ...ROT, phase: 'failed', error: 'disk full' }, null, onRetry)
  expect(failed).toContain('Vault key rotation stopped: disk full')
  expect(failed).toContain('The old key stays in use and nothing is lost.')
  handlers.Retry!()
  expect(onRetry).toHaveBeenCalledOnce()

  // 183 R2: a peer reads by the name the engine recorded; an id never shows.
  const unknown = progress({ ...ROT, peers: [{ id: 'abcdef0123456789', name: 'lab', online: true, has_key: false, old_build: false }] })
  expect(unknown).toContain('lab')
  expect(unknown).not.toContain('abcdef01')
  expect(sweeping).toContain('aria-label="Re-encrypting secrets"')
  expect(sweeping).not.toContain('role="status" class="flex flex-col')

  expect(progress(null)).toContain('No vault-key rotation reported yet.')
  expect(progress(null)).toContain('Rotate now')
  const refused = progress(null, 'disk full')
  expect(refused).toContain('Vault key rotation stopped: disk full')
  expect(refused).toContain('Retry')
})

test('the old Settings › Mesh module is gone and nothing imports it', () => {
  expect(Object.keys(import.meta.glob('./Mesh.tsx'))).toEqual([])
  const sources = import.meta.glob<string>('/src/**/*.{ts,tsx}', { query: '?raw', import: 'default', eager: true })
  expect(Object.entries(sources).filter(([, src]) => /settings\/Mesh['"]/.test(src)).map(([f]) => f)).toEqual([])
})

// --- DOM tests: real Dialog, real effects ---------------------------------------------------------------------

const qcNew = () => new QueryClient({ defaultOptions: { queries: { retry: false } } })
const roots: (() => Promise<void>)[] = []
async function mount(el: ReactNode, qc = qcNew()) {
  state.on = true
  const host = document.createElement('div')
  document.body.append(host)
  const root = createRoot(host)
  await run(async () => root.render(createElement(QueryClientProvider, { client: qc, children: el })))
  roots.push(() => run(() => root.unmount()))
  return { qc, unmount: roots[roots.length - 1]! }
}
// React Query notifies observers on a macrotask: every step waits one out (the real clock, fake timers or not).
const realTimeout = setTimeout
const tick = () => new Promise<void>((r) => realTimeout(r, 0))
const run = (fn: () => unknown) =>
  act(async () => {
    await fn()
    await tick()
  })
const settle = () => run(() => undefined)
const button = (label: string) => [...document.body.querySelectorAll('button')].find((b) => b.textContent?.trim() === label)

afterEach(async () => {
  for (const u of roots.splice(0)) await u()
  document.body.innerHTML = ''
  state.on = false
  vi.useRealTimers()
})

function Flow() {
  const f = usePairFlow()
  return createElement('div', null, createElement('button', { id: 'dial', onClick: () => f.start(XEUS2) }, 'dial'), f.dialog)
}
const incoming = (qc: QueryClient, c: PairCeremony = CEREMONY) => run(async () => void qc.setQueryData(queryKeys.meshPair, { [c.id]: c }))

test('pair dialog focus: Cancel has the initial focus, never Pair', async () => {
  const { qc } = await mount(createElement(Flow))
  await incoming(qc)
  expect(button('Pair')).toBeDefined()
  expect(document.activeElement).toBe(button('Cancel'))
  expect(document.activeElement).not.toBe(button('Pair'))
})

test('pair dialog focus: the dialer\'s Cancel keeps the focus when Connecting… turns into the code', async () => {
  let resolve!: (c: PairCeremony) => void
  api.mesh_pair_nearby.mockImplementationOnce(() => new Promise<PairCeremony>((r) => (resolve = r)))
  await mount(createElement(Flow))
  await run(async () => document.getElementById('dial')!.click())
  expect(document.activeElement).toBe(button('Cancel'))
  await run(async () => resolve(CEREMONY))
  expect(document.body.textContent).toContain('482 913')
  expect(document.activeElement).toBe(button('Cancel'))
})

test('usePairFlow: an incoming waiting ceremony opens with name, os, address and short id; Cancel refuses it and a seen one never reopens', async () => {
  const { qc } = await mount(createElement(Flow))
  expect(document.body.textContent).not.toContain('Pair with')
  await incoming(qc, { ...CEREMONY, peer: 'abcdef0123456789' })
  expect(document.body.textContent).toContain('Pair with xeus-2')
  expect(document.body.textContent).toContain('Arch Linux · 192.168.8.140:4433 · abcdef01')
  expect(document.body.textContent).toContain('482 913')
  await run(async () => button('Cancel')!.click())
  expect(api.mesh_pair_answer).toHaveBeenCalledWith('c1', false)
  expect(document.body.textContent).not.toContain('Pair with')
  await incoming(qc, { ...CEREMONY, peer: 'abcdef0123456789' })
  expect(document.body.textContent).not.toContain('Pair with')
})

test('usePairFlow: a second ceremony does not blank an open dialog\'s code', async () => {
  const { qc } = await mount(createElement(Flow))
  await incoming(qc)
  await run(async () => void qc.setQueryData(queryKeys.meshPair, { c1: CEREMONY, c2: { ...CEREMONY, id: 'c2', code: '111222' } }))
  expect(document.body.textContent).toContain('482 913')
  expect(document.body.textContent).not.toContain('111 222')
})

test('usePairFlow: Cancel while Connecting answers false when the id arrives and does not reopen', async () => {
  let resolve!: (c: PairCeremony) => void
  api.mesh_pair_nearby.mockImplementationOnce(() => new Promise<PairCeremony>((r) => (resolve = r)))
  const { qc } = await mount(createElement(Flow))
  await run(async () => document.getElementById('dial')!.click())
  expect(document.body.textContent).toContain('Connecting to xeus-2')
  await run(async () => button('Cancel')!.click())
  expect(api.mesh_pair_answer).not.toHaveBeenCalled()
  // The engine's event can beat the reply: the cache already holds the ceremony.
  await incoming(qc)
  expect(document.body.textContent).not.toContain('Pair with')
  await run(async () => resolve(CEREMONY))
  await settle()
  expect(api.mesh_pair_answer).toHaveBeenCalledWith('c1', false)
  expect(document.body.textContent).not.toContain('Pair with')
})

test('183 R5 usePairFlow: a cancelled dial\'s late reply never lands on the next dialog', async () => {
  const resolves: ((c: PairCeremony) => void)[] = []
  api.mesh_pair_nearby.mockImplementation(() => new Promise<PairCeremony>((r) => resolves.push(r)))
  await mount(createElement(Flow))
  await run(async () => document.getElementById('dial')!.click())
  await run(async () => button('Cancel')!.click())
  await run(async () => document.getElementById('dial')!.click())
  expect(document.body.textContent).toContain('Connecting to xeus-2')
  await run(async () => resolves[0]!(CEREMONY))
  await settle()
  expect(document.body.textContent).toContain('Connecting to xeus-2')
  expect(document.body.textContent).not.toContain('482 913')
  expect(api.mesh_pair_answer).toHaveBeenCalledWith('c1', false)
  await run(async () => resolves[1]!({ ...CEREMONY, id: 'c2', code: '111222' }))
  expect(document.body.textContent).toContain('111 222')
  expect(api.mesh_pair_answer).not.toHaveBeenCalledWith('c2', false)
})

test('183 R5 a11y: the pair dialog is described by the vault warning; the code is a labelled group', async () => {
  const { qc } = await mount(createElement(Flow))
  await incoming(qc)
  const dialog = document.querySelector('[role=dialog]')!
  const warning = document.getElementById(dialog.getAttribute('aria-describedby')!)!
  expect(warning.textContent).toBe('Pairing shares your secrets vault, sessions and tasks — only pair your own machines.')
  const code = document.querySelector('[role=group]')!
  expect(code.getAttribute('aria-label')).toBe('Pairing code')
  expect(code.textContent).toBe('482 913')
})

test('183 R5 Keep paired…: a confirm with the name, short id, who added it and the vault warning; Cancel keeps nothing', async () => {
  useConnection.setState({ state: 'open' })
  api.mesh_status.mockResolvedValue({ ...MESH, quarantined: [{ id: 'ghost0123456789', name: 'ghost-pc', added_by: 'old', added_by_name: 'old-laptop' }] })
  await mount(createElement(MachinesTab))
  await settle()
  await run(async () => document.querySelector<HTMLButtonElement>('[aria-label="Keep ghost-pc paired"]')!.click())
  const dialog = document.querySelector('[role=alertdialog]')!
  expect(dialog.textContent).toContain('Keep ghost-pc paired?')
  expect(dialog.textContent).toContain('ghost-pc · ghost012 was added by old-laptop after old-laptop was unpaired')
  // P2-F1: described by both the "added by" line and the vault warning.
  const described = dialog.getAttribute('aria-describedby')!.split(' ').map((id) => document.getElementById(id)!.textContent)
  expect(described).toEqual(['ghost-pc · ghost012 was added by old-laptop after old-laptop was unpaired. Keep it only if it is your own machine.', 'Pairing shares your secrets vault, sessions and tasks — only pair your own machines.'])
  expect(document.activeElement).toBe(button('Cancel'))
  await run(async () => button('Cancel')!.click())
  expect(api.mesh_confirm_peer).not.toHaveBeenCalled()
  await run(async () => document.querySelector<HTMLButtonElement>('[aria-label="Keep ghost-pc paired"]')!.click())
  await run(async () => button('Keep paired')!.click())
  await vi.waitFor(() => expect(api.mesh_confirm_peer).toHaveBeenCalledWith('ghost0123456789', true))
  api.mesh_status.mockImplementation(async () => MESH)
})

test('Nearby lifetime: mount sends watch(true), a heartbeat re-sends it every 10 s, unmount sends false', async () => {
  vi.useFakeTimers()
  useConnection.setState({ state: 'open' })
  const { unmount } = await mount(createElement(MachinesTab))
  expect(api.mesh_nearby_watch.mock.calls).toEqual([[true]])
  await run(async () => void vi.advanceTimersByTime(10_000))
  expect(api.mesh_nearby_watch.mock.calls).toEqual([[true], [true]])
  await run(async () => void vi.advanceTimersByTime(20_000))
  expect(api.mesh_nearby_watch).toHaveBeenCalledTimes(4)
  await run(async () => void useConnection.setState({ state: 'reconnecting' }))
  await run(async () => void vi.advanceTimersByTime(30_000))
  expect(api.mesh_nearby_watch).toHaveBeenCalledTimes(4)
  await run(async () => void useConnection.setState({ state: 'open' }))
  expect(api.mesh_nearby_watch).toHaveBeenCalledTimes(5)
  await unmount()
  expect(api.mesh_nearby_watch).toHaveBeenLastCalledWith(false)
  await run(async () => void vi.advanceTimersByTime(30_000))
  expect(api.mesh_nearby_watch).toHaveBeenCalledTimes(6)
})

function Listeners() {
  useInvalidateOn()
  return null
}

test('the nearby, pair and SecretsRotation events write the cache', async () => {
  const { qc } = await mount(createElement(Listeners))
  await run(async () => void dispatch([{ key: 'mesh.events@1', event: { type: 'nearby', data: [XEUS2] } }]))
  expect(qc.getQueryData(queryKeys.meshNearby)).toEqual([XEUS2])
  await run(async () => void dispatch([{ key: 'mesh.events@1', event: { type: 'pair', data: CEREMONY } }, { key: 'mesh.events@1', event: { type: 'pair', data: { ...CEREMONY, id: 'c2' } } }]))
  expect(Object.keys(qc.getQueryData(queryKeys.meshPair) as object)).toEqual(['c1', 'c2'])
  await run(async () => void dispatch([{ key: 'secrets.events@1', event: { type: 'SecretsRotation', ...ROT } }]))
  expect(qc.getQueryData(queryKeys.vaultRotation)).toEqual(ROT)
})

test('useRotation: retry calls vault_rotate', async () => {
  let rot!: ReturnType<typeof useRotation>
  function Probe() {
    rot = useRotation()
    return null
  }
  await mount(createElement(Probe))
  await run(async () => void rot.rotate.mutate())
  await settle()
  expect(api.vault_rotate).toHaveBeenCalledOnce()
  expect(rot.rotate.error?.message).toBe('disk full')
})

test('Link / Devices row labels relay failures as "relay disconnected" or "not connected to the relay", never a bare "Disconnected"', async () => {
  const { DevicesTab, relayFailureText, relayPresenceText } = await import('./Link')
  expect(relayFailureText({ kind: 'idle', title: 'Disconnected after inactivity', detail: 'Reconnecting.' }).title).toBe('Relay disconnected after inactivity')
  expect(relayFailureText({ kind: 'queue-full', title: 'Disconnected: the relay queue for this device overflowed', detail: 'Reconnecting.' }).title).toBe('Relay disconnected: the relay queue for this device overflowed')
  expect(relayFailureText({ kind: 'engine-offline', title: 'Studio Mac is not connected to the relay', detail: 'Waiting…' }).title).toBe('Studio Mac is not connected to the relay')
  expect(relayPresenceText('mesh', 'offline')).toBe('relay disconnected')
  expect(relayPresenceText('engine', 'offline')).toBe('relay disconnected')
  expect(relayPresenceText('engine', 'online')).toBe('relay connected')

  // NodeSide Relay row when enabled but disconnected says "relay disconnected", never bare "Disconnected".
  ;(api as Record<string, unknown>).link_status = vi.fn(async () => ({
    enabled: true,
    connected: false,
    relay: 'https://relay.theuth.io',
    name: 'laptop',
    harness_id: 'abcd1234efgh5678',
    mesh: null,
    devices: [],
    pairing: null,
  }))
  ;(api as Record<string, unknown>).devices_list = vi.fn(async () => [])
  try {
    await mount(createElement(DevicesTab))
    await settle()
    expect(document.body.textContent).toContain('relay disconnected')
    expect(document.body.innerHTML).not.toMatch(/>Disconnected</)
  } finally {
    delete (api as Record<string, unknown>).link_status
    delete (api as Record<string, unknown>).devices_list
  }
})

test('MachinesTab queries update_status only for reachable peers without blocking, shows Update button calling update_apply, and displays RPC error in row', async () => {
  let resolveXeus!: (v: unknown) => void
  const engineCall = vi.fn((peerId: string, method: string, _params: unknown) => {
    if (method === 'update_status') {
      if (peerId === 'xeus') return new Promise((r) => { resolveXeus = r })
      if (peerId === 'cloud') return Promise.resolve(null)
      return Promise.reject(new Error(`should not query offline peer ${peerId}`))
    }
    if (method === 'update_apply') {
      return Promise.reject(new Error('update_apply: checksum mismatch'))
    }
    return Promise.resolve(null)
  })
  ;(api as Record<string, unknown>).engine_call = engineCall
  try {
    useConnection.setState({ state: 'open' })
    await mount(createElement(MachinesTab))
    await settle()

    // List renders immediately without blocking on pending update_status.
    expect(document.body.textContent).toContain('xeus')
    expect(document.body.textContent).toContain('cloud-1')
    expect(document.body.textContent).toContain('studio')
    // Only online peers (xeus, cloud) were queried; offline peer (studio) was never queried.
    expect(engineCall).toHaveBeenCalledWith('xeus', 'update_status', {})
    expect(engineCall).toHaveBeenCalledWith('cloud', 'update_status', {})
    expect(engineCall).not.toHaveBeenCalledWith('studio', 'update_status', expect.anything())
    expect(document.querySelector('[data-update-peer="xeus"]')).toBeNull()
    expect(document.querySelector('[data-update-peer="cloud"]')).toBeNull()

    // Resolve xeus with an available newer release -> visible Update button appears only on xeus.
    await run(async () => resolveXeus({ version: '2.35.0', applied: false }))
    await vi.waitFor(() => {
      const btn = document.querySelector<HTMLButtonElement>('[data-update-peer="xeus"]')
      expect(btn?.textContent).toContain('Update 2.35.0')
    })
    expect(document.querySelector('[data-update-peer="cloud"]')).toBeNull()
    expect(document.querySelector('[data-update-peer="studio"]')).toBeNull()

    // Clicking calls engine_call('xeus', 'update_apply', {}) and displays the RPC error in the row on failure.
    await run(async () => document.querySelector<HTMLButtonElement>('[data-update-peer="xeus"]')!.click())
    await vi.waitFor(() => expect(engineCall).toHaveBeenCalledWith('xeus', 'update_apply', {}))
    await vi.waitFor(() => {
      const err = document.querySelector('[data-update-error="xeus"]')
      expect(err?.getAttribute('role')).toBe('alert')
      expect(err?.textContent).toContain('update_apply: checksum mismatch')
    })
  } finally {
    delete (api as Record<string, unknown>).engine_call
  }
})

async function openPeerMenu(name: string) {
  const trigger = document.querySelector<HTMLButtonElement>(`button[aria-label="${name}: Open terminal · Rename · Unpair"]`)
  expect(trigger).not.toBeNull()
  await run(async () => trigger!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })))
  return [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')]
}

test('reachable peer Open terminal calls engine_shell with peer id and opens a node-terminal popout without navigating or calling engine_terminal', async () => {
  useConnection.setState({ state: 'open' })
  await mount(createElement(MachinesTab))
  await settle()

  const items = await openPeerMenu('xeus')
  expect(items.map((el) => el.textContent?.trim())).toEqual(['Open terminal', 'Rename', 'Unpair…'])
  const openItem = items[0]!
  expect(openItem.getAttribute('aria-disabled')).not.toBe('true')

  await run(async () => openItem.click())
  await vi.waitFor(() => expect(api.engine_shell).toHaveBeenCalledWith('xeus'))
  await vi.waitFor(() =>
    expect(mockOpenPopout).toHaveBeenCalledWith({
      kind: 'node-terminal',
      engine: 'xeus',
      shell: 'main',
      name: 'xeus',
    }),
  )
  expect(mockNavigate).not.toHaveBeenCalled()
  expect(api.engine_terminal).not.toHaveBeenCalled()
  expect(mockNotify).not.toHaveBeenCalled()
  expect(mockNotify.error).not.toHaveBeenCalled()
})

test('unreachable peer Open terminal is visible but disabled and does not call engine_shell', async () => {
  useConnection.setState({ state: 'open' })
  await mount(createElement(MachinesTab))
  await settle()

  const items = await openPeerMenu('studio')
  expect(items.map((el) => el.textContent?.trim())).toEqual(['Open terminal', 'Rename', 'Unpair…'])
  const openItem = items[0]!
  expect(openItem.getAttribute('aria-disabled')).toBe('true')

  await run(async () => openItem.click())
  await settle()
  expect(api.engine_shell).not.toHaveBeenCalled()
  expect(mockOpenPopout).not.toHaveBeenCalled()
  expect(mockNavigate).not.toHaveBeenCalled()
})

test('this machine Open terminal calls engine_shell with self_id and opens a node-terminal popout', async () => {
  useConnection.setState({ state: 'open' })
  await mount(createElement(MachinesTab))
  await settle()

  const btn = document.querySelector<HTMLButtonElement>(`[data-terminal-peer="${MESH.self_id}"]`)
  expect(btn).not.toBeNull()
  expect(btn?.textContent?.trim()).toBe('Open terminal')

  await run(async () => btn!.click())
  await vi.waitFor(() => expect(api.engine_shell).toHaveBeenCalledWith(MESH.self_id))
  await vi.waitFor(() =>
    expect(mockOpenPopout).toHaveBeenCalledWith({
      kind: 'node-terminal',
      engine: MESH.self_id,
      shell: 'main',
      name: MESH.self_name,
    }),
  )
  expect(mockNavigate).not.toHaveBeenCalled()
  expect(api.engine_terminal).not.toHaveBeenCalled()
})

test('rejected engine_shell renders the error inline in role="alert" without notify/toast', async () => {
  api.engine_shell.mockRejectedValueOnce(new Error('peer refused shell'))
  api.engine_shell.mockRejectedValueOnce(new Error('pty allocation failed'))
  useConnection.setState({ state: 'open' })
  await mount(createElement(MachinesTab))
  await settle()

  const items = await openPeerMenu('xeus')
  await run(async () => items[0]!.click())
  await vi.waitFor(() => {
    const err = document.querySelector('[data-terminal-error="xeus"]')
    expect(err?.getAttribute('role')).toBe('alert')
    expect(err?.className).toContain('font-mono')
    expect(err?.className).toContain('text-xs')
    expect(err?.className).toContain('whitespace-pre-wrap')
    expect(err?.className).toContain('break-words')
    expect(err?.className).toContain('text-danger')
    expect(err?.textContent).toBe('peer refused shell')
  })

  await run(async () => document.querySelector<HTMLButtonElement>(`[data-terminal-peer="${MESH.self_id}"]`)!.click())
  await vi.waitFor(() => {
    const err = document.querySelector(`[data-terminal-error="${MESH.self_id}"]`)
    expect(err?.getAttribute('role')).toBe('alert')
    expect(err?.textContent).toBe('pty allocation failed')
  })

  expect(mockOpenPopout).not.toHaveBeenCalled()
  expect(mockNavigate).not.toHaveBeenCalled()
  expect(mockNotify).not.toHaveBeenCalled()
  expect(mockNotify.error).not.toHaveBeenCalled()
})

test('iOS (!canPopout) Open terminal opens a bottom sheet without popout or session navigation', async () => {
  mockCanPopout.mockReturnValue(false)
  useConnection.setState({ state: 'open' })
  const { unmount } = await mount(createElement(MachinesTab))
  await settle()

  const items = await openPeerMenu('xeus')
  await run(async () => items[0]!.click())
  await vi.waitFor(() => {
    const term = document.querySelector('[data-node-terminal="xeus:main"]')
    expect(term).not.toBeNull()
    expect(term?.getAttribute('data-name')).toBe('xeus')
  })
  expect(document.querySelector('[data-slot="sheet-content"]')?.textContent).toContain('xeus')
  expect(mockOpenPopout).not.toHaveBeenCalled()
  expect(mockNavigate).not.toHaveBeenCalled()
  expect(api.engine_terminal).not.toHaveBeenCalled()

  await unmount()
  expect(api.shell_close).toHaveBeenCalledWith('main', 'xeus')
})

