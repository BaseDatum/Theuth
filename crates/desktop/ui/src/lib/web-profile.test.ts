import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import type { Cookie, CookieChange, OriginStorage } from '@/lib/web-profile-wire'
import { RpcError } from '@/rpc/client'
import { PUSH_DEBOUNCE_MS, STORAGE_MAX_BYTES, cookieKey, diffCookies, refusal, startWebProfileSync } from './web-profile'

/** One `web_profile_push` as the engine got it (its positional params by name). */
type Pushed = { device: string; epoch: number; changes: CookieChange[]; storage: OriginStorage[] }

const SECRET = 'sid-value-0123456789-must-never-be-logged'
const STORE_SECRET = 'storage-value-must-never-be-logged'

const shell = vi.hoisted(() => ({
  has: true,
  store: [] as Cookie[],
  storage: {} as Record<string, { origin: string; local_storage: [string, string][] }>,
  changed: null as null | (() => void),
  state: null as null | ((s: { label: string; url: string; title: string; loading: boolean }) => void),
  beforeClose: null as null | ((s: { origin: string; local_storage: [string, string][] }) => void),
  readProfile: vi.fn(),
  captureStorage: vi.fn(),
  clearProfile: vi.fn(),
}))
vi.mock('@/lib/web-view-native', () => ({
  nativeWebViews: () => shell.has,
  labelId: (label: string) => label.replace(/^web-/, ''),
  readProfile: shell.readProfile,
  captureStorage: shell.captureStorage,
  clearProfile: shell.clearProfile,
  onProfileChanged: async (cb: () => void) => {
    shell.changed = cb
    return () => (shell.changed = null)
  },
  onState: async (cb: typeof shell.state) => {
    shell.state = cb
    return () => (shell.state = null)
  },
  onBeforeClose: (cb: typeof shell.beforeClose) => {
    shell.beforeClose = cb
    return () => (shell.beforeClose = null)
  },
}))
const engine = vi.hoisted(() => {
  const push = vi.fn()
  return { pushes: [] as Pushed[], push, api: { web_profile_push: push }, active: 'e1', supports: true }
})
vi.mock('@/store/connection', () => ({
  useConnection: { getState: () => ({ api: engine.api, state: 'open', discovery: {} }) },
}))
vi.mock('@/rpc/discovery', async (original) => ({ ...(await original<object>()), ready: () => true, supports: () => engine.supports }))
vi.mock('@/link/engines', () => ({ useEngines: { getState: () => ({ active: engine.active }) } }))
vi.mock('@/lib/platform', () => ({ deviceName: () => 'iPad' }))
vi.mock('@/lib/web-views', () => ({ useWebViews: { getState: () => ({ views: { a1: {}, b2: {} } }) } }))

const cookie = (name: string, value: string, over: Partial<Cookie> = {}): Cookie => ({
  name,
  value,
  domain: 'app.example.com',
  host_only: true,
  path: '/',
  expires: null,
  secure: true,
  http_only: true,
  same_site: 'Lax',
  partitioned: false,
  ...over,
})

const local = new Map<string, string>()
let stop: () => void = () => {}

beforeEach(() => {
  shell.has = true
  shell.store = []
  shell.storage = {}
  shell.readProfile.mockReset().mockImplementation(async () => shell.store.map((c) => ({ ...c })))
  shell.captureStorage.mockReset().mockImplementation(async (id: string) => {
    const s = shell.storage[id]
    if (!s) throw 'no web view web-' + id
    return s
  })
  shell.clearProfile.mockReset().mockImplementation(async () => {
    shell.store = []
  })
  engine.pushes = []
  engine.active = 'e1'
  engine.supports = true
  engine.push.mockReset().mockImplementation(async (device: string, epoch: number, changes: CookieChange[], storage: OriginStorage[]) => {
    engine.pushes.push(structuredClone({ device, epoch, changes, storage }))
    return { applied: changes.length, sites: [], epoch }
  })
  local.clear()
  vi.stubGlobal('localStorage', {
    getItem: (k: string) => local.get(k) ?? null,
    setItem: (k: string, v: string) => void local.set(k, v),
  })
})
afterEach(() => {
  stop()
  vi.useRealTimers()
  vi.unstubAllGlobals()
})
/** Let every promise chain the sync started run out (no timer is involved: pushes after a store change go at once). */
async function settle(): Promise<void> {
  for (let i = 0; i < 50; i++) await Promise.resolve()
}
/** Fire the shell's `web-profile://changed` and let the push settle. */
async function changed(): Promise<void> {
  const n = engine.push.mock.calls.length + shell.readProfile.mock.calls.length
  shell.changed!()
  await vi.waitFor(() => expect(engine.push.mock.calls.length + shell.readProfile.mock.calls.length).toBeGreaterThan(n))
  await settle()
}
const started = async () => {
  stop = startWebProfileSync()
  await vi.waitFor(() => expect(shell.changed).not.toBeNull())
}

test('the diff keys cookies by (name, domain, path): new and changed ones are sets, gone ones removes', () => {
  const a = cookie('a', '1')
  const b = cookie('b', '2')
  const last = new Map([a, b].map((c) => [cookieKey(c), c]))
  expect(diffCookies(last, [a, b])).toEqual([])
  const b2 = { ...b, value: '3' }
  const c = cookie('a', '1', { path: '/x' })
  expect(diffCookies(last, [b2, c])).toEqual([
    { op: 'set', cookie: b2 },
    { op: 'set', cookie: c },
    { op: 'remove', name: 'a', domain: 'app.example.com', path: '/' },
  ])
  // An attribute alone (HttpOnly lost) is a change too.
  expect(diffCookies(last, [a, { ...b, http_only: false }])).toHaveLength(1)
})

test('a store change pushes the delta against what was last pushed, with each live tile\'s storage', async () => {
  await started()
  shell.store = [cookie('__Host-sid', SECRET), cookie('theme', 'dark', { http_only: false })]
  shell.storage.a1 = { origin: 'https://app.example.com', local_storage: [['token', STORE_SECRET]] }
  await changed()
  expect(engine.pushes).toHaveLength(1)
  expect(engine.pushes[0]).toMatchObject({ device: 'iPad', epoch: 0, storage: [{ origin: 'https://app.example.com', local_storage: [['token', STORE_SECRET]] }] })
  expect(engine.pushes[0]!.changes.map((c) => c.op)).toEqual(['set', 'set'])

  // One cookie refreshed, one gone; the storage is unchanged and not sent again.
  shell.store = [cookie('__Host-sid', `${SECRET}-2`)]
  await changed()
  expect(engine.pushes).toHaveLength(2)
  expect(engine.pushes[1]!.changes).toEqual([
    { op: 'set', cookie: cookie('__Host-sid', `${SECRET}-2`) },
    { op: 'remove', name: 'theme', domain: 'app.example.com', path: '/' },
  ])
  expect(engine.pushes[1]!.storage).toEqual([])

  // Nothing changed: no push.
  await changed()
  expect(engine.pushes).toHaveLength(2)
})

test('storage captured on load-finished and before close is pushed once, PUSH_DEBOUNCE_MS after the last capture', async () => {
  await started()
  vi.useFakeTimers()
  shell.storage.a1 = { origin: 'https://a.example', local_storage: [['k', 'v1']] }
  shell.storage.b2 = { origin: 'https://b.example', local_storage: [['k', 'x'.repeat(STORAGE_MAX_BYTES)]] }
  shell.state!({ label: 'web-a1', url: 'https://a.example/', title: 'A', loading: true })
  shell.state!({ label: 'web-a1', url: 'https://a.example/', title: 'A', loading: false })
  await vi.advanceTimersByTimeAsync(PUSH_DEBOUNCE_MS - 500)
  shell.state!({ label: 'web-b2', url: 'https://b.example/', title: 'B', loading: false })
  shell.beforeClose!({ origin: 'https://c.example', local_storage: [['k', 'v3']] })
  shell.beforeClose!({ origin: 'about:blank', local_storage: [] })
  await vi.advanceTimersByTimeAsync(PUSH_DEBOUNCE_MS - 1)
  expect(engine.push).not.toHaveBeenCalled()
  await vi.advanceTimersByTimeAsync(1)
  expect(engine.pushes).toHaveLength(1)
  // Only a.example's load-finished capture (the loading one is skipped), not b's oversized snapshot, nor about:blank.
  expect(engine.pushes[0]!.storage.map((s) => s.origin)).toEqual(['https://a.example', 'https://c.example'])
  expect(engine.pushes[0]!.changes).toEqual([])
  expect(shell.captureStorage).toHaveBeenCalledTimes(2)
})

test('without the native layer nothing listens and nothing is sent', async () => {
  shell.has = false
  stop = startWebProfileSync()
  await settle()
  expect(shell.changed).toBeNull()
  expect(shell.state).toBeNull()
  expect(shell.beforeClose).toBeNull()
  expect(shell.readProfile).not.toHaveBeenCalled()
  expect(engine.push).not.toHaveBeenCalled()
})

test('an engine without web_profile_push gets nothing; once it answers `profile off` the store is not even read', async () => {
  engine.supports = false
  await started()
  shell.store = [cookie('sid', SECRET)]
  shell.changed!()
  await settle()
  expect(shell.readProfile).not.toHaveBeenCalled()
  expect(engine.push).not.toHaveBeenCalled()

  engine.supports = true
  engine.push.mockRejectedValueOnce(new RpcError(-32000, 'browser profile is off'))
  await changed()
  expect(engine.push).toHaveBeenCalledTimes(1)
  shell.store = [cookie('sid', `${SECRET}-2`)]
  shell.changed!()
  await settle()
  expect(shell.readProfile).toHaveBeenCalledTimes(1)
  expect(engine.push).toHaveBeenCalledTimes(1)
})

test('stale-epoch: a device that pushed under an older epoch clears its store; a new one adopts the epoch', async () => {
  expect(refusal(new RpcError(-32000, 'stale-epoch', { epoch: 4 }))).toEqual({ kind: 'stale-epoch', epoch: 4 })
  expect(refusal(new RpcError(-32000, 'stale-epoch: 7'))).toEqual({ kind: 'stale-epoch', epoch: 7 })
  expect(refusal(new RpcError(-32000, 'boom'))).toBeNull()

  await started()
  shell.store = [cookie('sid', SECRET)]
  // Never pushed to this engine: the same change goes again under the current epoch, the store stays.
  engine.push.mockRejectedValueOnce(new RpcError(-32000, 'stale-epoch', { epoch: 2 }))
  await changed()
  await vi.waitFor(() => expect(engine.pushes).toHaveLength(1))
  expect(engine.pushes[0]).toMatchObject({ epoch: 2, changes: [{ op: 'set' }] })
  expect(shell.clearProfile).not.toHaveBeenCalled()

  // Signed out everywhere since: the store is cleared and nothing of it is pushed back.
  shell.store = [cookie('sid', `${SECRET}-2`)]
  engine.push.mockRejectedValueOnce(new RpcError(-32000, 'stale-epoch', { epoch: 3 }))
  await changed()
  await vi.waitFor(() => expect(shell.clearProfile).toHaveBeenCalledTimes(1))
  expect(engine.pushes).toHaveLength(1)
  expect(local.get('theuth.web-profile.epoch.e1')).toBe('3')
})

test('cookie and storage values never reach the console, on any failure path', async () => {
  const spies = (['log', 'info', 'warn', 'error', 'debug'] as const).map((m) => vi.spyOn(console, m).mockImplementation(() => {}))
  await started()
  shell.store = [cookie('sid', SECRET)]
  shell.storage.a1 = { origin: 'https://app.example.com', local_storage: [['t', STORE_SECRET]] }
  engine.push.mockRejectedValueOnce(new RpcError(-32603, `internal: ${SECRET}`))
  await changed()
  shell.readProfile.mockRejectedValueOnce(new Error('web_profile_read: store unavailable'))
  await changed()
  engine.push.mockRejectedValueOnce(new RpcError(-32000, 'stale-epoch', { epoch: 5 }))
  local.set('theuth.web-profile.epoch.e1', '1')
  shell.clearProfile.mockRejectedValueOnce('unknown command web_profile_clear')
  await changed()
  await settle()
  const logged = JSON.stringify(spies.flatMap((s) => s.mock.calls))
  expect(spies.some((s) => s.mock.calls.length > 0)).toBe(true)
  expect(logged).not.toContain(SECRET)
  expect(logged).not.toContain(STORE_SECRET)
})
