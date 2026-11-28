// 224: the engine switcher carries "Add a cloud node" directly above "Pair with a mesh…", where the engine has cloud
// nodes; choosing it opens the wizard's add-to-mesh entry.
import '@/components/settings/test-dom'
import { act, createElement } from 'react'
import { type Root, createRoot } from 'react-dom/client'
import { afterEach, expect, test, vi } from 'vitest'
import type * as Platform from '@/lib/platform'
import type * as Connection from '@/store/connection'
import { EngineSwitcher, manageSettingsTab } from './EngineSwitcher'
import type { Discovery } from '@/rpc/discovery'
import { scope } from '@/store/engine-scope'

const env = vi.hoisted(() => ({ cloud: true, navigate: vi.fn(), engines: [] as { id: string; kind: 'engine' | 'mesh'; pairedAt: number }[], active: 'local' }))

vi.mock('@tanstack/react-router', () => ({ useNavigate: () => env.navigate }))
vi.mock('@/lib/platform', async (original) => ({ ...(await original<typeof Platform>()), isIos: () => true }))
vi.mock('@/link/nodes', () => ({ retryPair: vi.fn() }))
vi.mock('@/link/engines', () => ({
  LOCAL: 'local',
  LOCAL_NAME: 'This iPhone',
  hasLocal: () => true,
  useEngineTree: () => [],
  useEngines: (sel: (s: object) => unknown) => sel({ engines: env.engines, active: env.active, presence: {}, pairError: {}, setActive: vi.fn() }),
}))
vi.mock('@/store/connection', async (original) => ({ ...(await original<typeof Connection>()), useConnection: (sel: (s: object) => unknown) => sel({ info: null }) }))
vi.mock('@/rpc/capabilities', () => ({ Capability: ({ children }: { children: unknown }) => (env.cloud ? children : null) }))
// The menu rendered open, flat: each item a `menuitem` whose click is its `onSelect`.
// `vi.mock` factories run hoisted above the file's imports, so they import what they render inside.
vi.mock('@/components/ui/dropdown-menu', async () => {
  const { createElement: h } = await import('react')
  const pass = ({ children }: { children: unknown }) => children
  return {
    DropdownMenu: pass,
    DropdownMenuTrigger: pass,
    DropdownMenuContent: ({ children }: { children: unknown }) => h('div', { role: 'menu' }, children as never),
    DropdownMenuSeparator: () => h('hr'),
    DropdownMenuItem: ({ children, onSelect }: { children: unknown; onSelect?: () => void }) => h('div', { role: 'menuitem', onClick: onSelect }, children as never),
  }
})

const MACHINE_METHODS = ['config_flag_get', 'config_flag_set', 'mesh_rename', 'mesh_set_hub', 'mesh_set_nearby', 'mesh_status', 'sessions_status', 'vault_rotate', 'vault_rotation']
const makeDiscovery = (methods: readonly string[], phase: Discovery['phase'] = 'ready'): Discovery => ({
  phase,
  capabilities: null,
  unavailable: new Set(),
  document: phase === 'ready' ? ({ methods: methods.map((name) => ({ name, params: [], result: { schema: {} } })), 'x-theuth': { events: [], subscriptions: [] } } as never) : null,
})

let root: Root | null = null
let host: HTMLElement | null = null
afterEach(() => {
  act(() => root?.unmount())
  root = null
  host?.remove()
  host = null
  env.cloud = true
  env.engines = []
  env.active = 'local'
  env.navigate.mockClear()
  scope('local').connection.setState({ discovery: makeDiscovery([], 'connecting') })
  scope('mesh-1').connection.setState({ discovery: makeDiscovery([], 'connecting') })
})

function render() {
  host = document.body.appendChild(document.createElement('div'))
  root = createRoot(host)
  act(() => root!.render(createElement(EngineSwitcher)))
  return [...host.querySelectorAll('[role="menuitem"]')].map((e) => e.textContent ?? '')
}

test('"Add a cloud node" sits directly above "Pair with a mesh…" and opens the add-to-mesh wizard', () => {
  const items = render()
  const add = items.findIndex((t) => t.includes('Add a cloud node'))
  expect(add).toBeGreaterThanOrEqual(0)
  expect(items[add + 1]).toContain('Pair with a mesh…')
  act(() => host!.querySelector<HTMLButtonElement>('[role="menuitem"] button')!.click())
  expect(env.navigate).toHaveBeenCalledTimes(1)
  expect(env.navigate).toHaveBeenCalledWith({ to: '/cloud/new', search: { entry: 'add-to-mesh' } })
})

test('an engine without cloud nodes shows no entry', () => {
  env.cloud = false
  expect(render().some((t) => t.includes('Add a cloud node'))).toBe(false)
})

test('Manage… opens devices on unpaired iOS and machine on iOS paired to a mesh hub', () => {
  scope('local').connection.setState({ discovery: makeDiscovery([]) })
  render()
  const manage = [...host!.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((e) => e.textContent?.includes('Manage…'))!
  act(() => manage.click())
  expect(env.navigate).toHaveBeenLastCalledWith({ to: '/settings/$tab', params: { tab: 'devices' }, search: {} })

  act(() => root?.unmount())
  host!.remove()
  env.engines = [{ id: 'mesh-1', kind: 'mesh', pairedAt: 1 }]
  scope('mesh-1').connection.setState({ discovery: makeDiscovery(MACHINE_METHODS) })
  render()
  const managePaired = [...host!.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((e) => e.textContent?.includes('Manage…'))!
  act(() => managePaired.click())
  expect(env.navigate).toHaveBeenLastCalledWith({ to: '/settings/$tab', params: { tab: 'machine' }, search: {} })
})

test('manageSettingsTab: desktop stays on machine; paired hub opens machine; unpaired or incapable hub opens devices', () => {
  const iosLocal = makeDiscovery([])
  const hubReady = makeDiscovery(MACHINE_METHODS)
  const hubConnecting = makeDiscovery([], 'connecting')
  const hubIncapable = makeDiscovery(['mesh_status'])
  const hubFailed = makeDiscovery([], 'failed')

  expect(manageSettingsTab(true, iosLocal)).toBe('machine')
  expect(manageSettingsTab(false, iosLocal, hubReady)).toBe('machine')
  expect(manageSettingsTab(false, iosLocal, hubConnecting)).toBe('machine')
  expect(manageSettingsTab(false, iosLocal, hubIncapable)).toBe('devices')
  expect(manageSettingsTab(false, iosLocal, hubFailed)).toBe('devices')
  expect(manageSettingsTab(false, iosLocal)).toBe('devices')
  expect(manageSettingsTab(false, hubReady)).toBe('machine')
})
