// 216 contract §11.4 (B3): the `plugin` item's screen — the card's lines and changed files from `plugin_get`, Approve
// with exactly the card's hashes, Review to Settings › Plugins; a plugin's notice has no Approve.
import '@/components/settings/test-dom'
import { QueryClient, QueryClientProvider, notifyManager } from '@tanstack/react-query'
import { act, createElement, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, expect, test, vi } from 'vitest'
import type * as Router from '@tanstack/react-router'
import type * as Rpc from '@/lib/rpc'
import type { InstallCard, PluginDetail, PluginInfo } from '@/rpc/bindings/method.plugins.output'
import type { AttentionItem } from '@/rpc/bindings/method.roster.output'
import { dismissHint, pluginRef } from './labels'
import { PluginInstallScreen } from './PluginInstallScreen'

const M = 'c'.repeat(64)
const C = 'd'.repeat(64)
const ID = `plugin:turns:${M.slice(0, 8)}:${C.slice(0, 8)}:1`

const card: InstallCard = {
  id: ID,
  plugin: 'turns',
  version: '0.3.0',
  kind: 'code_changed',
  manifest_sha256: M,
  code_sha256: C,
  risk: 'destructive',
  lines: [
    { key: 'run', text: 'Runs python3 main.py from ~/.theuth/plugins/turns as you, with your files and network', risk: 'destructive', new: false },
    { key: 'secret', text: 'Gets the secret github.pat in its environment', risk: 'destructive', new: true },
  ],
  changed_paths: ['main.py', 'lib/count.py'],
  unverified: true,
  dir: '/home/me/.theuth/plugins/turns',
  at_ms: 0,
}
const info = { name: 'turns', version: '0.3.0', description: '', state: 'running', error: null, enabled: true, needs_approval: true, card: ID, placements: [], tools: [], dir: card.dir, log: '', approved_at: null, order: 0 } as PluginInfo
const detail = { info, manifest: {}, requested: {}, granted: null, card, catalog: [], stderr_tail: [], warnings: [] } as unknown as PluginDetail

const engine = vi.hoisted(() => ({ calls: [] as [string, unknown[]][] }))
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({
    plugin_get: async (name: string) => {
      engine.calls.push(['plugin_get', [name]])
      return detail
    },
    plugin_approve: async (...args: unknown[]) => {
      engine.calls.push(['plugin_approve', args])
      return info
    },
  }),
}))
// Only the plugin methods answer: the header's session, task and mesh lookups stay off.
vi.mock('@/rpc/capabilities', () => ({ useQuerySupport: (methods: string[]) => methods.every((m) => m.startsWith('plugin_')) }))
vi.mock('@tanstack/react-router', async (original) => ({
  ...(await original<typeof Router>()),
  Link: ({ children, to, params, search }: { children: ReactNode; to: string; params?: Record<string, string>; search?: Record<string, string> }) =>
    createElement('a', { href: `${to.replace('$tab', params?.tab ?? '')}?${new URLSearchParams(search ?? {}).toString()}` }, children),
  useNavigate: () => () => undefined,
}))
vi.mock('@/components/tasks/queries', async (original) => ({
  ...(await original<object>()),
  useMesh: () => ({ data: undefined }),
  useTask: () => ({ data: undefined }),
  useTasks: () => ({ data: undefined }),
}))
vi.mock('@/lib/notify', () => ({ notify: Object.assign(vi.fn(), { error: vi.fn(), message: vi.fn(), success: vi.fn() }) }))
notifyManager.setScheduler(queueMicrotask)

const item = (over: Partial<AttentionItem>): AttentionItem =>
  ({ id: ID, kind: 'plugin', title: 'Plugin turns changed its code', detail: null, at_ms: Date.now(), task: null, agent: null, session: null, approval: null, decision: null, packet: null, ask: null, turn: null, ...over }) as AttentionItem

let unmount: (() => void) | null = null
afterEach(() => {
  unmount?.()
  unmount = null
  engine.calls.length = 0
})

async function mount(it: AttentionItem) {
  const el = document.createElement('div')
  document.body.append(el)
  const root = createRoot(el)
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  await act(async () => root.render(createElement(QueryClientProvider, { client: qc, children: createElement(PluginInstallScreen, { item: it }) })))
  unmount = () => {
    act(() => root.unmount())
    el.remove()
  }
  const button = (label: string) => [...el.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent === label)
  return { el, button }
}

test('a code-changed card: attribution, its lines with risks, the changed files, the folder, the unverified notice; Approve sends exactly its hashes', async () => {
  const { el, button } = await mount(item({}))
  await vi.waitFor(() => expect(el.textContent).toContain('Gets the secret github.pat'))
  const text = el.textContent ?? ''
  expect(text).toContain('plugin · turns')
  expect(text).toContain('Plugin turns changed its code')
  expect(text).toContain('Runs python3 main.py')
  expect(el.querySelectorAll('li[data-grant]')).toHaveLength(2)
  expect(el.querySelector('li[data-new="true"]')?.textContent).toContain('github.pat')
  expect(text).toContain('main.py')
  expect(text).toContain('lib/count.py')
  expect(text).toContain('/home/me/.theuth/plugins/turns')
  expect(text).toContain('could not be verified')
  expect(el.querySelector('a[href="/settings/plugins?focus=turns"]')?.textContent).toBe('Review')
  await act(async () => button('Approve')!.click())
  await vi.waitFor(() => expect(engine.calls.filter(([m]) => m === 'plugin_approve')).toEqual([['plugin_approve', ['turns', M, C]]]))
})

test('an item whose card was replaced keeps Approve off', async () => {
  const { el, button } = await mount(item({ id: `plugin:turns:${M.slice(0, 8)}:${C.slice(0, 8)}:0` }))
  await vi.waitFor(() => expect(el.textContent).toContain('replaced by a newer one'))
  expect(button('Approve')?.disabled).toBe(true)
})

test('a plugin’s notice: its title and body, the way to the plugin, no Approve and no card fetch', async () => {
  const notice = item({ id: 'plugin-notify:turns:3', title: 'Turn limit reached', detail: '50 turns today' })
  const { el, button } = await mount(notice)
  expect(el.textContent).toContain('Turn limit reached')
  expect(el.textContent).toContain('50 turns today')
  expect(el.textContent).toContain('plugin · turns')
  expect(button('Approve')).toBeUndefined()
  expect(el.querySelector('a[href="/settings/plugins?focus=turns"]')?.textContent).toBe('Open plugin')
  expect(engine.calls.filter(([m]) => m === 'plugin_get')).toEqual([])
})

test('ids: a card and a notice name their plugin; dismissing a card says the plugin stays unapproved', () => {
  expect(pluginRef(item({}))).toEqual({ name: 'turns', notify: false })
  expect(pluginRef(item({ id: 'plugin-notify:turns:3' }))).toEqual({ name: 'turns', notify: true })
  expect(pluginRef(item({ kind: 'home', id: 'plugin:turns' }))).toBeNull()
  expect(dismissHint(item({}), true)).toBe('Dismiss: the plugin stays unapproved until it changes')
  expect(dismissHint(item({ id: 'plugin-notify:turns:3' }), false)).toBe('Hide this item')
})
