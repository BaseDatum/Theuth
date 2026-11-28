import './test-dom'
import { QueryClient, QueryClientProvider, useQuery } from '@tanstack/react-query'
import { act, createElement as h } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, expect, test, vi } from 'vitest'
import { MachineScope, type SettingsMachines, SettingsMachinesProvider, aimedAt, pickerTargetFor, resolveTabBinding } from '@/components/settings/machine-target'
import { useAction } from '@/components/settings/shared'
import type { Machine } from '@/lib/mesh-machines'
import { useApi } from '@/lib/rpc'
import type { Api } from '@/rpc/bindings/client'
import { RpcError } from '@/rpc/client'
import type { Discovery } from '@/rpc/discovery'
import { useConnection } from '@/store/connection'
import { LOCAL } from '@/link/engines'
import { scope } from '@/store/engine-scope'

/** Everything through the active (`local`) scope's client: the Mac's engine itself, P50 by `engine_call`. */
const machine = (id: string, name: string, m: Partial<Machine> = {}): Machine => ({ id, name, kind: 'peer', connected: false, client: LOCAL, forwarded: true, hub: false, online: true, sealed: false, path: 'direct', ...m })
const mac = machine('mac', 'This Mac', { kind: 'local', connected: true, forwarded: false, hub: true, path: null })
const p50 = machine('p50', 'P50 · ThinkPad')

function fakeApi() {
  return {
    info: vi.fn(async () => ({ name: 'mac' })),
    title_model: vi.fn(async () => ({ id: 'here-model' })),
    set_title_model: vi.fn(async () => null),
    engine_call: vi.fn(async (_e: string, method: string) => (method === 'title_model' ? { id: 'p50-model' } : method === 'info' ? { name: 'p50' } : null)),
    engine_call_all: vi.fn(),
    engine_caps: vi.fn(),
    settings_values: vi.fn(),
  }
}

/** A tab that reads and writes through `useApi()`, as every settings tab does. */
function Tab() {
  const api = useApi()
  const model = useQuery({ queryKey: ['title-model'], queryFn: () => api.title_model() })
  const set = useAction((id: string) => api.set_title_model(id))
  return h('button', { 'data-model': model.data?.id ?? 'loading', onClick: () => set.mutate('openai/gpt-6') })
}

afterEach(() => {
  document.body.innerHTML = ''
})

async function mount(scope: 'machine' | 'everywhere', api: ReturnType<typeof fakeApi>) {
  useConnection.setState({ api: api as unknown as Api, state: 'open' })
  const settings: SettingsMachines = { machines: [mac, p50], target: p50, connected: mac, api: api as unknown as Api, local: api as unknown as Api, meshClient: LOCAL, qc: new QueryClient(), tab: null, open: () => {} }
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const host = document.createElement('div')
  document.body.append(host)
  await act(async () =>
    createRoot(host).render(h(QueryClientProvider, { client: qc }, h(SettingsMachinesProvider, { value: settings }, h(MachineScope, { machine: aimedAt({ scope }, p50), children: h(Tab) })))),
  )
  const button = () => host.querySelector<HTMLButtonElement>('button[data-model]')
  await vi.waitFor(() => expect(button()?.dataset.model ?? host.innerHTML).not.toMatch(/^loading$|^<p/))
  return button()!
}

test('227: with P50 picked, a machine tab reads and writes P50 through engine_call, never the connected engine', async () => {
  const api = fakeApi()
  const button = await mount('machine', api)
  expect(button.dataset.model).toBe('p50-model')
  expect(api.engine_call).toHaveBeenCalledWith('p50', 'title_model', {})
  expect(api.title_model).not.toHaveBeenCalled()
  const reads = api.engine_call.mock.calls.filter(([, m]) => m === 'title_model').length
  await act(async () => button.click())
  await vi.waitFor(() => expect(api.engine_call).toHaveBeenCalledWith('p50', 'set_title_model', { model: 'openai/gpt-6' }))
  expect(api.set_title_model).not.toHaveBeenCalled()
  // The write invalidated P50's queries: the tab re-read it there.
  await vi.waitFor(() => expect(api.engine_call.mock.calls.filter(([, m]) => m === 'title_model').length).toBeGreaterThan(reads))
})

test('227: with P50 picked, an everywhere tab ignores the pick: the connected engine answers, nothing is forwarded', async () => {
  const api = fakeApi()
  const button = await mount('everywhere', api)
  expect(button.dataset.model).toBe('here-model')
  await act(async () => button.click())
  await vi.waitFor(() => expect(api.set_title_model).toHaveBeenCalledWith('openai/gpt-6'))
  expect(api.engine_call).not.toHaveBeenCalled()
})

test('227: an offline target says so instead of the tab', async () => {
  const api = fakeApi()
  api.engine_call.mockImplementation(async () => {
    throw new RpcError(-32051, 'offline')
  })
  useConnection.setState({ api: api as unknown as Api, state: 'open' })
  const xeus = machine('xeus', 'xeus')
  const settings: SettingsMachines = { machines: [mac, xeus], target: xeus, connected: mac, api: api as unknown as Api, local: api as unknown as Api, meshClient: LOCAL, qc: new QueryClient(), tab: null, open: () => {} }
  const host = document.createElement('div')
  document.body.append(host)
  await act(async () =>
    createRoot(host).render(h(QueryClientProvider, { client: new QueryClient() }, h(SettingsMachinesProvider, { value: settings }, h(MachineScope, { machine: xeus, children: h(Tab) })))),
  )
  await vi.waitFor(() => expect(host.querySelector('[data-machine-unreachable]')?.textContent).toBe('xeus is offline. Its settings show when it reconnects.'))
  expect(host.querySelector('button[data-model]')).toBeNull()
})

test('227: an engine with a client of its own (the hub\'s while this device\'s engine is active) is used directly: no engine_call', async () => {
  const mine = fakeApi()
  const hub = fakeApi()
  hub.title_model.mockImplementation(async () => ({ id: 'hub-model' }))
  useConnection.setState({ api: mine as unknown as Api, state: 'open' })
  scope('hub-row').connection.setState({ api: hub as unknown as Api, state: 'open' })
  const mbp = machine('mbp', '333 · MacBook Pro', { client: 'hub-row', forwarded: false, kind: 'hub', hub: true })
  const settings: SettingsMachines = { machines: [mac, mbp], target: mbp, connected: mac, api: hub as unknown as Api, local: mine as unknown as Api, meshClient: 'hub-row', qc: new QueryClient(), tab: null, open: () => {} }
  const host = document.createElement('div')
  document.body.append(host)
  await act(async () =>
    createRoot(host).render(h(QueryClientProvider, { client: new QueryClient() }, h(SettingsMachinesProvider, { value: settings }, h(MachineScope, { machine: mbp, children: h(Tab) })))),
  )
  await vi.waitFor(() => expect(host.querySelector<HTMLElement>('button[data-model]')?.dataset.model).toBe('hub-model'))
  await act(async () => host.querySelector<HTMLButtonElement>('button[data-model]')!.click())
  await vi.waitFor(() => expect(hub.set_title_model).toHaveBeenCalledWith('openai/gpt-6'))
  expect([mine.title_model, mine.engine_call, hub.engine_call].map((f) => f.mock.calls.length)).toEqual([0, 0, 0])
})

test('227: a connectedOnly tab (its methods are not forwardable) stays on the connected machine when a forwarded one is picked', () => {
  const mbp = machine('mbp', '333', { client: 'hub-row', forwarded: false })
  expect(aimedAt({ scope: 'machine', connectedOnly: true }, p50)).toBeNull()
  // A machine with a client of its own is reached directly: no forwarding involved.
  expect(aimedAt({ scope: 'machine', connectedOnly: true }, mbp)).toBe(mbp)
  expect(aimedAt({ scope: 'machine' }, p50)).toBe(p50)
  expect(aimedAt({ scope: 'everywhere' }, p50)).toBeNull()
})

function discoveryWith(methods: readonly string[], phase: Discovery['phase'] = 'ready'): Discovery {
  return {
    phase,
    capabilities: null,
    unavailable: new Set(),
    document:
      phase === 'ready'
        ? ({ methods: methods.map((name) => ({ name, params: [], result: { schema: {} } })), 'x-theuth': { events: [], subscriptions: [] } } as never)
        : null,
  }
}

test('resolveTabBinding: paired phone on LOCAL binds Network everywhere tabs to hub scope and machine tabs to hub by default', () => {
  const phone = machine(LOCAL, 'This iPhone', { kind: 'local', connected: true, client: LOCAL, forwarded: false, path: null })
  const hub = machine('mbp', '333 · MacBook Pro', { kind: 'hub', connected: false, client: 'hub-row', forwarded: false, hub: true, path: null })
  const iosDiscovery = discoveryWith([])
  const hubDiscovery = discoveryWith(['mesh_status', 'link_status', 'a2a_status', 'sandbox_create'])
  const hubConnecting = discoveryWith([], 'connecting')
  const hubWithoutVms = discoveryWith(['mesh_status', 'link_status', 'a2a_status'])

  const machineTab = { id: 'machine', scope: 'machine' as const, required: { methods: ['mesh_status'] as const } }
  const machinesTab = { id: 'machines', scope: 'everywhere' as const, required: { methods: ['mesh_status'] as const } }
  const devicesTab = { id: 'devices', scope: 'everywhere' as const }
  const vmsTab = { id: 'vms', scope: 'machine' as const, required: { methods: ['sandbox_create'] as const } }
  const sharingTab = { id: 'shared', scope: 'everywhere' as const, required: { methods: ['mesh_status'] as const } }

  // Default (picked === null): Network machine tab aims directly at the hub's own machine (no forwarding).
  expect(
    resolveTabBinding({
      categoryId: 'network',
      tab: machineTab,
      target: phone,
      picked: null,
      machines: [phone, hub],
      active: LOCAL,
      meshClient: 'hub-row',
      discovery: iosDiscovery,
      hubDiscovery,
    }),
  ).toEqual({ aimed: hub, scopeClient: null, loading: false })

  // Default before mesh_status populates the hub machine row: scopes directly to meshClient rather than flashing Unavailable.
  expect(
    resolveTabBinding({
      categoryId: 'network',
      tab: machineTab,
      target: phone,
      picked: null,
      machines: [phone],
      active: LOCAL,
      meshClient: 'hub-row',
      discovery: iosDiscovery,
      hubDiscovery,
    }),
  ).toEqual({ aimed: null, scopeClient: 'hub-row', loading: false })

  // Everywhere Network tabs (Machines, Devices) re-scope to meshClient with aimed = null.
  expect(
    resolveTabBinding({
      categoryId: 'network',
      tab: machinesTab,
      target: phone,
      picked: null,
      machines: [phone, hub],
      active: LOCAL,
      meshClient: 'hub-row',
      discovery: iosDiscovery,
      hubDiscovery,
    }),
  ).toEqual({ aimed: null, scopeClient: 'hub-row', loading: false })
  expect(
    resolveTabBinding({
      categoryId: 'network',
      tab: devicesTab,
      target: phone,
      picked: null,
      machines: [phone, hub],
      active: LOCAL,
      meshClient: 'hub-row',
      discovery: iosDiscovery,
      hubDiscovery,
    }),
  ).toEqual({ aimed: null, scopeClient: 'hub-row', loading: false })

  // Sharing (and other non-Network everywhere tabs) are NOT re-scoped.
  expect(
    resolveTabBinding({
      categoryId: 'sharing',
      tab: sharingTab,
      target: phone,
      picked: null,
      machines: [phone, hub],
      active: LOCAL,
      meshClient: 'hub-row',
      discovery: iosDiscovery,
      hubDiscovery,
    }),
  ).toEqual({ aimed: null, scopeClient: null, loading: false })

  // Explicit This iPhone pick is respected: machine tab stays on phone so Capability shows Unavailable.
  expect(
    resolveTabBinding({
      categoryId: 'network',
      tab: machineTab,
      target: phone,
      picked: LOCAL,
      machines: [phone, hub],
      active: LOCAL,
      meshClient: 'hub-row',
      discovery: iosDiscovery,
      hubDiscovery,
    }),
  ).toEqual({ aimed: phone, scopeClient: null, loading: false })

  // Hub discovery still connecting: required Network tabs report loading; Devices still mounts in hub scope so Reachable shows.
  expect(
    resolveTabBinding({
      categoryId: 'network',
      tab: machineTab,
      target: phone,
      picked: null,
      machines: [phone],
      active: LOCAL,
      meshClient: 'hub-row',
      discovery: iosDiscovery,
      hubDiscovery: hubConnecting,
    }),
  ).toEqual({ aimed: null, scopeClient: null, loading: true })
  expect(
    resolveTabBinding({
      categoryId: 'network',
      tab: machinesTab,
      target: phone,
      picked: null,
      machines: [phone],
      active: LOCAL,
      meshClient: 'hub-row',
      discovery: iosDiscovery,
      hubDiscovery: hubConnecting,
    }),
  ).toEqual({ aimed: null, scopeClient: null, loading: true })
  expect(
    resolveTabBinding({
      categoryId: 'network',
      tab: devicesTab,
      target: phone,
      picked: null,
      machines: [phone],
      active: LOCAL,
      meshClient: 'hub-row',
      discovery: iosDiscovery,
      hubDiscovery: hubConnecting,
    }),
  ).toEqual({ aimed: null, scopeClient: 'hub-row', loading: false })

  // Hub that does not advertise VMs does not override aim.
  expect(
    resolveTabBinding({
      categoryId: 'network',
      tab: vmsTab,
      target: phone,
      picked: null,
      machines: [phone, hub],
      active: LOCAL,
      meshClient: 'hub-row',
      discovery: iosDiscovery,
      hubDiscovery: hubWithoutVms,
    }),
  ).toEqual({ aimed: phone, scopeClient: null, loading: false })

  // Unpaired phone (meshClient === active) or desktop whose active engine already serves the tab: no re-scoping.
  expect(
    resolveTabBinding({
      categoryId: 'network',
      tab: devicesTab,
      target: phone,
      picked: null,
      machines: [phone],
      active: LOCAL,
      meshClient: LOCAL,
      discovery: iosDiscovery,
    }),
  ).toEqual({ aimed: null, scopeClient: null, loading: false })
  expect(
    resolveTabBinding({
      categoryId: 'network',
      tab: devicesTab,
      target: mac,
      picked: null,
      machines: [mac, hub],
      active: LOCAL,
      meshClient: 'hub-row',
      discovery: hubDiscovery,
      hubDiscovery,
    }),
  ).toEqual({ aimed: null, scopeClient: null, loading: false })

  // pickerTargetFor: highlights hub while picked === null and Network tab aims/scopes to hub; keeps target on explicit pick or other categories.
  expect(pickerTargetFor({ aimed: hub, scopeClient: null, loading: false }, null, [phone, hub], phone)).toBe(hub)
  expect(pickerTargetFor({ aimed: null, scopeClient: 'hub-row', loading: false }, null, [phone, hub], phone)).toBe(hub)
  expect(pickerTargetFor({ aimed: phone, scopeClient: null, loading: false }, LOCAL, [phone, hub], phone)).toBe(phone)
  expect(pickerTargetFor({ aimed: null, scopeClient: null, loading: false }, null, [phone, hub], phone)).toBe(phone)
})