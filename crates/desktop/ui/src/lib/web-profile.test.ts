import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import type { Cookie, CookieChange, OriginStorage } from '@/lib/web-profile-wire'
import { RpcError } from '@/rpc/client'
import {
  CONTENDED_MS,
  OFF_REPROBE_MS,
  PUSH_DEBOUNCE_MS,
  RETRY_MS,
  STORAGE_MAX_BYTES,
  cookieKey,
  diffCookies,
  planApply,
  profileDevice,
  refusal,
  signedOutEverywhere,
  siteOf,
  startWebProfileSync,
  useWebProfileMode,
} from './web-profile'

/** One `web_profile_push` as the engine got it (its positional params by name). */
type Pushed = { device: string; epoch: number; changes: CookieChange[]; storage: OriginStorage[]; basis?: number }
/** The device name every push carries: `iPad` and the first 6 characters of this install's id. */
const DEVICE = expect.stringMatching(/^iPad-[0-9a-f]{6}$/)

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
  writeProfile: vi.fn(),
  /** What the store reads back for a written cookie: WebKit spells `None` as `""` (209 §4.5), may move a path. */
  normalise: (c: Cookie): Cookie => c,
  /** Runs once the fake write has applied: a live page re-setting its cookie, later (`setTimeout`). */
  afterWrite: null as null | (() => void),
  closed: null as null | ((s: { label: string }) => void),
}))
vi.mock('@/lib/web-view-native', () => ({
  nativeWebViews: () => shell.has,
  labelId: (label: string) => label.replace(/^web-/, ''),
  readProfile: shell.readProfile,
  captureStorage: shell.captureStorage,
  clearProfile: shell.clearProfile,
  writeProfile: shell.writeProfile,
  onProfileChanged: async (cb: () => void) => {
    shell.changed = cb
    return () => (shell.changed = null)
  },
  onState: async (cb: typeof shell.state) => {
    shell.state = cb
    return () => (shell.state = null)
  },
  onClosed: async (cb: typeof shell.closed) => {
    shell.closed = cb
    return () => (shell.closed = null)
  },
  onBeforeClose: (cb: typeof shell.beforeClose) => {
    shell.beforeClose = cb
    return () => (shell.beforeClose = null)
  },
}))
const engine = vi.hoisted(() => {
  const push = vi.fn()
  const pull = vi.fn()
  return {
    pushes: [] as Pushed[],
    push,
    pull,
    api: { web_profile_push: push, web_profile_pull: pull } as object,
    discovery: {} as object,
    active: 'e1',
    supports: true,
    /** `web_profile_pull` advertised: off for the P1 cases, which predate it. */
    pulls: false,
    /** The profile's cookies and epoch as `web_profile_pull` answers them. */
    profile: [] as Cookie[],
    /** Its localStorage snapshots, with their site. */
    storage: [] as { origin: string; local_storage: [string, string][]; site: string }[],
    /** Its forget markers. */
    forgotten: [] as { site: string; forgotten_ms: number }[],
    /** The engine's clock in every pull answer. */
    nowMs: 0,
    /** Cookies per pull page; 0: one page. */
    pageSize: 0,
    epoch: 1,
    connection: new Set<() => void>(),
  }
})
vi.mock('@/store/connection', () => ({
  useConnection: {
    getState: () => ({ api: engine.api, state: 'open', discovery: engine.discovery }),
    subscribe: (cb: () => void) => {
      engine.connection.add(cb)
      return () => engine.connection.delete(cb)
    },
  },
}))
vi.mock('@/rpc/discovery', async (original) => ({
  ...(await original<object>()),
  ready: () => true,
  supports: (_d: unknown, r: { methods?: string[] }) => engine.supports && (engine.pulls || !r.methods?.includes('web_profile_pull')),
}))
vi.mock('@/link/engines', () => ({ useEngines: { getState: () => ({ active: engine.active }), subscribe: () => () => {} } }))
vi.mock('@/lib/platform', () => ({ deviceName: () => 'iPad' }))
const events = vi.hoisted(() => ({ changed: null as null | ((e: { type: 'WebProfileChanged'; sites: string[]; epoch: number }) => void) }))
vi.mock('@/store/sessions', () => ({
  on: (topic: string, type: string, cb: NonNullable<typeof events.changed>) => {
    if (topic !== 'browser.web-profile-changed@1' || type !== 'WebProfileChanged') throw new Error(`unexpected ${topic} ${type}`)
    events.changed = cb
    return () => (events.changed = null)
  },
}))

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
  shell.normalise = (c) => c
  shell.afterWrite = null
  shell.closed = null
  shell.writeProfile
    .mockReset()
    .mockImplementation(async (set: Cookie[], remove: Pick<Cookie, 'name' | 'domain' | 'path'>[], storage: unknown[] = []) => {
      const gone = new Set(remove.map(cookieKey))
      shell.store = shell.store.filter((c) => !gone.has(cookieKey(c)))
      for (const c of set) {
        const n = shell.normalise({ ...c })
        shell.store = [...shell.store.filter((s) => cookieKey(s) !== cookieKey(n)), n]
      }
      const page = shell.afterWrite
      if (page) setTimeout(page, 0)
      const applied = Object.fromEntries(storage.map((x) => [(x as { origin: string }).origin, 1]))
      return { set: set.length, removed: remove.length, skipped: 0, storage_applied: applied, timed_out: false }
    })
  engine.pushes = []
  engine.active = 'e1'
  engine.supports = true
  engine.pulls = false
  engine.profile = []
  engine.storage = []
  engine.forgotten = []
  engine.nowMs = Date.now()
  engine.pageSize = 0
  engine.epoch = 1
  engine.connection.clear()
  engine.api = { web_profile_push: engine.push, web_profile_pull: engine.pull }
  engine.discovery = {}
  engine.push
    .mockReset()
    .mockImplementation(async (device: string, epoch: number, changes: CookieChange[], storage: OriginStorage[], basis?: number) => {
      engine.pushes.push(structuredClone({ device, epoch, changes, storage, basis }))
      return { applied: changes.length, sites: [], epoch: Math.max(epoch, 1), profile: 'shared', refused: [] }
    })
  // As the engine answers (209 §4.4): each cookie with its site, row writer and time; the sites covered; the forget
  // markers; its clock; pages of `pageSize` cookies (the cursor is the next index).
  engine.pull.mockReset().mockImplementation(async (sites?: string[] | null, cursor?: string | null) => {
    const siteOfCookie = (c: Cookie) => (c.domain === 'other.org' ? 'other.org' : 'example.com')
    const all = engine.profile.filter((c) => !sites || sites.includes(siteOfCookie(c)))
    const from = Number(cursor ?? 0)
    const to = engine.pageSize > 0 ? Math.min(from + engine.pageSize, all.length) : all.length
    const first = from === 0
    return {
      epoch: engine.epoch,
      sites: first ? (sites ?? [...new Set([...all.map(siteOfCookie), ...engine.storage.map((s) => s.site)])]) : [],
      cookies: all.slice(from, to).map((c) => ({ ...c, site: siteOfCookie(c), writer: 'obscura:studio', updated_ms: 1 })),
      storage: first ? engine.storage.filter((s) => !sites || sites.includes(s.site)).map((s) => ({ ...s, captured_ms: 1, writer: 'obscura:studio', updated_ms: 1 })) : [],
      forgotten: first ? engine.forgotten.filter((f) => !sites || sites.includes(f.site)) : [],
      now_ms: engine.nowMs,
      next: to < all.length ? String(to) : null,
      profile: 'shared',
    }
  })
  events.changed = null
  useWebProfileMode.setState({ engine: null, mode: null })
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
  expect(engine.pushes[0]).toMatchObject({ device: DEVICE, epoch: 0, storage: [], basis: 0 })
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
  // Not connected at start, so the connect pull does not retry the seed before the first change.
  engine.supports = false
  await started()
  engine.supports = true
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

  // The structured `profile` first, else the engine's wordings as prefixes; any other error is a failure, retried.
  expect(refusal(new RpcError(-32000, 'refused', { profile: 'off' }))).toEqual({ kind: 'off' })
  expect(refusal(new RpcError(-32000, 'browser profile is off (config)'))).toEqual({ kind: 'off' })
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

// 209 D5 (P2): the profile into the tile store.

const other = (name: string, value: string) => cookie(name, value, { domain: 'other.org' })
const keys = (cs: readonly Cookie[]) => new Map(cs.map((c) => [cookieKey(c), c]))
/** Fire `WebProfileChanged` and let the pull it starts (and any write) land. */
async function profileChanged(sites: string[], epoch = engine.epoch): Promise<void> {
  const n = engine.pull.mock.calls.length + shell.clearProfile.mock.calls.length
  events.changed!({ type: 'WebProfileChanged', sites, epoch })
  await vi.waitFor(() => expect(engine.pull.mock.calls.length + shell.clearProfile.mock.calls.length).toBeGreaterThan(n))
  await settle()
}
/** Start with `web_profile_pull` advertised and let the connect pull land. */
async function startedPulling(): Promise<void> {
  engine.pulls = true
  await started()
  await vi.waitFor(() => expect(engine.pull).toHaveBeenCalled())
  await settle()
}

test('the plan writes only real differences: same cookies (as WebKit spells them), expired ones and other sites stay', () => {
  const a = cookie('a', '1')
  const b = cookie('b', '1')
  const o = other('o', '1')
  const jar = [a, b, o]
  const plan = planApply(
    jar,
    [{ ...a, expires: null }, cookie('b', '2'), cookie('c', '1'), cookie('d', '1', { expires: 1 })],
    null,
    new Set(),
    keys(jar),
  )
  expect(plan).toEqual({ set: [cookie('b', '2'), cookie('c', '1')], remove: [], localChanges: false })
  // SameSite: WebKit reads `None` back as `""`, so a store `""` is a profile `None` or `""`; `Lax` and `Strict` only
  // themselves, so a profile that lowered the attribute is written.
  const blank = cookie('s', '1', { same_site: '' })
  const sameSite = (store: Cookie, profile: Cookie) => planApply([store], [profile], null, new Set(), keys([store])).set
  expect(sameSite(blank, { ...blank, same_site: 'None' })).toEqual([])
  expect(sameSite(blank, blank)).toEqual([])
  expect(sameSite(blank, { ...blank, same_site: 'Lax' })).toEqual([{ ...blank, same_site: 'Lax' }])
  expect(sameSite({ ...blank, same_site: 'Strict' }, blank)).toEqual([blank])
  expect(sameSite({ ...blank, same_site: 'Lax' }, { ...blank, same_site: 'Strict' })).toHaveLength(1)
  // A site the pull holds nothing of is forgotten: all of its cookies go, no other site's.
  expect(planApply(jar, [], ['example.com'], new Set(), keys(jar)).remove).toEqual([
    { name: 'a', domain: 'app.example.com', path: '/' },
    { name: 'b', domain: 'app.example.com', path: '/' },
  ])
  // Within a live site only a cookie the profile was seen to hold, and no longer does, goes.
  expect(planApply(jar, [a], ['example.com'], new Set([cookieKey(a), cookieKey(b)]), keys(jar)).remove).toEqual([{ name: 'b', domain: 'app.example.com', path: '/' }])
  expect(planApply(jar, [a], ['example.com'], new Set([cookieKey(a)]), keys(jar)).remove).toEqual([])
  // A change of this device not pushed yet wins over the profile: neither overwritten nor removed.
  const mine = [cookie('a', 'typed-here'), b]
  expect(planApply(mine, [cookie('a', '2')], ['example.com'], new Set(), keys(jar))).toEqual({ set: [], remove: [], localChanges: true })
  // The longest named site owns a cookie.
  expect(siteOf('x.user.github.io', ['github.io', 'user.github.io'])).toBe('user.github.io')
  expect(siteOf('.Example.com', ['example.com'])).toBe('example.com')
  expect(siteOf('notexample.com', ['example.com'])).toBeNull()
})

test('on connect the profile is pulled after the seed and only its differences written; a load-finished then pushes nothing back', async () => {
  shell.store = [cookie('a', '1'), cookie('b', '1')]
  engine.profile = [cookie('a', '1'), cookie('b', `${SECRET}-agent`), cookie('c', '1')]
  engine.profile = [cookie('a', '1'), cookie('b', `${SECRET}-agent`), cookie('c', '1', { same_site: 'None' })]
  // WebKit reads a written `None` back as `""`: the fold takes what the store reads, not what was sent.
  shell.normalise = (c) => ({ ...c, same_site: c.same_site === 'None' ? '' : c.same_site })
  await startedPulling()
  expect(engine.pull).toHaveBeenCalledTimes(1)
  expect(engine.pull.mock.calls[0]).toEqual([undefined])
  expect(shell.writeProfile).toHaveBeenCalledTimes(1)
  expect(shell.writeProfile.mock.calls[0]).toEqual([[cookie('b', `${SECRET}-agent`), cookie('c', '1', { same_site: 'None' })], [], []])
  // A later pull of the same profile writes nothing: `""` in the store is the profile's `None`.
  await profileChanged(['example.com'])
  expect(shell.writeProfile).toHaveBeenCalledTimes(1)
  expect(engine.push).not.toHaveBeenCalled()

  vi.useFakeTimers()
  shell.storage.t1 = { origin: 'https://app.example.com', local_storage: [] }
  shell.state!({ label: 'web-t1', url: 'https://app.example.com/', title: 'App', loading: false })
  await vi.advanceTimersByTimeAsync(PUSH_DEBOUNCE_MS)
  expect(shell.readProfile.mock.calls.length).toBeGreaterThan(3)
  // The load's storage capture goes; no cookie does.
  expect(engine.pushes.flatMap((p) => p.changes)).toEqual([])
})

test('a push followed by its own WebProfileChanged writes nothing back into the store', async () => {
  await startedPulling()
  engine.push.mockImplementationOnce(async (device: string, epoch: number, changes: CookieChange[], storage: OriginStorage[]) => {
    engine.pushes.push(structuredClone({ device, epoch, changes, storage }))
    for (const c of changes) if (c.op === 'set') engine.profile.push(c.cookie)
    return { applied: changes.length, sites: ['example.com'], epoch: 1, profile: 'shared' }
  })
  shell.store = [cookie('sid', SECRET)]
  await changed()
  expect(engine.pushes).toHaveLength(1)
  await profileChanged(['example.com'])
  expect(engine.pull).toHaveBeenLastCalledWith(['example.com'])
  expect(shell.writeProfile).not.toHaveBeenCalled()
  expect(useWebProfileMode.getState()).toEqual({ engine: 'e1', mode: 'shared' })
})

test('a forgotten site leaves the store, other sites stay, and the removal is not pushed back', async () => {
  shell.store = [cookie('a', '1'), cookie('b', '1'), other('o', '1')]
  engine.profile = [cookie('a', '1'), cookie('b', '1'), other('o', '1')]
  await startedPulling()
  expect(shell.writeProfile).not.toHaveBeenCalled()

  engine.profile = [other('o', '1')]
  await profileChanged(['example.com'])
  expect(shell.writeProfile).toHaveBeenCalledTimes(1)
  expect(shell.writeProfile.mock.calls[0]).toEqual([
    [],
    [
      { name: 'a', domain: 'app.example.com', path: '/' },
      { name: 'b', domain: 'app.example.com', path: '/' },
    ],
    [],
  ])
  expect(shell.store).toEqual([other('o', '1')])
  await changed()
  expect(engine.push).not.toHaveBeenCalled()
})

test('a newer epoch in WebProfileChanged clears the store first, then adopts the epoch and writes the new profile', async () => {
  local.set('theuth.web-profile.epoch.e1', '1')
  shell.store = [cookie('old', SECRET)]
  await startedPulling()
  expect(shell.clearProfile).not.toHaveBeenCalled()

  engine.epoch = 2
  engine.profile = [cookie('fresh', '1')]
  await profileChanged([], 2)
  await vi.waitFor(() => expect(shell.writeProfile).toHaveBeenCalledTimes(1))
  expect(shell.clearProfile).toHaveBeenCalledTimes(1)
  expect(shell.clearProfile.mock.invocationCallOrder[0]).toBeLessThan(shell.writeProfile.mock.invocationCallOrder[0]!)
  expect(shell.writeProfile.mock.calls[0]).toEqual([[cookie('fresh', '1')], [], []])
  expect(local.get('theuth.web-profile.epoch.e1')).toBe('2')
  expect(shell.store).toEqual([cookie('fresh', '1')])
  // The same epoch again clears nothing more, and nothing of the old store was pushed.
  events.changed!({ type: 'WebProfileChanged', sites: [], epoch: 2 })
  await settle()
  expect(shell.clearProfile).toHaveBeenCalledTimes(1)
  expect(engine.push).not.toHaveBeenCalled()
})

test('this device\'s own sign out everywhere clears its store even without a saved epoch', async () => {
  shell.store = [cookie('sid', SECRET)]
  await started()
  expect(local.get('theuth.web-profile.epoch.e1')).toBeUndefined()
  signedOutEverywhere(3)
  await vi.waitFor(() => expect(shell.clearProfile).toHaveBeenCalledTimes(1))
  await settle()
  expect(local.get('theuth.web-profile.epoch.e1')).toBe('3')
  await changed()
  expect(engine.push).not.toHaveBeenCalled()
})

test('a pull answering `profile: "off"` writes nothing; without the native layer nothing is pulled at all', async () => {
  engine.pull.mockResolvedValueOnce({ epoch: 1, sites: [], cookies: [], storage: [], profile: 'off' })
  engine.profile = [cookie('a', '1')]
  await startedPulling()
  expect(shell.writeProfile).not.toHaveBeenCalled()
  expect(useWebProfileMode.getState()).toEqual({ engine: 'e1', mode: 'off' })
  stop()

  shell.has = false
  engine.pull.mockClear()
  stop = startWebProfileSync()
  await settle()
  expect(events.changed).toBeNull()
  expect(engine.pull).not.toHaveBeenCalled()
})

test('a failed write keeps the pull for a retry and logs no value', async () => {
  const spies = (['log', 'info', 'warn', 'error', 'debug'] as const).map((m) => vi.spyOn(console, m).mockImplementation(() => {}))
  shell.writeProfile.mockRejectedValueOnce(new Error(`web_profile_write: bad cookie ${SECRET}`))
  engine.profile = [cookie('sid', SECRET)]
  engine.pulls = true
  vi.useFakeTimers()
  stop = startWebProfileSync()
  await vi.waitFor(() => expect(shell.writeProfile).toHaveBeenCalledTimes(1))
  await settle()
  await vi.advanceTimersByTimeAsync(RETRY_MS)
  await vi.waitFor(() => expect(shell.writeProfile).toHaveBeenCalledTimes(2))
  await settle()
  expect(shell.store).toEqual([cookie('sid', SECRET)])
  expect(JSON.stringify(spies.flatMap((s) => s.mock.calls))).not.toContain(SECRET)
})

// 209 P2 review fixes.

/** Let timers already due (a fake page's re-set) run, then every promise chain. */
async function tick(): Promise<void> {
  await new Promise((r) => setTimeout(r, 0))
  await settle()
}

test('this install has one id, kept in localStorage: every push names `<device>-<first 6 of it>`', async () => {
  await started()
  shell.store = [cookie('sid', SECRET)]
  await changed()
  const id = local.get('theuth.device.id')!
  expect(id).toMatch(/^[0-9a-f]{8}-/)
  expect(engine.pushes[0]!.device).toBe(`iPad-${id.slice(0, 6)}`)
  expect(profileDevice()).toBe(`iPad-${id.slice(0, 6)}`)
  expect(local.get('theuth.device.id')).toBe(id)
})

test('pulls are paged until `next` is null; every push carries the engine clock of the last complete pull as basis_ms', async () => {
  engine.pageSize = 1
  engine.profile = [cookie('a', '1'), cookie('b', '1')]
  engine.nowMs = 1_795_000_000_000
  await startedPulling()
  expect(engine.pull.mock.calls).toEqual([[undefined], [null, '1']])
  expect(shell.writeProfile.mock.calls[0]![0]).toEqual([cookie('a', '1'), cookie('b', '1')])
  shell.store = [...shell.store, cookie('c', '1')]
  await changed()
  expect(engine.push.mock.calls[0]![4]).toBe(1_795_000_000_000)
  expect(engine.pushes[0]!.changes).toEqual([{ op: 'set', cookie: cookie('c', '1') }])
})

test('a site forgotten while this device was away leaves the store on the next whole pull, once', async () => {
  shell.store = [cookie('a', '1'), other('o', SECRET)]
  engine.profile = [cookie('a', '1')]
  engine.forgotten = [{ site: 'other.org', forgotten_ms: engine.nowMs - 1_000 }]
  await startedPulling()
  expect(shell.writeProfile.mock.calls[0]).toEqual([[], [{ name: 'o', domain: 'other.org', path: '/' }], []])
  expect(shell.store).toEqual([cookie('a', '1')])
  await changed()
  expect(engine.push).not.toHaveBeenCalled()

  // Signed in again here since: the next whole pull (a reconnect) applies the same forget no more.
  shell.store = [cookie('a', '1'), other('o', 'again')]
  await changed()
  expect(engine.pushes).toHaveLength(1)
  engine.api = { web_profile_push: engine.push, web_profile_pull: engine.pull }
  for (const cb of engine.connection) cb()
  await vi.waitFor(() => expect(engine.pull).toHaveBeenCalledTimes(2))
  await settle()
  expect(shell.writeProfile).toHaveBeenCalledTimes(1)
  expect(shell.store).toContainEqual(other('o', 'again'))

  // Forgotten and signed into again elsewhere since: what the store kept of the site goes, the profile's comes in, and
  // a store cookie equal to the profile's stays as it is (unpushed changes of the site included).
  const kept = other('k', '1')
  const jar = [other('old', '1'), kept, other('mine', 'typed')]
  const plan = planApply(jar, [kept, other('mine', 'profile'), other('n', '1')], null, new Set(), keys([other('old', '1'), kept]), {
    universe: ['other.org'],
    drop: new Set(['other.org']),
  })
  expect(plan.set).toEqual([other('mine', 'profile'), other('n', '1')])
  expect(plan.remove).toEqual([{ name: 'old', domain: 'other.org', path: '/' }])
})

test('a site the push answers `refused` leaves the store, and its removal is not pushed back', async () => {
  shell.store = [cookie('a', '1'), other('o', '1')]
  await startedPulling()
  engine.push.mockImplementationOnce(async (device: string, epoch: number, changes: CookieChange[], storage: OriginStorage[]) => {
    engine.pushes.push(structuredClone({ device, epoch, changes, storage }))
    return { applied: 0, sites: [], epoch: 1, profile: 'shared', refused: ['example.com'] }
  })
  shell.store = [cookie('a', SECRET), other('o', '1')]
  await changed()
  await vi.waitFor(() => expect(shell.writeProfile).toHaveBeenCalledTimes(1))
  expect(shell.writeProfile.mock.calls[0]).toEqual([[], [{ name: 'a', domain: 'app.example.com', path: '/' }], []])
  expect(shell.store).toEqual([other('o', '1')])
  await changed()
  expect(engine.pushes).toHaveLength(1)
})

test('a scoped pull removes only what the answer names: never by suffix of the asked site alone', async () => {
  shell.store = [cookie('a', '1')]
  engine.profile = [cookie('a', '1')]
  await startedPulling()
  // The engine names no site for the asked one (an older engine, a malformed name): nothing is removed.
  engine.pull.mockResolvedValueOnce({ epoch: 1, sites: [], cookies: [], storage: [], forgotten: [], now_ms: 1, next: null, profile: 'shared' })
  await profileChanged(['example.com'])
  expect(shell.writeProfile).not.toHaveBeenCalled()
  // A live child site keeps its cookies when the bare suffix is forgotten.
  const jar = [cookie('x', '1', { domain: 'user.github.io' }), cookie('y', '1', { domain: 'github.io' })]
  expect(planApply(jar, [], ['github.io'], new Set(), keys(jar), { universe: ['github.io', 'user.github.io'] }).remove).toEqual([
    { name: 'y', domain: 'github.io', path: '/' },
  ])
})

test('two devices: a live page re-setting a cookie a pull wrote makes it contended on the second time; pulls then leave it, pushes go on', async () => {
  shell.store = [cookie('sid', 'page')]
  engine.profile = [cookie('sid', 'other-1')]
  // This device's page sets its own value again right after every write of the profile's.
  shell.afterWrite = () => {
    shell.store = shell.store.map((c) => (c.name === 'sid' ? cookie('sid', 'page') : c))
  }
  await startedPulling()
  expect(shell.writeProfile).toHaveBeenCalledTimes(1)
  await tick()
  await changed()
  expect(engine.pushes.at(-1)!.changes).toEqual([{ op: 'set', cookie: cookie('sid', 'page') }])

  // The other device's page answers the same way: the profile flips back.
  engine.profile = [cookie('sid', 'other-2')]
  await profileChanged(['example.com'])
  expect(shell.writeProfile).toHaveBeenCalledTimes(2)
  await tick()
  await changed()
  expect(engine.pushes).toHaveLength(2)

  engine.profile = [cookie('sid', 'other-3')]
  await profileChanged(['example.com'])
  expect(shell.writeProfile).toHaveBeenCalledTimes(2)
  expect(shell.store).toEqual([cookie('sid', 'page')])
  // This device's own changes still go out.
  shell.store = [cookie('sid', 'page-2')]
  await changed()
  expect(engine.pushes.at(-1)!.changes).toEqual([{ op: 'set', cookie: cookie('sid', 'page-2') }])

  // After CONTENDED_MS pulls write it again.
  vi.useFakeTimers({ now: Date.now() + CONTENDED_MS, toFake: ['Date'] })
  engine.profile = [cookie('sid', 'other-4')]
  await profileChanged(['example.com'])
  expect(shell.writeProfile).toHaveBeenCalledTimes(3)
})

test('a cookie WebKit keeps under another path is written once: the read-back key becomes its alias', async () => {
  shell.normalise = (c) => (c.path === '/app/' ? { ...c, path: '/app' } : c)
  engine.profile = [cookie('k', SECRET, { path: '/app/' })]
  await startedPulling()
  expect(shell.writeProfile).toHaveBeenCalledTimes(1)
  expect(shell.store).toEqual([cookie('k', SECRET, { path: '/app' })])
  await profileChanged(['example.com'])
  await profileChanged(['example.com'])
  expect(shell.writeProfile).toHaveBeenCalledTimes(1)
  await changed()
  expect(engine.push).not.toHaveBeenCalled()
  // Removed from the profile: the store's cookie under the alias goes.
  engine.profile = []
  await profileChanged(['example.com'])
  expect(shell.writeProfile.mock.calls[1]).toEqual([[], [{ name: 'k', domain: 'app.example.com', path: '/app' }], []])
})

test('a write that times out half-way folds what it applied: nothing of it is pushed, the rest is written on the retry', async () => {
  const spies = (['log', 'info', 'warn', 'error', 'debug'] as const).map((m) => vi.spyOn(console, m).mockImplementation(() => {}))
  engine.profile = [cookie('a', SECRET), cookie('b', SECRET)]
  engine.pulls = true
  shell.writeProfile.mockImplementationOnce(async (set: Cookie[]) => {
    shell.store = [...shell.store, set[0]!]
    return { set: 1, removed: 0, skipped: 0, storage_applied: {}, timed_out: true }
  })
  vi.useFakeTimers()
  stop = startWebProfileSync()
  await vi.waitFor(() => expect(shell.writeProfile).toHaveBeenCalledTimes(1))
  await settle()
  shell.changed!()
  await vi.advanceTimersByTimeAsync(0)
  expect(engine.push).not.toHaveBeenCalled()
  await vi.advanceTimersByTimeAsync(RETRY_MS)
  await vi.waitFor(() => expect(shell.writeProfile).toHaveBeenCalledTimes(2))
  expect(shell.writeProfile.mock.calls[1]![0]).toEqual([cookie('b', SECRET)])
  expect(shell.store).toEqual([cookie('a', SECRET), cookie('b', SECRET)])
  expect(JSON.stringify(spies.flatMap((s) => s.mock.calls))).not.toContain(SECRET)
})

test('a write the store partly refuses is warned by count, and the refused cookie is not pushed as removed', async () => {
  const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
  engine.profile = [cookie('a', '1'), cookie('__Host-bad', '1', { secure: false })]
  shell.writeProfile.mockImplementationOnce(async (set: Cookie[]) => {
    shell.store = [...shell.store, set[0]!]
    return { set: 1, removed: 0, skipped: 1, storage_applied: {}, timed_out: false }
  })
  await startedPulling()
  expect(warn.mock.calls.map((c) => String(c[0]))).toContain('web_profile_write: 1 of 2 cookies refused by the store')
  await changed()
  expect(engine.push).not.toHaveBeenCalled()
})

test('a rejected write is folded too: what the shell applied before failing is not pushed as this device\'s change', async () => {
  vi.spyOn(console, 'warn').mockImplementation(() => {})
  engine.profile = [cookie('a', '1'), cookie('b', '1')]
  shell.writeProfile.mockImplementationOnce(async (set: Cookie[]) => {
    shell.store = [...shell.store, set[0]!]
    throw 'the main thread did not run the web view call within 5 s'
  })
  await startedPulling()
  await changed()
  expect(engine.push).not.toHaveBeenCalled()
})

test('profile localStorage flows into an open tile of its origin; the next capture of it is not pushed back, a change of the tile is', async () => {
  await startedPulling()
  vi.useFakeTimers()
  shell.state!({ label: 'web-t1', url: 'https://app.example.com/home', title: 'App', loading: true })
  engine.storage = [
    { origin: 'https://app.example.com', local_storage: [['t', STORE_SECRET]], site: 'example.com' },
    { origin: 'https://closed.example.com', local_storage: [['t', 'x']], site: 'example.com' },
  ]
  await profileChanged(['example.com'])
  expect(shell.writeProfile).toHaveBeenCalledTimes(1)
  expect(shell.writeProfile.mock.calls[0]![2]).toEqual([{ origin: 'https://app.example.com', local_storage: [['t', STORE_SECRET]] }])

  // The load finishes with what was written: nothing goes back.
  shell.storage.t1 = { origin: 'https://app.example.com', local_storage: [['t', STORE_SECRET]] }
  shell.state!({ label: 'web-t1', url: 'https://app.example.com/home', title: 'App', loading: false })
  await vi.advanceTimersByTimeAsync(PUSH_DEBOUNCE_MS)
  expect(engine.push).not.toHaveBeenCalled()
  // The same snapshot pulled again is not written again.
  await profileChanged(['example.com'])
  expect(shell.writeProfile).toHaveBeenCalledTimes(1)

  // The tile changes it: that goes.
  shell.storage.t1 = { origin: 'https://app.example.com', local_storage: [['t', 'mine']] }
  shell.state!({ label: 'web-t1', url: 'https://app.example.com/home', title: 'App', loading: false })
  await vi.advanceTimersByTimeAsync(PUSH_DEBOUNCE_MS)
  expect(engine.pushes.map((p) => p.storage.map((s) => s.local_storage))).toEqual([[[['t', 'mine']]]])

  // A closed tile gets nothing.
  shell.closed!({ label: 'web-t1' })
  engine.storage = [{ origin: 'https://app.example.com', local_storage: [['t', 'agent']], site: 'example.com' }]
  await profileChanged(['example.com'])
  expect(shell.writeProfile).toHaveBeenCalledTimes(1)
})

test('a failed clear is retried with the backoff, and at once by a new sign out everywhere, without a reconnect', async () => {
  vi.spyOn(console, 'warn').mockImplementation(() => {})
  shell.store = [cookie('sid', SECRET)]
  await started()
  shell.clearProfile.mockRejectedValueOnce('web_profile_clear: store busy').mockRejectedValueOnce('web_profile_clear: store busy')
  vi.useFakeTimers()
  signedOutEverywhere(3)
  await vi.advanceTimersByTimeAsync(0)
  expect(shell.clearProfile).toHaveBeenCalledTimes(1)
  expect(shell.store).toEqual([cookie('sid', SECRET)])
  // The backoff tries again.
  await vi.advanceTimersByTimeAsync(RETRY_MS)
  expect(shell.clearProfile).toHaveBeenCalledTimes(2)
  // Pressed again (a new epoch): cleared at once, the epoch adopted, and pushes resume under it.
  signedOutEverywhere(4)
  await vi.advanceTimersByTimeAsync(0)
  expect(shell.clearProfile).toHaveBeenCalledTimes(3)
  expect(shell.store).toEqual([])
  expect(local.get('theuth.web-profile.epoch.e1')).toBe('4')
  shell.store = [cookie('new', '1')]
  shell.changed!()
  await vi.advanceTimersByTimeAsync(0)
  expect(engine.pushes).toEqual([expect.objectContaining({ epoch: 4, changes: [{ op: 'set', cookie: cookie('new', '1') }] })])
})

test('a shell without the browser profile (`unsupported`) stops the sync for the session: no retries', async () => {
  vi.spyOn(console, 'warn').mockImplementation(() => {})
  shell.readProfile.mockRejectedValue('unsupported: the browser profile needs macOS 14 or iOS 17 and later')
  engine.pulls = true
  vi.useFakeTimers()
  stop = startWebProfileSync()
  await vi.advanceTimersByTimeAsync(0)
  expect(shell.readProfile).toHaveBeenCalledTimes(1)
  shell.changed?.()
  shell.state?.({ label: 'web-a', url: 'https://a.example/', title: 'A', loading: false })
  await vi.advanceTimersByTimeAsync(5 * 60_000)
  expect(shell.readProfile).toHaveBeenCalledTimes(1)
  expect(engine.pull).not.toHaveBeenCalled()
  expect(engine.push).not.toHaveBeenCalled()
})
