import '../settings/test-dom'
import { readyConnection } from '@/components/settings/test-engine'
import type * as ConnectionModule from '@/store/connection'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement } from 'react'
import { createRoot } from 'react-dom/client'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import { empty } from '@/store/reduce'
import { useSessions } from '@/store/sessions'
import { band, ContextMeter, meterSource } from './ContextMeter'

vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => ({ list_models: async () => [] }) }))
vi.mock('@/store/connection', async (original) => {
  const m = await original<typeof ConnectionModule>()
  return { ...m, useConnection: readyConnection(m.createConnection()) }
})

test('bands at 75 / 90 / 95 % (75 §5); no limit is normal', () => {
  expect([null, 0, 74, 75, 89, 90, 95, 96, 120].map(band)).toEqual([
    'normal',
    'normal',
    'normal',
    'warning',
    'warning',
    'danger',
    'danger',
    'critical',
    'critical',
  ])
})

test('141 R3 T14: a watch-only omp child reads its own contextTokens/contextWindow; without them the session figures stand', () => {
  const omp = { detail: { contextTokens: 9037, contextWindow: 1_000_000 } }
  expect(meterSource(omp, 0, true, undefined)).toEqual({ used: 9037, estimated: false, limit: 1_000_000 })
  // Tail-only (no window from feed 1): the harness's tokens over the stored size.
  expect(meterSource({ detail: { contextTokens: 717 } }, 0, true, 200_000)).toEqual({ used: 717, estimated: false, limit: 200_000 })
  expect(meterSource({ detail: null }, 1234, true, 200_000)).toEqual({ used: 1234, estimated: true, limit: 200_000 })
  expect(meterSource(null, undefined, undefined, undefined)).toEqual({ used: undefined, estimated: false, limit: undefined })
})

test('Sky 2026-11-17: the meter shows for an epochs session and not for an endless (autocontext) one, in either header form', async () => {
  useSessions.setState({ sessions: { s1: { ...empty('s1'), context: 50_000, contextSize: 200_000 } } })
  const host = document.createElement('div')
  document.body.append(host)
  const root = createRoot(host)
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const meter = async (memory: 'epochs' | 'endless' | undefined, compact: boolean) => {
    await act(async () => root.render(createElement(QueryClientProvider, { client, children: createElement(ContextMeter, { session: 's1', memory, compact }) })))
    return host.querySelector('span[title]')?.getAttribute('title') ?? null
  }
  for (const compact of [false, true]) {
    expect(await meter('epochs', compact)).toMatch(/^Context window used/)
    expect(await meter(undefined, compact)).toMatch(/^Context window used/)
    expect(await meter('endless', compact)).toBeNull()
    expect(host.innerHTML).toBe('')
  }
  await act(async () => root.unmount())
})
