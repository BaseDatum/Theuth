// 224: the engine switcher carries "Add a cloud node" directly above "Pair with a mesh…", where the engine has cloud
// nodes; choosing it opens the wizard's add-to-mesh entry.
import '@/components/settings/test-dom'
import { act, createElement } from 'react'
import { type Root, createRoot } from 'react-dom/client'
import { afterEach, expect, test, vi } from 'vitest'
import type * as Platform from '@/lib/platform'
import type * as Connection from '@/store/connection'
import { EngineSwitcher } from './EngineSwitcher'

const env = vi.hoisted(() => ({ cloud: true, navigate: vi.fn() }))

vi.mock('@tanstack/react-router', () => ({ useNavigate: () => env.navigate }))
vi.mock('@/lib/platform', async (original) => ({ ...(await original<typeof Platform>()), isIos: () => true }))
vi.mock('@/link/nodes', () => ({ retryPair: vi.fn() }))
vi.mock('@/link/engines', () => ({
  LOCAL: 'local',
  LOCAL_NAME: 'This iPhone',
  hasLocal: () => true,
  useEngineTree: () => [],
  useEngines: (sel: (s: object) => unknown) => sel({ engines: [], active: 'local', presence: {}, pairError: {}, setActive: vi.fn() }),
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

let root: Root | null = null
let host: HTMLElement
afterEach(() => {
  act(() => root?.unmount())
  host.remove()
  env.cloud = true
  env.navigate.mockClear()
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
  act(() => host.querySelector<HTMLButtonElement>('[role="menuitem"] button')!.click())
  expect(env.navigate).toHaveBeenCalledTimes(1)
  expect(env.navigate).toHaveBeenCalledWith({ to: '/cloud/new', search: { entry: 'add-to-mesh' } })
})

test('an engine without cloud nodes shows no entry', () => {
  env.cloud = false
  expect(render().some((t) => t.includes('Add a cloud node'))).toBe(false)
})
