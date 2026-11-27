import '@/components/settings/test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import type { PeerSync } from '@/rpc/bindings/method.sessionsync.receiver'
import { type Discovery, disconnected } from '@/rpc/discovery'
import { useConnection } from '@/store/connection'
import {
  BOOTSTRAPPING_LABEL,
  PEER_SYNC_BOOTSTRAPPING_MS,
  PEER_SYNC_IDLE_MS,
  bootstrappingLabel,
  isPeerBootstrapping,
  peerSyncById,
  peerSyncQueryKey,
  usePeerSync,
} from './peer-sync'

const sessionsSync = vi.fn<() => Promise<PeerSync[]>>(async () => [])

vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({ sessions_sync: sessionsSync }),
}))

const discoveryWith = (methods: string[]): Discovery => ({
  phase: 'ready',
  capabilities: null,
  unavailable: new Set(),
  document: {
    openrpc: '1.3.2',
    info: { title: 'test', version: '0.0.0' },
    methods: methods.map((name) => ({ name, params: [], result: { schema: true, 'x-theuth-receiverSchema': true } })),
    components: { schemas: {} },
    'x-theuth': { protocolVersion: { major: 1, minor: 0 }, plugins: [], events: [], subscriptions: [] },
  },
})

const syncRow = (over: Partial<PeerSync> = {}): PeerSync => ({
  peer: 'cloud-1',
  up: true,
  head: 1000,
  acked: 0,
  behind_out: 1000,
  held: false,
  held_by_peer: false,
  ...over,
})

test('isPeerBootstrapping: true only for unheld peer with head > 0 and acked null/undefined/0', () => {
  // Freshly connected before Hello (`acked: null`) and after Hello while snapshot is in flight (`acked: 0`).
  expect(isPeerBootstrapping(syncRow({ acked: 0, behind_out: 1000 }))).toBe(true)
  expect(isPeerBootstrapping(syncRow({ up: false, acked: null, behind_out: null }))).toBe(true)
  expect(isPeerBootstrapping(syncRow({ acked: undefined }))).toBe(true)

  // Initial snapshot already landed (`acked > 0`), even if still catching up on recent rows.
  expect(isPeerBootstrapping(syncRow({ acked: 1, behind_out: 999 }))).toBe(false)
  expect(isPeerBootstrapping(syncRow({ acked: 1000, behind_out: 0 }))).toBe(false)

  // Empty mesh (`head === 0`): nothing to bootstrap.
  expect(isPeerBootstrapping(syncRow({ head: 0, acked: 0, behind_out: 0 }))).toBe(false)
  expect(isPeerBootstrapping(syncRow({ head: 0, acked: null, behind_out: null }))).toBe(false)

  // Older-schema peer held by us: not bootstrapping.
  expect(isPeerBootstrapping(syncRow({ held: true, acked: 0 }))).toBe(false)
  expect(isPeerBootstrapping(syncRow({ held: true, acked: null }))).toBe(false)

  // Missing entry.
  expect(isPeerBootstrapping(undefined)).toBe(false)
})

test('peerSyncById and bootstrappingLabel', () => {
  const a = syncRow({ peer: 'a', acked: 0 })
  const b = syncRow({ peer: 'b', acked: 500 })
  const map = peerSyncById([a, b])
  expect(map.get('a')).toEqual(a)
  expect(map.get('b')).toEqual(b)
  expect(map.get('missing')).toBeUndefined()
  expect(peerSyncById(undefined).size).toBe(0)
  expect(bootstrappingLabel(a)).toBe(BOOTSTRAPPING_LABEL)
  expect(BOOTSTRAPPING_LABEL).toBe('Syncing mesh history…')
})

let root: Root | null = null

beforeEach(() => {
  vi.clearAllMocks()
  sessionsSync.mockResolvedValue([])
  useConnection.getState().setDiscovery(discoveryWith(['sessions_sync']))
})

afterEach(async () => {
  if (root) {
    await act(async () => root?.unmount())
    root = null
  }
  useConnection.getState().setDiscovery(disconnected())
  document.body.innerHTML = ''
  vi.useRealTimers()
})

test('usePeerSync: degrades to empty map when engine lacks sessions_sync capability', async () => {
  useConnection.getState().setDiscovery(discoveryWith(['mesh_status']))
  let result!: ReturnType<typeof usePeerSync>
  function Probe() {
    result = usePeerSync()
    return null
  }
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const host = document.createElement('div')
  document.body.append(host)
  root = createRoot(host)
  await act(async () => root!.render(createElement(QueryClientProvider, { client: qc }, createElement(Probe))))
  expect(sessionsSync).not.toHaveBeenCalled()
  expect(result.byPeer.size).toBe(0)
  expect(result.anyBootstrapping).toBe(false)
  expect(result.data).toBeUndefined()
})

test('usePeerSync: polls every 3 s while bootstrapping and slows to 30 s once acked > 0', async () => {
  vi.useFakeTimers()
  const flush = async (ms = 0) => {
    if (ms > 0) {
      await act(async () => {
        await vi.advanceTimersByTimeAsync(ms)
      })
    }
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1)
    })
  }
  sessionsSync.mockResolvedValue([syncRow({ peer: 'cloud-1', head: 1000, acked: 0 })])
  let result!: ReturnType<typeof usePeerSync>
  function Probe() {
    result = usePeerSync()
    return null
  }
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const host = document.createElement('div')
  document.body.append(host)
  root = createRoot(host)
  await act(async () => root!.render(createElement(QueryClientProvider, { client: qc }, createElement(Probe))))
  await flush()
  expect(sessionsSync).toHaveBeenCalledTimes(1)
  expect(result.anyBootstrapping).toBe(true)
  expect(result.byPeer.get('cloud-1')?.acked).toBe(0)

  // Fast 3 s polling while bootstrapping.
  const settled = [syncRow({ peer: 'cloud-1', head: 1000, acked: 1000, behind_out: 0 })]
  sessionsSync.mockResolvedValue(settled)
  await flush(PEER_SYNC_BOOTSTRAPPING_MS)
  expect(sessionsSync).toHaveBeenCalledTimes(2)
  expect(qc.getQueryData(peerSyncQueryKey)).toEqual(settled)
  expect(result.anyBootstrapping).toBe(false)
  expect(result.byPeer.get('cloud-1')?.acked).toBe(1000)

  // Once settled, no poll at 3 s; next poll happens at 30 s.
  await flush(PEER_SYNC_BOOTSTRAPPING_MS)
  expect(sessionsSync).toHaveBeenCalledTimes(2)
  await flush(PEER_SYNC_IDLE_MS - PEER_SYNC_BOOTSTRAPPING_MS)
  expect(sessionsSync).toHaveBeenCalledTimes(3)
  expect(qc.getQueryData(peerSyncQueryKey)).toHaveLength(1)
})
