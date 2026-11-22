// The tile's half of the shared browser profile (209 §4.2, §4.5, §5): this device's tile store goes to the engine as
// deltas. At start (and after an engine switch) one `web_profile_read` seeds the baseline without pushing it; on
// `web-profile://changed` (already debounced 2 s in the shell) the sync reads the store, diffs it against that
// baseline (key `(name, domain, path)`) and calls `web_profile_push` at once; a tile that finished loading has the
// store read and diffed the same way and its storage captured, and one about to close has its storage captured, both
// pushed `PUSH_DEBOUNCE_MS` after the last such event, and what is still
// captured when the page goes away is pushed best-effort. One push is in flight at a time; work that arrives meanwhile
// goes in the next one. A failed read or push keeps its delta and retries with a backoff capped at `MAX_RETRY_MS`.
// Every push names this install (`profileDevice`) and carries `basis_ms`, the engine's clock at this device's last
// complete pull, so the engine refuses a set of a site forgotten since; a refused site leaves the store.
//
// Accepted cost of the seed: cookies already in the tile store before this app launch (or before an engine switch)
// reach the profile only when they next change. Pushing them instead would overwrite, on every start, values another
// writer (the agent's browser) rotated since.
//
// The other way (209 D5, P2): on connect (after the seed) every site of the profile, and on each `WebProfileChanged`
// the sites it names, are read with `web_profile_pull` (paged until `next` is null), compared with the store and only
// the real differences written with `web_profile_write`, the pulled localStorage of origins with an open tile with
// them. Echo suppression both ways: a pulled cookie equal to the store's is not written (what this device pushed comes
// back that way), a key the store changed since the baseline is left to the next push, and what the store reads back
// after a write (also a failed or timed-out one) is folded into the baseline, so the next re-read does not push it back
// as this device's change. A key WebKit stores under another domain or path is remembered as an alias of the written
// one. A key a live page re-sets twice within `CONTENDED_MS`, each time within `RESET_WINDOW_MS` of a pull writing it,
// is not written from pulls for `CONTENDED_MS` (two devices' pages would otherwise ping-pong it through the profile).
// A site the profile forgot after this device last pushed it is deleted from the store. A newer epoch (sign out
// everywhere) clears the store first.
//
// Values (cookie values, storage values) live only in this closure's memory and the push/write params: never logged,
// never persisted, never in a store a component renders. Kept in localStorage: this install's id, and per engine the
// epoch and the engine's clock at the last complete pull (`basis_ms`).

import { create } from 'zustand'
import { useEngines } from '@/link/engines'
import { deviceName } from '@/lib/platform'
import type { Cookie, CookieChange, ForgottenSite, OriginStorage, ProfileMode, PulledStorage, WebProfilePullResult, WebProfilePushResult } from '@/lib/web-profile-wire'
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
/** A local change of a key read this soon after a pull wrote it is a page re-setting it. */
export const RESET_WINDOW_MS = 5_000
/** Two such re-sets of one key within this long make it contended: not written from pulls for this long. */
export const CONTENDED_MS = 10 * 60_000

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

/** A store `same_site` against the profile's: WebKit has no distinct `None` policy and reads it back as `""`, so a
 * store `""` is a profile `None` or `""`; `Lax` and `Strict` match only themselves (209 §4.5). */
export const sameSiteInStore = (store: string | undefined, profile: string | undefined): boolean => {
  const s = store ?? ''
  const p = profile ?? ''
  return s === p || (s === '' && p === 'None')
}

/** The store's cookie `s` is the profile's `p` as far as the tile store can tell: WebKit may put `expires` a second off
 * and never keeps `partitioned` (209 D1, §4.5). */
const sameInStore = (s: Cookie, p: Cookie): boolean =>
  s.value === p.value &&
  !!s.host_only === !!p.host_only &&
  !!s.secure === !!p.secure &&
  !!s.http_only === !!p.http_only &&
  sameSiteInStore(s.same_site, p.same_site) &&
  (s.expires == null ? p.expires == null : p.expires != null && Math.abs(s.expires - p.expires) <= 1)

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

type CookieId = Pick<Cookie, 'name' | 'domain' | 'path'>
const idOf = (c: CookieId): CookieId => ({ name: c.name, domain: c.domain, path: c.path })

/** What `web_profile_write` gets: cookies to set and keys to remove. */
export type WritePlan = { set: Cookie[]; remove: CookieId[]; localChanges: boolean }

/** What `planApply` knows beyond the pull. */
export type PlanOptions = {
  /** The sites a store cookie can belong to (the longest that claims it wins); default `sites`. */
  universe?: readonly string[]
  /** Sites whose every store cookie goes, unpushed changes included (forgotten since this device last pushed them,
   * or refused at a push). */
  drop?: ReadonlySet<string>
  /** Store keys a live page keeps re-setting: neither set nor removed. */
  contended?: ReadonlySet<string>
  /** Profile key → the key the store keeps that cookie under (WebKit canonicalised its domain or path). */
  aliases?: ReadonlyMap<string, string>
  nowSecs?: number
}

/**
 * 209 D5: what to write into the tile store so it holds what the profile pulled.
 * - `pulled`: the cookies `web_profile_pull` answered; one that already expired is not written.
 * - `sites`: the sites the answer names (each answered whole), or `null` for a pull of every site (connect). Only a
 *   store cookie claimed by a named site can go, and only one the profile was seen to hold (`known`) and no longer
 *   does: a site the answer holds no cookie of (live through a storage snapshot only, or every cookie removed) never
 *   empties the store of cookies the profile never had, such as ones kept from before this launch (209 P2 proof).
 *   A forgotten site goes through `drop`. A pull of every site only sets (and drops `drop`).
 * - `baseline`: what the store held at the last push, seed or write. A key whose store cookie differs from it is a
 *   change of this device not pushed yet: left alone (`localChanges`), the next push carries it.
 */
export function planApply(
  jar: readonly Cookie[],
  pulled: readonly Cookie[],
  sites: readonly string[] | null,
  known: ReadonlySet<string>,
  baseline: ReadonlyMap<string, Cookie>,
  opts: PlanOptions = {},
): WritePlan {
  const { universe = sites ?? [], drop = new Set<string>(), contended = new Set<string>(), aliases = new Map<string, string>(), nowSecs = Date.now() / 1000 } = opts
  const store = keyed(jar)
  const profileKey = new Map([...aliases].map(([p, s]) => [s, p]))
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
      const at = aliases.get(key) ?? key
      if (contended.has(at)) return false
      const s = store.get(at)
      if (s && sameInStore(s, c)) return false
      // A site forgotten and signed into again since: the profile's cookies replace whatever the store kept of it.
      const site = siteOf(c.domain, universe)
      return (site !== null && drop.has(site)) || settled(at)
    })
    .map(bare)
  const remove: CookieId[] = []
  const pulledKeys = new Set(live.map(cookieKey))
  const named = sites === null ? null : new Set(sites)
  for (const c of jar) {
    const key = cookieKey(c)
    const s = siteOf(c.domain, universe)
    if (s === null || contended.has(key)) continue
    const p = profileKey.get(key) ?? key
    if (pulledKeys.has(p)) continue
    if (drop.has(s)) remove.push(idOf(c))
    else if (named?.has(s) && known.has(p) && settled(key)) remove.push(idOf(c))
  }
  return { set, remove, localChanges }
}

/** What the connected engine last said of `browser.profile` (209 D6): the `profile` of the last push or pull result
 * (or a `profile off` refusal) for `engine`; `null` until one answered. Settings asks `web_profile_status` first. */
export const useWebProfileMode = create<{ engine: string | null; mode: ProfileMode | null }>()(() => ({ engine: null, mode: null }))
const setMode = (engine: string, mode: ProfileMode) => useWebProfileMode.setState({ engine, mode })

const DEVICE_ID_KEY = 'theuth.device.id'
/** The id of this run when localStorage cannot keep one. */
let runId: string | null = null
/** This install's id: generated once and kept in localStorage, so it is stable across launches. */
function installId(): string {
  try {
    const saved = globalThis.localStorage?.getItem(DEVICE_ID_KEY)
    if (saved && /^[0-9a-f]{6}/i.test(saved)) return saved
    const id = crypto.randomUUID()
    globalThis.localStorage?.setItem(DEVICE_ID_KEY, id)
    if (globalThis.localStorage?.getItem(DEVICE_ID_KEY) === id) return id
  } catch {
    // No storage: an id for this run.
  }
  runId ??= crypto.randomUUID()
  return runId
}
/** This device as a profile writer (209 §4.1 `writer: tile:<device>`): the device name and the first 6 characters of
 * this install's id (`iPad-7f3a2b`), so two iPads are two writers. Sealed into rows: stable across launches. */
export const profileDevice = (): string => `${deviceName()}-${installId().slice(0, 6).toLowerCase()}`

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

/** The engine's wordings of "the profile is off", a fallback after the structured `profile` field, matched as prefixes. */
const OFF_PREFIXES = ['browser profile is off', 'browser.profile is off', 'browser profile unavailable: browser.profile is off']

/** Read a `web_profile_push` error: `profile off` (`data.profile`, else the message's start), `stale-epoch` /
 * `unknown-epoch` (epoch in `data`, `data.epoch` or the message). An epoch refusal without a readable epoch is `null`:
 * a plain failure, retried with the backoff. */
export function refusal(e: unknown): Refusal | null {
  if (!(e instanceof RpcError)) return null
  const d = e.data
  const data = d !== null && typeof d === 'object' ? (d as Record<string, unknown>) : null
  if (data?.profile === 'off') return { kind: 'off' }
  for (const kind of ['stale-epoch', 'unknown-epoch'] as const) {
    if (!e.message.includes(kind)) continue
    const raw = typeof d === 'number' ? d : data && 'epoch' in data ? Number(data.epoch) : Number(new RegExp(`${kind}\\D*(\\d+)`).exec(e.message)?.[1])
    return Number.isSafeInteger(raw) ? { kind, epoch: raw } : null
  }
  const message = e.message.trim()
  return OFF_PREFIXES.some((p) => message.startsWith(p)) ? { kind: 'off' } : null
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

/** The engine's clock (`now_ms` of the first page) at this device's last complete pull: the push's `basis_ms`, and the
 * line under which a forget was applied already. 0: never pulled. */
const basisKey = (engine: string) => `theuth.web-profile.basis.${engine}`
function loadBasis(engine: string): number {
  try {
    const n = Number(globalThis.localStorage?.getItem(basisKey(engine)) ?? 0)
    return Number.isSafeInteger(n) && n > 0 ? n : 0
  } catch {
    return 0
  }
}
function saveBasis(engine: string, ms: number): void {
  try {
    globalThis.localStorage?.setItem(basisKey(engine), String(ms))
  } catch {
    // The next pull saves it again; until then pushes carry the older basis.
  }
}

/** An http(s) origin's snapshot, or `null` (another scheme, an opaque origin, over `STORAGE_MAX_BYTES`). */
function snapshot(s: native.StorageCapture): OriginStorage | null {
  if (!/^https?:\/\/[^/]+$/.test(s.origin)) return null
  if (new TextEncoder().encode(JSON.stringify(s.local_storage)).length > STORAGE_MAX_BYTES) return null
  return { origin: s.origin, local_storage: s.local_storage, captured_ms: Date.now() }
}

/** A snapshot's `local_storage` as compared with what was sent or written: its pairs sorted by key. The tile reads
 * pairs in `localStorage.key(i)` order and the engine answers its own, so one content in two orders is one snapshot
 * (209 P2 proof: a reload of a page the pull had written pushed it back, the agent's row rewritten as this device's). */
const storageJson = (pairs: readonly (readonly [string, string])[]): string =>
  JSON.stringify([...pairs].sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0)))

/** The connected engine's api when it can take a push now, else `null`. */
function pushTarget() {
  const { api, state, discovery } = useConnection.getState()
  if (!api || state !== 'open' || !ready(discovery) || !supports(discovery, { methods: ['web_profile_push'] })) return null
  return { api, discovery }
}

/** Every page of one `web_profile_pull`, joined. */
type Pull = {
  epoch: number
  profile: ProfileMode | undefined
  nowMs: number | null
  sites: string[]
  cookies: WebProfilePullResult['cookies']
  storage: PulledStorage[]
  forgotten: ForgottenSite[]
}

/** Pull `sites` (`null`: every site) page by page until `next` is null; `'off'` when the profile is off. */
async function pullPages(api: Api, sites: string[] | null): Promise<Pull | 'off'> {
  let out: Pull | null = null
  let cursor: string | null = null
  do {
    const page: WebProfilePullResult = cursor === null ? await api.web_profile_pull(sites ?? undefined) : await api.web_profile_pull(sites, cursor)
    if (page.profile === 'off') return 'off'
    if (out === null) out = { epoch: page.epoch, profile: page.profile, nowMs: page.now_ms ?? null, sites: [], cookies: [], storage: [], forgotten: [] }
    // A sign out everywhere between two pages: the pages are of two profiles; the pull is retried whole.
    else if (page.epoch !== out.epoch) throw new Error('the profile epoch changed between pages')
    out.sites.push(...(page.sites ?? []))
    out.cookies.push(...page.cookies)
    out.storage.push(...(page.storage ?? []))
    out.forgotten.push(...(page.forgotten ?? []))
    cursor = page.next ?? null
  } while (cursor !== null)
  return out
}

/**
 * 209 §4.5: push this device's tile store to the connected engine as it changes, and (209 D5) write the profile's
 * changes into the store. A no-op without the native layer, and stopped for the session once the shell answers
 * `unsupported`; silent toward an engine without `web_profile_push`, toward a connection that answered `profile off`
 * (probed again on a real change after `OFF_REPROBE_MS`, or at once on a new connection), and toward a connection
 * whose sign out everywhere this shell could not apply yet (`web_profile_clear` failed: retried with the backoff, and
 * at once on the next sign out). Pulls need `web_profile_pull` too. Once per main window. Returns the stop.
 */
export function startWebProfileSync(): () => void {
  if (!native.nativeWebViews()) return () => {}
  let stopped = false
  /** The shell answered `unsupported`: nothing more is tried this session. */
  let unsupported = false
  /** The baseline, by `cookieKey`: the cookies last pushed, the seed read, or what a pull wrote; `null` until a
   * seed read succeeded. */
  let last: Map<string, Cookie> | null = null
  /** Origin → the `local_storage` JSON last pushed, or written into a tile from a pull. */
  const sentStorage = new Map<string, string>()
  /** Captured, not yet pushed; the newest capture per origin. */
  const captured = new Map<string, OriginStorage>()
  let cookiesDirty = false
  let engine = useEngines.getState().active
  /** The connection that answered `profile off`, and when. */
  let off: { api: unknown; discovery: unknown; at: number } | null = null
  /** The connection whose sign out everywhere (to `epoch`) this shell could not clear yet: nothing is pushed to it, and
   * the clear is tried again from `retryAt` (the backoff), or at once on the next sign out. */
  let stuck: { api: unknown; epoch: number; retryAt: number } | null = null
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
  /** Sites a push answered `refused`, not deleted from the store yet. */
  const dropQueue = new Set<string>()
  /** The newest epoch a `WebProfileChanged` carried, not compared yet. */
  let eventEpoch: number | null = null
  /** An epoch this device's own sign out everywhere answered, not applied yet. */
  let ownSignOut: number | null = null
  /** The epoch whose sign out this store was last cleared for (one clear per sign out). */
  let clearedFor: number | null = null
  /** Keys a pull showed in the profile: one gone from a later pull of its site was removed there. */
  const known = new Set<string>()
  /** Sites the profile holds, as the pulls named them: what a store cookie's site is decided among. */
  let liveSites = new Set<string>()
  /** Site → when (local ms) a push of this device last wrote it. */
  const pushedAt = new Map<string, number>()
  /** Store key → when a pull last wrote or removed it. */
  const writtenAt = new Map<string, number>()
  /** Store key → when a page re-set it soon after a pull wrote it (within `CONTENDED_MS`). */
  const strikes = new Map<string, number[]>()
  /** Store key → until when pulls leave it alone. */
  const contendedUntil = new Map<string, number>()
  /** Profile key → the key the store keeps it under (WebKit canonicalised it), learned this run. */
  const aliases = new Map<string, string>()
  /** Tile label → its top-level origin: where a pulled snapshot can go. */
  const tileOrigins = new Map<string, string>()

  const markUnsupported = (e: unknown): boolean => {
    // The shell has no browser profile here (Windows, Linux, macOS < 14, iOS < 17): nothing will ever work.
    if (!String(e).includes('unsupported:')) return false
    if (!unsupported) console.warn('web profile: the shell has no browser profile here; sync stopped')
    unsupported = true
    clearTimeout(timer)
    return true
  }

  let seedGen = 0
  /** Read the store into the baseline without pushing it; a newer seed (engine switch) wins over an older one. */
  const seed = (): Promise<void> => {
    const gen = ++seedGen
    return native.readProfile().then(
      (cookies) => {
        if (gen === seedGen) last = keyed(cookies)
      },
      (e: unknown) => {
        if (!markUnsupported(e)) console.warn('web_profile_read failed', String(e))
      },
    )
  }
  let seeding = seed()

  const schedule = (ms: number) => {
    clearTimeout(timer)
    if (!stopped && !unsupported) timer = setTimeout(flush, ms)
  }
  /** Run again after the backoff; answers the wait. */
  const retry = () => {
    const ms = Math.min(RETRY_MS * 2 ** failures++, MAX_RETRY_MS)
    schedule(ms)
    return ms
  }
  const capture = (s: native.StorageCapture) => {
    const snap = snapshot(s)
    if (snap) captured.set(snap.origin, snap)
  }
  /** Captured snapshots that differ from what was last pushed for their origin. */
  const pendingStorage = () => [...captured.values()].filter((s) => sentStorage.get(s.origin) !== storageJson(s.local_storage))
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
  /** Keys pulls leave alone now. */
  const contendedKeys = () => {
    const now = Date.now()
    for (const [key, until] of contendedUntil) if (until <= now) contendedUntil.delete(key)
    return new Set(contendedUntil.keys())
  }
  /** The changes read at `at` include keys a pull wrote just before: a page re-setting them. The second time within
   * `CONTENDED_MS` makes the key contended. */
  const noteResets = (changes: readonly CookieChange[], at: number) => {
    for (const ch of changes) {
      const key = cookieKey(ch.op === 'set' ? ch.cookie : ch)
      const wrote = writtenAt.get(key)
      if (wrote === undefined) continue
      writtenAt.delete(key)
      if (at - wrote > RESET_WINDOW_MS) continue
      const recent = (strikes.get(key) ?? []).filter((t) => at - t < CONTENDED_MS)
      recent.push(at)
      if (recent.length < 2) {
        strikes.set(key, recent)
        continue
      }
      strikes.delete(key)
      contendedUntil.set(key, at + CONTENDED_MS)
      console.warn('web profile: a page keeps re-setting a cookie the profile wrote; pulls leave it alone for 10 min')
    }
  }

  function flush(): void {
    clearTimeout(timer)
    if (stopped || unsupported) return
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
   * is `stuck` on this connection (nothing more pushed or written; the clear is retried with the backoff), so its
   * cookies never sign the profile back in. `false` when stuck.
   */
  async function adopt(api: unknown, epoch: number, force: boolean): Promise<boolean> {
    const saved = loadEpoch(engine)
    if ((force || (saved !== null && epoch > saved)) && clearedFor !== epoch) {
      try {
        await native.clearProfile()
      } catch (err) {
        if (markUnsupported(err)) return false
        console.warn('web_profile_clear failed', String(err))
        stuck = { api, epoch, retryAt: Date.now() + retry() }
        return false
      }
      stuck = null
      clearedFor = epoch
      last = new Map()
      sentStorage.clear()
      captured.clear()
      known.clear()
      writtenAt.clear()
      strikes.clear()
      contendedUntil.clear()
      aliases.clear()
      liveSites = new Set()
      dropQueue.clear()
      cookiesDirty = false
      failures = 0
      // The profile of the new epoch (normally empty, or what was signed in since) flows in whole.
      pullAll = true
    }
    if (epoch !== saved) saveEpoch(engine, epoch)
    return true
  }

  async function run(): Promise<void> {
    if (unsupported) return
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
      dropQueue.clear()
      liveSites = new Set()
      pushedAt.clear()
      writtenAt.clear()
      strikes.clear()
      contendedUntil.clear()
      aliases.clear()
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
    if (stuck !== null && stuck.api === api) {
      captured.clear()
      // A change while the clear waits for its backoff keeps that wait (the flush cleared its timer).
      if (Date.now() < stuck.retryAt) return schedule(stuck.retryAt - Date.now())
      if (!(await adopt(api, stuck.epoch, true))) return
    }
    if (pulledFrom !== api) {
      pulledFrom = api
      pullAll = true
    }
    // A sign out everywhere (this device's own, or a newer epoch an event carried) empties the store before anything
    // of it could be pushed.
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
    // Push first: what this device changed reaches the profile before the profile is written back into the store.
    if (await push(api, discovery, last)) await pull(api, discovery)
  }

  /** Push the store's diff and the captured storage; `true` when nothing is left (pushed, or nothing to push). */
  async function push(api: Api, discovery: Discovery, base: Map<string, Cookie>): Promise<boolean> {
    let read: Cookie[] | null = null
    let readAt = 0
    if (cookiesDirty) {
      cookiesDirty = false
      try {
        read = await native.readProfile()
        readAt = Date.now()
      } catch (e) {
        if (markUnsupported(e)) return false
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
      result = await api.web_profile_push(profileDevice(), epoch ?? 0, changes, storage, loadBasis(engine))
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
    const now = Date.now()
    for (const s of result.sites ?? []) pushedAt.set(s, now)
    if (read) {
      noteResets(changes, readAt)
      last = keyed(read)
    }
    for (const s of storage) sentStorage.set(s.origin, storageJson(s.local_storage))
    saveEpoch(engine, result.epoch)
    // A site forgotten since this device's last pull: the profile did not take it, and the store lets it go too.
    for (const s of result.refused ?? []) dropQueue.add(s)
    return true
  }

  /**
   * Write `set`/`remove` (and `storage`) into the store, then fold what the store reads back into the baseline — also
   * after a rejected or timed-out write, so a half-applied plan never goes out as this device's change. A written
   * cookie the store keeps under another key becomes an alias. `true` when the whole plan was applied.
   */
  async function write(jar: readonly Cookie[], set: Cookie[], remove: CookieId[], storage: native.StorageWrite[], universe: readonly string[]): Promise<boolean> {
    let ok = false
    /** Origins whose snapshot some open tile took. */
    let applied: string[] = []
    try {
      const written = await native.writeProfile(set, remove, storage)
      ok = written.timed_out !== true
      applied = storage.filter((s) => (written.storage_applied?.[s.origin] ?? 0) > 0).map((s) => s.origin)
      if (written.skipped > 0) console.warn(`web_profile_write: ${written.skipped} of ${set.length} cookies refused by the store`)
      if (!ok) console.warn('web_profile_write timed out; what was applied is kept')
    } catch (e) {
      // The error is the shell's and the arguments carry values: neither is logged.
      if (!markUnsupported(e)) console.warn('web_profile_write failed')
    } finally {
      await fold(jar, set, remove, ok, universe)
    }
    for (const s of storage) if (applied.includes(s.origin)) sentStorage.set(s.origin, storageJson(s.local_storage))
    return ok
  }

  async function fold(jar: readonly Cookie[], set: readonly Cookie[], remove: readonly CookieId[], ok: boolean, universe: readonly string[]): Promise<void> {
    const base = last ?? new Map<string, Cookie>()
    let after: Map<string, Cookie>
    try {
      after = keyed(await native.readProfile())
    } catch {
      if (!ok) {
        // Neither what was applied nor what was not is known: the store is taken as it is at the next run (seed).
        last = null
        return
      }
      for (const c of set) base.set(cookieKey(c), c)
      for (const r of remove) base.delete(cookieKey(r))
      last = base
      return
    }
    const before = keyed(jar)
    const now = Date.now()
    const take = (key: string) => {
      const c = after.get(key)
      if (c) base.set(key, c)
      else base.delete(key)
    }
    for (const c of set) {
      const key = cookieKey(c)
      let at = aliases.get(key) ?? key
      if (!after.has(at)) {
        // WebKit keeps it under another domain or path: the same name and site and the value just written, new or
        // changed in this read if there is such a one.
        const site = siteOf(c.domain, universe)
        const same = [...after.values()].filter((a) => a.name === c.name && site !== null && siteOf(a.domain, universe) === site && sameInStore(a, c))
        const alias = same.find((a) => {
          const b = before.get(cookieKey(a))
          return !b || !sameCookie(a, b)
        }) ?? same[0]
        if (alias) {
          at = cookieKey(alias)
          aliases.set(key, at)
        } else at = key
      }
      take(at)
      if (after.has(at)) writtenAt.set(at, now)
    }
    for (const r of remove) {
      const key = cookieKey(r)
      take(key)
      writtenAt.set(key, now)
    }
    last = base
  }

  /** Delete the cookies of the sites a push answered `refused` from the store; `false` when that has to wait. */
  async function dropRefused(): Promise<boolean> {
    const drop = new Set(dropQueue)
    let jar: Cookie[]
    try {
      jar = await native.readProfile()
    } catch (e) {
      if (!markUnsupported(e)) retry()
      return false
    }
    const universe = [...new Set([...liveSites, ...drop])]
    const plan = planApply(jar, [], [], known, last ?? new Map(), { universe, drop, contended: contendedKeys() })
    if (plan.remove.length > 0 && !(await write(jar, [], plan.remove, [], universe))) {
      retry()
      return false
    }
    for (const s of drop) dropQueue.delete(s)
    return true
  }

  /** 209 D5: pull what changed in the profile and write the differences into the store. */
  async function pull(api: Api, discovery: Discovery): Promise<void> {
    if (dropQueue.size > 0 && !(await dropRefused())) return
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
    let answer: Pull | 'off'
    try {
      answer = await pullPages(api, sites)
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
    if (answer === 'off') {
      markOff(api, discovery)
      return
    }
    if (answer.profile === 'shared') setMode(engine, 'shared')
    // A newer epoch here is a sign out everywhere this device missed: clear, then write what the pull holds.
    if (!(await adopt(api, answer.epoch, false))) return
    const basis = loadBasis(engine)
    const nowMs = answer.nowMs
    const done = () => {
      if (nowMs !== null) saveBasis(engine, nowMs)
    }

    // Forgotten after this device's last complete pull and after it last pushed the site: the store's cookies of the
    // site go before this answer's are written (a site signed into again since is in `cookies` too).
    const drop = new Set(answer.forgotten.filter((f) => f.forgotten_ms > Math.max(pushedAt.get(f.site) ?? 0, basis)).map((f) => f.site))
    // The sites the answer names that hold anything now, each answered whole.
    const holds = new Set([...answer.cookies.map((c) => c.site), ...answer.storage.map((s) => s.site)])
    if (sites === null) liveSites = new Set(answer.sites)
    for (const s of answer.sites) {
      if (holds.has(s)) liveSites.add(s)
      else liveSites.delete(s)
    }
    for (const s of drop) if (!holds.has(s)) liveSites.delete(s)
    const named = [...new Set([...answer.sites, ...drop])]
    const universe = [...new Set([...liveSites, ...named])]
    // A pull of every site only sets (a site answered with snapshots and no cookie must not empty the store), and drops.
    const covered = sites === null ? null : named
    // localStorage for the origins a tile has open now, unless the tile has an unpushed capture or it is what this
    // device already sent or wrote.
    const open = new Set(tileOrigins.values())
    const storage = answer.storage
      .filter((s) => open.has(s.origin) && !captured.has(s.origin) && sentStorage.get(s.origin) !== storageJson(s.local_storage))
      .map((s) => ({ origin: s.origin, local_storage: s.local_storage }))

    // Every site, none held, nothing forgotten: nothing to write, and no store read needed.
    if (covered === null && answer.cookies.length === 0 && drop.size === 0 && storage.length === 0) {
      known.clear()
      failures = 0
      done()
      return
    }
    let jar: Cookie[]
    try {
      jar = await native.readProfile()
    } catch (e) {
      if (markUnsupported(e)) return
      requeue()
      retry()
      console.warn('web_profile_read failed', String(e))
      return
    }
    const base = last ?? new Map<string, Cookie>()
    const plan = planApply(jar, answer.cookies, covered, known, base, { universe, drop, contended: contendedKeys(), aliases })
    // `known` follows the profile: a whole pull replaces it, a pull of some sites replaces those sites' keys.
    if (covered === null) known.clear()
    else for (const key of [...known]) if (siteOf(JSON.parse(key)[1] as string, covered) !== null) known.delete(key)
    for (const c of answer.cookies) known.add(cookieKey(c))
    // A change of this device left alone goes out with the next push.
    if (plan.localChanges) {
      cookiesDirty = true
      again = true
    }
    if (plan.set.length === 0 && plan.remove.length === 0 && storage.length === 0) {
      failures = 0
      done()
      return
    }
    if (!(await write(jar, plan.set, plan.remove, storage, universe))) {
      requeue()
      retry()
      return
    }
    failures = 0
    done()
  }

  /** The page is going away (app quit, window closed): push what is still captured, best-effort, without waiting. */
  const onLeave = () => {
    const target = pushTarget()
    if (stopped || unsupported || captured.size === 0 || !target || last === null) return
    const { api, discovery } = target
    if (useEngines.getState().active !== engine || stuck?.api === api || isOff(api, discovery)) return
    const storage = pendingStorage()
    captured.clear()
    if (storage.length === 0) return
    void api.web_profile_push(profileDevice(), loadEpoch(engine) ?? 0, [], storage, loadBasis(engine)).then(
      () => {
        for (const s of storage) sentStorage.set(s.origin, storageJson(s.local_storage))
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
      try {
        tileOrigins.set(s.label, new URL(s.url).origin)
      } catch {
        tileOrigins.delete(s.label)
      }
      if (s.loading || unsupported) return
      // A finished load also re-reads the store: on the iPad simulator (iOS 26) the shell's cookie observer
      // never fired, so a sign-in's `Set-Cookie` reached the profile only through this (209 P1 proof).
      cookiesDirty = true
      void native.captureStorage(native.labelId(s.label)).then(capture, () => undefined).finally(() => schedule(PUSH_DEBOUNCE_MS))
    }),
  )
  keep(native.onClosed((s) => void tileOrigins.delete(s.label)))
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
      // A sign out everywhere at or after the one this shell could not clear: clear again now, not at the backoff.
      if (stuck !== null && e.epoch >= stuck.epoch) {
        ownSignOut = Math.max(ownSignOut ?? e.epoch, e.epoch)
        stuck = null
      } else eventEpoch = Math.max(eventEpoch ?? e.epoch, e.epoch)
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
    // A new sign out everywhere: a clear that failed before is tried again at once.
    ownSignOut = epoch
    stuck = null
    failures = 0
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
