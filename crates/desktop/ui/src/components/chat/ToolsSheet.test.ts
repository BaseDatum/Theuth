import { readyConnection } from '@/components/settings/test-engine'
import { useConnection } from '@/store/connection'
import type * as ConnectionModule from '@/store/connection'
import '@/components/settings/test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement as h, Fragment } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import { useDrafts } from '@/lib/drafts'
import type { BuiltinCategory } from '@/lib/builtins'
import type { McpServer } from "@/rpc/bindings/method.mcp_2dserver.output"
import { SessionMenu } from './SessionMenu'
import { ToolsSheet } from './ToolsSheet'
import { useToolsSheet } from './tools-sheet'

vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn(), message: vi.fn(), success: vi.fn() }) }))
vi.mock('@tanstack/react-router', async (original) => ({
  ...(await original<object>()),
  useNavigate: () => vi.fn(),
  useRouterState: ({ select }: { select: (s: { location: { pathname: string } }) => unknown }) => select({ location: { pathname: '/c/s1' } }),
}))

const listed: (string | null)[] = []
const fs = { name: 'fs', connected: true, gate: null, tools: [], repo: null, auth: 'none', home: null, home_name: null, home_here: true, everywhere: false, state: 'running', reason: null } as McpServer
const files: BuiltinCategory = {
  id: 'files',
  label: 'Files',
  description: 'Read, search and edit files on this machine',
  tools: [{ name: 'read_file', description: '', risk: 'read_only', direct: true, disabled: false, hidden: false }],
}
// Every other reader answers an empty list; the menu and the sheet only need these.
const answers: Record<string, (...a: never[]) => unknown> = {
  mesh_status: () => ({ self_id: 'me', self_name: 'me', peers: [] }),
  get_approvals: () => ({ global: true, session: true }),
  get_communication: () => ({ global: true, effective: true, scope: 'owner', alias: null }),
  mcp_status: () => ({ connecting: [], connected: [], failed: [], homes_problem: null }),
  list_mcp: (session: string | null) => {
    listed.push(session)
    return [fs]
  },
  list_builtins: () => [files],
}
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => new Proxy({}, { get: (_, k: string) => async (...a: never[]) => (answers[k] ?? (() => []))(...a) }),
}))
vi.mock('@/store/connection', async (original) => {
  const m = await original<typeof ConnectionModule>()
  return { ...m, useConnection: readyConnection(m.createConnection()) }
})

type Discovery = { document: { methods: { name: string }[] }; unavailable: Set<string> }
const connection = useConnection as unknown as { getState(): { discovery: Discovery }; setState(p: object): void }
/** The contract's discovery before a test overrode it; restored after each test. */
let contract: Discovery | null = null
/** Whether the engine answers `list_builtins` (206), whatever the exported contract of this checkout lists. */
function builtins(on: boolean) {
  const d = connection.getState().discovery
  contract ??= d
  const methods = d.document.methods.filter((m) => m.name !== 'list_builtins' && m.name !== 'set_builtins_disabled')
  const added = on ? [{ name: 'list_builtins' }, { name: 'set_builtins_disabled' }] : []
  connection.setState({ discovery: { ...d, document: { ...d.document, methods: [...methods, ...added] } } })
}
const ANSWERS = new Set(Object.keys(answers))
afterEach(() => {
  if (contract) connection.setState({ discovery: contract })
  contract = null
  for (const k of Object.keys(answers)) if (!ANSWERS.has(k)) delete answers[k]
  listed.length = 0
  useToolsSheet.setState({ session: null, tab: 'builtins' })
  useDrafts.setState({ settings: {}, opening: null })
})
const dialog = () => document.querySelector('[role="dialog"]')
const tabTrigger = (label: string) => [...document.querySelectorAll<HTMLElement>('[role="tab"]')].find((t) => t.textContent === label)!

test('⋯ › Tools… opens the Tools sheet of that chat only, on Builtins; its MCP servers tab lists the servers for that chat', async () => {
  builtins(true)
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  // Two panes: s1 (its menu and sheet) and s2 (its sheet).
  await act(async () =>
    root.render(h(QueryClientProvider, { client: qc }, h(Fragment, null, h(SessionMenu, { session: 's1' }), h(ToolsSheet, { session: 's1' }), h(ToolsSheet, { session: 's2' })))),
  )
  const trigger = host.querySelector<HTMLButtonElement>('[aria-label="Session menu"]')!
  await act(async () => trigger.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })))
  const item = await vi.waitFor(() => {
    const found = [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((m) => m.textContent === 'Tools…')
    expect(found).toBeDefined()
    return found!
  })
  await act(async () => item.click())
  expect(useToolsSheet.getState().session).toBe('s1')
  await vi.waitFor(() => {
    const dialogs = document.querySelectorAll('[role="dialog"]')
    expect(dialogs).toHaveLength(1)
    expect(dialogs[0]!.textContent).toContain('built-in tools and MCP servers')
    expect(dialogs[0]!.textContent).toContain('Files')
  })
  expect(listed).toEqual([])
  // Radix tabs switch on mousedown.
  await act(async () => tabTrigger('MCP servers').dispatchEvent(new MouseEvent('mousedown', { bubbles: true, button: 0 })))
  await vi.waitFor(() => expect(dialog()?.textContent).toContain('fs'))
  expect(useToolsSheet.getState().tab).toBe('mcp')
  expect(listed).toEqual(['s1'])
  await act(async () => root.unmount())
  host.remove()
})

test('the draft menu edits pending messaging and approvals and opens its own Tools sheet without creating a chat', async () => {
  builtins(true)
  // The sheet keeps the tab it was last on: Tools… opens it on MCP servers here.
  useToolsSheet.setState({ tab: 'mcp' })
  const setters = { set_approvals: vi.fn(), set_communication: vi.fn(), create_session: vi.fn(), new_session: vi.fn() }
  Object.assign(answers, setters)
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  await act(async () => root.render(h(QueryClientProvider, { client: qc }, h(Fragment, null, h(SessionMenu, { session: '' }), h(ToolsSheet, { session: '' })))))
  const trigger = host.querySelector<HTMLButtonElement>('[aria-label="Session menu"]')!
  const show = async () => act(async () => trigger.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })))
  await show()
  const messaging = await vi.waitFor(() => {
    const item = [...document.querySelectorAll<HTMLElement>('[role="menuitemcheckbox"]')].find((m) => m.textContent === 'Agent messaging')!
    expect(item.getAttribute('aria-disabled')).not.toBe('true')
    return item
  })
  await act(async () => messaging.click())
  expect(useDrafts.getState().settings.communication).toBe(false)
  await show()
  const approvals = [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((m) => m.textContent?.includes('Approvals:'))!
  await act(async () => approvals.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true })))
  const off = await vi.waitFor(() => {
    const item = [...document.querySelectorAll<HTMLElement>('[role="menuitemradio"]')].find((m) => m.textContent === 'Off')
    expect(item).toBeDefined()
    return item!
  })
  await act(async () => off.click())
  expect(useDrafts.getState().settings.approvals).toBe(false)
  await show()
  expect(document.querySelector('[role="menuitemcheckbox"]')?.getAttribute('aria-checked')).toBe('false')
  const tools = [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((m) => m.textContent === 'Tools…')!
  await act(async () => tools.click())
  await vi.waitFor(() => expect(document.querySelector('[role="dialog"]')?.textContent).toContain('fs'))
  expect(listed).toEqual([null])
  expect(useToolsSheet.getState().tab).toBe('mcp')
  for (const setter of Object.values(setters)) expect(setter).not.toHaveBeenCalled()
  await act(async () => root.unmount())
  host.remove()
})

test('show(session, tab) opens on that tab; an engine without list_builtins gets the MCP list alone, as before', async () => {
  builtins(true)
  useToolsSheet.setState({ tab: 'mcp' })
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  await act(async () => root.render(h(QueryClientProvider, { client: qc }, h(ToolsSheet, { session: 's1' }))))
  await act(async () => useToolsSheet.getState().show('s1', 'builtins'))
  await vi.waitFor(() => expect(dialog()?.textContent).toContain('Files'))
  expect(tabTrigger('Builtins').getAttribute('aria-selected')).toBe('true')
  await act(async () => builtins(false))
  await vi.waitFor(() => expect(dialog()?.textContent).toContain('fs'))
  expect(document.querySelector('[role="tablist"]')).toBeNull()
  expect(dialog()?.textContent).toContain('The MCP servers and tools this chat')
  await act(async () => useToolsSheet.getState().hide())
  await act(async () => root.unmount())
  host.remove()
})
