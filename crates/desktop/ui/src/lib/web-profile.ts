// The tile's half of the shared browser profile (209 §4.2, §4.5, §5): this device's tile store goes to the engine as
// deltas. At start (and after an engine switch) one `web_profile_read` seeds the baseline without pushing it; on
// `web-profile://changed` (already debounced 2 s in the shell) the sync reads the store, diffs it against that
// baseline (key `(name, domain, path)`) and calls `web_profile_push` at once; a tile that finished loading or is about
// to close has its storage captured and pushed `PUSH_DEBOUNCE_MS` after the last such capture, and what is still
// captured when the page goes away is pushed best-effort. One push is in flight at a time; work that arrives meanwhile
// goes in the next one. A failed read or push keeps its delta and retries with a backoff capped at `MAX_RETRY_MS`.
//
// Accepted cost of the seed: cookies already in the tile store before this app launch (or before an engine switch)
// reach the profile only when they next change. Pushing them instead would overwrite, on every start, values another
// writer (the agent's browser) rotated since.
//
// Values (cookie values, storage values) live only in this closure's memory and the push params: never logged, never
// persisted, never in a store a component renders. The epoch alone is kept in localStorage, per engine.

import { useEngines } from '@/link/engines'
import { deviceName } from '@/lib/platform'
import type { Cookie, CookieChange, OriginStorage, WebProfilePushResult } from '@/lib/web-profile-wire'
import * as native from '@/lib/web-view-native'
import { RpcError } from '@/rpc/client'
import { ready, supports } from '@/rpc/discovery'
import { useConnection } from '@/store/connection'

/** Storage captured on load or close waits this long for more before it is pushed (209 D3). */
export const PUSH_DEBOUNCE_MS = 2_000
/** The first retry of a read or push that failed (no connection, a failed call); each further one doubles. */
export const RETRY_MS = 10_000
/** The longest wait between two retries. */
export const MAX_RETRY_MS = 60_000
/** After `profile off`, the next real change probes the same connection again at most this often. */
export const OFF_REPROBE_MS = 60_000
/** One origin's snapshot over this is dropped whole, never cut (209 §4.1). */
export const STORAGE_MAX_BYTES = 96 * 1024

/** The identity of a cookie in a store, the profile and obscura: `(name, domain, path)` (209 §4.2). */
export const cookieKey = (c: Pick<Cookie, 'name' | 'domain' | 'path'>): string => JSON.stringify([c.name, c.domain, c.path])

const sameCookie = (a: Cookie, b: Cookie): boolean =>
  a.value === b.value &&
  a.host_only === b.host_only &&
  a.expires === b.expires &&
  a.secure === b.secure &&
  a.http_only === b.http_only &&
  a.same_site === b.same_site &&
  a.partitioned === b.partitioned

/** The changes from `last` (what was pushed) to `now` (the store): a new or changed cookie is a `set`, a gone one a `remove`. */
export function diffCookies(last: ReadonlyMap<string, Cookie>, now: readonly Cookie[]): CookieChange[] {
  const changes: CookieChange[] = []
  const seen = new Set<string>()
  for (const c of now) {
    const key = cookieKey(c)
    seen.add(key)
    const old = last.get(key)
    if (!old || !sameCookie(old, c)) changes.push({ op: 'set', cookie: c })
  }
  for (const [key, c] of last) if (!seen.has(key)) changes.push({ op: 'remove', name: c.name, domain: c.domain, path: c.path })
  return changes
}

const keyed = (cookies: readonly Cookie[]): Map<string, Cookie> => new Map(cookies.map((c) => [cookieKey(c), c]))

/**
 * Why the engine refused a push (209 §4.4): `stale-epoch` (the profile was signed out everywhere since this device's
 * epoch) or `unknown-epoch` (the engine does not know this device's epoch), each with the engine's current epoch; or
 * the profile is off (`browser.profile: off`).
 */
export type Refusal = { kind: 'stale-epoch' | 'unknown-epoch'; epoch: number } | { kind: 'off' }

/** The engine's wording of "the profile is off", matched whole: the push result's `profile` is the primary signal. */
const OFF_MESSAGES = new Set(['browser profile is off', 'browser.profile is off', 'browser profile unavailable: browser.profile is off'])

/** Read a `web_profile_push` error: `stale-epoch`/`unknown-epoch` (epoch in `data`, `data.epoch` or the message), or
 * `profile off`. An epoch refusal without a readable epoch is `null`: a plain failure, retried with the backoff. */
export function refusal(e: unknown): Refusal | null {
  if (!(e instanceof RpcError)) return null
  for (const kind of ['stale-epoch', 'unknown-epoch'] as const) {
    if (!e.message.includes(kind)) continue
    const d = e.data
    const raw =
      typeof d === 'number'
        ? d
        : d !== null && typeof d === 'object' && 'epoch' in d
          ? Number(d.epoch)
          : Number(new RegExp(`${kind}\\D*(\\d+)`).exec(e.message)?.[1])
    return Number.isSafeInteger(raw) ? { kind, epoch: raw } : null
  }
  return OFF_MESSAGES.has(e.message.trim()) ? { kind: 'off' } : null
}

const epochKey = (engine: string) => `theuth.web-profile.epoch.${engine}`
function loadEpoch(engine: string): number | null {
  try {
    const n = Number(globalThis.localStorage?.getItem(epochKey(engine)) ?? Number.NaN)
    return Number.isSafeInteger(n) ? n : null
  } catch {
    return null
  }
}
function saveEpoch(engine: string, epoch: number): void {
  try {
    globalThis.localStorage?.setItem(epochKey(engine), String(epoch))
  } catch {
    // The epoch is relearned from the next `stale-epoch`.
  }
}

/** An http(s) origin's snapshot, or `null` (another scheme, an opaque origin, over `STORAGE_MAX_BYTES`). */
function snapshot(s: native.StorageCapture): OriginStorage | null {
  if (!/^https?:\/\/[^/]+$/.test(s.origin)) return null
  if (new TextEncoder().encode(JSON.stringify(s.local_storage)).length > STORAGE_MAX_BYTES) return null
  return { origin: s.origin, local_storage: s.local_storage, captured_ms: Date.now() }
}

/** The connected engine's api when it can take a push now, else `null`. */
function pushTarget() {
  const { api, state, discovery } = useConnection.getState()
  if (!api || state !== 'open' || !ready(discovery) || !supports(discovery, { methods: ['web_profile_push'] })) return null
  return { api, discovery }
}

/**
 * 209 §4.5: push this device's tile store to the connected engine as it changes. A no-op without the native layer;
 * silent toward an engine without `web_profile_push`, toward a connection that answered `profile off` (probed again
 * on a real change after `OFF_REPROBE_MS`, or at once on a new connection), and toward a connection whose sign out
 * everywhere this shell could not apply (`web_profile_clear` failed). Once per main window. Returns the stop.
 */
export function startWebProfileSync(): () => void {
  if (!native.nativeWebViews()) return () => {}
  let stopped = false
  /** The baseline, by `cookieKey`: the cookies last pushed, or the seed read; `null` until a seed read succeeded. */
  let last: Map<string, Cookie> | null = null
  /** Origin → the `local_storage` JSON last pushed. */
  const sentStorage = new Map<string, string>()
  /** Captured, not yet pushed; the newest capture per origin. */
  const captured = new Map<string, OriginStorage>()
  let cookiesDirty = false
  let engine = useEngines.getState().active
  /** The connection that answered `profile off`, and when. */
  let off: { api: unknown; discovery: unknown; at: number } | null = null
  /** The connection whose `stale-epoch` this shell could not clear: nothing more is pushed to it. */
  let stuck: unknown = null
  let failures = 0
  let timer: ReturnType<typeof setTimeout> | undefined
  let running = false
  let again = false

  let seedGen = 0
  /** Read the store into the baseline without pushing it; a newer seed (engine switch) wins over an older one. */
  const seed = (): Promise<void> => {
    const gen = ++seedGen
    return native.readProfile().then(
      (cookies) => {
        if (gen === seedGen) last = keyed(cookies)
      },
      (e: unknown) => console.warn('web_profile_read failed', String(e)),
    )
  }
  let seeding = seed()

  const schedule = (ms: number) => {
    clearTimeout(timer)
    if (!stopped) timer = setTimeout(flush, ms)
  }
  const retry = () => schedule(Math.min(RETRY_MS * 2 ** failures++, MAX_RETRY_MS))
  const capture = (s: native.StorageCapture) => {
    const snap = snapshot(s)
    if (snap) captured.set(snap.origin, snap)
  }
  /** Captured snapshots that differ from what was last pushed for their origin. */
  const pendingStorage = () => [...captured.values()].filter((s) => sentStorage.get(s.origin) !== JSON.stringify(s.local_storage))
  const requeueStorage = (storage: readonly OriginStorage[]) => {
    for (const s of storage) if (!captured.has(s.origin)) captured.set(s.origin, s)
  }
  /** This connection answered `profile off` less than `OFF_REPROBE_MS` ago: nothing is read or pushed for it. */
  const isOff = (api: unknown, discovery: unknown) =>
    off !== null && off.api === api && off.discovery === discovery && Date.now() - off.at < OFF_REPROBE_MS

  function flush(): void {
    clearTimeout(timer)
    if (stopped) return
    if (running) {
      again = true
      return
    }
    running = true
    void run().finally(() => {
      running = false
      if (again) {
        again = false
        flush()
      }
    })
  }

  async function run(): Promise<void> {
    const target = pushTarget()
    if (!target) {
      if (cookiesDirty || captured.size > 0) retry()
      return
    }
    const { api, discovery } = target
    // Another engine may be another mesh: what was pushed before does not count there, and the store as it is now
    // is the new baseline (not pushed, as at start).
    const active = useEngines.getState().active
    if (active !== engine) {
      engine = active
      last = null
      sentStorage.clear()
      seeding = seed()
    }
    await seeding
    if (last === null) {
      seeding = seed()
      await seeding
    }
    const base = last
    if (base === null) {
      retry()
      return
    }
    if (isOff(api, discovery)) return
    if (stuck === api) {
      captured.clear()
      return
    }
    let read: Cookie[] | null = null
    if (cookiesDirty) {
      cookiesDirty = false
      try {
        read = await native.readProfile()
      } catch (e) {
        // The delta stays: the shell only signals again when the store changes again.
        cookiesDirty = true
        console.warn('web_profile_read failed', String(e))
        retry()
        return
      }
    }
    const changes = read ? diffCookies(base, read) : []
    const storage = pendingStorage()
    captured.clear()
    if (changes.length === 0 && storage.length === 0) {
      failures = 0
      return
    }
    const epoch = loadEpoch(engine)
    const requeue = () => {
      if (read) cookiesDirty = true
      requeueStorage(storage)
    }
    let result: WebProfilePushResult
    try {
      result = await api.web_profile_push(deviceName(), epoch ?? 0, changes, storage)
    } catch (e) {
      const why = refusal(e)
      if (why?.kind === 'off') {
        off = { api, discovery, at: Date.now() }
        requeue()
        return
      }
      if (why && why.epoch !== epoch && (why.kind === 'unknown-epoch' || epoch === null)) {
        // Never pushed to this engine, or it does not know our epoch: nothing here was signed out; the same changes
        // go under the engine's epoch.
        saveEpoch(engine, why.epoch)
        requeue()
        again = true
        return
      }
      if (why?.kind === 'stale-epoch' && why.epoch !== epoch) {
        // Sign out everywhere happened elsewhere (209 §6): empty the store. A shell that cannot keeps its baseline and
        // its old epoch and pushes nothing more to this connection, so its cookies never sign the profile back in.
        try {
          await native.clearProfile()
        } catch (err) {
          stuck = api
          console.warn('web_profile_clear failed', String(err))
          return
        }
        saveEpoch(engine, why.epoch)
        last = new Map()
        sentStorage.clear()
        captured.clear()
        cookiesDirty = false
        failures = 0
        return
      }
      requeue()
      retry()
      console.warn('web_profile_push failed', e instanceof RpcError ? e.code : 'error')
      return
    }
    // `profile` is new in the result (209 §4.4); an older engine without it signals off by the error alone.
    if ('profile' in result && result.profile === 'off') {
      off = { api, discovery, at: Date.now() }
      requeue()
      return
    }
    off = null
    failures = 0
    if (read) last = keyed(read)
    for (const s of storage) sentStorage.set(s.origin, JSON.stringify(s.local_storage))
    saveEpoch(engine, result.epoch)
  }

  /** The page is going away (app quit, window closed): push what is still captured, best-effort, without waiting. */
  const onLeave = () => {
    const target = pushTarget()
    if (stopped || captured.size === 0 || !target || last === null) return
    const { api, discovery } = target
    if (useEngines.getState().active !== engine || stuck === api || isOff(api, discovery)) return
    const storage = pendingStorage()
    captured.clear()
    if (storage.length === 0) return
    void api.web_profile_push(deviceName(), loadEpoch(engine) ?? 0, [], storage).then(
      () => {
        for (const s of storage) sentStorage.set(s.origin, JSON.stringify(s.local_storage))
      },
      () => requeueStorage(storage),
    )
  }

  const offs: (() => void)[] = []
  const keep = (p: Promise<() => void>) => void p.then((off) => (stopped ? off() : offs.push(off)))
  keep(
    native.onProfileChanged(() => {
      cookiesDirty = true
      flush()
    }),
  )
  keep(
    native.onState((s) => {
      if (s.loading) return
      void native.captureStorage(native.labelId(s.label)).then((c) => {
        capture(c)
        schedule(PUSH_DEBOUNCE_MS)
      }, () => undefined)
    }),
  )
  offs.push(
    native.onBeforeClose((c) => {
      capture(c)
      schedule(PUSH_DEBOUNCE_MS)
    }),
  )
  if (typeof globalThis.addEventListener === 'function') {
    for (const type of ['pagehide', 'beforeunload'] as const) {
      globalThis.addEventListener(type, onLeave)
      offs.push(() => globalThis.removeEventListener(type, onLeave))
    }
  }
  return () => {
    stopped = true
    clearTimeout(timer)
    for (const off of offs) off()
  }
}
