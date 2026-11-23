// 209 §4.7, D8: the client side of an agent driving a web view tile. On `WebViewCallRequested` for a tile this window
// holds (its last `web_view_attach` landed here and no pop-out window took it, `web-view-native.holds`) and that belongs
// to the asking session (or the engine marks the request `delegated`), answer `Claimed` at once; once the engine accepts
// the claim (the first one wins: another device may show the same tile), run the op through the shell's
// `web_view_call`, then answer `Done` or `Failed`. A window that does not hold the tile stays silent, so the holder (a
// pop-out window, the iPad, the desktop app) is the one that claims. A tile whose frame is not on screen (unmounted, or
// under an overlay) is still driven: `useHiddenAgents` names it, for the window-level banner. Stop (the bar in
// `WebFrame`, or the banner) fails the tile's calls in flight, sends the shell's `stop` op (the page stops loading, its
// waits end), and refuses its next calls for 60 s in every window of this app (localStorage). Arguments and results
// are never logged: they carry page text and form values.

import { useEffect, useState } from 'react'
import { create } from 'zustand'
import { profileDevice } from '@/lib/web-profile'
import * as native from '@/lib/web-view-native'
import type { WebViewCallAnswer, WebViewCallRequested } from '@/lib/web-view-call-wire'
import { useWebViews } from '@/lib/web-views'
import type { Api } from '@/rpc/bindings/client'
import { supports } from '@/rpc/discovery'
import { useConnection } from '@/store/connection'
import { on } from '@/store/sessions'

/** The bar stays this long after the last call ended, so a read-then-act loop does not flicker it. */
export const LINGER_MS = 3_000
/** Stop refuses the tile's calls this long. */
export const REFUSE_MS = 60_000
/** `Failed` reason for a call the user stopped or a call refused after Stop. */
export const STOPPED = 'stopped by the user'
/** `Failed` reason for a call whose window went away (closed, reloaded) before it answered. */
export const WINDOW_CLOSED = 'window closed'
/** localStorage key prefix of a tile's Stop refusal: `<prefix><tile id>` → the time it ends (ms since the epoch). */
export const REFUSED_KEY = 'theuth.web-view-agent.refused.'

/** One tile's agent activity: the calls in flight, when the last one ended, until when Stop refuses calls, and
 * whether the shell's `stop` is still running. */
export type Activity = { calls: string[]; endedAt: number | null; refusedUntil: number; stopping: boolean }
export const IDLE: Activity = { calls: [], endedAt: null, refusedUntil: 0, stopping: false }

export const started = (a: Activity, call: string): Activity => ({ ...a, calls: [...a.calls, call] })
/** `call` is over; the same object when it was not in flight (stopped or resolved before). */
export const ended = (a: Activity, call: string, now: number): Activity =>
  a.calls.includes(call) ? { ...a, calls: a.calls.filter((c) => c !== call), endedAt: now } : a
/** Stop: nothing in flight, no linger, the shell's `stop` running, refuse until `now + REFUSE_MS`. */
export const stopped = (now: number): Activity => ({ calls: [], endedAt: null, refusedUntil: now + REFUSE_MS, stopping: true })
/** The bar shows: a call in flight, Stop still settling, or the last call ended less than `LINGER_MS` ago. */
export const acting = (a: Activity | undefined, now: number): boolean =>
  a !== undefined && (a.calls.length > 0 || a.stopping || (a.endedAt !== null && now - a.endedAt < LINGER_MS))

/** The end of tile `id`'s Stop refusal kept in localStorage (0: none); an expired one is removed. */
export function persistedRefusal(id: string, now: number): number {
  try {
    const until = Number(globalThis.localStorage?.getItem(REFUSED_KEY + id) ?? 0)
    if (Number.isFinite(until) && until > now) return until
    globalThis.localStorage?.removeItem(REFUSED_KEY + id)
  } catch {
    // No storage: this window's own refusal still holds.
  }
  return 0
}
function persistRefusal(id: string, until: number): void {
  try {
    globalThis.localStorage?.setItem(REFUSED_KEY + id, String(until))
  } catch {
    // No storage: this window's own refusal still holds.
  }
}
/** Stop refuses tile `id` now: in this window's state, or in any window's (or an earlier page load's) through localStorage. */
export const refusing = (id: string, a: Activity | undefined, now: number): boolean =>
  (a !== undefined && now < a.refusedUntil) || persistedRefusal(id, now) > now

/** Per tile id, this window's agent activity; `shown`: per tile id, how many of this window's frames show its page
 * (mounted, native view live and not under an overlay). */
export const useTileAgent = create<{ tiles: Record<string, Activity>; shown: Record<string, number> }>()(() => ({ tiles: {}, shown: {} }))
const update = (id: string, f: (a: Activity) => Activity) => {
  const tiles = useTileAgent.getState().tiles
  const next = f(tiles[id] ?? IDLE)
  if (next !== tiles[id]) useTileAgent.setState({ tiles: { ...tiles, [id]: next } })
}

/** A frame of tile `id` shows its page in this window; returns the undo (unmount, overlay, not live). */
export function showFrame(id: string): () => void {
  const bump = (n: number) => {
    const shown = useTileAgent.getState().shown
    const count = (shown[id] ?? 0) + n
    const { [id]: _, ...rest } = shown
    useTileAgent.setState({ shown: count > 0 ? { ...rest, [id]: count } : rest })
  }
  bump(1)
  return () => bump(-1)
}

/** A tile the agent acts on that no frame of this window shows: its id, the page's host, and whether Stop is settling. */
export type HiddenAgent = { id: string; host: string; stopping: boolean }
/** Tiles with a call in flight (or Stop settling) and no frame showing them, in id order. */
export function hiddenAgents(tiles: Record<string, Activity>, shown: Record<string, number>, urls: Record<string, string>): HiddenAgent[] {
  return Object.entries(tiles)
    .filter(([id, a]) => (a.calls.length > 0 || a.stopping) && !((shown[id] ?? 0) > 0))
    .map(([id, a]) => ({ id, host: hostOf(urls[id] ?? ''), stopping: a.stopping }))
    .sort((a, b) => a.id.localeCompare(b.id))
}
const hostOf = (url: string): string => {
  try {
    return new URL(url).host || 'a page'
  } catch {
    return 'a page'
  }
}
/** `hiddenAgents` of this window, live: the window-level banner (and the phone's pill, 209 P4) read it. */
export function useHiddenAgents(): HiddenAgent[] {
  const tiles = useTileAgent((s) => s.tiles)
  const shown = useTileAgent((s) => s.shown)
  const views = useWebViews((s) => s.views)
  const urls: Record<string, string> = {}
  for (const id of Object.keys(tiles)) urls[id] = views[id]?.current || views[id]?.url || ''
  return hiddenAgents(tiles, shown, urls)
}

/** The tile id of a request's `view`: the bare id the engine sends, also `web:<id>` or the shell label `web-<id>`. */
export const tileOf = (view: string): string | null => {
  const id = view.replace(/^web[:-]/, '')
  return /^[\w-]+$/.test(id) ? id : null
}

/** 209 §9: this window may act for `e` on tile `id` only for the session that opened the tile: the tile's session is
 * unknown here (a pop-out window's store has none) or is `e.session`, or the engine marks the request `delegated` (the
 * asking session is another of the tile session's tree). */
export function mayClaim(e: WebViewCallRequested, id: string): boolean {
  const owner = useWebViews.getState().views[id]?.session ?? ''
  return owner === '' || owner === e.session || e.delegated
}

/** The `Done` answer of the shell's reply: `snapshot`'s `{ png }` moves to `png`. */
export function doneOf(op: string, value: unknown): WebViewCallAnswer {
  if (op === 'snapshot' && value !== null && typeof value === 'object' && !Array.isArray(value)) {
    const { png, ...rest } = value as Record<string, unknown>
    if (typeof png === 'string') return { kind: 'done', result: rest, png }
  }
  return { kind: 'done', result: value ?? null }
}

/** Calls this window claimed and has not answered yet: call id → tile id, the connection that claimed (the engine
 * takes `done` / `failed` only from it) and the asking session (the answer's routing). */
type Running = { id: string; api: Api; session: string }
const running = new Map<string, Running>()

const answer = (r: Omit<Running, 'id'>, call: string, a: WebViewCallAnswer) =>
  void r.api.answer_web_view_call(call, a, r.session).catch(() => undefined)

/** `Failed` reason of a `fill_form` whose values the engine no longer has (not the claiming connection, fetched once,
 * or the card is over). */
export const NO_VALUES = 'the form values are gone'
const noValues = new Error(NO_VALUES)

/**
 * 209 §4.10, ruling A: a `fill_form` op arrives without the user's values (an event reaches every viewer and the
 * relay; its params say `values_from: "element_form_values"`); the claiming connection fetches them once with
 * `element_form_values(call id, session)` and they go only into the shell call's arguments. Any other op passes
 * through as it came.
 */
export async function withFormValues(api: Api, e: WebViewCallRequested): Promise<unknown> {
  const args = e.args as { arguments?: { cmd?: unknown; params?: Record<string, unknown> } } | null
  if (e.op !== 'script' || args?.arguments?.cmd !== 'fill_form') return e.args
  const values = await api.element_form_values(e.id, e.session).catch(() => null)
  if (!values || typeof values !== 'object') throw noValues
  const { values_from: _, ...params } = args.arguments.params ?? {}
  return { ...args, arguments: { ...args.arguments, params: { ...params, values } } }
}

async function run(e: WebViewCallRequested): Promise<void> {
  const id = tileOf(e.view)
  if (id === null || running.has(e.id)) return
  const { api, discovery } = useConnection.getState()
  if (!api || !supports(discovery, { methods: ['answer_web_view_call'], topics: ['artifact.web-view-call-requested@1'] })) return
  if (useWebViews.getState().views[id]?.popped || !native.holds(id) || !mayClaim(e, id)) return
  const r = { id, api, session: e.session }
  // Unclaimed: the engine takes this one `failed` reason from any owner connection.
  if (refusing(id, useTileAgent.getState().tiles[id], Date.now())) return answer(r, e.id, { kind: 'failed', reason: STOPPED })
  running.set(e.id, r)
  const won = await api.answer_web_view_call(e.id, { kind: 'claimed', client: profileDevice() }, e.session).catch(() => false)
  // Another client claimed first, the engine gave up, or Stop came while the claim was out.
  if (!won || running.get(e.id)?.id !== id) {
    running.delete(e.id)
    return
  }
  update(id, (a) => started(a, e.id))
  let out: WebViewCallAnswer
  try {
    out = doneOf(e.op, await native.viewCall(id, e.op, await withFormValues(api, e)))
  } catch (err) {
    out = { kind: 'failed', reason: err === noValues ? NO_VALUES : String(err) }
  }
  // Stopped or resolved by the engine meanwhile: already answered, or nobody waits.
  if (running.get(e.id)?.id !== id) return
  running.delete(e.id)
  update(id, (a) => ended(a, e.id, Date.now()))
  answer(r, e.id, out)
}

/** Fail with `reason` each running call for which `pick` holds, and forget it. */
function failAll(reason: string, pick: (id: string) => boolean = () => true): void {
  for (const [call, r] of running) {
    if (!pick(r.id)) continue
    running.delete(call)
    answer(r, call, { kind: 'failed', reason })
  }
}

/** Stop the agent on tile `id`: its calls in flight fail with `STOPPED`, the shell's `stop` ends the page's loading
 * and the tile's waits ("Stopping…" until it settles), and its next calls are refused for `REFUSE_MS` in every window. */
export function stopAgent(id: string): void {
  failAll(STOPPED, (t) => t === id)
  const now = Date.now()
  persistRefusal(id, now + REFUSE_MS)
  update(id, () => stopped(now))
  void native
    .viewCall(id, 'stop', {})
    .catch(() => undefined)
    .then(() => update(id, (a) => (a.stopping ? { ...a, stopping: false } : a)))
}

/** Whether the "Agent is using this page" bar shows for tile `id`; re-renders when the linger runs out. */
export function useAgentActing(id: string): boolean {
  const a = useTileAgent((s) => s.tiles[id])
  const [, tick] = useState(0)
  useEffect(() => {
    if (!a || a.calls.length > 0 || a.endedAt === null) return
    const left = a.endedAt + LINGER_MS - Date.now()
    if (left <= 0) return
    const timer = setTimeout(() => tick((n) => n + 1), left)
    return () => clearTimeout(timer)
  }, [a])
  return acting(a, Date.now())
}

/** Listen for tile calls in this window (main and pop-out windows: each claims only what it holds). Returns the stop,
 * which, like the window going away (`pagehide`), fails the calls this window still runs: nobody else would. */
export function startWebViewAgent(): () => void {
  if (!native.nativeWebViews()) return () => {}
  let gone = false
  const leave = () => failAll(WINDOW_CLOSED)
  const offs = [
    on('artifact.web-view-call-requested@1', 'WebViewCallRequested', (e) => void run(e)),
    on('artifact.web-view-call-resolved@1', 'WebViewCallResolved', (e) => {
      const r = running.get(e.id)
      if (!r) return
      running.delete(e.id)
      update(r.id, (a) => ended(a, e.id, Date.now()))
    }),
  ]
  globalThis.addEventListener?.('pagehide', leave)
  globalThis.addEventListener?.('beforeunload', leave)
  offs.push(() => {
    globalThis.removeEventListener?.('pagehide', leave)
    globalThis.removeEventListener?.('beforeunload', leave)
  })
  void native.onClosed((s) => native.release(s.label)).then((off) => (gone ? off() : offs.push(off)))
  return () => {
    gone = true
    for (const off of offs) off()
    leave()
  }
}
