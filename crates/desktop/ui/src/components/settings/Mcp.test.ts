import './test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { parse } from 'smol-toml'
import { expect, test, vi } from 'vitest'
import { MCP_SHARED_LINE, mcpActions, mcpConfig } from '@/lib/mcp-config'
import type * as Rpc from '@/lib/rpc'
import type * as Tauri from '@/lib/tauri'
import type { AuditLine as McpAuditLine } from "@/rpc/bindings/method.mcp_2dserver.output"
import type { McpServerConfig } from '@/rpc/bindings/method.mcp_2dserver.input'
import type { McpServerStatus } from "@/rpc/bindings/method.mcp_2dserver.output"
import { Mcp, ready } from './Mcp'

vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn(), message: vi.fn(), success: vi.fn() }) }))
// The app's own engine listens on a port of its own, not a fixed one.
vi.mock('@/lib/tauri', async (original) => ({
  ...await original<typeof Tauri>(),
  connection: async () => ({ url: 'ws://127.0.0.1:18788/', token: 't', data_dir: '/d', owned: true }),
}))

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
  { at: '2026-10-10T12:00:00Z', transport: 'http', client: 'claude-code', tool: 'meeting_list', args_sha256: 'ab'.repeat(32), ok: true, ms: 12 },
  { at: '2026-10-10T11:59:00Z', transport: 'stdio', client: 'codex', tool: 'knowledge_get', args_sha256: 'cd'.repeat(32), ok: false, ms: 3 },
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
  }),
}))

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
  const view = document.createElement('div')
  view.innerHTML = html
  const rows = [...view.querySelectorAll('.settings-table tr')]
  expect(rows.map((row) => row.querySelector('[data-label="Tool"]')?.textContent)).toEqual(['meeting_list', 'knowledge_get'])
  expect(rows.map((row) => row.querySelector('[data-label="Result"]')?.textContent)).toEqual(['ok', 'error'])
  expect(rows.map((row) => row.querySelector('[data-label="Duration"]')?.textContent)).toEqual(['12 ms', '3 ms'])
  expect(html).toContain('Rotate')
  expect(html).toContain(MCP_SHARED_LINE)
  expect(MCP_SHARED_LINE).toContain('read and write')
  expect(html).toContain('Loopback only (127.0.0.1); RPC is on port 18788.')
})
