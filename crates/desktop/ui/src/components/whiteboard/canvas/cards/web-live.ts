// Web cards' native web views (205 §4.3 child webviews; 209: the tiles' shared store, so the user's sign-ins apply).
// Most sites refuse framing (X-Frame-Options / CSP frame-ancestors) and the app's own CSP `frame-src` admits no http
// origin, so an entered web card (or its sheet) shows a native child webview over the card's rectangle: positioned
// every frame, hidden while the rectangle moves (pan, zoom, drag) or is not wholly inside its clip, and destroyed on
// exit. One shows at a time (`claim`: the newest claim owns it). The canvas preview is a WebKit snapshot of the page,
// taken in an off-window view when the card is placed or changed, on "Refresh preview", and on the live view's first load.

import { useSyncExternalStore } from 'react'
import type * as Y from 'yjs'
import { isMacDesktop } from '@/lib/hotkeys'
import { base64Blob } from '@/lib/wb-capture'
import * as native from '@/lib/web-view-native'
import type { Rect } from '@/lib/web-view-native'
import type { CanvasHost } from '../api'
import type { Id } from '../model'
import { setSnap } from '../ops'

/** A stable 16-hex web view id for `key` (FNV-1a 64), so the label is `web-<16 hex>` as the shell requires. */
export function webId(key: string): string {
  let h = 0xcbf29ce484222325n
  for (const b of new TextEncoder().encode(`wb:${key}`)) h = ((h ^ BigInt(b)) * 0x100000001b3n) & 0xffffffffffffffffn
  return h.toString(16).padStart(16, '0')
}

// ---- one live web view at a time ----

const owners: string[] = []
const subs = new Set<() => void>()
const notify = () => subs.forEach((f) => f())

/** `id` wants to be the live web view; the newest claim wins, and an earlier one gets it back when the newer is released. */
export function claim(id: string): () => void {
  const drop = () => {
    const i = owners.lastIndexOf(id)
    if (i >= 0) owners.splice(i, 1)
  }
  drop()
  owners.push(id)
  notify()
  return () => {
    drop()
    notify()
  }
}

export function useOwns(id: string): boolean {
  return useSyncExternalStore(
    (f) => {
      subs.add(f)
      return () => subs.delete(f)
    },
    () => owners.at(-1) === id,
  )
}

// ---- following a placeholder ----

/** How long a rectangle must hold still before the view shows there again. */
export const SETTLE_MS = 120
const MIN_SIDE = 40

export type Follow = { last: Rect | null; since: number; shown: boolean; attached: boolean }
export const following = (): Follow => ({ last: null, since: 0, shown: false, attached: false })

/** One frame of following `r` (the placeholder) within `clip`: hide on any move or when it leaves the clip; show (attach
 * the first time) once it held still for `SETTLE_MS` wholly inside. */
export function step(s: Follow, r: Rect, clip: Rect | null, now: number): 'attach' | 'show' | 'hide' | null {
  const l = s.last
  const moved = l === null || r.x !== l.x || r.y !== l.y || r.width !== l.width || r.height !== l.height
  if (moved) {
    s.last = r
    s.since = now
  }
  const fits =
    clip !== null && r.width >= MIN_SIDE && r.height >= MIN_SIDE &&
    r.x >= clip.x - 0.5 && r.y >= clip.y - 0.5 && r.x + r.width <= clip.x + clip.width + 0.5 && r.y + r.height <= clip.y + clip.height + 0.5
  if (s.shown && (moved || !fits)) {
    s.shown = false
    return 'hide'
  }
  if (!s.shown && fits && !moved && now - s.since >= SETTLE_MS) {
    s.shown = true
    if (s.attached) return 'show'
    s.attached = true
    return 'attach'
  }
  return null
}

const rectOf = (el: Element): Rect => {
  const r = el.getBoundingClientRect()
  return { x: r.left, y: r.top, width: r.width, height: r.height }
}

/**
 * Keep web view `id` (at `url`) over `el`, clipped to `clip(el)`, until the returned stop, which destroys it. `onLoaded`
 * runs after each finished main-frame load while it shows; `onFail` with the shell's refusal of the attach.
 */
export function follow(o: { id: string; url: string; el: HTMLElement; clip(el: HTMLElement): Element | null; onLoaded?(): void; onFail?(e: string): void }): () => void {
  const s = following()
  const label = native.webLabel(o.id)
  let raf = 0
  let gone = false
  let off: (() => void) | null = null
  void native
    .onState((st) => {
      if (st.label === label && !st.loading && st.url && s.shown) o.onLoaded?.()
    })
    .then((f) => (gone ? f() : (off = f)))
  const tick = (now: number) => {
    if (gone) return
    const c = o.clip(o.el)
    const r = rectOf(o.el)
    switch (step(s, r, c ? rectOf(c) : null, now)) {
      case 'attach':
        native.attach(o.id, o.url, r).catch((e: unknown) => o.onFail?.(String(e)))
        break
      case 'show':
        void native.bounds(o.id, r).catch(() => undefined)
        void native.visible(o.id, true).catch(() => undefined)
        break
      case 'hide':
        void native.visible(o.id, false).catch(() => undefined)
        break
    }
    raf = requestAnimationFrame(tick)
  }
  raf = requestAnimationFrame(tick)
  return () => {
    gone = true
    cancelAnimationFrame(raf)
    off?.()
    if (s.attached) void native.close(o.id).catch(() => undefined)
  }
}

// ---- previews ----

/** A WebKit snapshot is macOS only (`web_view_call` answers `CALL_UNSUPPORTED` elsewhere). */
export const canSnapshot = (): boolean => isMacDesktop()

/** `snapshot` of a shown web view as a PNG blob, or null. */
export async function snapshotOf(id: string): Promise<Blob | null> {
  if (!canSnapshot()) return null
  try {
    const r = (await native.viewCall(id, 'snapshot', {})) as { png?: unknown }
    return typeof r?.png === 'string' ? base64Blob(r.png) : null
  } catch {
    return null
  }
}

/** Store `blob` as the card's snapshot of version `v`. */
export async function storeSnap(doc: Y.Doc, host: CanvasHost, card: Id, v: number, theme: 'light' | 'dark', blob: Blob): Promise<void> {
  await host.blobPut(`snap/${card}/${v}`, blob)
  setSnap(doc, card, { v, at: Date.now(), theme })
}

const LOAD_MS = 15_000
const PAINT_MS = 800
const PREVIEW_W = 1024
const PREVIEW_ID = webId('preview')
let chain: Promise<unknown> = Promise.resolve()
const asked = new Set<string>()

/** Wait for `label`'s first finished load with a URL, or `ms`. */
function loaded(label: string, ms: number): Promise<void> {
  const { promise, resolve } = Promise.withResolvers<void>()
  let off: (() => void) | null = null
  let over = false
  const end = () => {
    if (over) return
    over = true
    clearTimeout(t)
    off?.()
    resolve()
  }
  const t = setTimeout(end, ms)
  void native
    .onState((st) => st.label === label && !st.loading && st.url !== '' && end())
    .then((f) => (over ? f() : (off = f)))
  return promise
}

/** Load `url` in an off-window web view `PREVIEW_W` wide with the card's aspect, snapshot it, destroy it. One at a time. */
function capture(url: string, aspect: number): Promise<Blob | null> {
  const run = async () => {
    const width = PREVIEW_W
    const height = Math.round(Math.min(Math.max(width / aspect, 480), 1600))
    const label = native.webLabel(PREVIEW_ID)
    const ready = loaded(label, LOAD_MS)
    try {
      // Off the window's left edge: never seen, still in the window, so WebKit lays it out and paints it.
      await native.attach(PREVIEW_ID, url, { x: -width - 200, y: 0, width, height })
      await ready
      await new Promise((r) => setTimeout(r, PAINT_MS))
      return await snapshotOf(PREVIEW_ID)
    } catch {
      return null
    } finally {
      await native.close(PREVIEW_ID).catch(() => undefined)
    }
  }
  const next = chain.then(run, run)
  chain = next.catch(() => undefined)
  return next
}

/**
 * Capture and store the preview of `card`'s version `v` (once per card version per window unless `force`). Resolves
 * true when stored.
 */
export async function preview(o: { doc: Y.Doc; host: CanvasHost; card: Id; v: number; url: string; aspect: number; theme: 'light' | 'dark'; force?: boolean }): Promise<boolean> {
  if (!canSnapshot()) return false
  const key = `${o.host.doc}/${o.card}/${o.v}`
  if (asked.has(key) && !o.force) return false
  asked.add(key)
  const blob = await capture(o.url, o.aspect > 0 ? o.aspect : 4 / 3)
  if (!blob) return false
  try {
    await storeSnap(o.doc, o.host, o.card, o.v, o.theme, blob)
    return true
  } catch {
    return false
  }
}
