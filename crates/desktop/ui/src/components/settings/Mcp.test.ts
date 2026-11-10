import './test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { renderToStaticMarkup } from 'react-dom/server'
import { parse } from 'smol-toml'
import { expect, test, vi } from 'vitest'
import { engines } from '@/components/tasks/home'
import { MCP_SHARED_LINE, mcpActions, mcpConfig } from '@/lib/mcp-config'
import { homeActions, homeChip, stateText } from '@/lib/mcp-home'
import type * as Rpc from '@/lib/rpc'
import { queryKeys } from '@/lib/rpc'
import type { McpAuditLine } from '@/rpc/bindings/McpAuditLine'
import type { McpServer } from '@/rpc/bindings/McpServer'
import type { McpServerConfig } from '@/rpc/bindings/McpServerConfig'
import type { McpServerStatus } from '@/rpc/bindings/McpServerStatus'
import type { MeshStatus } from '@/rpc/bindings/MeshStatus'
import { Mcp, ready } from './Mcp'

vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn(), message: vi.fn(), success: vi.fn() }) }))

const TOKEN = 'a'.repeat(64)
const status: McpServerStatus = {
  enabled: true,
  port: 7788,
  running: true,
  url: 'http://127.0.0.1:7788/mcp',
  token: TOKEN,
  binary: '/Applications/theuth.app/Contents/MacOS/t',
}
const audit: McpAuditLine[] = [
  { at: '2026-10-10T12:00:00Z', transport: 'http', client: 'claude-code', tool: 'meeting_list', args_sha256: 'ab'.repeat(32), ok: true, ms: 12n },
  { at: '2026-10-10T11:59:00Z', transport: 'stdio', client: 'codex', tool: 'knowledge_get', args_sha256: 'cd'.repeat(32), ok: false, ms: 3n },
]

// 187: this is xeus; mikrotik was added on 333.
const mesh = {
  self_id: 'id-xeus',
  self_name: 'xeus',
  peers: [{ id: 'id-333', name: '333', reachable: true, last_seen_ms: 0n }],
} as unknown as MeshStatus
const MACHINES = engines(mesh)
const server = (over: Partial<McpServer> = {}): McpServer => ({
  name: 'mikrotik',
  connected: false,
  gate: null,
  tools: [],
  repo: null,
  auth: 'none',
  home: 'id-333',
  home_name: '333',
  home_here: false,
  everywhere: false,
  state: 'elsewhere',
  reason: null,
  ...over,
})
const servers: McpServer[] = [
  server(),
  server({ name: 'grafana', home: 'id-xeus', home_name: 'xeus', home_here: true, state: 'running', connected: true }),
  server({ name: 'portable', everywhere: true, state: 'unavailable_here', reason: 'portable-mcp is not installed on xeus' }),
]

const calls: [string, unknown][] = []
vi.mock('@/lib/rpc', async (original) => ({
  ...await original<typeof Rpc>(),
  useApi: () => ({
    mcp_server_get: async () => {
      calls.push(['mcp_server_get', null])
      return status
    },
    mcp_server_audit: async (limit: number) => {
      calls.push(['mcp_server_audit', limit])
      return audit
    },
    list_mcp: async () => servers,
    mesh_status: async () => mesh,
    set_mcp_home: async (name: string, action: string) => {
      calls.push(['set_mcp_home', [name, action]])
      return null
    },
  }),
}))

test('187: each row names its home and what it does on this machine', () => {
  expect([homeChip(server(), MACHINES), stateText(server(), MACHINES)]).toEqual(['on 333', 'not running here (home is 333)'])
  // The mesh's current name wins; the stamped one stands in for an engine the mesh does not know.
  expect(homeChip(server({ home: 'id-gone', home_name: 'old-box' }), MACHINES)).toBe('on old-box')
  expect(homeChip(server({ home: null, home_name: null }), MACHINES)).toBe('home not claimed yet')
  expect(homeChip(server({ home_here: true }), MACHINES)).toBe('on xeus')
  const states = (['running', 'connecting', 'off_here', 'off', 'unavailable_here', 'error'] as const).map((state) => stateText(server({ state }), MACHINES))
  expect(states).toEqual(['running', 'starting…', 'turned off here', 'off', 'not installed here', 'error'])
})

test('187: actions follow the row: Run everywhere toggles, Only on <home>, Move home here, Turn off / on here', () => {
  const acts = (s: McpServer, machines = MACHINES) => homeActions(s, machines).map((a) => [a.action, a.label, a.pressed ?? null])
  // Homed elsewhere and not running here: nothing to turn off.
  expect(acts(server())).toEqual([
    ['run_everywhere', 'Run everywhere', false],
    ['move_here', 'Move home here', null],
  ])
  expect(acts(server({ everywhere: true, state: 'unavailable_here' }))).toEqual([
    ['only_home', 'Run everywhere', true],
    ['only_home', 'Only on 333', null],
    ['move_here', 'Move home here', null],
    ['off_here', 'Turn off here', null],
  ])
  // Homed here and running: nothing to move; turned off here: turn it back on.
  expect(acts(server({ home_here: true, state: 'running' }))).toEqual([
    ['run_everywhere', 'Run everywhere', false],
    ['off_here', 'Turn off here', null],
  ])
  expect(acts(server({ home_here: true, state: 'off_here' }))).toEqual([
    ['run_everywhere', 'Run everywhere', false],
    ['on_here', 'Turn on here', null],
  ])
  // Alone (unpaired, or a reset identity): only a home on a machine nobody knows can be taken.
  const alone = engines({ ...mesh, peers: [] })
  expect(acts(server(), alone)).toEqual([['move_here', 'Move home here', null]])
  expect(acts(server({ home: null }), alone)).toEqual([['move_here', 'Move home here', null]])
  expect(acts(server({ home_here: true, state: 'running' }), alone)).toEqual([])
})

test('187: no home chip until the mesh answers (no "on this machine" flicker)', () => {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  qc.setQueryData(queryKeys.mcp(null), servers)
  const html = renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(Mcp) as ReactNode }))
  expect(html).toContain('mikrotik')
  expect(html).not.toMatch(/>on (xeus|333|this machine)</)
  expect(html).not.toContain('Move home here')
})

test('187: the pane lists every server with its home and state; a button sets the home of its own row', async () => {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const el = document.createElement('div')
  document.body.append(el)
  const root = createRoot(el)
  await act(async () => root.render(createElement(QueryClientProvider, { client: qc, children: createElement(Mcp) as ReactNode })))
  await vi.waitFor(() => expect(el.textContent).toContain('portable-mcp is not installed on xeus'))
  const html = el.innerHTML
  for (const t of ['on 333', 'not running here (home is 333)', 'on xeus', 'running', 'not installed here'])
    expect(html).toContain(t)
  const button = (label: string) => [...el.querySelectorAll('button')].filter((b) => b.textContent === label)
  expect(button('Move home here')).toHaveLength(2)
  expect(button('Run everywhere')[0]?.className).toContain('pointer-coarse:min-h-11')
  const homes = () => calls.filter(([m]) => m === 'set_mcp_home')
  await act(async () => button('Move home here')[0]!.click())
  await vi.waitFor(() => expect(homes()).toEqual([['set_mcp_home', ['mikrotik', 'move_here']]]))
  await vi.waitFor(() => expect(button('Only on 333')[0]?.disabled).toBe(false))
  await act(async () => button('Only on 333')[0]!.click())
  await vi.waitFor(() => expect(homes()).toEqual([
    ['set_mcp_home', ['mikrotik', 'move_here']],
    ['set_mcp_home', ['portable', 'only_home']],
  ]))
  act(() => root.unmount())
  el.remove()
})

test('http clients copy exactly the §12.6 block with the token filled in and the port respected', () => {
  const expected = {
    mcpServers: { theuth: { type: 'http', url: 'http://127.0.0.1:7788/mcp', headers: { Authorization: `Bearer ${TOKEN}` } } },
  }
  expect(JSON.parse(mcpConfig('claude', status, '/d'))).toEqual(expected)
  expect(mcpConfig('cursor', status, '/d')).toBe(mcpConfig('claude', status, '/d'))
  const other = JSON.parse(mcpConfig('claude', { ...status, url: 'http://127.0.0.1:9000/mcp' }, '/d'))
  expect(other.mcpServers.theuth.url).toBe('http://127.0.0.1:9000/mcp')
  expect(JSON.parse(mcpConfig('claude', { ...status, token: null }, '/d')).mcpServers.theuth.headers.Authorization).toBe('Bearer <token>')
})

test('Codex gets config.toml: [mcp_servers.theuth] with url and the Authorization header', () => {
  const text = mcpConfig('codex', status, '/d')
  expect(text).toContain('[mcp_servers.theuth]')
  expect(parse(text)).toEqual({ mcp_servers: { theuth: { url: 'http://127.0.0.1:7788/mcp', http_headers: { Authorization: `Bearer ${TOKEN}` } } } })
})

test('stdio copies the §12.6 command block with this engine binary and data dir', () => {
  expect(JSON.parse(mcpConfig('stdio', status, '/Users/x/.theuth'))).toEqual({
    mcpServers: { theuth: { command: '/Applications/theuth.app/Contents/MacOS/t', args: ['--data-dir', '/Users/x/.theuth', 'mcp'] } },
  })
  // No binary reported → `t` from PATH.
  expect(JSON.parse(mcpConfig('stdio', { ...status, binary: null }, '/d')).mcpServers.theuth.command).toBe('t')
})

test('toggle, port and rotate call the RPCs with the frozen shapes', async () => {
  const seen: [string, unknown][] = []
  const acts = mcpActions({
    mcp_server_set: async (c: McpServerConfig) => {
      seen.push(['mcp_server_set', c])
      return { ...status, ...c }
    },
    mcp_server_rotate_token: async () => {
      seen.push(['mcp_server_rotate_token', null])
      return null
    },
  })
  await acts.setEnabled(status, false)
  await acts.setPort(status, 7790)
  await acts.rotate()
  expect(seen).toEqual([
    ['mcp_server_set', { enabled: false, port: 7788 }],
    ['mcp_server_set', { enabled: true, port: 7790 }],
    ['mcp_server_rotate_token', null],
  ])
})

test('ready, and the pane renders status, masked token, the last-20 audit tail and the shared line', async () => {
  expect(ready).toBe(true)
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const render = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(Mcp) as ReactNode }))
  render()
  for (const query of qc.getQueryCache().findAll()) await qc.fetchQuery({ ...query.options, queryKey: query.queryKey })
  const html = render()
  expect(calls).toContainEqual(['mcp_server_get', null])
  expect(calls).toContainEqual(['mcp_server_audit', 20])
  expect(html).toContain('running on http://127.0.0.1:7788/mcp')
  expect(html).toContain('aaaaaaaa…')
  expect(html).not.toContain(TOKEN)
  expect(html).toContain('meeting_list')
  expect(html).toContain('knowledge_get')
  expect(html).toContain('Rotate')
  expect(html).toContain(MCP_SHARED_LINE)
  expect(MCP_SHARED_LINE).toContain('read and write')
})
