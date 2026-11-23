import '@/components/settings/test-dom'
import { act, createElement as h } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, expect, test, vi } from 'vitest'
import type { Api } from '@/rpc/bindings/client'
import { useConnection } from '@/store/connection'
import { usePublicShares } from '@/store/public-shares'
import { applyStopped, takePassword } from './actions'
import type { Share, Stopped } from './wire'
import { PublicShareStrip } from './PublicShareStrip'

vi.mock('@tanstack/react-router', () => ({ Link: ({ children }: { children: unknown }) => h('a', null, children as never) }))
vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn(), success: vi.fn(), warning: vi.fn() }) }))

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
  usePublicShares.getState().setEdgePending(null)
  useConnection.setState({ api: null })
})

const strip = () => document.querySelector('[data-public-share-strip]')

async function mount() {
  const el = document.createElement('div')
  document.body.append(el)
  root = createRoot(el)
  await act(async () => root!.render(h(PublicShareStrip)))
}

/** An engine whose `share_stop` stops the rows and, like the plugin, emits `share.changed@1` for each. */
function engine(rows: Share[], edge_pending: string | null) {
  const calls: [string, unknown[]][] = []
  const api = {
    share_stop: async (id: string): Promise<Stopped> => {
      calls.push(['share_stop', [id]])
      for (const s of rows) usePublicShares.getState().changed({ ...s, state: 'stopped', stopped_by: 'user' })
      return { stopped: rows.length, edge_pending }
    },
  }
  useConnection.setState({ api: api as unknown as Api })
  usePublicShares.getState().replace(rows)
  return calls
}

const stopAllButton = () => [...document.querySelectorAll('button')].find((b) => b.textContent === 'Stop all')

test('G4: the red strip shows while a share is live; Stop all calls share_stop("all") with no approval and the strip goes', async () => {
  const calls = engine([live('a'), live('b')], null)
  await mount()
  expect(strip()?.textContent).toContain('Sharing publicly · 2 live')

  await act(async () => stopAllButton()!.click())

  expect(calls).toEqual([['share_stop', ['all']]])
  expect(strip()).toBeNull()
})

test('FIX-A M7: a Stop all the edge did not take says "pending, will be retried", not stopped, until the edge reattaches', async () => {
  engine([live('a')], 'edge detached')
  await mount()
  await act(async () => stopAllButton()!.click())

  expect(strip()?.textContent).toContain('the edge stop is pending and will be retried (edge detached)')
  expect(strip()?.textContent).not.toMatch(/^Stopped$/)

  // A webview reconnect keeps it; the edge attaching (the engine re-sends the stop at its welcome) clears it.
  await act(async () => usePublicShares.getState().reset())
  expect(strip()).not.toBeNull()
  const status = (state: 'dialing' | 'attached') =>
    usePublicShares.getState().setStatus({ type: 'ShareStatus', edge: 'edge1', domain: 'tun.zip', clock_off_ms: null, state: state === 'attached' ? { state } : { state, attempt: 1 } })
  await act(async () => status('dialing'))
  expect(strip()).not.toBeNull()
  await act(async () => status('attached'))
  expect(strip()).toBeNull()
})

test('the tray Stop all (run by the shell) reports edge_pending to the same store', async () => {
  await mount()
  await act(async () => applyStopped('all', { stopped: 0, edge_pending: 'refused: 503' }))
  expect(strip()?.textContent).toContain('pending and will be retried (refused: 503)')
  await act(async () => applyStopped('all', { stopped: 0, edge_pending: null }))
  expect(strip()).toBeNull()
})

test('nothing active, no strip', async () => {
  usePublicShares.getState().replace([{ ...live('a'), state: 'stopped' }])
  await mount()
  expect(strip()).toBeNull()
})

test('FIX-A M9: the password comes from share_password once per share, then never again', async () => {
  const asked: string[] = []
  useConnection.setState({
    api: { share_password: async (id: string) => (asked.push(id), 'hunter2-generated') } as unknown as Api,
  })
  expect(await takePassword('s1')).toBe('hunter2-generated')
  expect(await takePassword('s1')).toBeNull()
  expect(asked).toEqual(['s1'])
  // Only ids are remembered, never the password (where the environment has storage).
  if (typeof localStorage !== 'undefined') expect(JSON.stringify({ ...localStorage })).not.toContain('hunter2')

  // Settings' publish: `share_password` first; the RPC `Published.password` if the engine has none.
  useConnection.setState({ api: { share_password: async () => null } as unknown as Api })
  expect(await takePassword('s2', 'from-published')).toBe('from-published')
  // A non-owner (or no engine) gets nothing.
  useConnection.setState({ api: { share_password: async () => Promise.reject(new Error('owner only')) } as unknown as Api })
  expect(await takePassword('s3')).toBeNull()
})
