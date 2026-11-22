// The tile's half of the shared browser profile (209 §4.2, §4.5, §5): this device's tile store goes to the engine as
// deltas. On `web-profile://changed` (already debounced 2 s in the shell) the sync reads the store, diffs it against
// the cookies it last pushed (key `(name, domain, path)`), captures each live tile's localStorage and calls
// `web_profile_push` at once; a tile that finished loading or is about to close has its storage captured and pushed
// `PUSH_DEBOUNCE_MS` after the last such capture. One push is in flight at a time; work that arrives meanwhile goes
// in the next one.
//
// Values (cookie values, storage values) live only in this closure's memory and the push params: never logged, never
// persisted, never in a store a component renders. The epoch alone is kept in localStorage, per engine.

import { useEngines } from '@/link/engines'
import { deviceName } from '@/lib/platform'
import type { Cookie, CookieChange, OriginStorage } from '@/lib/web-profile-wire'
import * as native from '@/lib/web-view-native'
import { useWebViews } from '@/lib/web-views'
import { RpcError } from '@/rpc/client'
import { ready, supports } from '@/rpc/discovery'
import { useConnection } from '@/store/connection'

/** Storage captured on load or close waits this long for more before it is pushed (209 D3). */
export const PUSH_DEBOUNCE_MS = 2_000
/** A push that could not go out (no connection, a failed call) is tried again after this long. */
export const RETRY_MS = 10_000
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

/** Why the engine refused a push (209 §4.4): `stale-epoch` with its current epoch, or the profile is off (`browser.profile: off`). */
export type Refusal = { kind: 'stale-epoch'; epoch: number } | { kind: 'off' }

/** Read a `web_profile_push` error: `stale-epoch` (epoch in `data`, `data.epoch` or the message) or `profile off`. */
export function refusal(e: unknown): Refusal | null {
  if (!(e instanceof RpcError)) return null
  if (e.message.includes('stale-epoch')) {
    const d = e.data
    const raw = typeof d === 'number' ? d : d !== null && typeof d === 'object' && 'epoch' in d ? Number(d.epoch) : Number(/stale-epoch\D*(\d+)/.exec(e.message)?.[1])
    return Number.isSafeInteger(raw) ? { kind: 'stale-epoch', epoch: raw } : null
  }
  return /profile (is )?off/.test(e.message) ? { kind: 'off' } : null
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

/**
 * 209 §4.5: push this device's tile store to the connected engine as it changes. A no-op without the native layer;
 * silent toward an engine without `web_profile_push` and, once it answered `profile off`, toward that connection.
 * Once per main window. Returns the stop.
 */
export function startWebProfileSync(): () => void {
  if (!native.nativeWebViews()) return () => {}
  let stopped = false
  /** The cookies last pushed, by `cookieKey`; empty at start, so the first push carries the whole store. */
  let last = new Map<string, Cookie>()
  /** Origin → the `local_storage` JSON last pushed. */
  const sentStorage = new Map<string, string>()
  /** Captured, not yet pushed; the newest capture per origin. */
  const captured = new Map<string, OriginStorage>()
  let cookiesDirty = false
  let engine = useEngines.getState().active
  let offFor: unknown = null
  let timer: ReturnType<typeof setTimeout> | undefined
  let running = false
  let again = false

  const schedule = (ms: number) => {
    clearTimeout(timer)
    if (!stopped) timer = setTimeout(flush, ms)
  }
  const capture = (s: native.StorageCapture) => {
    const snap = snapshot(s)
    if (snap) captured.set(snap.origin, snap)
  }

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
    const { api, state, discovery } = useConnection.getState()
    if (!api || state !== 'open' || !ready(discovery) || !supports(discovery, { methods: ['web_profile_push'] })) {
      if (cookiesDirty || captured.size > 0) schedule(RETRY_MS)
      return
    }
    // Another engine may be another mesh: what was pushed before does not count there.
    const active = useEngines.getState().active
    if (active !== engine) {
      engine = active
      last = new Map()
      sentStorage.clear()
      cookiesDirty = true
    }
    if (offFor === api) {
      captured.clear()
      return
    }
    let read: Cookie[] | null = null
    if (cookiesDirty) {
      cookiesDirty = false
      try {
        read = await native.readProfile()
      } catch (e) {
        console.warn('web_profile_read failed', String(e))
      }
    }
    const changes = read ? diffCookies(last, read) : []
    const storage = [...captured.values()].filter((s) => sentStorage.get(s.origin) !== JSON.stringify(s.local_storage))
    captured.clear()
    if (changes.length === 0 && storage.length === 0) {
      if (read) last = keyed(read)
      return
    }
    const epoch = loadEpoch(engine)
    const requeue = () => {
      if (read) cookiesDirty = true
      for (const s of storage) if (!captured.has(s.origin)) captured.set(s.origin, s)
    }
    try {
      const r = await api.web_profile_push(deviceName(), epoch ?? 0, changes, storage)
      if (read) last = keyed(read)
      for (const s of storage) sentStorage.set(s.origin, JSON.stringify(s.local_storage))
      saveEpoch(engine, r.epoch)
    } catch (e) {
      const why = refusal(e)
      if (why?.kind === 'off') {
        offFor = api
        if (read) cookiesDirty = true
        return
      }
      if (why?.kind === 'stale-epoch') {
        saveEpoch(engine, why.epoch)
        if (epoch === null) {
          // Never pushed to this engine: nothing here was signed out; the same changes go under the current epoch.
          requeue()
          again = true
          return
        }
        // Sign out everywhere happened elsewhere (209 §6): empty the store. A shell without the command keeps its
        // cookies, which then count as pushed, so they do not sign the profile back in.
        sentStorage.clear()
        try {
          await native.clearProfile()
          last = new Map()
        } catch (err) {
          console.warn('web_profile_clear failed', String(err))
          last = keyed(read ?? (await native.readProfile().catch(() => [])))
        }
        return
      }
      requeue()
      schedule(RETRY_MS)
      console.warn('web_profile_push failed', e instanceof RpcError ? e.code : 'error')
    }
  }

  /** Every live tile's storage, then the push (the cookie event is already debounced by the shell). */
  const onChanged = async () => {
    cookiesDirty = true
    const ids = Object.keys(useWebViews.getState().views)
    await Promise.all(ids.map((id) => native.captureStorage(id).then(capture, () => undefined)))
    flush()
  }

  const offs: (() => void)[] = []
  const keep = (p: Promise<() => void>) => void p.then((off) => (stopped ? off() : offs.push(off)))
  keep(native.onProfileChanged(() => void onChanged()))
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
  return () => {
    stopped = true
    clearTimeout(timer)
    for (const off of offs) off()
  }
}
