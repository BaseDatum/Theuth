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
import type { McpServer } from '@/rpc/bindings/method.mcp_2dserver.output'
import { McpMenu } from './McpMenu'
import { useToolsSheet } from './tools-sheet'

vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn(), message: vi.fn(), success: vi.fn() }) }))

const tool = (name: string, over: Partial<BuiltinTool> = {}): BuiltinTool => ({ name, description: '', risk: 'read_only', direct: true, disabled: false, hidden: false, ...over })
const category = (id: string, label: string, tools: BuiltinTool[]): BuiltinCategory => ({ id, label, description: `${label} tools`, tools })
const categories: BuiltinCategory[] = [
  category('files', 'Files', [tool('read_file'), tool('write_file')]),
  category('shell', 'Shell and jobs', [tool('shell', { hidden: true }), tool('start')]),
  category('web', 'Web', [tool('read_url', { hidden: true }), tool('search_web', { hidden: true })]),
  category('browser', 'Browser', [tool('browser_open', { disabled: true }), tool('browser_click', { disabled: true })]),
]
const fs = {
  name: 'fs',
  connected: true,
  gate: null,
  tools: [{ name: 'ls', description: '', gate: null, risk: 'read_only', action: 'allow', registered: 'ls', hidden: false }],
  repo: null,
  auth: 'none',
  home: null,
  home_name: null,
  home_here: true,
  everywhere: false,
  state: 'running',
  reason: null,
} as McpServer

const calls: unknown[][] = []
const listed: (string | null)[] = []
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({
    list_builtins: async (session: string | null) => {
      listed.push(session)
      return categories
    },
    list_mcp: async () => [fs],
    set_mcp_hidden: async (...args: unknown[]) => {
      calls.push(args)
      return null
    },
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
