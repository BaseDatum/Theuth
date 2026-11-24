import { readyConnection } from '@/components/settings/test-engine'
import type * as ConnectionModule from '@/store/connection'
import '@/components/settings/test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement as h } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import type { BuiltinCategory, BuiltinTool } from '@/lib/builtins'
import { useDrafts } from '@/lib/drafts'
import type { McpServer, McpToolInfo } from '@/rpc/bindings/method.mcp_2dserver.output'
import { McpMenu } from './McpMenu'
import { useToolsSheet } from './tools-sheet'

vi.mock('@/lib/notify', () => ({ notify: Object.assign(vi.fn(), { error: vi.fn(), message: vi.fn(), success: vi.fn() }) }))

const tool = (name: string, over: Partial<BuiltinTool> = {}): BuiltinTool => ({ name, description: '', risk: 'read_only', direct: true, disabled: false, hidden: false, ...over })
const category = (id: string, label: string, tools: BuiltinTool[]): BuiltinCategory => ({ id, label, description: `${label} tools`, tools })
const categories: BuiltinCategory[] = [
  category('files', 'Files', [tool('read_file'), tool('write_file')]),
  category('shell', 'Shell and jobs', [tool('shell', { hidden: true }), tool('start')]),
  category('web', 'Web', [tool('read_url', { hidden: true }), tool('search_web', { hidden: true })]),
  category('browser', 'Browser', [tool('browser_open', { disabled: true }), tool('browser_click', { disabled: true })]),
]
const mcpTool = (name: string, hidden = false): McpToolInfo => ({ name, description: '', gate: null, risk: 'read_only', action: 'allow', registered: name, hidden })
const server = (name: string, over: Partial<McpServer> = {}): McpServer => ({
  name,
  connected: true,
  gate: null,
  tools: [mcpTool(`${name}_a`)],
  repo: null,
  auth: 'none',
  home: null,
  home_name: null,
  home_here: true,
  everywhere: false,
  state: 'running',
  reason: null,
  ...over,
})
const fs = server('fs', { tools: [mcpTool('ls')] })
// `cold` is off in mcp/config.yaml for every chat, so never launched.
const cold = server('cold', { connected: false, gate: 'off', tools: [], state: 'off' })
const several = (): McpServer[] => [
  server('alpha'),
  server('bravo', { tools: [mcpTool('bravo_a', true)] }),
  server('charlie', { tools: [mcpTool('charlie_a', true), mcpTool('charlie_b')] }),
  cold,
]

const calls: unknown[][] = []
const listed: (string | null)[] = []
let servers: McpServer[] = [fs]
const power = {
  mcp_start: vi.fn(async (_server: string) => null),
  set_mcp_gate: vi.fn(async (_server: string, _tool: string | null, _gate: string | null) => null),
}
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({
    list_builtins: async (session: string | null) => {
      listed.push(session)
      return categories
    },
    list_mcp: async () => servers,
    set_mcp_hidden: async (...args: unknown[]) => {
      calls.push(args)
      return null
    },
    ...power,
  }),
}))
vi.mock('@/store/connection', async (original) => {
  const m = await original<typeof ConnectionModule>()
  return { ...m, useConnection: readyConnection(m.createConnection()) }
})
// The engine's methods: everything but `unsupported` (an older engine lacks `list_builtins`).
const unsupported = new Set<string>()
vi.mock('@/rpc/capabilities', () => {
  const all = (methods: readonly string[] = []) => methods.every((m) => !unsupported.has(m))
  return { useQuerySupport: all, useSupported: (r: { methods?: readonly string[] }) => all(r.methods) }
})

afterEach(() => {
  calls.length = 0
  listed.length = 0
  servers = [fs]
  power.mcp_start.mockReset().mockImplementation(async () => null)
  power.set_mcp_gate.mockReset().mockImplementation(async () => null)
  unsupported.clear()
  useDrafts.setState({ settings: {}, opening: null })
  useToolsSheet.setState({ session: null, tab: 'builtins' })
})

async function mount(session = 's1') {
  const el = document.body.appendChild(document.createElement('div'))
  const root = createRoot(el)
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  await act(async () => root.render(h(QueryClientProvider, { client: qc }, h(McpMenu, { session }))))
  // "Tools and MCP servers" with builtins, "MCP servers" as before without; no button when neither is supported.
  const trigger = el.querySelector<HTMLButtonElement>('[aria-label="Tools and MCP servers"], [aria-label="MCP servers"]')
  if (trigger) await act(async () => trigger.click())
  const rows = () => [...document.querySelectorAll<HTMLButtonElement>('[role="menuitemcheckbox"]')]
  const row = (label: string) => rows().find((r) => r.textContent?.includes(label))!
  const tab = (name: string) => [...document.querySelectorAll<HTMLElement>('[role="tab"]')].find((t) => t.textContent === name)!
  const pick = (name: string) => act(async () => tab(name).dispatchEvent(new MouseEvent('mousedown', { bubbles: true, button: 0 })))
  const unmount = () => {
    act(() => root.unmount())
    el.remove()
  }
  return { rows, row, tab, pick, unmount }
}

test('the popover has a Builtins and an MCP servers tab; Builtins lists one checkbox per category for this chat', async () => {
  const { rows, row, tab, unmount } = await mount()
  expect([...document.querySelectorAll('[role="tab"]')].map((t) => t.textContent)).toEqual(['Builtins', 'MCP servers'])
  expect(tab('Builtins').getAttribute('aria-selected')).toBe('true')
  await vi.waitFor(() => expect(rows()).toHaveLength(4))
  expect(listed).toEqual(['s1'])
  expect(rows().map((r) => r.getAttribute('aria-checked'))).toEqual(['true', 'mixed', 'false', 'false'])
  expect(row('Files').textContent).toContain('2/2')
  expect(row('Shell and jobs').textContent).toContain('1/2')
  expect(row('Web').textContent).toContain('off')
  unmount()
})

test('an on category is hidden for this chat by its key; an off or mixed one is shown again with its tools', async () => {
  const { row, rows, unmount } = await mount()
  await vi.waitFor(() => expect(rows()).toHaveLength(4))
  await act(async () => row('Files').click())
  await vi.waitFor(() => expect(calls).toEqual([['s1', ['cat:files'], true]]))
  await vi.waitFor(() => expect(row('Web').disabled).toBe(false))
  await act(async () => row('Web').click())
  await vi.waitFor(() => expect(calls[1]).toEqual(['s1', ['cat:web', 'read_url', 'search_web'], false]))
  await vi.waitFor(() => expect(row('Shell and jobs').disabled).toBe(false))
  await act(async () => row('Shell and jobs').click())
  await vi.waitFor(() => expect(calls[2]).toEqual(['s1', ['cat:shell', 'shell', 'start'], false]))
  unmount()
})

test('a category that is off always is greyed with the way to Settings and cannot be switched here', async () => {
  const { row, rows, unmount } = await mount()
  await vi.waitFor(() => expect(rows()).toHaveLength(4))
  const browser = row('Browser')
  expect(browser.disabled).toBe(true)
  expect(browser.title).toBe('Off always (Settings › Tools › Builtins)')
  expect(browser.textContent).toContain('off')
  unmount()
})

test('the new-chat draft keeps its category hides in the draft and calls no engine', async () => {
  const { row, rows, unmount } = await mount('')
  await vi.waitFor(() => expect(rows()).toHaveLength(4))
  expect(listed).toEqual([null])
  await act(async () => row('Files').click())
  await vi.waitFor(() => expect(useDrafts.getState().settings.hidden).toEqual({ 'cat:files': true }))
  await vi.waitFor(() => expect(row('Files').getAttribute('aria-checked')).toBe('false'))
  expect(calls).toEqual([])
  unmount()
})

test('the MCP servers tab still switches a server, and the bottom button opens the sheet on the tab shown', async () => {
  const { row, pick, unmount } = await mount()
  await pick('MCP servers')
  expect(useToolsSheet.getState().tab).toBe('mcp')
  await vi.waitFor(() => expect(row('fs')).toBeDefined())
  await act(async () => row('fs').click())
  await vi.waitFor(() => expect(calls).toEqual([['s1', ['mcp:fs'], true]]))
  const all = [...document.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent?.includes('All tools & servers'))!
  await act(async () => all.click())
  expect(useToolsSheet.getState()).toMatchObject({ session: 's1', tab: 'mcp' })
  expect(document.querySelector('[role="tab"]')).toBeNull()
  unmount()
})

const allTools = () => [...document.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent?.includes('All tools & servers'))

test('an engine without list_builtins gets exactly the old popover: the server list alone, "MCP servers", w-64', async () => {
  unsupported.add('list_builtins')
  const { row, rows, unmount } = await mount()
  await vi.waitFor(() => expect(row('fs')).toBeDefined())
  expect(rows()).toHaveLength(1)
  expect(document.querySelector('[role="tab"]')).toBeNull()
  expect(document.querySelector('[aria-label="MCP servers"]')).not.toBeNull()
  expect(document.querySelector('[role="dialog"]')?.className).toContain('w-64')
  expect(allTools()).toBeDefined()
  expect(listed).toEqual([])
  unmount()
})

test('an engine without the MCP methods gets the Builtins panel alone and no sheet button (no sheet is mounted)', async () => {
  unsupported.add('list_mcp')
  const { rows, unmount } = await mount()
  await vi.waitFor(() => expect(rows()).toHaveLength(4))
  expect(document.querySelector('[role="tab"]')).toBeNull()
  expect(document.querySelector('[aria-label="Tools and MCP servers"]')).not.toBeNull()
  expect(allTools()).toBeUndefined()
  unmount()
})

test('an engine with neither shows no tools button at all', async () => {
  unsupported.add('list_mcp')
  unsupported.add('list_builtins')
  const { rows, unmount } = await mount()
  expect(document.querySelector('[aria-label="Tools and MCP servers"], [aria-label="MCP servers"]')).toBeNull()
  expect(rows()).toHaveLength(0)
  unmount()
})

// The MCP servers tab's rows: the checkbox (this chat) by its label, the switch (every chat) by its aria-label.
const box = (name: string) => [...document.querySelectorAll<HTMLButtonElement>('[role="menuitemcheckbox"]')].find((r) => r.textContent?.startsWith(name))!
const sw = (name: string) => document.querySelector<HTMLButtonElement>(`[role="switch"][aria-label="${name} for every chat"]`)
const line = (name: string) => box(name).parentElement!.parentElement!
const click = (el: Element) => act(async () => void (el as HTMLElement).click())

async function mountServers(list: McpServer[] = several()) {
  servers = list
  const m = await mount()
  await m.pick('MCP servers')
  await vi.waitFor(() => expect(box(list.at(-1)!.name)).toBeDefined())
  return m
}

test('checkbox: on, off, mixed in this chat; unchecked and disabled while the server is off for every chat', async () => {
  const { unmount } = await mountServers()
  expect(box('alpha').getAttribute('aria-checked')).toBe('true')
  expect(box('bravo').getAttribute('aria-checked')).toBe('false')
  expect(box('charlie').getAttribute('aria-checked')).toBe('mixed')
  expect(box('cold').getAttribute('aria-checked')).toBe('false')
  expect(box('cold').disabled).toBe(true)
  expect(line('cold').textContent).toContain('off for every chat')
  for (const n of ['alpha', 'bravo', 'charlie']) expect(sw(n)!.getAttribute('aria-checked')).toBe('true')
  expect(sw('cold')!.getAttribute('aria-checked')).toBe('false')
  unmount()
})

test('a server that is not running has its checkbox disabled and says why', async () => {
  const { unmount } = await mountServers([server('down', { connected: false, tools: [], state: 'error', reason: 'boom' })])
  expect(box('down').disabled).toBe(true)
  expect(box('down').getAttribute('aria-checked')).toBe('false')
  expect(line('down').textContent).toContain('boom')
  expect(sw('down')!.getAttribute('aria-checked')).toBe('false')
  unmount()
})

test('the checkbox hides or shows the server in this chat only (set_mcp_hidden), never the harness-wide gate', async () => {
  const { unmount } = await mountServers()
  await click(box('alpha'))
  await vi.waitFor(() => expect(calls.at(-1)).toEqual(['s1', ['mcp:alpha'], true]))
  await vi.waitFor(() => expect(box('bravo').disabled).toBe(false))
  await click(box('bravo'))
  await vi.waitFor(() => expect(calls.at(-1)).toEqual(['s1', ['mcp:bravo', 'bravo_a'], false]))
  await vi.waitFor(() => expect(box('charlie').disabled).toBe(false))
  await click(box('charlie'))
  // A mixed server is shown whole again: its entry and every per-tool hide go.
  await vi.waitFor(() => expect(calls.at(-1)).toEqual(['s1', ['mcp:charlie', 'charlie_a', 'charlie_b'], false]))
  expect(power.set_mcp_gate).not.toHaveBeenCalled()
  expect(power.mcp_start).not.toHaveBeenCalled()
  unmount()
})

test('switch on: clears the off gate, starts the server (starting…, checkbox disabled), then it is checked here', async () => {
  const start = Promise.withResolvers<null>()
  power.mcp_start.mockImplementation(() => start.promise)
  const { unmount } = await mountServers()
  await click(sw('cold')!)
  await vi.waitFor(() => expect(power.mcp_start).toHaveBeenCalledWith('cold'))
  expect(power.set_mcp_gate.mock.calls).toEqual([['cold', null, null]])
  expect(line('cold').textContent).toContain('starting…')
  expect(sw('cold')!.getAttribute('aria-checked')).toBe('true')
  expect(sw('cold')!.disabled).toBe(true)
  expect(box('cold').disabled).toBe(true)
  servers = servers.map((s) => (s.name === 'cold' ? server('cold') : s))
  await act(async () => start.resolve(null))
  await vi.waitFor(() => expect(box('cold').disabled).toBe(false))
  expect(box('cold').getAttribute('aria-checked')).toBe('true')
  expect(line('cold').textContent).not.toContain('starting…')
  expect(calls).toEqual([])
  unmount()
})

test('switch off: off for every chat (stopping…); no start, no per-chat hide', async () => {
  const stop = Promise.withResolvers<null>()
  power.set_mcp_gate.mockImplementation(() => stop.promise)
  const { unmount } = await mountServers()
  await click(sw('alpha')!)
  await vi.waitFor(() => expect(power.set_mcp_gate.mock.calls).toEqual([['alpha', null, 'off']]))
  expect(line('alpha').textContent).toContain('stopping…')
  expect(sw('alpha')!.getAttribute('aria-checked')).toBe('false')
  expect(box('alpha').disabled).toBe(true)
  await act(async () => stop.resolve(null))
  await vi.waitFor(() => expect(line('alpha').textContent).not.toContain('stopping…'))
  expect(power.mcp_start).not.toHaveBeenCalled()
  expect(calls).toEqual([])
  unmount()
})

test('switch on keeps a server-wide risk gate: a failed server with one only starts', async () => {
  const { unmount } = await mountServers([server('risky', { connected: false, gate: 'read_only', tools: [], state: 'error', reason: 'boom' })])
  await click(sw('risky')!)
  await vi.waitFor(() => expect(power.mcp_start).toHaveBeenCalledWith('risky'))
  expect(power.set_mcp_gate).not.toHaveBeenCalled()
  unmount()
})

test('a failed start restores the off gate, shows the error (at most 120 chars) and retries', async () => {
  const long = `cold: connect failed: ${'x'.repeat(200)}`
  power.mcp_start.mockRejectedValueOnce(new Error(long))
  const { unmount } = await mountServers()
  await click(sw('cold')!)
  const alert = () => line('cold').querySelector('[role="alert"]')
  await vi.waitFor(() => expect(alert()?.textContent).toBe(`${long.slice(0, 120)}…`))
  expect(alert()?.getAttribute('title')).toBe(long)
  expect(power.set_mcp_gate.mock.calls).toEqual([
    ['cold', null, null],
    ['cold', null, 'off'],
  ])
  expect(sw('cold')!.getAttribute('aria-checked')).toBe('false')
  expect(sw('cold')!.disabled).toBe(false)
  expect(line('cold').textContent).not.toContain('starting…')
  const retry = [...line('cold').querySelectorAll('button')].find((b) => b.textContent === 'Retry')!
  await click(retry)
  await vi.waitFor(() => expect(power.mcp_start).toHaveBeenCalledTimes(2))
  await vi.waitFor(() => expect(alert()).toBeNull())
  unmount()
})

test('an engine without mcp_start keeps only the off direction; one without set_mcp_gate has no switch', async () => {
  unsupported.add('mcp_start')
  const first = await mountServers()
  expect(sw('alpha')!.disabled).toBe(false)
  expect(sw('cold')!.disabled).toBe(true)
  await click(sw('alpha')!)
  await vi.waitFor(() => expect(power.set_mcp_gate).toHaveBeenCalledWith('alpha', null, 'off'))
  first.unmount()
  unsupported.add('set_mcp_gate')
  const second = await mountServers()
  expect(document.querySelector('[role="switch"]')).toBeNull()
  expect(box('alpha').getAttribute('aria-checked')).toBe('true')
  second.unmount()
})
