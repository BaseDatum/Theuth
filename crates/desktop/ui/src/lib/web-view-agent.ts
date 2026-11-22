// 209 §4.7, D8: the client side of an agent driving a web view tile. On `WebViewCallRequested` for a tile this window
// holds (its last `web_view_attach` landed here and no pop-out window took it, `web-view-native.holds`), answer
// `Claimed` at once; once the engine accepts the claim (the first one wins: another device may show the same tile),
// run the op through the shell's `web_view_call`, then answer `Done` or `Failed`. A window that does not hold the tile
// stays silent, so the holder (a pop-out window, the iPad, the desktop app) is the one that claims. A tile hidden
// behind another pane is still driven; nothing here shows a hidden tile. Stop (the bar in `WebFrame`) fails the tile's
// calls in flight and refuses its next ones for 60 s. Arguments and results are never logged: they carry page text and
// form values.

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

/** One tile's agent activity: the calls in flight, when the last one ended, and until when Stop refuses calls. */
export type Activity = { calls: string[]; endedAt: number | null; refusedUntil: number }
export const IDLE: Activity = { calls: [], endedAt: null, refusedUntil: 0 }

export const started = (a: Activity, call: string): Activity => ({ ...a, calls: [...a.calls, call] })
/** `call` is over; the same object when it was not in flight (stopped or resolved before). */
export const ended = (a: Activity, call: string, now: number): Activity =>
  a.calls.includes(call) ? { ...a, calls: a.calls.filter((c) => c !== call), endedAt: now } : a
/** Stop: nothing in flight, no linger, refuse until `now + REFUSE_MS`. */
export const stopped = (now: number): Activity => ({ calls: [], endedAt: null, refusedUntil: now + REFUSE_MS })
/** The bar shows: a call in flight, or the last one ended less than `LINGER_MS` ago. */
export const acting = (a: Activity | undefined, now: number): boolean =>
  a !== undefined && (a.calls.length > 0 || (a.endedAt !== null && now - a.endedAt < LINGER_MS))
export const refusing = (a: Activity | undefined, now: number): boolean => a !== undefined && now < a.refusedUntil

/** Per tile id, this window's agent activity. */
export const useTileAgent = create<{ tiles: Record<string, Activity> }>()(() => ({ tiles: {} }))
const update = (id: string, f: (a: Activity) => Activity) => {
  const tiles = useTileAgent.getState().tiles
  const next = f(tiles[id] ?? IDLE)
  if (next !== tiles[id]) useTileAgent.setState({ tiles: { ...tiles, [id]: next } })
}

/** The tile id of a request's `view`: the bare id the engine sends, also `web:<id>` or the shell label `web-<id>`. */
export const tileOf = (view: string): string | null => {
  const id = view.replace(/^web[:-]/, '')
  return /^[\w-]+$/.test(id) ? id : null
}

/** The `Done` answer of the shell's reply: `snapshot`'s `{ png }` moves to `png`. */
export function doneOf(op: string, value: unknown): WebViewCallAnswer {
  if (op === 'snapshot' && value !== null && typeof value === 'object' && !Array.isArray(value)) {
    const { png, ...rest } = value as Record<string, unknown>
    if (typeof png === 'string') return { kind: 'done', result: rest, png }
  }
  return { kind: 'done', result: value ?? null }
}

/** Calls this window claimed and has not answered yet: call id → tile id and the connection that asked. */
const running = new Map<string, { id: string; api: Api }>()

const answer = (api: Api, call: string, a: WebViewCallAnswer) => void api.answer_web_view_call(call, a).catch(() => undefined)

async function run(e: WebViewCallRequested): Promise<void> {
  const id = tileOf(e.view)
  if (id === null || !native.holds(id) || useWebViews.getState().views[id]?.popped || running.has(e.id)) return
  const { api, discovery } = useConnection.getState()
  if (!api || !supports(discovery, { methods: ['answer_web_view_call'], topics: ['artifact.web-view-call-requested@1'] })) return
  if (refusing(useTileAgent.getState().tiles[id], Date.now())) return answer(api, e.id, { kind: 'failed', reason: STOPPED })
  running.set(e.id, { id, api })
  const won = await api.answer_web_view_call(e.id, { kind: 'claimed', client: profileDevice() }).catch(() => false)
  // Another client claimed first, the engine gave up, or Stop came while the claim was out.
  if (!won || running.get(e.id)?.id !== id) {
    running.delete(e.id)
    return
  }
  update(id, (a) => started(a, e.id))
  let out: WebViewCallAnswer
  try {
    out = doneOf(e.op, await native.viewCall(id, e.op, e.args))
  } catch (err) {
    out = { kind: 'failed', reason: String(err) }
  }
  // Stopped or resolved by the engine meanwhile: already answered, or nobody waits.
  if (running.get(e.id)?.id !== id) return
  running.delete(e.id)
  update(id, (a) => ended(a, e.id, Date.now()))
  answer(api, e.id, out)
}

/** Stop the agent on tile `id`: its calls in flight fail with `STOPPED`, its next calls are refused for `REFUSE_MS`. */
export function stopAgent(id: string): void {
  for (const [call, r] of running) {
    if (r.id !== id) continue
    running.delete(call)
    answer(r.api, call, { kind: 'failed', reason: STOPPED })
  }
  update(id, () => stopped(Date.now()))
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

/** Listen for tile calls in this window (main and pop-out windows: each claims only what it holds). Returns the stop. */
export function startWebViewAgent(): () => void {
  if (!native.nativeWebViews()) return () => {}
  let gone = false
  const offs = [
    on('artifact.web-view-call-requested@1', 'WebViewCallRequested', (e) => void run(e)),
    on('artifact.web-view-call-resolved@1', 'WebViewCallResolved', (e) => {
      const r = running.get(e.id)
      if (!r) return
      running.delete(e.id)
      update(r.id, (a) => ended(a, e.id, Date.now()))
    }),
  ]
  void native.onClosed((s) => native.release(s.label)).then((off) => (gone ? off() : offs.push(off)))
  return () => {
    gone = true
    for (const off of offs) off()
  }
}
