import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { beforeEach, expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import type * as SwitchUi from '@/components/ui/switch'
import type * as SelectUi from '@/components/ui/select'
import type * as Shared from './shared'
import type * as Connection from '@/store/connection'
import { AutoArchive, splitAfter } from './AutoArchive'

// What each control was given, so a test can fire it without a DOM (the Loop.test.ts pattern).
type Fire = { onCheckedChange?(v: boolean): void; onValueChange?(v: string): void; action?: { mutate(n: number): void }; disabled?: boolean; value?: unknown }
const controls = vi.hoisted(() => ({}) as Record<string, Fire>)
vi.mock('./shared', async (original) => {
  const m = await original<typeof Shared>()
  return { ...m, Count: (p: Parameters<typeof m.Count>[0]) => ((controls[p['aria-label'] ?? ''] = p), createElement(m.Count, p)) }
})
vi.mock('@/components/ui/switch', async (original) => {
  const m = await original<typeof SwitchUi>()
  return { ...m, Switch: (p: Parameters<typeof m.Switch>[0]) => ((controls[p['aria-label'] ?? ''] = p as Fire), createElement(m.Switch, p)) }
})
vi.mock('@/components/ui/select', async (original) => {
  const m = await original<typeof SelectUi>()
  return { ...m, Select: (p: Parameters<typeof m.Select>[0]) => ((controls.unit = p as Fire), createElement(m.Select, p)) }
})

const conn = vi.hoisted(() => ({ info: null as object | null }))
vi.mock('@/store/connection', async (original) => {
  const m = await original<typeof Connection>()
  const live = m.useConnection
  const hook = (selector: (s: object) => unknown = (s) => s) => selector(conn.info ? { ...live.getState(), info: conn.info } : live.getState())
  for (const k of ['getState', 'getInitialState', 'setState', 'subscribe'] as const) Object.defineProperty(hook, k, { get: () => live[k] })
  return { ...m, useConnection: hook }
})

const rpc = vi.hoisted(() => ({ set_auto_archive: vi.fn(async (_on: boolean, _secs: number) => null) }))
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({ ...rpc, info: async () => conn.info }),
}))

const DAY = 86_400

function render(): string {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(AutoArchive) as ReactNode }))
}

const field = (html: string, label: string) => html.match(new RegExp(`<input[^>]*aria-label="${label}"[^>]*>`))?.[0] ?? ''
const c = (label: string) => controls[label] ?? expect.fail(`no control "${label}"`)

beforeEach(() => {
  rpc.set_auto_archive.mockClear()
  for (const k of Object.keys(controls)) delete controls[k]
})

test('splitAfter shows whole days as days, anything else as hours', () => {
  expect(splitAfter(14 * DAY)).toEqual({ n: 14, unit: 'days' })
  expect(splitAfter(36 * 3600)).toEqual({ n: 36, unit: 'hours' })
  expect(splitAfter(48 * 3600)).toEqual({ n: 2, unit: 'days' })
  expect(splitAfter(1800)).toEqual({ n: 1, unit: 'hours' })
})

test('on: the switch and the engine value (14 days) show, the duration is editable', () => {
  conn.info = { auto_archive_enabled: true, auto_archive_after_secs: 14 * DAY }
  const html = render()
  expect(html).toContain('>Auto-archive inactive chats<')
  expect(html).toMatch(/aria-checked="true"[^>]*aria-label="Auto-archive inactive chats"/)
  expect(field(html, 'Archive after')).toMatch(/min="1"[^>]*value="14"/)
  expect(field(html, 'Archive after')).not.toContain('disabled=""')
  expect(c('unit').disabled).toBe(false)
  expect(c('unit').value).toBe('days')
})

test('off: the duration controls are disabled', () => {
  conn.info = { auto_archive_enabled: false, auto_archive_after_secs: 36 * 3600 }
  const html = render()
  expect(html).toMatch(/aria-checked="false"[^>]*aria-label="Auto-archive inactive chats"/)
  expect(field(html, 'Archive after')).toMatch(/value="36"[^>]*disabled=""|disabled=""[^>]*value="36"/)
  expect(c('unit').disabled).toBe(true)
  expect(c('unit').value).toBe('hours')
})

test('each control writes enabled and seconds through set_auto_archive', async () => {
  conn.info = { auto_archive_enabled: true, auto_archive_after_secs: 14 * DAY }
  render()
  c('Auto-archive inactive chats').onCheckedChange?.(false)
  await vi.waitFor(() => expect(rpc.set_auto_archive).toHaveBeenLastCalledWith(false, 14 * DAY))
  c('Archive after').action?.mutate(3)
  await vi.waitFor(() => expect(rpc.set_auto_archive).toHaveBeenLastCalledWith(true, 3 * DAY))
  c('unit').onValueChange?.('hours')
  await vi.waitFor(() => expect(rpc.set_auto_archive).toHaveBeenLastCalledWith(true, 14 * 3600))
})
