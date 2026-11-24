import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import type * as SwitchUi from '@/components/ui/switch'
import type * as Shared from './shared'
import type * as Connection from '@/store/connection'
import type { ConfigFlagKey } from '@/rpc/bindings/method.rpc_2dcompose.input'
import { Loop } from './Loop'
import { agent } from './categories/agent'

// What each control was given, by its aria-label, so a test can fire it without a DOM (the Memory.test.ts pattern).
type Fire = { onCheckedChange?(v: boolean): void; action?: { mutate(n: number): void } }
const controls = vi.hoisted(() => ({}) as Record<string, Fire>)
vi.mock('./shared', async (original) => {
  const m = await original<typeof Shared>()
  return { ...m, Count: (p: Parameters<typeof m.Count>[0]) => ((controls[p['aria-label'] ?? ''] = p), createElement(m.Count, p)) }
})
vi.mock('@/components/ui/switch', async (original) => {
  const m = await original<typeof SwitchUi>()
  return { ...m, Switch: (p: Parameters<typeof m.Switch>[0]) => ((controls[p['aria-label'] ?? ''] = p as Fire), createElement(m.Switch, p)) }
})

// The connection info the page reads (a server render only sees zustand's initial state, so the test overrides `info` here).
const conn = vi.hoisted(() => ({ info: null as object | null }))
vi.mock('@/store/connection', async (original) => {
  const m = await original<typeof Connection>()
  const live = m.useConnection
  const hook = (selector: (s: object) => unknown = (s) => s) => selector(conn.info ? { ...live.getState(), info: conn.info } : live.getState())
  for (const k of ['getState', 'getInitialState', 'setState', 'subscribe'] as const) Object.defineProperty(hook, k, { get: () => live[k] })
  return { ...m, useConnection: hook }
})

// config.yaml as the engine reports it: peeks and status on, automatic titles off; all three apply without a restart.
const saved: Partial<Record<ConfigFlagKey, boolean>> = { 'knowledge.peek': true, 'agent.status_tool': true, 'agent.auto_title': false, 'agent.suggestions': true }
const flag = (key: ConfigFlagKey) => ({ key, enabled: saved[key] ?? true, running: saved[key] ?? true, restart_required: false })
const rpc = vi.hoisted(() => ({
  set_peek_threshold: vi.fn(async (_: number) => null),
  set_hint_limit: vi.fn(async (_: number) => null),
  set_status_every: vi.fn(async (_: number) => null),
  config_flag_set: vi.fn(async (_key: string, _on: boolean) => ({})),
}))
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({
    ...rpc,
    config_flag_get: async (key: ConfigFlagKey) => flag(key),
    config_flag_set: async (key: ConfigFlagKey, on: boolean) => (rpc.config_flag_set(key, on), flag(key)),
    info: async () => conn.info,
  }),
}))

conn.info = { stream_idle_secs: 120, max_salvages: 2, max_tool_rounds: 32, subagent_timeout_secs: 600, pty_idle_secs: 5, schedule_run_timeout_secs: 1800, peek_threshold: 0.65, hint_limit: 3, status_every: 8 }

async function render(): Promise<string> {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const paint = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(Loop) as ReactNode }))
  paint()
  for (const query of qc.getQueryCache().findAll()) await qc.fetchQuery({ ...query.options, queryKey: query.queryKey })
  return paint()
}

const field = (html: string, label: string) => html.match(new RegExp(`<input[^>]*aria-label="${label}"[^>]*>`))?.[0] ?? ''

test('212 §14: the Memory and status section shows its seven rows with the engine values', async () => {
  const html = await render()
  expect(html).toContain('Memory and status')
  for (const label of ['Memory peeks', 'Peek relevance threshold', 'Memory hints per nudge', 'Status updates', 'Status reminder every N tool calls', 'Automatic titles', 'Next-step suggestions']) {
    expect(html).toContain(`>${label}<`)
  }
  expect(html).toContain('Off removes peek_memory and its mention from the hints.')
  expect(html).toContain('0 turns the reminder off; the tool stays available.')
  expect(html).toMatch(/aria-checked="true"[^>]*aria-label="Memory peeks"/)
  expect(html).toMatch(/aria-checked="true"[^>]*aria-label="Status updates"/)
  expect(html).toMatch(/aria-checked="false"[^>]*aria-label="Automatic titles"/)
  expect(html).toMatch(/aria-checked="true"[^>]*aria-label="Next-step suggestions"/)
  expect(field(html, 'Peek relevance threshold')).toMatch(/min="0.05" max="0.95" step="0.05"[^>]*value="0.65"/)
  expect(field(html, 'Memory hints per nudge')).toMatch(/min="0"[^>]*value="3"/)
  expect(field(html, 'Status reminder every N tool calls')).toMatch(/min="0"[^>]*value="8"/)
  // No restart: the four switches apply to new sessions.
  expect(html).not.toContain('Applies after theuth restarts.')
  // The tab needs every method the section calls.
  const methods = agent.tabs.find((t) => t.id === 'loop')?.required?.methods ?? []
  for (const m of ['config_flag_get', 'config_flag_set', 'set_peek_threshold', 'set_hint_limit', 'set_status_every']) expect(methods).toContain(m)
})

test('212 §15: each row writes through its own RPC', async () => {
  await render()
  const c = (label: string) => controls[label] ?? expect.fail(`no control "${label}"`)
  c('Peek relevance threshold').action?.mutate(0.8)
  c('Memory hints per nudge').action?.mutate(0)
  c('Status reminder every N tool calls').action?.mutate(12)
  c('Memory peeks').onCheckedChange?.(false)
  c('Status updates').onCheckedChange?.(false)
  c('Automatic titles').onCheckedChange?.(true)
  c('Next-step suggestions').onCheckedChange?.(false)
  await vi.waitFor(() => expect(rpc.config_flag_set).toHaveBeenCalledTimes(4))
  expect(rpc.set_peek_threshold).toHaveBeenCalledWith(0.8)
  expect(rpc.set_hint_limit).toHaveBeenCalledWith(0)
  expect(rpc.set_status_every).toHaveBeenCalledWith(12)
  expect(rpc.config_flag_set.mock.calls).toEqual([
    ['knowledge.peek', false],
    ['agent.status_tool', false],
    ['agent.auto_title', true],
    ['agent.suggestions', false],
  ])
})
