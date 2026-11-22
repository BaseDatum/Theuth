// The tile's half of the shared browser profile (209 §4.2, §4.5, §5): this device's tile store goes to the engine as
// deltas. At start (and after an engine switch) one `web_profile_read` seeds the baseline without pushing it; on
// `web-profile://changed` (already debounced 2 s in the shell) the sync reads the store, diffs it against that
// baseline (key `(name, domain, path)`) and calls `web_profile_push` at once; a tile that finished loading has the
// store read and diffed the same way and its storage captured, and one about to close has its storage captured, both
// pushed `PUSH_DEBOUNCE_MS` after the last such event, and what is still
// captured when the page goes away is pushed best-effort. One push is in flight at a time; work that arrives meanwhile
// goes in the next one. A failed read or push keeps its delta and retries with a backoff capped at `MAX_RETRY_MS`.
//
// Accepted cost of the seed: cookies already in the tile store before this app launch (or before an engine switch)
// reach the profile only when they next change. Pushing them instead would overwrite, on every start, values another
// writer (the agent's browser) rotated since.
//
// The other way (209 D5, P2): on connect (after the seed) every site of the profile, and on each `WebProfileChanged`
// the sites it names, are read with `web_profile_pull`, compared with the store and only the real differences written
// with `web_profile_write`. Echo suppression both ways: a pulled cookie equal to the store's is not written (what this
// device pushed comes back that way), a key the store changed since the baseline is left to the next push, and what
// was written is folded into the baseline, so the next re-read does not push it back as this device's change. A
// newer epoch (sign out everywhere) clears the store first.
//
// Values (cookie values, storage values) live only in this closure's memory and the push/write params: never logged,
// never persisted, never in a store a component renders. The epoch alone is kept in localStorage, per engine.

import { create } from 'zustand'
import { useEngines } from '@/link/engines'
import { deviceName } from '@/lib/platform'
import type { Cookie, CookieChange, OriginStorage, ProfileMode, WebProfilePushResult } from '@/lib/web-profile-wire'
import * as native from '@/lib/web-view-native'
import type { Api } from '@/rpc/bindings/client'
import { RpcError } from '@/rpc/client'
import { type Discovery, ready, supports } from '@/rpc/discovery'
import { useConnection } from '@/store/connection'
import { on } from '@/store/sessions'

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

/** The site of a cookie `domain` among `sites`: the longest site the domain equals or is a subdomain of, else `null`.
 * The profile groups by registrable domain (209 §4.2); without a public suffix list here, the sites the engine named
 * stand in for it. */
export function siteOf(domain: string, sites: Iterable<string>): string | null {
  const d = domain.toLowerCase().replace(/^\./, '')
  let best: string | null = null
  for (const s of sites) if ((d === s || d.endsWith(`.${s}`)) && (best === null || s.length > best.length)) best = s
  return best
}

/** The same cookie as far as the tile store can tell: WebKit reads a default `Lax` and a `None` back as `""`, may put
 * `expires` a second off, and never keeps `partitioned` (209 D1, §4.5). */
const sameInStore = (a: Cookie, b: Cookie): boolean =>
  a.value === b.value &&
  !!a.host_only === !!b.host_only &&
  !!a.secure === !!b.secure &&
  !!a.http_only === !!b.http_only &&
  (!a.same_site || !b.same_site || a.same_site === b.same_site) &&
  (a.expires == null ? b.expires == null : b.expires != null && Math.abs(a.expires - b.expires) <= 1)

/** The ten cookie fields of 209 §4.1, without what a pull adds (`site`, `writer`, `updated_ms`). */
const bare = (c: Cookie): Cookie => ({
  name: c.name,
  value: c.value,
  domain: c.domain,
  host_only: c.host_only,
  path: c.path,
  expires: c.expires,
  secure: c.secure,
  http_only: c.http_only,
  same_site: c.same_site,
  partitioned: c.partitioned,
})

/** What `web_profile_write` gets: cookies to set and keys to remove. */
export type WritePlan = { set: Cookie[]; remove: Pick<Cookie, 'name' | 'domain' | 'path'>[]; localChanges: boolean }

/**
 * 209 D5: what to write into the tile store so it holds what the profile pulled.
 * - `pulled`: the cookies `web_profile_pull` answered; one that already expired is not written.
 * - `sites`: the sites pulled (a `WebProfileChanged`), or `null` for every site (connect). Only a named site loses
 *   cookies: all of the site's when the pull has none of it (forgotten, or every cookie removed), else the ones the
 *   profile was seen to hold (`known`) and no longer does. A pull of every site only sets.
 * - `baseline`: what the store held at the last push, seed or write. A key whose store cookie differs from it is a
 *   change of this device not pushed yet: left alone (`localChanges`), the next push carries it.
 */
export function planApply(
  jar: readonly Cookie[],
  pulled: readonly Cookie[],
  sites: readonly string[] | null,
  known: ReadonlySet<string>,
  baseline: ReadonlyMap<string, Cookie>,
  nowSecs = Date.now() / 1000,
): WritePlan {
  const store = keyed(jar)
  let localChanges = false
  /** The store's cookie under `key` is what the baseline says: this device has nothing unpushed there. */
  const settled = (key: string) => {
    const s = store.get(key)
    const b = baseline.get(key)
    const ok = s === undefined ? b === undefined : b !== undefined && sameCookie(s, b)
    if (!ok) localChanges = true
    return ok
  }
  const live = pulled.filter((c) => c.expires == null || c.expires > nowSecs)
  const set = live
    .filter((c) => {
      const key = cookieKey(c)
      const s = store.get(key)
      return !(s && sameInStore(s, c)) && settled(key)
    })
    .map(bare)
  const remove: WritePlan['remove'] = []
  if (sites !== null) {
    const pulledKeys = new Set(live.map(cookieKey))
    const held = new Set<string>()
    for (const c of live) {
      const s = siteOf(c.domain, sites)
      if (s) held.add(s)
    }
    for (const c of jar) {
      const key = cookieKey(c)
      const s = siteOf(c.domain, sites)
      if (s === null || pulledKeys.has(key)) continue
      if ((!held.has(s) || known.has(key)) && settled(key)) remove.push({ name: c.name, domain: c.domain, path: c.path })
    }
  }
  return { set, remove, localChanges }
}

/** What the connected engine last said of `browser.profile` (209 D6): no RPC reads the mode, so this is the `profile`
 * of the last push or pull result (or a `profile off` refusal) for `engine`; `null` until one answered. */
export const useWebProfileMode = create<{ engine: string | null; mode: ProfileMode | null }>()(() => ({ engine: null, mode: null }))
const setMode = (engine: string, mode: ProfileMode) => useWebProfileMode.setState({ engine, mode })

/** The running sync's hook for a sign out everywhere done on this device; `null` while no sync runs. */
let signedOut: ((epoch: number) => void) | null = null

/** 209 §6: `web_profile_clear` answered `epoch` on this device: clear this device's tile store as every other client
 * does, even when this device never pushed (and so holds no epoch to compare). */
export function signedOutEverywhere(epoch: number): void {
  signedOut?.(epoch)
}

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
 * 209 §4.5: push this device's tile store to the connected engine as it changes, and (209 D5) write the profile's
 * changes into the store. A no-op without the native layer; silent toward an engine without `web_profile_push`,
 * toward a connection that answered `profile off` (probed again on a real change after `OFF_REPROBE_MS`, or at once
 * on a new connection), and toward a connection whose sign out everywhere this shell could not apply
 * (`web_profile_clear` failed). Pulls need `web_profile_pull` too. Once per main window. Returns the stop.
 */
export function startWebProfileSync(): () => void {
  if (!native.nativeWebViews()) return () => {}
  let stopped = false
  /** The baseline, by `cookieKey`: the cookies last pushed, the seed read, or what a pull wrote; `null` until a
   * seed read succeeded. */
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
  /** The connection every site was last pulled from: a new one is pulled whole (209 D5 "on attach"). */
  let pulledFrom: unknown = null
  /** Pull every site on the next run. */
  let pullAll = false
  /** Sites a `WebProfileChanged` named, not pulled yet. */
  const pullSites = new Set<string>()
  /** The newest epoch a `WebProfileChanged` carried, not compared yet. */
  let eventEpoch: number | null = null
  /** An epoch this device's own sign out everywhere answered, not applied yet. */
  let ownSignOut: number | null = null
  /** The epoch whose sign out this store was last cleared for (one clear per sign out). */
  let clearedFor: number | null = null
  /** Keys a pull showed in the profile: one gone from a later pull of its site was removed there. */
  const known = new Set<string>()

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
  /** This connection answered `profile off` less than `OFF_REPROBE_MS` ago: nothing is read, pushed or pulled for it. */
  const isOff = (api: unknown, discovery: unknown) =>
    off !== null && off.api === api && off.discovery === discovery && Date.now() - off.at < OFF_REPROBE_MS
  const markOff = (api: unknown, discovery: unknown) => {
    off = { api, discovery, at: Date.now() }
    setMode(engine, 'off')
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

  /**
   * Take `epoch` as this engine's. A sign out everywhere since this device's epoch (`epoch` newer than a known one,
   * or `force`) empties the store first, once per epoch. A shell that cannot clear keeps its old epoch and baseline and
   * is `stuck` on this connection (nothing more pushed or written), so its cookies never sign the profile back in.
   * `false` when stuck.
   */
  async function adopt(api: unknown, epoch: number, force: boolean): Promise<boolean> {
    const saved = loadEpoch(engine)
    if ((force || (saved !== null && epoch > saved)) && clearedFor !== epoch) {
      try {
        await native.clearProfile()
      } catch (err) {
        stuck = api
        console.warn('web_profile_clear failed', String(err))
        return false
      }
      clearedFor = epoch
      last = new Map()
      sentStorage.clear()
      captured.clear()
      known.clear()
      cookiesDirty = false
      failures = 0
      // The profile of the new epoch (normally empty, or what was signed in since) flows in whole.
      pullAll = true
    }
    if (epoch !== saved) saveEpoch(engine, epoch)
    return true
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
      known.clear()
      pullSites.clear()
      eventEpoch = null
      ownSignOut = null
      seeding = seed()
    }
    await seeding
    if (last === null) {
      seeding = seed()
      await seeding
    }
    if (last === null) {
      retry()
      return
    }
    if (isOff(api, discovery)) return
    if (stuck === api) {
      captured.clear()
      return
    }
    if (pulledFrom !== api) {
      pulledFrom = api
      pullAll = true
    }
    // Push first: what this device changed reaches the profile before the profile is written back into the store.
    if (await push(api, discovery, last)) await pull(api, discovery)
  }

  /** Push the store's diff and the captured storage; `true` when nothing is left (pushed, or nothing to push). */
  async function push(api: Api, discovery: Discovery, base: Map<string, Cookie>): Promise<boolean> {
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
        return false
      }
    }
    const changes = read ? diffCookies(base, read) : []
    const storage = pendingStorage()
    captured.clear()
    if (changes.length === 0 && storage.length === 0) {
      failures = 0
      return true
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
        markOff(api, discovery)
        requeue()
        return false
      }
      if (why && why.epoch !== epoch && (why.kind === 'unknown-epoch' || epoch === null)) {
        // Never pushed to this engine, or it does not know our epoch: nothing here was signed out; the same changes
        // go under the engine's epoch.
        saveEpoch(engine, why.epoch)
        requeue()
        again = true
        return false
      }
      // Sign out everywhere happened elsewhere (209 §6): empty the store; nothing of it is pushed back.
      if (why?.kind === 'stale-epoch' && why.epoch !== epoch) return adopt(api, why.epoch, true)
      requeue()
      retry()
      console.warn('web_profile_push failed', e instanceof RpcError ? e.code : 'error')
      return false
    }
    // `profile` is new in the result (209 §4.4); an older engine without it signals off by the error alone.
    if ('profile' in result && result.profile === 'off') {
      markOff(api, discovery)
      requeue()
      return false
    }
    off = null
    setMode(engine, 'shared')
    failures = 0
    if (read) last = keyed(read)
    for (const s of storage) sentStorage.set(s.origin, JSON.stringify(s.local_storage))
    saveEpoch(engine, result.epoch)
    return true
  }

  /** 209 D5: pull what changed in the profile and write the differences into the store. */
  async function pull(api: Api, discovery: Discovery): Promise<void> {
    if (ownSignOut !== null) {
      const epoch = ownSignOut
      ownSignOut = null
      if (!(await adopt(api, epoch, true))) return
    }
    if (eventEpoch !== null) {
      const epoch = eventEpoch
      eventEpoch = null
      if (!(await adopt(api, epoch, false))) return
    }
    if (!pullAll && pullSites.size === 0) return
    if (!supports(discovery, { methods: ['web_profile_pull'] })) {
      pullAll = false
      pullSites.clear()
      return
    }
    const sites = pullAll ? null : [...pullSites].sort()
    pullAll = false
    pullSites.clear()
    const requeue = () => {
      if (sites === null) pullAll = true
      else for (const s of sites) pullSites.add(s)
    }
    let pulled: Cookie[]
    /** The sites the answer covers, each whole: the asked ones (as the engine names them) for a scoped pull. */
    let covered: string[] | null
    try {
      const result = await api.web_profile_pull(sites ?? undefined)
      if (result.profile === 'off') {
        markOff(api, discovery)
        return
      }
      if (result.profile === 'shared') setMode(engine, 'shared')
      // A newer epoch here is a sign out everywhere this device missed: clear, then write what the pull holds.
      if (!(await adopt(api, result.epoch, false))) return
      pulled = result.cookies
      // A pull of every site only sets: a site answered with snapshots and no cookie must not empty the store.
      covered = sites && [...new Set([...sites, ...(result.sites ?? [])])]
    } catch (e) {
      if (refusal(e)?.kind === 'off') {
        markOff(api, discovery)
        return
      }
      requeue()
      retry()
      console.warn('web_profile_pull failed', e instanceof RpcError ? e.code : 'error')
      return
    }
    // Every site, none held: nothing to write, and no store read needed.
    if (covered === null && pulled.length === 0) {
      known.clear()
      return
    }
    let jar: Cookie[]
    try {
      jar = await native.readProfile()
    } catch (e) {
      requeue()
      retry()
      console.warn('web_profile_read failed', String(e))
      return
    }
    const base = last ?? new Map<string, Cookie>()
    const plan = planApply(jar, pulled, covered, known, base)
    // `known` follows the profile: a whole pull replaces it, a pull of some sites replaces those sites' keys.
    if (covered === null) known.clear()
    else for (const key of [...known]) if (siteOf(JSON.parse(key)[1] as string, covered) !== null) known.delete(key)
    for (const c of pulled) known.add(cookieKey(c))
    // A change of this device left alone goes out with the next push.
    if (plan.localChanges) {
      cookiesDirty = true
      again = true
    }
    if (plan.set.length === 0 && plan.remove.length === 0) {
      failures = 0
      return
    }
    try {
      await native.writeProfile(plan.set, plan.remove)
    } catch {
      requeue()
      retry()
      // The error is the shell's and the arguments carry values: neither is logged.
      console.warn('web_profile_write failed')
      return
    }
    failures = 0
    // Fold what was written into the baseline as the store reads it back, so the next read does not push it.
    const folded = last ?? base
    for (const c of plan.set) folded.set(cookieKey(c), c)
    for (const r of plan.remove) folded.delete(cookieKey(r))
    try {
      const after = keyed(await native.readProfile())
      for (const key of [...plan.set, ...plan.remove].map(cookieKey)) {
        const now = after.get(key)
        if (now) folded.set(key, now)
        else folded.delete(key)
      }
    } catch {
      // The plan itself stays folded.
    }
    last = folded
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
      // A finished load also re-reads the store: on the iPad simulator (iOS 26) the shell's cookie observer
      // never fired, so a sign-in's `Set-Cookie` reached the profile only through this (209 P1 proof).
      cookiesDirty = true
      void native.captureStorage(native.labelId(s.label)).then(capture, () => undefined).finally(() => schedule(PUSH_DEBOUNCE_MS))
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
  // 209 D5: a profile change elsewhere (or this device's own push echoed) names its sites; the run pulls them.
  offs.push(
    on('browser.web-profile-changed@1', 'WebProfileChanged', (e) => {
      for (const s of e.sites) pullSites.add(s)
      eventEpoch = Math.max(eventEpoch ?? e.epoch, e.epoch)
      flush()
    }),
  )
  // A connection this sync has not pulled from (connect, reconnect, engine switch) is pulled whole once usable. The
  // connection store is per engine, so its subscription follows the active engine.
  const onConnection = () => {
    const target = pushTarget()
    if (target && target.api !== pulledFrom) flush()
  }
  let offConnection = useConnection.subscribe(onConnection)
  offs.push(
    useEngines.subscribe(() => {
      offConnection()
      offConnection = useConnection.subscribe(onConnection)
      onConnection()
    }),
    () => offConnection(),
  )
  const hook = (epoch: number) => {
    ownSignOut = epoch
    flush()
  }
  signedOut = hook
  offs.push(() => {
    if (signedOut === hook) signedOut = null
  })
  // Connected already: the first pull follows the seed.
  void seeding.then(onConnection)
  return () => {
    stopped = true
    clearTimeout(timer)
    for (const off of offs) off()
  }
}
