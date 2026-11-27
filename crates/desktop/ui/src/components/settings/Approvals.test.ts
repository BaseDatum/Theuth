import { readyConnection } from '@/components/settings/test-engine'
import type * as ConnectionModule from '@/store/connection'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import { Approvals, entryScope } from './Approvals'

/** The owner's `tools.always_ask` of 2026-11-26: the Bitwarden connection's seeded keys and `secret_delete`. */
const OWNER = ['secret:bw/vw:delete', 'secret:bw/vw:read', 'secret:bw/vw:send', 'secret:bw/vw:write', 'secret_delete']
let global: string[] = OWNER

vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({
    get_always_ask: async () => ({ global, session: [], inherited: [] }),
    list_builtins: async () => [],
    list_mcp: async () => [],
    list_secret_connections: async () => [],
    set_approvals: async () => undefined,
    set_always_ask: async () => undefined,
  }),
}))
vi.mock('@/store/connection', async (original) => {
  const m = await original<typeof ConnectionModule>()
  const store = m.createConnection()
  // Approvals off harness-wide, like the owner's `tools.approvals: false`.
  store.setState({ info: { approvals: { global: false } } } as never)
  return { ...m, useConnection: readyConnection(store) }
})

async function render(): Promise<string> {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const paint = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(Approvals) as ReactNode }))
  for (let pass = 0; pass < 3; pass++) {
    paint()
    for (const query of qc.getQueryCache().findAll()) {
      if ('enabled' in query.options && query.options.enabled === false) continue
      await qc.fetchQuery({ ...query.options, queryKey: query.queryKey })
    }
  }
  return paint()
}

test('the always-ask list names each entry, folds a connection into one summary line and says approvals are off', async () => {
  global = OWNER
  const html = await render()
  expect(html).toContain('Always ask is on for 5 entries: vw (Bitwarden): read, write, send, delete; secret_delete.')
  for (const d of ['read', 'write', 'send', 'delete']) {
    expect(html).toContain(`vw (Bitwarden) · ${d}`)
    expect(html).toContain(`aria-label="Stop always asking for secret:bw/vw:${d}"`)
  }
  expect(html).toContain('aria-label="Stop always asking for secret_delete"')
  expect(html).toContain('shell commands with a {{secret:…}} reference included.')
  expect(html).toContain('The secret_delete tool.')
  expect(html).toContain('Off: tools run without asking, except the always-ask entries below and the mandatory gates.')
  expect(html).toContain('tools.always_ask')
})

test('an empty list says so and has no summary', async () => {
  global = []
  const html = await render()
  expect(html).toContain('No tool always asks.')
  expect(html).not.toContain('data-always-ask-summary')
})

test('each entry kind says what it covers', () => {
  expect(entryScope('secret:bw/vw:read')).toBe('Any tool that reads a secret through this connection, shell commands with a {{secret:…}} reference included.')
  expect(entryScope('mcp:k8s')).toBe('Every tool of the MCP server k8s.')
  expect(entryScope('shell')).toBe('The shell tool.')
})
