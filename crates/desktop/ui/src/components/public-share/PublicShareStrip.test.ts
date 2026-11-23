import '@/components/settings/test-dom'
import { act, createElement as h } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, expect, test, vi } from 'vitest'
import type { RpcClient } from '@/rpc/client'
import { useConnection } from '@/store/connection'
import { usePublicShares } from '@/store/public-shares'
import type { Share } from './wire'
import { PublicShareStrip } from './PublicShareStrip'

vi.mock('@tanstack/react-router', () => ({ Link: ({ children }: { children: unknown }) => h('a', null, children as never) }))
vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn(), success: vi.fn() }) }))

const live = (id: string): Share => ({
  id,
  engine: 'e1',
  edge: 'edge1',
  domain: 'tun.zip',
  host: `${id}.sky.tun.zip`,
  path: '/',
  url: `https://${id}.sky.tun.zip/`,
  source: { port: 5173 },
  access: 'public',
  interstitial: true,
  created_ms: 1,
  live_ms: 1,
  expires_ms: null,
  created_by: 'settings',
  approved_by: { approval_id: '', surface: '' },
  state: 'live',
  stopped_ms: null,
  stopped_by: null,
})

let root: Root | undefined
afterEach(async () => {
  await act(async () => root?.unmount())
  root = undefined
  document.body.innerHTML = ''
  usePublicShares.getState().reset()
  useConnection.setState({ client: null })
})

const strip = () => document.querySelector('[data-public-share-strip]')

test('G4: the red strip shows while a share is live; Stop all calls share_stop("all") with no approval and the strip goes', async () => {
  const calls: [string, unknown[]][] = []
  let rows: Share[] = [live('a'), live('b')]
  const call = async (method: string, params: unknown[]) => {
    calls.push([method, params])
    if (method === 'share_stop') {
      const n = rows.length
      rows = rows.map((s) => ({ ...s, state: 'stopped' as const, stopped_by: 'user' as const }))
      return { stopped: n }
    }
    if (method === 'share_list') return rows
    throw new Error(`unexpected ${method}`)
  }
  useConnection.setState({ client: { call } as unknown as RpcClient })
  usePublicShares.getState().replace(rows)

  const el = document.createElement('div')
  document.body.append(el)
  root = createRoot(el)
  await act(async () => root!.render(h(PublicShareStrip)))
  expect(strip()?.textContent).toContain('Sharing publicly · 2 live')

  const stop = [...document.querySelectorAll('button')].find((b) => b.textContent === 'Stop all')
  expect(stop).toBeDefined()
  await act(async () => stop!.click())

  expect(calls.map(([m]) => m)).toEqual(['share_stop', 'share_list'])
  expect(calls[0]![1]).toEqual(['all'])
  expect(strip()).toBeNull()
})

test('nothing active, no strip', async () => {
  usePublicShares.getState().replace([{ ...live('a'), state: 'stopped' }])
  const el = document.createElement('div')
  document.body.append(el)
  root = createRoot(el)
  await act(async () => root!.render(h(PublicShareStrip)))
  expect(strip()).toBeNull()
})
