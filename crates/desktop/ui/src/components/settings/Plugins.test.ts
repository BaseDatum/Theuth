// 216 contract §11.4 (B3): Settings › Plugins against a fake engine — rows, the state chip, Approve only on a waiting
// card with exactly its hashes, Uninstall behind a confirmation, per-placement Hide, New plugin, Open folder only in a
// desktop shell.
import './test-dom'
import { QueryClient, QueryClientProvider, notifyManager } from '@tanstack/react-query'
import { act, createElement } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import type * as Platform from '@/lib/platform'
import type { InstallCard, PluginDetail, PluginInfo } from '@/rpc/bindings/method.plugins.output'
import { Plugins, moved } from './Plugins'

const M = 'a'.repeat(64)
const C = 'b'.repeat(64)

const info = (over: Partial<PluginInfo>): PluginInfo => ({
  name: 'turns',
  version: '0.2.0',
  description: 'Counts turns per day',
  state: 'running',
  error: null,
  enabled: true,
  needs_approval: false,
  card: null,
  placements: [],
  tools: [],
  dir: '/home/me/.theuth/plugins/turns',
  log: '/home/me/.theuth/logs/plugins/turns.log.2026-11-25',
  approved_at: '2026-11-24T10:00:00Z',
  order: 0,
  ...over,
})
const turns = info({
  placements: [
    { id: 'turns', kind: 'sidebar', label: 'Turns today', icon: 'gauge', status: true, open: 'popover', size: null, format: 'openui', fallback: 'text', hidden: false, command: null },
    { id: 'reset', kind: 'command', label: 'Reset', icon: 'puzzle', status: false, open: null, size: null, format: null, fallback: 'text', hidden: false, command: { name: 'reset', slash: '/turns:reset', description: 'Reset the count', args: null } },
  ],
  tools: ['turns_today'],
})
const card: InstallCard = {
  id: `plugin:widget:${M.slice(0, 8)}:${C.slice(0, 8)}:0`,
  plugin: 'widget',
  version: '1.0.0',
  kind: 'install',
  manifest_sha256: M,
  code_sha256: C,
  risk: 'destructive',
  lines: [{ key: 'run', text: 'Runs node main.js from ~/.theuth/plugins/widget as you, with your files and network', risk: 'destructive', new: false }],
  changed_paths: [],
  unverified: false,
  dir: '/home/me/.theuth/plugins/widget',
  at_ms: 0,
}
const widget = info({ name: 'widget', version: '1.0.0', description: '', state: 'awaiting_approval', enabled: false, needs_approval: true, card: card.id, approved_at: null, order: 1, dir: card.dir })

const detail = (p: PluginInfo): PluginDetail => ({
  info: p,
  manifest: {},
  requested: { subscribe: [], emits: [], placements: [], tools: p.tools.map((name) => ({ name, risk: 'read_only' })), capabilities: { notify: false, input: false, call: null }, secrets: [], run: ['python3', 'main.py'], cwd: '.', env: {} },
  granted: p.approved_at ? { subscribe: ['agent.events#TurnFinished'], emits: [], placements: ['turns', 'reset'], tools: [], capabilities: { notify: false, input: false, call: null }, secrets: [], run: ['python3', 'main.py'], cwd: '.', env: {} } : null,
  card: p.needs_approval ? card : null,
  catalog: [],
  stderr_tail: [],
  warnings: [],
})

const engine = vi.hoisted(() => ({ calls: [] as [string, unknown[]][], desktop: false }))
vi.mock('@/lib/rpc', async (original) => {
  const record =
    (method: string, reply: unknown = null) =>
    async (...args: unknown[]) => {
      engine.calls.push([method, args])
      return reply
    }
  return {
    ...(await original<typeof Rpc>()),
    useApi: () => ({
      plugin_list: async () => [turns, widget],
      plugin_get: async (name: string) => detail(name === 'widget' ? widget : turns),
      plugin_logs: async (name: string) => ({ name, path: `/logs/${name}`, lines: ['started'] }),
      plugin_approve: record('plugin_approve', widget),
      plugin_set_enabled: record('plugin_set_enabled', turns),
      plugin_set_hidden: record('plugin_set_hidden', turns),
      plugin_set_order: record('plugin_set_order', [turns, widget]),
      plugin_restart: record('plugin_restart', turns),
      plugin_uninstall: record('plugin_uninstall'),
      plugin_new: record('plugin_new', { name: 'demo', dir: '/p/demo', lang: 'sh', raw: true, files: ['plugin.yaml'], next: 'awaiting approval' }),
    }),
  }
})
vi.mock('@/rpc/capabilities', () => ({ useQuerySupport: () => true }))
vi.mock('@/lib/notify', () => ({ notify: Object.assign(vi.fn(), { error: vi.fn(), message: vi.fn(), success: vi.fn() }) }))
vi.mock('@/lib/platform', async (original) => ({ ...(await original<typeof Platform>()), isDesktop: () => engine.desktop }))
vi.mock('@tauri-apps/plugin-opener', () => ({ openPath: vi.fn(async () => undefined) }))
notifyManager.setScheduler(queueMicrotask)

let unmount: (() => void) | null = null
beforeEach(() => {
  engine.calls.length = 0
  engine.desktop = false
})
afterEach(() => {
  unmount?.()
  unmount = null
  document.body.innerHTML = ''
})

async function mount(focus: string | null = null) {
  const el = document.createElement('div')
  document.body.append(el)
  const root = createRoot(el)
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  await act(async () => root.render(createElement(QueryClientProvider, { client: qc, children: createElement(Plugins, { focus }) })))
  // The waiting card opens its row; its detail answers before the test looks.
  await vi.waitFor(() => expect(el.textContent).toContain('Runs node main.js'))
  unmount = () => {
    act(() => root.unmount())
    el.remove()
  }
  const row = (name: string) => el.querySelector<HTMLElement>(`[data-plugin="${name}"]`)!
  const button = (scope: ParentNode, label: string) => [...scope.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent === label || b.getAttribute('aria-label') === label)
  const click = (b: HTMLElement | undefined) => act(async () => b!.click())
  return { el, row, button, click }
}

const calls = (method: string) => engine.calls.filter(([m]) => m === method).map(([, args]) => args)

test('rows from the engine: name, version, state chip; Approve only on the waiting card, with exactly its hashes', async () => {
  const { row, button, click } = await mount()
  expect(row('turns').textContent).toContain('turns')
  expect(row('turns').textContent).toContain('0.2.0')
  expect(row('turns').textContent).toContain('running')
  expect(row('widget').textContent).toContain('needs approval')
  expect(button(row('turns'), 'Approve')).toBeUndefined()
  // Opened, the running plugin shows what it was granted, still without Approve.
  await click(button(row('turns'), 'turns0.2.0'))
  await vi.waitFor(() => expect(row('turns').textContent).toContain('Reads agent.events#TurnFinished'))
  expect(button(row('turns'), 'Approve')).toBeUndefined()
  await click(button(row('widget'), 'Approve'))
  await vi.waitFor(() => expect(calls('plugin_approve')).toEqual([['widget', M, C]]))
})

test('Uninstall asks first; the confirmation carries Keep its data', async () => {
  const { row, button, click } = await mount()
  await click(button(row('turns'), 'Uninstall turns'))
  const dialog = () => document.querySelector<HTMLElement>('[role=alertdialog]')
  expect(dialog()?.textContent).toContain('Uninstall turns?')
  expect(calls('plugin_uninstall')).toEqual([])
  await click(dialog()!.querySelector<HTMLElement>('[role=switch]')!)
  await click(button(dialog()!, 'Uninstall'))
  await vi.waitFor(() => expect(calls('plugin_uninstall')).toEqual([['turns', true]]))
})

test('a placement’s switch hides it with plugin_set_hidden; the enable switch and reorder call theirs', async () => {
  const { row, button, click } = await mount()
  await click(button(row('turns'), 'turns0.2.0'))
  await vi.waitFor(() => expect(button(row('turns'), 'Show Turns today')).toBeDefined())
  await click(button(row('turns'), 'Show Turns today'))
  await vi.waitFor(() => expect(calls('plugin_set_hidden')).toEqual([['turns', 'turns', true]]))
  await click(button(row('turns'), 'Enable turns'))
  await vi.waitFor(() => expect(calls('plugin_set_enabled')).toEqual([['turns', false]]))
  expect(button(row('turns'), 'Move turns up')?.disabled).toBe(true)
  await click(button(row('turns'), 'Move turns down'))
  await vi.waitFor(() => expect(calls('plugin_set_order')).toEqual([[['widget', 'turns']]]))
})

test('New plugin scaffolds with the name, the language and Raw', async () => {
  const { el, button, click } = await mount()
  await click(button(el, 'New plugin'))
  const input = document.querySelector<HTMLInputElement>('[aria-label="Plugin name"]')!
  await act(async () => {
    Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')!.set!.call(input, 'demo')
    input.dispatchEvent(new Event('input', { bubbles: true }))
  })
  const raw = () => document.querySelector<HTMLButtonElement>('[aria-label="Raw protocol, no SDK"]')!
  // Raw has templates for py and sh only: off for Go.
  await click(button(document.body, 'go'))
  expect(raw().disabled).toBe(true)
  await click(button(document.body, 'sh'))
  expect(raw().disabled).toBe(false)
  await click(raw())
  await click(button(document.body, 'Create'))
  await vi.waitFor(() => expect(calls('plugin_new')).toEqual([['demo', 'sh', true]]))
})

test('Open folder only in a desktop shell (not on the web or the phone)', async () => {
  const web = await mount()
  expect(web.button(web.row('turns'), 'Open folder')).toBeUndefined()
  unmount?.()
  engine.desktop = true
  const desktop = await mount()
  expect(desktop.button(desktop.row('turns'), 'Open folder')).toBeDefined()
})

test('moved swaps a name with its neighbour and leaves the ends alone', () => {
  expect(moved(['a', 'b', 'c'], 1, -1)).toEqual(['b', 'a', 'c'])
  expect(moved(['a', 'b', 'c'], 2, 1)).toEqual(['a', 'b', 'c'])
})
