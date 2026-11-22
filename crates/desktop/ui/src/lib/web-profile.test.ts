import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import type { Cookie, CookieChange, OriginStorage } from '@/lib/web-profile-wire'
import { RpcError } from '@/rpc/client'
import { OFF_REPROBE_MS, PUSH_DEBOUNCE_MS, RETRY_MS, STORAGE_MAX_BYTES, cookieKey, diffCookies, refusal, startWebProfileSync } from './web-profile'

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
  return { pushes: [] as Pushed[], push, api: { web_profile_push: push } as object, discovery: {} as object, active: 'e1', supports: true }
})
vi.mock('@/store/connection', () => ({
  useConnection: { getState: () => ({ api: engine.api, state: 'open', discovery: engine.discovery }) },
}))
vi.mock('@/rpc/discovery', async (original) => ({ ...(await original<object>()), ready: () => true, supports: () => engine.supports }))
vi.mock('@/link/engines', () => ({ useEngines: { getState: () => ({ active: engine.active }) } }))
vi.mock('@/lib/platform', () => ({ deviceName: () => 'iPad' }))

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
const leave = new Map<string, () => void>()
let stop: () => void = () => {}

beforeEach(() => {
  shell.has = true
  shell.store = []
  shell.storage = {}
  shell.changed = null
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
  engine.api = { web_profile_push: engine.push }
  engine.discovery = {}
  engine.push.mockReset().mockImplementation(async (device: string, epoch: number, changes: CookieChange[], storage: OriginStorage[]) => {
    engine.pushes.push(structuredClone({ device, epoch, changes, storage }))
    return { applied: changes.length, sites: [], epoch: Math.max(epoch, 1), profile: 'shared' }
  })
  local.clear()
  leave.clear()
  vi.stubGlobal('localStorage', {
    getItem: (k: string) => local.get(k) ?? null,
    setItem: (k: string, v: string) => void local.set(k, v),
  })
  vi.stubGlobal('addEventListener', (type: string, cb: () => void) => leave.set(type, cb))
  vi.stubGlobal('removeEventListener', (type: string) => leave.delete(type))
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
/** Start the sync and let its seed read land. */
const started = async () => {
  stop = startWebProfileSync()
  await vi.waitFor(() => expect(shell.changed).not.toBeNull())
  await vi.waitFor(() => expect(shell.readProfile).toHaveBeenCalled())
  await settle()
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

test('the store at start is the baseline and is not pushed; only later changes are, against what was last pushed', async () => {
  shell.store = [cookie('__Host-sid', SECRET), cookie('old', 'x')]
  await started()
  expect(engine.push).not.toHaveBeenCalled()
  // A change event without a change: nothing to push.
  await changed()
  expect(engine.push).not.toHaveBeenCalled()

  shell.store = [cookie('__Host-sid', `${SECRET}-2`), cookie('old', 'x'), cookie('theme', 'dark', { http_only: false })]
  await changed()
  expect(engine.pushes).toHaveLength(1)
  expect(engine.pushes[0]).toMatchObject({ device: 'iPad', epoch: 0, storage: [] })
  expect(engine.pushes[0]!.changes).toEqual([
    { op: 'set', cookie: cookie('__Host-sid', `${SECRET}-2`) },
    { op: 'set', cookie: cookie('theme', 'dark', { http_only: false }) },
  ])

  shell.store = [cookie('__Host-sid', `${SECRET}-2`), cookie('theme', 'dark', { http_only: false })]
  await changed()
  expect(engine.pushes).toHaveLength(2)
  expect(engine.pushes[1]!.changes).toEqual([{ op: 'remove', name: 'old', domain: 'app.example.com', path: '/' }])
  expect(local.get('theuth.web-profile.epoch.e1')).toBe('1')
  // A cookie change captures no tile storage (only load-finished and close do).
  expect(shell.captureStorage).not.toHaveBeenCalled()
})

test('an engine switch re-seeds: the store as it is then is not pushed to the new engine', async () => {
  await started()
  shell.store = [cookie('sid', SECRET)]
  await changed()
  expect(engine.pushes).toHaveLength(1)

  engine.active = 'e2'
  shell.store = [cookie('sid', SECRET), cookie('more', 'y')]
  await changed()
  expect(engine.pushes).toHaveLength(1)
  shell.store = [cookie('sid', SECRET), cookie('more', 'y'), cookie('new', 'z')]
  await changed()
  expect(engine.pushes).toHaveLength(2)
  expect(engine.pushes[1]!.changes).toEqual([{ op: 'set', cookie: cookie('new', 'z') }])
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

test('a load-finished pushes the cookies the load set, without any web-profile://changed', async () => {
  await started()
  vi.useFakeTimers()
  shell.storage.a1 = { origin: 'https://a.example', local_storage: [] }
  // A sign-in's redirect landed: the store has the session cookie, and the shell's observer stayed silent.
  shell.store = [cookie('sid', SECRET)]
  shell.state!({ label: 'web-a1', url: 'https://a.example/', title: 'A', loading: false })
  await vi.advanceTimersByTimeAsync(PUSH_DEBOUNCE_MS)
  expect(engine.pushes).toHaveLength(1)
  expect(engine.pushes[0]!.changes).toEqual([{ op: 'set', cookie: cookie('sid', SECRET) }])
})

test('what is still captured when the page goes away is pushed at once, without waiting for the debounce', async () => {
  await started()
  shell.beforeClose!({ origin: 'https://c.example', local_storage: [['k', 'v3']] })
  leave.get('pagehide')!()
  leave.get('beforeunload')!()
  expect(engine.push).toHaveBeenCalledTimes(1)
  expect(engine.push.mock.calls[0]![3]).toMatchObject([{ origin: 'https://c.example', local_storage: [['k', 'v3']] }])
  stop()
  expect(leave.size).toBe(0)
})

test('one push in flight: changes arriving meanwhile go in exactly one follow-up push, in order, nothing dropped', async () => {
  await started()
  const first = Promise.withResolvers<{ applied: number; sites: string[]; epoch: number }>()
  engine.push.mockImplementationOnce(async (device: string, epoch: number, changes: CookieChange[], storage: OriginStorage[]) => {
    engine.pushes.push(structuredClone({ device, epoch, changes, storage }))
    return first.promise
  })
  shell.store = [cookie('a', '1')]
  await changed()
  expect(engine.push).toHaveBeenCalledTimes(1)

  // While the first push is out: two store changes and a closed tile's storage.
  shell.store = [cookie('a', '2')]
  shell.changed!()
  shell.store = [cookie('a', '2'), cookie('b', '1')]
  shell.changed!()
  shell.beforeClose!({ origin: 'https://c.example', local_storage: [['k', 'v']] })
  await settle()
  expect(engine.push).toHaveBeenCalledTimes(1)

  first.resolve({ applied: 1, sites: [], epoch: 1 })
  await vi.waitFor(() => expect(engine.push).toHaveBeenCalledTimes(2))
  await settle()
  expect(engine.push).toHaveBeenCalledTimes(2)
  expect(engine.pushes[0]!.changes).toEqual([{ op: 'set', cookie: cookie('a', '1') }])
  expect(engine.pushes[1]!.changes).toEqual([
    { op: 'set', cookie: cookie('a', '2') },
    { op: 'set', cookie: cookie('b', '1') },
  ])
  expect(engine.pushes[1]!.storage.map((s) => s.origin)).toEqual(['https://c.example'])
})

test('a failed read keeps the delta and is retried after RETRY_MS, then with a growing backoff', async () => {
  await started()
  vi.useFakeTimers()
  shell.store = [cookie('sid', SECRET)]
  shell.readProfile.mockRejectedValueOnce(new Error('web_profile_read: store unavailable'))
  shell.readProfile.mockRejectedValueOnce(new Error('web_profile_read: store unavailable'))
  shell.changed!()
  await vi.advanceTimersByTimeAsync(0)
  expect(engine.push).not.toHaveBeenCalled()
  await vi.advanceTimersByTimeAsync(RETRY_MS)
  expect(shell.readProfile).toHaveBeenCalledTimes(3)
  expect(engine.push).not.toHaveBeenCalled()
  // The second retry waits twice as long.
  await vi.advanceTimersByTimeAsync(RETRY_MS * 2 - 1)
  expect(engine.push).not.toHaveBeenCalled()
  await vi.advanceTimersByTimeAsync(1)
  expect(engine.pushes).toHaveLength(1)
  expect(engine.pushes[0]!.changes).toEqual([{ op: 'set', cookie: cookie('sid', SECRET) }])
})

test('a failed seed is retried before anything is pushed, and its store is still not pushed', async () => {
  shell.store = [cookie('old', 'x')]
  shell.readProfile.mockRejectedValueOnce(new Error('web_profile_read: store unavailable'))
  await started()
  shell.store = [cookie('old', 'x'), cookie('new', 'y')]
  await changed()
  // The seed read the store with `new` already in it: nothing is pushed.
  expect(engine.push).not.toHaveBeenCalled()
  shell.store = [cookie('new', 'y')]
  await changed()
  expect(engine.pushes[0]!.changes).toEqual([{ op: 'remove', name: 'old', domain: 'app.example.com', path: '/' }])
})

test('an engine without web_profile_push gets nothing; once it answers `browser profile is off` the store is not even read', async () => {
  engine.supports = false
  await started()
  shell.store = [cookie('sid', SECRET)]
  shell.changed!()
  await settle()
  expect(shell.readProfile).toHaveBeenCalledTimes(1)
  expect(engine.push).not.toHaveBeenCalled()

  engine.supports = true
  engine.push.mockRejectedValueOnce(new RpcError(-32000, 'browser profile is off'))
  await changed()
  expect(engine.push).toHaveBeenCalledTimes(1)
  shell.store = [cookie('sid', `${SECRET}-2`)]
  shell.changed!()
  await settle()
  expect(shell.readProfile).toHaveBeenCalledTimes(2)
  expect(engine.push).toHaveBeenCalledTimes(1)

  // Only the exact wording means off; any other error is a failure, retried.
  expect(refusal(new RpcError(-32000, 'browser.profile is off'))).toEqual({ kind: 'off' })
  expect(refusal(new RpcError(-32000, 'browser profile unavailable: browser.profile is off'))).toEqual({ kind: 'off' })
  expect(refusal(new RpcError(-32000, 'the profile is off by one'))).toBeNull()
  expect(refusal(new RpcError(-32000, 'browser profile unavailable: store locked'))).toBeNull()
})

test('`profile: "off"` in the result stops the sync for that connection until it changes or OFF_REPROBE_MS passed', async () => {
  await started()
  engine.push.mockResolvedValueOnce({ applied: 0, sites: [], epoch: 1, profile: 'off' })
  shell.store = [cookie('a', '1')]
  await changed()
  expect(engine.push).toHaveBeenCalledTimes(1)
  expect(local.get('theuth.web-profile.epoch.e1')).toBeUndefined()

  shell.store = [cookie('a', '1'), cookie('b', '1')]
  shell.changed!()
  await settle()
  expect(engine.push).toHaveBeenCalledTimes(1)

  // A new connection (the user turned the profile on, the engine restarted): the next change pushes everything kept.
  engine.api = { web_profile_push: engine.push }
  await changed()
  expect(engine.push).toHaveBeenCalledTimes(2)
  expect(engine.pushes[0]!.changes.map((c) => c.op === 'set' && c.cookie.name)).toEqual(['a', 'b'])

  // Off again on this connection: a real change after OFF_REPROBE_MS probes it again.
  engine.push.mockResolvedValueOnce({ applied: 0, sites: [], epoch: 1, profile: 'off' })
  shell.store = [cookie('c', '1')]
  await changed()
  expect(engine.push).toHaveBeenCalledTimes(3)
  vi.useFakeTimers({ now: Date.now() + OFF_REPROBE_MS })
  shell.changed!()
  await vi.advanceTimersByTimeAsync(0)
  await settle()
  expect(engine.push).toHaveBeenCalledTimes(4)
})

test('stale-epoch: a device that pushed under an older epoch clears its store; a new one adopts the epoch', async () => {
  expect(refusal(new RpcError(-32000, 'stale-epoch', { epoch: 4 }))).toEqual({ kind: 'stale-epoch', epoch: 4 })
  expect(refusal(new RpcError(-32000, 'stale-epoch: 7'))).toEqual({ kind: 'stale-epoch', epoch: 7 })
  expect(refusal(new RpcError(-32000, 'stale-epoch'))).toBeNull()
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
  await changed()
  expect(engine.pushes).toHaveLength(1)
})

test('unknown-epoch: the engine\'s epoch is adopted and the same delta goes again, the store kept', async () => {
  expect(refusal(new RpcError(-32000, 'unknown-epoch', { epoch: 9 }))).toEqual({ kind: 'unknown-epoch', epoch: 9 })
  local.set('theuth.web-profile.epoch.e1', '40')
  await started()
  shell.store = [cookie('sid', SECRET)]
  engine.push.mockRejectedValueOnce(new RpcError(-32000, 'unknown-epoch', { epoch: 9 }))
  await changed()
  await vi.waitFor(() => expect(engine.pushes).toHaveLength(1))
  expect(engine.pushes[0]).toMatchObject({ epoch: 9, changes: [{ op: 'set', cookie: { name: 'sid' } }] })
  expect(shell.clearProfile).not.toHaveBeenCalled()
})

test('a shell that cannot clear keeps its epoch and pushes nothing more to that connection', async () => {
  local.set('theuth.web-profile.epoch.e1', '1')
  await started()
  shell.clearProfile.mockRejectedValue('unknown command web_profile_clear')
  shell.store = [cookie('sid', SECRET)]
  engine.push.mockRejectedValueOnce(new RpcError(-32000, 'stale-epoch', { epoch: 5 }))
  await changed()
  expect(shell.clearProfile).toHaveBeenCalledTimes(1)
  expect(local.get('theuth.web-profile.epoch.e1')).toBe('1')

  shell.store = [cookie('sid', SECRET), cookie('more', 'x')]
  shell.changed!()
  shell.beforeClose!({ origin: 'https://c.example', local_storage: [['k', 'v']] })
  leave.get('pagehide')!()
  await settle()
  expect(engine.push).toHaveBeenCalledTimes(1)

  // A new connection tries again: refused again, the clear is tried again, still nothing of the store is pushed.
  engine.api = { web_profile_push: engine.push }
  engine.push.mockRejectedValueOnce(new RpcError(-32000, 'stale-epoch', { epoch: 5 }))
  await changed()
  expect(engine.push).toHaveBeenCalledTimes(2)
  expect(engine.pushes).toHaveLength(0)
  expect(shell.clearProfile).toHaveBeenCalledTimes(2)
})

test('cookie and storage values never reach the console, on any failure path', async () => {
  const spies = (['log', 'info', 'warn', 'error', 'debug'] as const).map((m) => vi.spyOn(console, m).mockImplementation(() => {}))
  await started()
  shell.store = [cookie('sid', SECRET)]
  shell.beforeClose!({ origin: 'https://app.example.com', local_storage: [['t', STORE_SECRET]] })
  engine.push.mockRejectedValueOnce(new RpcError(-32603, `internal: ${SECRET}`))
  await changed()
  shell.readProfile.mockRejectedValueOnce(new Error('web_profile_read: store unavailable'))
  await changed()
  engine.push.mockRejectedValueOnce(new RpcError(-32000, 'stale-epoch', { epoch: 5 }))
  local.set('theuth.web-profile.epoch.e1', '1')
  shell.clearProfile.mockRejectedValueOnce('unknown command web_profile_clear')
  shell.store = [cookie('sid', `${SECRET}-2`)]
  engine.api = { web_profile_push: engine.push }
  await changed()
  await settle()
  const logged = JSON.stringify(spies.flatMap((s) => s.mock.calls))
  expect(spies.some((s) => s.mock.calls.length > 0)).toBe(true)
  expect(logged).not.toContain(SECRET)
  expect(logged).not.toContain(STORE_SECRET)
})
